// sea-orm-cli generate entity -u sqlite:./et.db -o easytier-web/src/db/entity/ --with-serde both --with-copy-enums
#[allow(unused_imports)]
pub mod entity;

use easytier::common::config::{ConfigSource, NetworkConfig};
use easytier_core::management::remote_client::{ListNetworkProps, Storage};
use entity::user_running_network_configs;
use sea_orm::{
    ActiveModelTrait as _, ColumnTrait as _, DatabaseConnection, DbErr, EntityTrait,
    IntoActiveModel as _, QueryFilter as _, Set, SqlxSqliteConnector, TransactionTrait as _,
    sea_query::OnConflict,
};
use sea_orm_migration::MigratorTrait as _;
use sqlx::{Sqlite, SqlitePool, migrate::MigrateDatabase as _, types::chrono};
use std::collections::{HashMap, HashSet};
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt as _;
use uuid::Uuid;

use crate::migrator;
use async_trait::async_trait;

pub fn hash_web_login_password(plaintext: &str) -> String {
    // Prefer argon2(plaintext). Legacy clients stored argon2(md5(plaintext)); login upgrades.
    password_auth::generate_hash(plaintext)
}

pub fn generate_config_token() -> String {
    format!("et_{}", uuid::Uuid::new_v4().simple())
}

/// Sentinel for "config token already taken" failures.
/// Compare with [`is_config_token_taken_err`] (exact match) instead of
/// `contains("Token already exists")` so UNIQUE-constraint text from the DB
/// is never confused with this pre-check.
pub const CONFIG_TOKEN_ALREADY_EXISTS_MSG: &str = "Token already exists";

/// Exact-match check for the [`CONFIG_TOKEN_ALREADY_EXISTS_MSG`] pre-check sentinel.
pub fn is_config_token_taken_err(e: &DbErr) -> bool {
    matches!(e, DbErr::Custom(msg) if msg == CONFIG_TOKEN_ALREADY_EXISTS_MSG)
}

/// Best-effort check for a DB-level UNIQUE violation (final defense for races
/// the pre-check cannot see). Maps to 409 in the REST layer.
///
/// Matches common SQLite / MySQL / Postgres unique-constraint texts only —
/// avoid bare `"unique"` which can false-positive on unrelated messages.
pub fn is_unique_violation_err(e: &DbErr) -> bool {
    let msg = e.to_string().to_ascii_lowercase();
    msg.contains("unique constraint failed")
        || msg.contains("sqlite_constraint_unique")
        || msg.contains("duplicate key")
        || msg.contains("duplicate entry")
        || msg.contains("unique violation")
}

/// Exact-match check for token-shape validation failures (maps to 400).
/// Kept as explicit sentinels so the REST layer never sniffs substrings.
pub fn is_config_token_validation_err(e: &DbErr) -> bool {
    matches!(
        e,
        DbErr::Custom(msg)
            if msg == "Token cannot be empty"
                || msg == "Token is too long (max 128 characters)"
                || msg == "Token may only contain letters, digits, '.', '_' or '-'"
                || msg == "Token must not start with 'revoked_' prefix"
    )
}

/// Number of username-collision retries for `auto_create_user` (`auto_<prefix>`).
pub const AUTO_CREATE_USERNAME_RETRIES: usize = 5;

/// Validate a config-server URL path token (e.g. `admin`).
/// Allowed: ASCII letters, digits, `.`, `_`, `-` (safe in URL paths).
pub fn validate_config_token_value(token: &str) -> Result<(), String> {
    let token = token.trim();
    if token.is_empty() {
        return Err("Token cannot be empty".to_string());
    }
    if token.starts_with("revoked_") {
        return Err("Token must not start with 'revoked_' prefix".to_string());
    }
    if token.len() > 128 {
        return Err("Token is too long (max 128 characters)".to_string());
    }
    if !token
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
    {
        return Err("Token may only contain letters, digits, '.', '_' or '-'".to_string());
    }
    Ok(())
}

fn md5_hex(input: &str) -> String {
    format!("{:x}", md5::compute(input.as_bytes()))
}

/// Verify a password against stored argon2 hash.
/// Accepts plaintext (preferred) and legacy md5-prehash / argon2(md5(plaintext)).
pub fn verify_web_login_password(input: &str, stored_hash: &str) -> bool {
    if password_auth::verify_password(input, stored_hash).is_ok() {
        return true;
    }
    let legacy = md5_hex(input);
    password_auth::verify_password(&legacy, stored_hash).is_ok()
}

/// Returns true when the stored hash is the legacy argon2(md5(plaintext)) form
/// and should be upgraded to argon2(plaintext).
pub fn web_login_password_needs_upgrade(plaintext: &str, stored_hash: &str) -> bool {
    if password_auth::verify_password(plaintext, stored_hash).is_ok() {
        return false;
    }
    let legacy = md5_hex(plaintext);
    password_auth::verify_password(&legacy, stored_hash).is_ok()
}

pub type UserIdInDb = i32;

#[derive(Debug)]
pub(crate) struct ManagedConfigUpsert {
    pub instance_id: Uuid,
    pub network_config: NetworkConfig,
}

#[derive(Debug, Clone)]
pub(crate) enum ManagedConfigExpectedRevision {
    Any,
    Exact(Option<String>),
}

#[derive(Debug)]
pub(crate) enum ManagedConfigUpdate {
    Full {
        upserts: Vec<ManagedConfigUpsert>,
        target_revision: Option<String>,
        expected_revision: ManagedConfigExpectedRevision,
    },
    Patch {
        upserts: Vec<ManagedConfigUpsert>,
        delete_instance_ids: Vec<Uuid>,
        target_revision: String,
        expected_revision: String,
    },
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum ManagedConfigApplyResult {
    Applied {
        deleted_web_instance_ids: Vec<Uuid>,
    },
    AlreadyApplied,
    RevisionConflict {
        expected: Option<String>,
        current: Option<String>,
    },
    OwnershipConflict {
        instance_id: Uuid,
    },
}

fn sqlx_db_error(error: sqlx::Error) -> DbErr {
    DbErr::Custom(error.to_string())
}

async fn read_managed_config_revision(
    transaction: &mut sqlx::Transaction<'_, Sqlite>,
    user_id: UserIdInDb,
    device_id: Uuid,
) -> Result<Option<String>, DbErr> {
    sqlx::query_scalar(
        r#"
        SELECT config_revision
        FROM managed_config_revisions
        WHERE user_id = ? AND device_id = ?
        "#,
    )
    .bind(user_id)
    .bind(device_id.to_string())
    .fetch_optional(&mut **transaction)
    .await
    .map_err(sqlx_db_error)
}

async fn clear_managed_config_revision(
    transaction: &mut sqlx::Transaction<'_, Sqlite>,
    user_id: UserIdInDb,
    device_id: Uuid,
) -> Result<(), DbErr> {
    sqlx::query(
        r#"
        DELETE FROM managed_config_revisions
        WHERE user_id = ? AND device_id = ?
        "#,
    )
    .bind(user_id)
    .bind(device_id.to_string())
    .execute(&mut **transaction)
    .await
    .map_err(sqlx_db_error)?;
    Ok(())
}

async fn write_managed_config_revision(
    transaction: &mut sqlx::Transaction<'_, Sqlite>,
    user_id: UserIdInDb,
    device_id: Uuid,
    config_revision: &str,
) -> Result<(), DbErr> {
    let now = chrono::Local::now().fixed_offset();
    sqlx::query(
        r#"
        INSERT INTO managed_config_revisions (
            user_id, device_id, config_revision, create_time, update_time
        ) VALUES (?, ?, ?, ?, ?)
        ON CONFLICT(user_id, device_id) DO UPDATE SET
            config_revision = excluded.config_revision,
            update_time = excluded.update_time
        "#,
    )
    .bind(user_id)
    .bind(device_id.to_string())
    .bind(config_revision)
    .bind(now)
    .bind(now)
    .execute(&mut **transaction)
    .await
    .map_err(sqlx_db_error)?;
    Ok(())
}

async fn read_config_source(
    transaction: &mut sqlx::Transaction<'_, Sqlite>,
    user_id: UserIdInDb,
    device_id: Uuid,
    instance_id: Uuid,
) -> Result<Option<String>, DbErr> {
    sqlx::query_scalar(
        r#"
        SELECT source
        FROM user_running_network_configs
        WHERE user_id = ? AND device_id = ? AND network_instance_id = ?
        "#,
    )
    .bind(user_id)
    .bind(device_id.to_string())
    .bind(instance_id.to_string())
    .fetch_optional(&mut **transaction)
    .await
    .map_err(sqlx_db_error)
}

fn network_secret_digest(network_name: &str, network_secret: &str) -> String {
    let digest = md5::compute(format!("{network_name}\n{network_secret}").as_bytes());
    format!("{:x}", digest)
}

fn extract_network_identity(network_config_json: &str) -> (String, String) {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(network_config_json) else {
        return (String::new(), String::new());
    };
    let network_name = value
        .get("network_name")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let network_secret = value
        .get("network_secret")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    (network_name, network_secret)
}

async fn upsert_network_credential_tx(
    transaction: &mut sqlx::Transaction<'_, Sqlite>,
    user_id: UserIdInDb,
    network_name: &str,
    network_secret: &str,
) -> Result<(), DbErr> {
    if network_name.is_empty() {
        return Ok(());
    }
    let now = chrono::Local::now().fixed_offset();
    let digest = network_secret_digest(network_name, network_secret);
    // 空密码只登记名称，不覆盖已有凭证，避免「保存时 secret 为空」把目录密码冲掉
    sqlx::query(
        r#"
        INSERT INTO networks (
            user_id, network_name, network_secret, network_secret_digest,
            create_time, update_time
        ) VALUES (?, ?, ?, ?, ?, ?)
        ON CONFLICT(user_id, network_name) DO UPDATE SET
            network_secret = CASE
                WHEN excluded.network_secret != '' THEN excluded.network_secret
                ELSE networks.network_secret
            END,
            network_secret_digest = CASE
                WHEN excluded.network_secret != '' THEN excluded.network_secret_digest
                ELSE networks.network_secret_digest
            END,
            update_time = excluded.update_time
        "#,
    )
    .bind(user_id)
    .bind(network_name)
    .bind(network_secret)
    .bind(digest)
    .bind(now)
    .bind(now)
    .execute(&mut **transaction)
    .await
    .map_err(sqlx_db_error)?;
    Ok(())
}

async fn upsert_network_config(
    transaction: &mut sqlx::Transaction<'_, Sqlite>,
    user_id: UserIdInDb,
    device_id: Uuid,
    instance_id: Uuid,
    network_config: &str,
    source: ConfigSource,
    web_only_update: bool,
) -> Result<bool, DbErr> {
    let now = chrono::Local::now().fixed_offset();
    let (network_name, network_secret) = extract_network_identity(network_config);
    let mut query = r#"
        INSERT INTO user_running_network_configs (
            user_id, device_id, network_instance_id, network_config,
            network_name, source, disabled, create_time, update_time
        ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)
        ON CONFLICT(user_id, device_id, network_instance_id) DO UPDATE SET
            network_config = excluded.network_config,
            network_name = excluded.network_name,
            source = excluded.source,
            disabled = excluded.disabled,
            update_time = excluded.update_time
    "#
    .to_string();
    if web_only_update {
        query.push_str(" WHERE user_running_network_configs.source = 'web'");
    }
    let result = sqlx::query(&query)
        .bind(user_id)
        .bind(device_id.to_string())
        .bind(instance_id.to_string())
        .bind(network_config)
        .bind(&network_name)
        .bind(source.as_str())
        .bind(false)
        .bind(now)
        .bind(now)
        .execute(&mut **transaction)
        .await
        .map_err(sqlx_db_error)?;
    if result.rows_affected() > 0 {
        upsert_network_credential_tx(transaction, user_id, &network_name, &network_secret).await?;
    }
    Ok(result.rows_affected() > 0)
}

#[cfg(unix)]
fn restrict_database_file_permissions(db_path: &str) -> anyhow::Result<()> {
    if db_path.ends_with(":memory:") || db_path.contains("mode=memory") {
        return Ok(());
    }
    let path = db_path
        .strip_prefix("sqlite://")
        .or_else(|| db_path.strip_prefix("sqlite:"))
        .unwrap_or(db_path);
    let path = path
        .strip_prefix("file:")
        .unwrap_or(path)
        .split('?')
        .next()
        .filter(|path| !path.is_empty());
    let Some(path) = path else {
        return Ok(());
    };
    let mut permissions = std::fs::metadata(path)?.permissions();
    permissions.set_mode(0o600);
    std::fs::set_permissions(path, permissions)?;
    Ok(())
}

#[cfg(not(unix))]
fn restrict_database_file_permissions(_db_path: &str) -> anyhow::Result<()> {
    Ok(())
}

#[derive(Debug, Clone)]
pub struct Db {
    db_path: String,
    db: SqlitePool,
    orm_db: DatabaseConnection,
}

impl Db {
    pub async fn new<T: ToString>(db_path: T) -> anyhow::Result<Self> {
        let db = Self::prepare_db(db_path.to_string().as_str()).await?;
        let orm_db = SqlxSqliteConnector::from_sqlx_sqlite_pool(db.clone());
        migrator::Migrator::up(&orm_db, None).await?;
        Self::backfill_empty_network_secret_digests(&orm_db).await?;

        Ok(Self {
            db_path: db_path.to_string(),
            db,
            orm_db,
        })
    }

    pub fn db_path(&self) -> &str {
        &self.db_path
    }

    async fn backfill_empty_network_secret_digests(
        orm_db: &DatabaseConnection,
    ) -> anyhow::Result<()> {
        use entity::networks as n;
        use sea_orm::ActiveModelTrait;

        let rows = n::Entity::find()
            .filter(n::Column::NetworkSecretDigest.eq(""))
            .all(orm_db)
            .await?;
        for row in rows {
            let digest = network_secret_digest(&row.network_name, &row.network_secret);
            let mut active = row.into_active_model();
            active.network_secret_digest = Set(digest);
            active.update(orm_db).await?;
        }
        Ok(())
    }

    pub async fn memory_db() -> Self {
        Self::new(":memory:").await.unwrap()
    }

    #[tracing::instrument(ret)]
    async fn prepare_db(db_path: &str) -> anyhow::Result<SqlitePool> {
        if !Sqlite::database_exists(db_path).await.unwrap_or(false) {
            tracing::info!("Database not found, creating a new one");
            Sqlite::create_database(db_path).await?;
        }
        restrict_database_file_permissions(db_path)?;

        let db = sqlx::pool::PoolOptions::new()
            .max_lifetime(None)
            .idle_timeout(None)
            .connect(db_path)
            .await?;

        // Enforce FK cascades (SQLite defaults to OFF per connection).
        sqlx::query("PRAGMA foreign_keys = ON").execute(&db).await?;

        Ok(db)
    }

    pub fn inner(&self) -> SqlitePool {
        self.db.clone()
    }

    pub fn orm_db(&self) -> &DatabaseConnection {
        &self.orm_db
    }

    pub async fn get_user_id<T: ToString>(
        &self,
        user_name: T,
    ) -> Result<Option<UserIdInDb>, DbErr> {
        use entity::users as u;

        let user = u::Entity::find()
            .filter(u::Column::Username.eq(user_name.to_string()))
            .one(self.orm_db())
            .await?;

        Ok(user.map(|u| u.id))
    }

    /// `password_hash` must be pre-hashed by the caller.
    /// Creates user + joins "users" group in one transaction. Returns the created user model.
    pub async fn create_user_and_join_users_group(
        &self,
        username: &str,
        password_hash: String,
    ) -> Result<entity::users::Model, DbErr> {
        self.create_user_and_join_groups(username, password_hash, &["users"], None)
            .await
    }

    /// Create user and join the given groups (e.g. `users`, `admins`) in one transaction.
    /// When `config_token` is `None`, a random token is generated.
    /// The token is stored both on `users.config_token` (legacy) and `user_config_tokens`.
    pub async fn create_user_and_join_groups(
        &self,
        username: &str,
        password_hash: String,
        group_names: &[&str],
        config_token: Option<String>,
    ) -> Result<entity::users::Model, DbErr> {
        use entity::{groups, user_config_tokens, users, users_groups};

        if group_names.is_empty() {
            return Err(DbErr::Custom("At least one group is required".to_string()));
        }

        let token = config_token.unwrap_or_else(generate_config_token);
        let token = token.trim().to_string();
        if let Err(e) = validate_config_token_value(&token) {
            return Err(DbErr::Custom(e));
        }
        if self.is_config_token_taken(&token, None, None).await? {
            return Err(DbErr::Custom(CONFIG_TOKEN_ALREADY_EXISTS_MSG.to_string()));
        }

        let txn = self.orm_db().begin().await?;

        let user_active = users::ActiveModel {
            username: Set(username.to_string()),
            password: Set(password_hash),
            config_token: Set(token.clone()),
            ..Default::default()
        };
        let insert_result = users::Entity::insert(user_active).exec(&txn).await?;

        let new_user = users::Entity::find_by_id(insert_result.last_insert_id)
            .one(&txn)
            .await?
            .ok_or_else(|| DbErr::Custom("Failed to find newly created user".to_string()))?;

        for name in group_names {
            let group = groups::Entity::find()
                .filter(groups::Column::Name.eq(*name))
                .one(&txn)
                .await?
                .ok_or_else(|| DbErr::Custom(format!("Group '{name}' not found")))?;

            let ug_active = users_groups::ActiveModel {
                user_id: Set(new_user.id),
                group_id: Set(group.id),
                ..Default::default()
            };
            users_groups::Entity::insert(ug_active).exec(&txn).await?;
        }

        let now = chrono::Local::now().fixed_offset();
        let token_row = user_config_tokens::ActiveModel {
            user_id: Set(new_user.id),
            token: Set(token),
            label: Set("default".to_string()),
            create_time: Set(now),
            update_time: Set(now),
            ..Default::default()
        };
        user_config_tokens::Entity::insert(token_row)
            .exec(&txn)
            .await?;

        txn.commit().await?;

        Ok(new_user)
    }

    pub async fn delete_user_by_id(&self, user_id: i32) -> Result<(), DbErr> {
        use entity::{user_config_tokens, users, users_groups};

        let txn = self.orm_db().begin().await?;

        // Explicitly remove this user's URL tokens too; do not rely on
        // SQLite foreign-key enforcement (often off without PRAGMA foreign_keys=ON).
        user_config_tokens::Entity::delete_many()
            .filter(user_config_tokens::Column::UserId.eq(user_id))
            .exec(&txn)
            .await?;

        users_groups::Entity::delete_many()
            .filter(users_groups::Column::UserId.eq(user_id))
            .exec(&txn)
            .await?;

        let result = users::Entity::delete_by_id(user_id).exec(&txn).await?;
        if result.rows_affected == 0 {
            return Err(DbErr::Custom("User not found".to_string()));
        }

        txn.commit().await?;
        Ok(())
    }

    pub async fn set_user_password_by_username(
        &self,
        username: &str,
        password_hash: String,
    ) -> Result<(), DbErr> {
        use entity::users;

        let mut user = users::Entity::find()
            .filter(users::Column::Username.eq(username))
            .one(self.orm_db())
            .await?
            .ok_or_else(|| DbErr::Custom(format!("User '{username}' not found")))?
            .into_active_model();
        user.password = Set(password_hash);

        users::Entity::update(user).exec(self.orm_db()).await?;
        Ok(())
    }

    pub async fn auto_create_user(&self, token: &str) -> Result<entity::users::Model, DbErr> {
        // Username is derived for display; config_token remains the URL secret.
        // Slice by chars (not bytes) so multi-byte input cannot panic.
        let token = token.trim().to_string();
        let prefix: String = token.chars().take(8).collect();
        let base_username = if token.chars().count() > 32 {
            format!("auto_{prefix}")
        } else {
            token.clone()
        };
        let random_password = uuid::Uuid::new_v4().to_string();
        let hashed_password =
            tokio::task::spawn_blocking(move || password_auth::generate_hash(&random_password))
                .await
                .map_err(|e| DbErr::Custom(format!("Failed to hash password: {}", e)))?;
        // `auto_<8-char prefix>` can collide across tokens sharing a prefix;
        // retry with a random suffix a few times before giving up.
        let mut attempt = 0usize;
        loop {
            let username = if attempt == 0 {
                base_username.clone()
            } else {
                format!(
                    "{}_{}",
                    base_username,
                    &uuid::Uuid::new_v4().simple().to_string()[..6]
                )
            };
            match self
                .create_user_and_join_groups(
                    &username,
                    hashed_password.clone(),
                    &["users"],
                    Some(token.clone()),
                )
                .await
            {
                Ok(user) => return Ok(user),
                Err(e) if is_config_token_taken_err(&e) => return Err(e),
                Err(e)
                    if attempt < AUTO_CREATE_USERNAME_RETRIES
                        && is_unique_violation_err(&e)
                        && e.to_string().contains("username") =>
                {
                    tracing::warn!(
                        username = %username,
                        attempt,
                        "auto-create username collision, retrying"
                    );
                    attempt += 1;
                }
                Err(e) => return Err(e),
            }
        }
    }

    pub async fn get_username_by_id(&self, user_id: UserIdInDb) -> Result<Option<String>, DbErr> {
        use entity::users as u;
        let user = u::Entity::find_by_id(user_id).one(self.orm_db()).await?;
        Ok(user.map(|u| u.username))
    }

    // Lookup by config-server URL path token (`udp://host/<token>`).
    // Auth only uses `user_config_tokens` so delete/update truly revoke access.
    // `users.config_token` is kept as a display/primary mirror and must stay in sync.
    pub async fn get_user_id_by_token<T: ToString>(
        &self,
        token: T,
    ) -> Result<Option<UserIdInDb>, DbErr> {
        use entity::user_config_tokens as t;

        let token = token.to_string();
        let token = token.trim().to_string();
        if token.is_empty() || token.starts_with("revoked_") {
            return Ok(None);
        }

        let row = t::Entity::find()
            .filter(t::Column::Token.eq(token))
            .one(self.orm_db())
            .await?;
        Ok(row.map(|r| r.user_id))
    }

    /// True if `token` is already used in `user_config_tokens` or `users.config_token`.
    async fn is_config_token_taken(
        &self,
        token: &str,
        exclude_token_row_id: Option<i32>,
        exclude_user_id: Option<UserIdInDb>,
    ) -> Result<bool, DbErr> {
        use entity::{user_config_tokens as t, users as u};

        let mut token_query = t::Entity::find().filter(t::Column::Token.eq(token.to_string()));
        if let Some(id) = exclude_token_row_id {
            token_query = token_query.filter(t::Column::Id.ne(id));
        }
        if token_query.one(self.orm_db()).await?.is_some() {
            return Ok(true);
        }

        let mut user_query = u::Entity::find().filter(u::Column::ConfigToken.eq(token.to_string()));
        if let Some(uid) = exclude_user_id {
            user_query = user_query.filter(u::Column::Id.ne(uid));
        }
        Ok(user_query.one(self.orm_db()).await?.is_some())
    }

    async fn set_user_primary_config_token(
        &self,
        user_id: UserIdInDb,
        token: String,
    ) -> Result<(), DbErr> {
        use entity::users as u;
        let mut model = u::Entity::find_by_id(user_id)
            .one(self.orm_db())
            .await?
            .ok_or_else(|| DbErr::Custom("User not found".to_string()))?
            .into_active_model();
        model.config_token = Set(token);
        u::Entity::update(model).exec(self.orm_db()).await?;
        Ok(())
    }

    /// After token table changes, keep `users.config_token` pointing at a live token
    /// (or a unique non-auth `revoked_*` placeholder when none remain).
    async fn sync_primary_config_token(
        &self,
        user_id: UserIdInDb,
        preferred: Option<&str>,
    ) -> Result<String, DbErr> {
        use entity::{user_config_tokens as t, users as u};
        use sea_orm::QueryOrder as _;

        if let Some(preferred) = preferred
            && t::Entity::find()
                .filter(t::Column::UserId.eq(user_id))
                .filter(t::Column::Token.eq(preferred.to_string()))
                .one(self.orm_db())
                .await?
                .is_some()
        {
            self.set_user_primary_config_token(user_id, preferred.to_string())
                .await?;
            return Ok(preferred.to_string());
        }

        if let Some(row) = t::Entity::find()
            .filter(t::Column::UserId.eq(user_id))
            .order_by_desc(t::Column::UpdateTime)
            .order_by_desc(t::Column::Id)
            .one(self.orm_db())
            .await?
        {
            self.set_user_primary_config_token(user_id, row.token.clone())
                .await?;
            return Ok(row.token);
        }

        // No live tokens: use a unique placeholder that cannot authenticate.
        let mut candidate = format!("revoked_{}", uuid::Uuid::new_v4().simple());
        while self
            .is_config_token_taken(&candidate, None, Some(user_id))
            .await?
        {
            candidate = format!("revoked_{}", uuid::Uuid::new_v4().simple());
        }
        let _ = u::Entity::find_by_id(user_id)
            .one(self.orm_db())
            .await?
            .ok_or_else(|| DbErr::Custom("User not found".to_string()))?;
        self.set_user_primary_config_token(user_id, candidate.clone())
            .await?;
        Ok(candidate)
    }

    pub async fn list_config_tokens(
        &self,
    ) -> Result<Vec<(entity::user_config_tokens::Model, String)>, DbErr> {
        use entity::{user_config_tokens as t, users};
        use sea_orm::{JoinType, QueryOrder, QuerySelect, RelationTrait};

        #[derive(sea_orm::FromQueryResult)]
        struct Row {
            id: i32,
            user_id: i32,
            token: String,
            label: String,
            create_time: chrono::DateTime<chrono::FixedOffset>,
            update_time: chrono::DateTime<chrono::FixedOffset>,
            username: String,
        }

        let rows = t::Entity::find()
            .column_as(t::Column::Id, "id")
            .column_as(t::Column::UserId, "user_id")
            .column_as(t::Column::Token, "token")
            .column_as(t::Column::Label, "label")
            .column_as(t::Column::CreateTime, "create_time")
            .column_as(t::Column::UpdateTime, "update_time")
            .column_as(users::Column::Username, "username")
            .join(JoinType::InnerJoin, t::Relation::Users.def())
            .order_by_desc(t::Column::UpdateTime)
            .into_model::<Row>()
            .all(self.orm_db())
            .await?;

        Ok(rows
            .into_iter()
            .map(|r| {
                (
                    entity::user_config_tokens::Model {
                        id: r.id,
                        user_id: r.user_id,
                        token: r.token,
                        label: r.label,
                        create_time: r.create_time,
                        update_time: r.update_time,
                    },
                    r.username,
                )
            })
            .collect())
    }

    pub async fn list_user_config_tokens(
        &self,
        user_id: UserIdInDb,
    ) -> Result<Vec<entity::user_config_tokens::Model>, DbErr> {
        use entity::user_config_tokens as t;
        use sea_orm::QueryOrder as _;
        t::Entity::find()
            .filter(t::Column::UserId.eq(user_id))
            .order_by_desc(t::Column::UpdateTime)
            .all(self.orm_db())
            .await
    }

    pub async fn create_config_token(
        &self,
        user_id: UserIdInDb,
        token: Option<String>,
        label: Option<String>,
    ) -> Result<entity::user_config_tokens::Model, DbErr> {
        use entity::{user_config_tokens as t, users};

        users::Entity::find_by_id(user_id)
            .one(self.orm_db())
            .await?
            .ok_or_else(|| DbErr::Custom("User not found".to_string()))?;

        let token = token.unwrap_or_else(generate_config_token);
        let token = token.trim().to_string();
        if let Err(e) = validate_config_token_value(&token) {
            return Err(DbErr::Custom(e));
        }
        if self.is_config_token_taken(&token, None, None).await? {
            return Err(DbErr::Custom(CONFIG_TOKEN_ALREADY_EXISTS_MSG.to_string()));
        }

        let now = chrono::Local::now().fixed_offset();
        let active = t::ActiveModel {
            user_id: Set(user_id),
            token: Set(token.clone()),
            label: Set(label.unwrap_or_default().trim().to_string()),
            create_time: Set(now),
            update_time: Set(now),
            ..Default::default()
        };
        let insert = t::Entity::insert(active).exec(self.orm_db()).await?;
        let created = t::Entity::find_by_id(insert.last_insert_id)
            .one(self.orm_db())
            .await?
            .ok_or_else(|| DbErr::Custom("Failed to load created token".to_string()))?;

        // Keep primary mirror in sync when user had no live tokens / revoked placeholder.
        let user = users::Entity::find_by_id(user_id)
            .one(self.orm_db())
            .await?
            .ok_or_else(|| DbErr::Custom("User not found".to_string()))?;
        if user.config_token.is_empty() || user.config_token.starts_with("revoked_") {
            self.set_user_primary_config_token(user_id, token).await?;
        }

        Ok(created)
    }

    pub async fn update_config_token(
        &self,
        id: i32,
        token: Option<String>,
        label: Option<String>,
    ) -> Result<entity::user_config_tokens::Model, DbErr> {
        use entity::{user_config_tokens as t, users as u};

        let existing = t::Entity::find_by_id(id)
            .one(self.orm_db())
            .await?
            .ok_or_else(|| DbErr::Custom("Token not found".to_string()))?;
        let user_id = existing.user_id;
        let old_token = existing.token.clone();
        let mut model = existing.into_active_model();

        let mut token_changed = false;
        if let Some(token) = token {
            let token = token.trim().to_string();
            if let Err(e) = validate_config_token_value(&token) {
                return Err(DbErr::Custom(e));
            }
            if token != old_token {
                if self
                    .is_config_token_taken(&token, Some(id), Some(user_id))
                    .await?
                {
                    return Err(DbErr::Custom(CONFIG_TOKEN_ALREADY_EXISTS_MSG.to_string()));
                }
                model.token = Set(token);
                token_changed = true;
            }
        }
        if let Some(label) = label {
            model.label = Set(label.trim().to_string());
        }
        model.update_time = Set(chrono::Local::now().fixed_offset());
        let updated = model.update(self.orm_db()).await?;

        if token_changed {
            let user = u::Entity::find_by_id(user_id)
                .one(self.orm_db())
                .await?
                .ok_or_else(|| DbErr::Custom("User not found".to_string()))?;
            if user.config_token == old_token {
                self.set_user_primary_config_token(user_id, updated.token.clone())
                    .await?;
            }
        }

        Ok(updated)
    }

    pub async fn delete_config_token(&self, id: i32) -> Result<(), DbErr> {
        use entity::user_config_tokens as t;

        let existing = t::Entity::find_by_id(id)
            .one(self.orm_db())
            .await?
            .ok_or_else(|| DbErr::Custom("Token not found".to_string()))?;
        let user_id = existing.user_id;

        let result = t::Entity::delete_by_id(id).exec(self.orm_db()).await?;
        if result.rows_affected == 0 {
            return Err(DbErr::Custom("Token not found".to_string()));
        }

        // Keep users.config_token in sync (or revoked_* when none remain).
        self.sync_primary_config_token(user_id, None).await?;
        Ok(())
    }

    /// Rotate: revoke existing tokens for the user and issue a new primary token.
    /// Delete + create + primary-mirror sync run in a single transaction so a
    /// crash cannot leave the user with zero tokens but a stale primary.
    pub async fn regenerate_user_config_token(&self, user_id: UserIdInDb) -> Result<String, DbErr> {
        use entity::{user_config_tokens as t, users as u};
        use sea_orm::ActiveModelTrait as _;

        let txn = self.orm_db().begin().await?;

        u::Entity::find_by_id(user_id)
            .one(&txn)
            .await?
            .ok_or_else(|| DbErr::Custom("User not found".to_string()))?;

        // Revoke all previous tokens so old URLs stop working.
        t::Entity::delete_many()
            .filter(t::Column::UserId.eq(user_id))
            .exec(&txn)
            .await?;

        let token = generate_config_token();
        let token = token.trim().to_string();
        if let Err(e) = validate_config_token_value(&token) {
            return Err(DbErr::Custom(e));
        }
        if t::Entity::find()
            .filter(t::Column::Token.eq(token.clone()))
            .one(&txn)
            .await?
            .is_some()
            || u::Entity::find()
                .filter(u::Column::ConfigToken.eq(token.clone()))
                .filter(u::Column::Id.ne(user_id))
                .one(&txn)
                .await?
                .is_some()
        {
            return Err(DbErr::Custom(CONFIG_TOKEN_ALREADY_EXISTS_MSG.to_string()));
        }

        let now = chrono::Local::now().fixed_offset();
        let created = t::ActiveModel {
            user_id: Set(user_id),
            token: Set(token.clone()),
            label: Set("regenerated".to_string()),
            create_time: Set(now),
            update_time: Set(now),
            ..Default::default()
        }
        .insert(&txn)
        .await?;

        let mut user = u::Entity::find_by_id(user_id)
            .one(&txn)
            .await?
            .ok_or_else(|| DbErr::Custom("User not found".to_string()))?
            .into_active_model();
        user.config_token = Set(created.token.clone());
        u::Entity::update(user).exec(&txn).await?;

        txn.commit().await?;
        Ok(created.token)
    }

    pub async fn get_managed_config_revision(
        &self,
        (user_id, device_id): (UserIdInDb, Uuid),
    ) -> Result<Option<String>, DbErr> {
        use entity::managed_config_revisions as mcr;

        let revision = mcr::Entity::find()
            .filter(mcr::Column::UserId.eq(user_id))
            .filter(mcr::Column::DeviceId.eq(device_id.to_string()))
            .one(self.orm_db())
            .await?;

        Ok(revision.map(|row| row.config_revision))
    }

    pub async fn set_managed_config_revision(
        &self,
        (user_id, device_id): (UserIdInDb, Uuid),
        config_revision: &str,
    ) -> Result<(), DbErr> {
        use entity::managed_config_revisions as mcr;

        let now = chrono::Local::now().fixed_offset();
        let on_conflict = OnConflict::columns([mcr::Column::UserId, mcr::Column::DeviceId])
            .update_columns([mcr::Column::ConfigRevision, mcr::Column::UpdateTime])
            .to_owned();
        let insert_m = mcr::ActiveModel {
            user_id: Set(user_id),
            device_id: Set(device_id.to_string()),
            config_revision: Set(config_revision.to_string()),
            create_time: Set(now),
            update_time: Set(now),
            ..Default::default()
        };

        mcr::Entity::insert(insert_m)
            .on_conflict(on_conflict)
            .exec(self.orm_db())
            .await?;
        Ok(())
    }

    pub(crate) async fn apply_managed_config_update(
        &self,
        (user_id, device_id): (UserIdInDb, Uuid),
        update: ManagedConfigUpdate,
    ) -> Result<ManagedConfigApplyResult, DbErr> {
        let (upserts, target_revision, expected_revision) = match &update {
            ManagedConfigUpdate::Full {
                upserts,
                target_revision,
                expected_revision,
            } => (
                upserts,
                target_revision.as_deref(),
                expected_revision.clone(),
            ),
            ManagedConfigUpdate::Patch {
                upserts,
                target_revision,
                expected_revision,
                ..
            } => (
                upserts,
                Some(target_revision.as_str()),
                ManagedConfigExpectedRevision::Exact(Some(expected_revision.clone())),
            ),
        };
        let serialized_upserts = upserts
            .iter()
            .map(|upsert| {
                serde_json::to_string(&upsert.network_config)
                    .map(|config| (upsert.instance_id, config))
                    .map_err(|error| DbErr::Json(error.to_string()))
            })
            .collect::<Result<Vec<_>, _>>()?;

        let mut transaction = self
            .db
            .begin_with("BEGIN IMMEDIATE")
            .await
            .map_err(sqlx_db_error)?;
        let current_revision =
            read_managed_config_revision(&mut transaction, user_id, device_id).await?;
        if target_revision.is_some() && current_revision.as_deref() == target_revision {
            transaction.commit().await.map_err(sqlx_db_error)?;
            return Ok(ManagedConfigApplyResult::AlreadyApplied);
        }
        if let ManagedConfigExpectedRevision::Exact(expected) = &expected_revision
            && current_revision.as_ref() != expected.as_ref()
        {
            let result = ManagedConfigApplyResult::RevisionConflict {
                expected: expected.clone(),
                current: current_revision,
            };
            transaction.commit().await.map_err(sqlx_db_error)?;
            return Ok(result);
        }

        let mut existing_sources = HashMap::new();
        match &update {
            ManagedConfigUpdate::Full { .. } => {
                let rows = sqlx::query_as::<_, (String, String)>(
                    r#"
                    SELECT network_instance_id, source
                    FROM user_running_network_configs
                    WHERE user_id = ? AND device_id = ?
                    "#,
                )
                .bind(user_id)
                .bind(device_id.to_string())
                .fetch_all(&mut *transaction)
                .await
                .map_err(sqlx_db_error)?;
                for (instance_id, source) in rows {
                    if let Ok(instance_id) = Uuid::parse_str(&instance_id) {
                        existing_sources.insert(instance_id, source);
                    }
                }
            }
            ManagedConfigUpdate::Patch {
                delete_instance_ids,
                ..
            } => {
                for instance_id in upserts
                    .iter()
                    .map(|upsert| upsert.instance_id)
                    .chain(delete_instance_ids.iter().copied())
                {
                    if let Some(source) =
                        read_config_source(&mut transaction, user_id, device_id, instance_id)
                            .await?
                    {
                        existing_sources.insert(instance_id, source);
                    }
                }
            }
        }

        let strict_ownership = target_revision.is_some();
        if strict_ownership
            && let Some(instance_id) = serialized_upserts
                .iter()
                .map(|(instance_id, _)| *instance_id)
                .chain(match &update {
                    ManagedConfigUpdate::Patch {
                        delete_instance_ids,
                        ..
                    } => delete_instance_ids.iter().copied(),
                    ManagedConfigUpdate::Full { .. } => [].iter().copied(),
                })
                .find(|instance_id| {
                    existing_sources
                        .get(instance_id)
                        .is_some_and(|source| source != ConfigSource::Web.as_str())
                })
        {
            transaction.commit().await.map_err(sqlx_db_error)?;
            return Ok(ManagedConfigApplyResult::OwnershipConflict { instance_id });
        }

        let desired_ids = serialized_upserts
            .iter()
            .map(|(instance_id, _)| *instance_id)
            .collect::<HashSet<_>>();
        for (instance_id, network_config) in &serialized_upserts {
            if !strict_ownership
                && existing_sources
                    .get(instance_id)
                    .is_some_and(|source| source != ConfigSource::Web.as_str())
            {
                continue;
            }
            let updated = upsert_network_config(
                &mut transaction,
                user_id,
                device_id,
                *instance_id,
                network_config,
                ConfigSource::Web,
                true,
            )
            .await?;
            if !updated {
                transaction.rollback().await.map_err(sqlx_db_error)?;
                return Ok(ManagedConfigApplyResult::OwnershipConflict {
                    instance_id: *instance_id,
                });
            }
        }

        let delete_instance_ids = match &update {
            ManagedConfigUpdate::Full { .. } => existing_sources
                .iter()
                .filter_map(|(instance_id, source)| {
                    (source == ConfigSource::Web.as_str() && !desired_ids.contains(instance_id))
                        .then_some(*instance_id)
                })
                .collect::<Vec<_>>(),
            ManagedConfigUpdate::Patch {
                delete_instance_ids,
                ..
            } => delete_instance_ids
                .iter()
                .filter(|instance_id| {
                    existing_sources
                        .get(instance_id)
                        .is_some_and(|source| source == ConfigSource::Web.as_str())
                })
                .copied()
                .collect(),
        };
        for instance_id in &delete_instance_ids {
            sqlx::query(
                r#"
                DELETE FROM user_running_network_configs
                WHERE user_id = ? AND device_id = ? AND network_instance_id = ?
                    AND source = 'web'
                "#,
            )
            .bind(user_id)
            .bind(device_id.to_string())
            .bind(instance_id.to_string())
            .execute(&mut *transaction)
            .await
            .map_err(sqlx_db_error)?;
        }

        match target_revision {
            Some(revision) => {
                write_managed_config_revision(&mut transaction, user_id, device_id, revision)
                    .await?;
            }
            None => {
                clear_managed_config_revision(&mut transaction, user_id, device_id).await?;
            }
        }
        transaction.commit().await.map_err(sqlx_db_error)?;
        Ok(ManagedConfigApplyResult::Applied {
            deleted_web_instance_ids: delete_instance_ids,
        })
    }

    pub async fn delete_web_network_configs(
        &self,
        (user_id, device_id): (UserIdInDb, Uuid),
        network_inst_ids: &[Uuid],
    ) -> Result<(), DbErr> {
        let mut transaction = self
            .db
            .begin_with("BEGIN IMMEDIATE")
            .await
            .map_err(sqlx_db_error)?;
        let mut deleted = false;
        for instance_id in network_inst_ids {
            let result = sqlx::query(
                r#"
                DELETE FROM user_running_network_configs
                WHERE user_id = ? AND device_id = ? AND network_instance_id = ?
                    AND source = 'web'
                "#,
            )
            .bind(user_id)
            .bind(device_id.to_string())
            .bind(instance_id.to_string())
            .execute(&mut *transaction)
            .await
            .map_err(sqlx_db_error)?;
            deleted |= result.rows_affected() > 0;
        }
        if deleted {
            clear_managed_config_revision(&mut transaction, user_id, device_id).await?;
        }
        transaction.commit().await.map_err(sqlx_db_error)?;
        Ok(())
    }

    /// Upsert device archive row from heartbeat / list activity.
    pub async fn upsert_device(
        &self,
        user_id: UserIdInDb,
        device_id: Uuid,
        hostname: &str,
        easytier_version: &str,
        client_url: &str,
        last_seen_at: i64,
    ) -> Result<(), DbErr> {
        let now = chrono::Local::now().fixed_offset();
        sqlx::query(
            r#"
            INSERT INTO devices (
                user_id, device_id, hostname, last_easytier_version,
                last_client_url, last_seen_at, create_time, update_time
            ) VALUES (?, ?, ?, ?, ?, ?, ?, ?)
            ON CONFLICT(user_id, device_id) DO UPDATE SET
                hostname = excluded.hostname,
                last_easytier_version = excluded.last_easytier_version,
                last_client_url = excluded.last_client_url,
                last_seen_at = excluded.last_seen_at,
                update_time = excluded.update_time
            "#,
        )
        .bind(user_id)
        .bind(device_id.to_string())
        .bind(hostname)
        .bind(easytier_version)
        .bind(client_url)
        .bind(last_seen_at)
        .bind(now)
        .bind(now)
        .execute(&self.db)
        .await
        .map_err(sqlx_db_error)?;
        Ok(())
    }

    /// Upsert a user-scoped network credential (by network_name).
    pub async fn upsert_network_credential(
        &self,
        user_id: UserIdInDb,
        network_name: &str,
        network_secret: &str,
    ) -> Result<(), DbErr> {
        let mut transaction = self
            .db
            .begin_with("BEGIN IMMEDIATE")
            .await
            .map_err(sqlx_db_error)?;
        upsert_network_credential_tx(&mut transaction, user_id, network_name, network_secret)
            .await?;
        transaction.commit().await.map_err(sqlx_db_error)?;
        Ok(())
    }

    /// Find network credentials for a user by exact network_name.
    pub async fn find_networks_by_name(
        &self,
        user_id: UserIdInDb,
        network_name: &str,
    ) -> Result<Option<entity::networks::Model>, DbErr> {
        use entity::networks as n;
        n::Entity::find()
            .filter(n::Column::UserId.eq(user_id))
            .filter(n::Column::NetworkName.eq(network_name))
            .one(self.orm_db())
            .await
    }

    /// List all network credentials for a user (newest first).
    pub async fn list_user_networks(
        &self,
        user_id: UserIdInDb,
    ) -> Result<Vec<entity::networks::Model>, DbErr> {
        use entity::networks as n;
        use sea_orm::QueryOrder as _;
        n::Entity::find()
            .filter(n::Column::UserId.eq(user_id))
            .order_by_desc(n::Column::UpdateTime)
            .all(self.orm_db())
            .await
    }

    /// List device archive rows for a user (newest seen first).
    pub async fn list_user_devices(
        &self,
        user_id: UserIdInDb,
    ) -> Result<Vec<entity::devices::Model>, DbErr> {
        use entity::devices as d;
        use sea_orm::QueryOrder as _;
        d::Entity::find()
            .filter(d::Column::UserId.eq(user_id))
            .order_by_desc(d::Column::LastSeenAt)
            .all(self.orm_db())
            .await
    }
}

#[async_trait]
impl Storage<(UserIdInDb, Uuid), user_running_network_configs::Model, DbErr> for Db {
    async fn insert_or_update_user_network_config(
        &self,
        (user_id, device_id): (UserIdInDb, Uuid),
        network_inst_id: Uuid,
        network_config: NetworkConfig,
        source: ConfigSource,
    ) -> Result<(), DbErr> {
        let network_config =
            serde_json::to_string(&network_config).map_err(|e| DbErr::Json(e.to_string()))?;
        let mut transaction = self
            .db
            .begin_with("BEGIN IMMEDIATE")
            .await
            .map_err(sqlx_db_error)?;
        let previous_source =
            read_config_source(&mut transaction, user_id, device_id, network_inst_id).await?;
        upsert_network_config(
            &mut transaction,
            user_id,
            device_id,
            network_inst_id,
            &network_config,
            source,
            false,
        )
        .await?;
        if source == ConfigSource::Web
            || previous_source.as_deref() == Some(ConfigSource::Web.as_str())
        {
            clear_managed_config_revision(&mut transaction, user_id, device_id).await?;
        }
        transaction.commit().await.map_err(sqlx_db_error)
    }

    async fn delete_network_configs(
        &self,
        (user_id, device_id): (UserIdInDb, Uuid),
        network_inst_ids: &[Uuid],
    ) -> Result<(), DbErr> {
        let mut transaction = self
            .db
            .begin_with("BEGIN IMMEDIATE")
            .await
            .map_err(sqlx_db_error)?;
        let mut deleted_web_config = false;
        for instance_id in network_inst_ids {
            deleted_web_config |=
                read_config_source(&mut transaction, user_id, device_id, *instance_id)
                    .await?
                    .as_deref()
                    == Some(ConfigSource::Web.as_str());
            sqlx::query(
                r#"
                DELETE FROM user_running_network_configs
                WHERE user_id = ? AND device_id = ? AND network_instance_id = ?
                "#,
            )
            .bind(user_id)
            .bind(device_id.to_string())
            .bind(instance_id.to_string())
            .execute(&mut *transaction)
            .await
            .map_err(sqlx_db_error)?;
        }
        if deleted_web_config {
            clear_managed_config_revision(&mut transaction, user_id, device_id).await?;
        }
        transaction.commit().await.map_err(sqlx_db_error)?;
        Ok(())
    }

    async fn update_network_config_state(
        &self,
        (user_id, device_id): (UserIdInDb, Uuid),
        network_inst_id: Uuid,
        disabled: bool,
    ) -> Result<(), DbErr> {
        let mut transaction = self
            .db
            .begin_with("BEGIN IMMEDIATE")
            .await
            .map_err(sqlx_db_error)?;
        let source =
            read_config_source(&mut transaction, user_id, device_id, network_inst_id).await?;
        let result = sqlx::query(
            r#"
            UPDATE user_running_network_configs
            SET disabled = ?, update_time = ?
            WHERE user_id = ? AND device_id = ? AND network_instance_id = ?
            "#,
        )
        .bind(disabled)
        .bind(chrono::Local::now().fixed_offset())
        .bind(user_id)
        .bind(device_id.to_string())
        .bind(network_inst_id.to_string())
        .execute(&mut *transaction)
        .await
        .map_err(sqlx_db_error)?;
        if result.rows_affected() > 0 && source.as_deref() == Some(ConfigSource::Web.as_str()) {
            clear_managed_config_revision(&mut transaction, user_id, device_id).await?;
        }
        transaction.commit().await.map_err(sqlx_db_error)?;
        Ok(())
    }

    async fn list_network_configs(
        &self,
        (user_id, device_id): (UserIdInDb, Uuid),
        props: ListNetworkProps,
    ) -> Result<Vec<user_running_network_configs::Model>, DbErr> {
        use entity::user_running_network_configs as urnc;

        let configs = urnc::Entity::find().filter(urnc::Column::UserId.eq(user_id));
        let configs = if matches!(
            props,
            ListNetworkProps::EnabledOnly | ListNetworkProps::DisabledOnly
        ) {
            configs
                .filter(urnc::Column::Disabled.eq(matches!(props, ListNetworkProps::DisabledOnly)))
        } else {
            configs
        };
        let configs = if !device_id.is_nil() {
            configs.filter(urnc::Column::DeviceId.eq(device_id.to_string()))
        } else {
            configs
        };

        let configs = configs.all(self.orm_db()).await?;

        Ok(configs)
    }

    async fn get_network_config(
        &self,
        (user_id, device_id): (UserIdInDb, Uuid),
        network_inst_id: &str,
    ) -> Result<Option<user_running_network_configs::Model>, DbErr> {
        use entity::user_running_network_configs as urnc;

        let config = urnc::Entity::find()
            .filter(urnc::Column::UserId.eq(user_id))
            .filter(urnc::Column::DeviceId.eq(device_id.to_string()))
            .filter(urnc::Column::NetworkInstanceId.eq(network_inst_id))
            .one(self.orm_db())
            .await?;

        Ok(config)
    }
}

#[cfg(test)]
mod tests {
    use easytier::{common::config::ConfigSource, proto::api::manage::NetworkConfig};
    use easytier_core::management::remote_client::{PersistentConfig, Storage};
    use sea_orm::{ActiveModelTrait, ColumnTrait, EntityTrait, QueryFilter as _, Set};

    use crate::db::{Db, ListNetworkProps, entity::user_running_network_configs};

    #[tokio::test]
    async fn test_user_network_config_management() {
        let db = Db::memory_db().await;
        let user_id = 1;
        let network_config = NetworkConfig {
            network_name: Some("test_config".to_string()),
            ..Default::default()
        };
        let network_config_json = serde_json::to_string(&network_config).unwrap();
        let inst_id = uuid::Uuid::new_v4();
        let device_id = uuid::Uuid::new_v4();

        db.insert_or_update_user_network_config(
            (user_id, device_id),
            inst_id,
            network_config,
            ConfigSource::User,
        )
        .await
        .unwrap();

        let result = user_running_network_configs::Entity::find()
            .filter(user_running_network_configs::Column::UserId.eq(user_id))
            .one(db.orm_db())
            .await
            .unwrap()
            .unwrap();
        println!("{:?}", result);
        assert_eq!(result.network_config, network_config_json);
        assert_eq!(result.network_name, "test_config");
        assert_eq!(result.get_network_config_source(), ConfigSource::User);
        let network = db
            .find_networks_by_name(user_id, "test_config")
            .await
            .unwrap()
            .expect("network credential should be upserted");
        assert_eq!(network.network_name, "test_config");

        // overwrite the config
        let network_config = NetworkConfig {
            network_name: Some("test_config2".to_string()),
            network_secret: Some("secret-2".to_string()),
            ..Default::default()
        };
        let network_config_json = serde_json::to_string(&network_config).unwrap();
        db.insert_or_update_user_network_config(
            (user_id, device_id),
            inst_id,
            network_config,
            ConfigSource::Web,
        )
        .await
        .unwrap();

        let result2 = user_running_network_configs::Entity::find()
            .filter(user_running_network_configs::Column::UserId.eq(user_id))
            .one(db.orm_db())
            .await
            .unwrap()
            .unwrap();
        println!("device: {}, {:?}", device_id, result2);
        assert_eq!(result2.network_config, network_config_json);
        assert_eq!(result2.network_name, "test_config2");
        assert_eq!(result2.get_network_config_source(), ConfigSource::Web);
        let network2 = db
            .find_networks_by_name(user_id, "test_config2")
            .await
            .unwrap()
            .expect("updated network credential");
        assert_eq!(network2.network_secret, "secret-2");
        assert_eq!(
            result2.get_runtime_network_config_source(),
            ConfigSource::Web
        );

        assert_eq!(result.create_time, result2.create_time);
        assert_ne!(result.update_time, result2.update_time);

        assert_eq!(
            db.list_network_configs((user_id, device_id), ListNetworkProps::All)
                .await
                .unwrap()
                .len(),
            1
        );

        db.delete_network_configs((user_id, device_id), &[inst_id])
            .await
            .unwrap();
        let result3 = user_running_network_configs::Entity::find()
            .filter(user_running_network_configs::Column::UserId.eq(user_id))
            .one(db.orm_db())
            .await
            .unwrap();
        assert!(result3.is_none());
    }

    #[tokio::test]
    async fn test_unknown_network_config_source_defaults_to_user_runtime_source() {
        let db = Db::memory_db().await;
        let user_id = 1;
        let inst_id = uuid::Uuid::new_v4();
        let device_id = uuid::Uuid::new_v4();

        user_running_network_configs::ActiveModel {
            user_id: Set(user_id),
            device_id: Set(device_id.to_string()),
            network_instance_id: Set(inst_id.to_string()),
            network_config: Set(serde_json::to_string(&NetworkConfig {
                network_name: Some("unknown-source".to_string()),
                ..Default::default()
            })
            .unwrap()),
            source: Set("unknown".to_string()),
            disabled: Set(false),
            create_time: Set(sqlx::types::chrono::Local::now().fixed_offset()),
            update_time: Set(sqlx::types::chrono::Local::now().fixed_offset()),
            ..Default::default()
        }
        .insert(db.orm_db())
        .await
        .unwrap();

        let result = user_running_network_configs::Entity::find()
            .filter(user_running_network_configs::Column::UserId.eq(user_id))
            .one(db.orm_db())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(result.get_network_config_source(), ConfigSource::User);
        assert_eq!(
            result.get_runtime_network_config_source(),
            ConfigSource::User
        );
    }

    #[tokio::test]
    async fn test_user_network_config_same_instance_id_is_scoped_by_device() {
        let db = Db::memory_db().await;
        let user_id = db.auto_create_user("user-1").await.unwrap().id;
        let device1 = uuid::Uuid::new_v4();
        let device2 = uuid::Uuid::new_v4();
        let inst_id = uuid::Uuid::new_v4();

        db.insert_or_update_user_network_config(
            (user_id, device1),
            inst_id,
            NetworkConfig {
                network_name: Some("cfg-1".to_string()),
                ..Default::default()
            },
            ConfigSource::User,
        )
        .await
        .unwrap();
        db.insert_or_update_user_network_config(
            (user_id, device2),
            inst_id,
            NetworkConfig {
                network_name: Some("cfg-2".to_string()),
                ..Default::default()
            },
            ConfigSource::User,
        )
        .await
        .unwrap();

        let first = db
            .get_network_config((user_id, device1), &inst_id.to_string())
            .await
            .unwrap()
            .unwrap();
        let second = db
            .get_network_config((user_id, device2), &inst_id.to_string())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(first.user_id, user_id);
        assert_eq!(first.device_id, device1.to_string());
        assert_eq!(second.user_id, user_id);
        assert_eq!(second.device_id, device2.to_string());

        let device1_configs = db
            .list_network_configs((user_id, device1), ListNetworkProps::All)
            .await
            .unwrap();
        let device2_configs = db
            .list_network_configs((user_id, device2), ListNetworkProps::All)
            .await
            .unwrap();
        assert_eq!(device1_configs.len(), 1);
        assert_eq!(device2_configs.len(), 1);
    }

    #[tokio::test]
    async fn web_owned_mutations_invalidate_managed_revision() {
        let db = Db::memory_db().await;
        let user_id = db
            .auto_create_user("managed-revision-invalidation")
            .await
            .unwrap()
            .id;
        let device_id = uuid::Uuid::new_v4();
        let inst_id = uuid::Uuid::new_v4();
        db.insert_or_update_user_network_config(
            (user_id, device_id),
            inst_id,
            NetworkConfig {
                network_name: Some("managed".to_string()),
                ..Default::default()
            },
            ConfigSource::Web,
        )
        .await
        .unwrap();

        db.set_managed_config_revision((user_id, device_id), "rev-before-disable")
            .await
            .unwrap();
        db.update_network_config_state((user_id, device_id), inst_id, true)
            .await
            .unwrap();
        assert!(
            db.get_managed_config_revision((user_id, device_id))
                .await
                .unwrap()
                .is_none()
        );

        db.set_managed_config_revision((user_id, device_id), "rev-before-delete")
            .await
            .unwrap();
        db.delete_network_configs((user_id, device_id), &[inst_id])
            .await
            .unwrap();
        assert!(
            db.get_managed_config_revision((user_id, device_id))
                .await
                .unwrap()
                .is_none()
        );
    }

    #[tokio::test]
    async fn user_owned_mutation_preserves_managed_revision() {
        let db = Db::memory_db().await;
        let user_id = db
            .auto_create_user("user-revision-preserved")
            .await
            .unwrap()
            .id;
        let device_id = uuid::Uuid::new_v4();
        let inst_id = uuid::Uuid::new_v4();
        db.set_managed_config_revision((user_id, device_id), "rev-user")
            .await
            .unwrap();

        db.insert_or_update_user_network_config(
            (user_id, device_id),
            inst_id,
            NetworkConfig {
                network_name: Some("user".to_string()),
                ..Default::default()
            },
            ConfigSource::User,
        )
        .await
        .unwrap();

        assert_eq!(
            db.get_managed_config_revision((user_id, device_id))
                .await
                .unwrap()
                .as_deref(),
            Some("rev-user")
        );
    }

    #[tokio::test]
    async fn upsert_device_archive_and_list() {
        let db = Db::memory_db().await;
        let user_id = db.auto_create_user("device-archive-user").await.unwrap().id;
        let device_id = uuid::Uuid::new_v4();

        db.upsert_device(
            user_id,
            device_id,
            "host-a",
            "2.6.4",
            "udp://127.0.0.1:22020",
            1_700_000_000,
        )
        .await
        .unwrap();
        db.upsert_device(
            user_id,
            device_id,
            "host-b",
            "2.6.5",
            "udp://127.0.0.1:22021",
            1_700_000_100,
        )
        .await
        .unwrap();

        let devices = db.list_user_devices(user_id).await.unwrap();
        assert_eq!(devices.len(), 1);
        assert_eq!(devices[0].hostname, "host-b");
        assert_eq!(devices[0].last_easytier_version, "2.6.5");
        assert_eq!(devices[0].last_seen_at, 1_700_000_100);
    }
}
