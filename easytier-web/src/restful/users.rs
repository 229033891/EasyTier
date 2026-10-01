use std::collections::HashSet;

use axum_login::{AuthUser, AuthnBackend, AuthzBackend, UserId};
use sea_orm::{
    ColumnTrait, EntityTrait, FromQueryResult, IntoActiveModel, JoinType, QueryFilter,
    QuerySelect as _, RelationTrait, Set,
};
use serde::{Deserialize, Serialize};
use tokio::task;

use crate::db::{self, entity};

#[derive(Clone, Serialize, Deserialize)]
pub struct User {
    pub(crate) db_user: entity::users::Model,
    pub tokens: Vec<String>,
}

// Here we've implemented `Debug` manually to avoid accidentally logging the
// password hash.
impl std::fmt::Debug for User {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("User")
            .field("id", &self.db_user.id)
            .field("username", &self.db_user.username)
            .field("password", &"[redacted]")
            .finish()
    }
}

impl AuthUser for User {
    type Id = i32;

    fn id(&self) -> Self::Id {
        self.db_user.id
    }

    fn session_auth_hash(&self) -> &[u8] {
        self.db_user.password.as_bytes() // We use the password hash as the auth
        // hash--what this means
        // is when the user changes their password the
        // auth session becomes invalid.
    }
}

// This allows us to extract the authentication fields from forms. We use this
// to authenticate requests with the backend.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Credentials {
    pub username: String,
    pub password: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct AdminCreateUser {
    pub username: String,
    pub password: String,
    /// When true, join both `admins` and `users` groups.
    #[serde(default)]
    pub is_admin: bool,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ChangePassword {
    pub new_password: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct UserInfo {
    pub id: i32,
    pub username: String,
    pub groups: Vec<String>,
    pub is_admin: bool,
    /// Primary config-server URL token (mirror of a live `user_config_tokens` row).
    pub config_token: String,
    /// All active config-server tokens for this user.
    #[serde(default)]
    pub config_tokens: Vec<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct MeResponse {
    pub id: i32,
    pub username: String,
    pub is_admin: bool,
    pub config_token: String,
    #[serde(default)]
    pub config_tokens: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct Backend {
    db: db::Db,
}

/// Load live URL tokens for a user without failing the whole request.
/// On DB error returns empty (caller falls back to the primary mirror) and logs.
async fn load_live_tokens_or_empty(db: &db::Db, user_id: i32) -> Vec<String> {
    match db.list_user_config_tokens(user_id).await {
        Ok(rows) => rows.into_iter().map(|t| t.token).collect(),
        Err(e) => {
            tracing::warn!(user_id, ?e, "failed to list user config tokens, using fallback");
            Vec::new()
        }
    }
}

/// Unified token semantics for list/get/me/OIDC paths:
/// live table rows win; when empty, a usable primary mirror fills in;
/// `revoked_*` placeholders and "" collapse to empty (never exposed to frontend).
/// Returns (display_primary, effective_tokens).
pub(crate) fn resolve_tokens_and_primary(
    primary: &str,
    table_tokens: Vec<String>,
) -> (String, Vec<String>) {
    if !table_tokens.is_empty() {
        if table_tokens.iter().any(|t| !t.starts_with("revoked_")) {
            let live: Vec<String> = table_tokens
                .into_iter()
                .filter(|t| !t.starts_with("revoked_"))
                .collect();
            let display = live.first().cloned().unwrap_or_default();
            return (display, live);
        }
        return (String::new(), Vec::new());
    }
    if primary.is_empty() || primary.starts_with("revoked_") {
        (String::new(), Vec::new())
    } else {
        (primary.to_string(), vec![primary.to_string()])
    }
}

impl Backend {
    pub fn new(db: db::Db) -> Self {
        Self { db }
    }

    pub fn db(&self) -> &db::Db {
        &self.db
    }

    pub async fn user_group_names(&self, user_id: i32) -> Result<Vec<String>, Error> {
        use entity::{groups, users_groups};

        #[derive(FromQueryResult)]
        struct GroupNameRow {
            name: String,
        }

        let rows = groups::Entity::find()
            .column_as(groups::Column::Name, "name")
            .join(JoinType::InnerJoin, groups::Relation::UsersGroups.def())
            .filter(users_groups::Column::UserId.eq(user_id))
            .into_model::<GroupNameRow>()
            .all(self.db.orm_db())
            .await?;

        Ok(rows.into_iter().map(|r| r.name).collect())
    }

    pub async fn user_is_admin(&self, user: &User) -> Result<bool, Error> {
        let groups = self.user_group_names(user.db_user.id).await?;
        Ok(groups.iter().any(|g| g == "admins"))
    }

    pub async fn create_user_by_admin(&self, req: &AdminCreateUser) -> anyhow::Result<UserInfo> {
        let username = req.username.trim();
        if username.is_empty() {
            anyhow::bail!("Username is required");
        }
        if req.password.is_empty() {
            anyhow::bail!("Password is required");
        }

        let hashed_password = db::hash_web_login_password(req.password.as_str());
        let groups: &[&str] = if req.is_admin {
            &["users", "admins"]
        } else {
            &["users"]
        };

        let db_user = self
            .db
            .create_user_and_join_groups(username, hashed_password, groups, None)
            .await?;

        Ok(UserInfo {
            id: db_user.id,
            username: db_user.username,
            groups: groups.iter().map(|s| (*s).to_string()).collect(),
            is_admin: req.is_admin,
            config_token: db_user.config_token.clone(),
            config_tokens: vec![db_user.config_token],
        })
    }

    pub async fn list_users(&self) -> Result<Vec<UserInfo>, Error> {
        use entity::{groups, users, users_groups};
        use sea_orm::QueryOrder;
        use std::collections::BTreeMap;

        #[derive(FromQueryResult)]
        struct UserGroupRow {
            id: i32,
            username: String,
            config_token: String,
            group_name: Option<String>,
        }

        let rows = users::Entity::find()
            .column_as(users::Column::Id, "id")
            .column_as(users::Column::Username, "username")
            .column_as(users::Column::ConfigToken, "config_token")
            .column_as(groups::Column::Name, "group_name")
            .join(JoinType::LeftJoin, users::Relation::UsersGroups.def())
            .join(
                JoinType::LeftJoin,
                users_groups::Relation::Groups.def(),
            )
            .order_by_asc(users::Column::Id)
            .into_model::<UserGroupRow>()
            .all(self.db.orm_db())
            .await?;

        let mut map: BTreeMap<i32, UserInfo> = BTreeMap::new();
        for row in rows {
            let entry = map.entry(row.id).or_insert_with(|| UserInfo {
                id: row.id,
                username: row.username.clone(),
                groups: Vec::new(),
                is_admin: false,
                config_token: row.config_token.clone(),
                config_tokens: Vec::new(),
            });
            if let Some(name) = row.group_name {
                if name == "admins" {
                    entry.is_admin = true;
                }
                if !entry.groups.contains(&name) {
                    entry.groups.push(name);
                }
            }
        }

        // Single IN query for all users' tokens, then group in memory (no N+1).
        // One user's failure must not fail the whole table: a batch failure
        // falls back per-user to the primary mirror instead of 500.
        let user_ids: Vec<i32> = map.keys().copied().collect();
        let tokens_by_user: std::collections::HashMap<i32, Vec<String>> = if user_ids.is_empty()
        {
            std::collections::HashMap::new()
        } else {
            use entity::user_config_tokens as t;
            match t::Entity::find()
                .filter(t::Column::UserId.is_in(user_ids))
                .all(self.db.orm_db())
                .await
            {
                Ok(rows) => {
                    let mut grouped: std::collections::HashMap<i32, Vec<String>> =
                        std::collections::HashMap::new();
                    for row in rows {
                        grouped.entry(row.user_id).or_default().push(row.token);
                    }
                    grouped
                }
                Err(e) => {
                    tracing::warn!(?e, "failed to batch-load user config tokens, using fallback");
                    std::collections::HashMap::new()
                }
            }
        };

        for user in map.values_mut() {
            let table_tokens = tokens_by_user.get(&user.id).cloned().unwrap_or_default();
            // Empty grouped result may mean "no tokens" or "batch query failed";
            // either way fall back to the primary mirror, never 500.
            let (display, effective) =
                resolve_tokens_and_primary(&user.config_token.clone(), table_tokens);
            user.config_token = display;
            user.config_tokens = effective;
        }

        Ok(map.into_values().collect())
    }

    pub async fn delete_user(&self, user_id: i32, actor_id: i32) -> anyhow::Result<()> {
        if user_id == actor_id {
            anyhow::bail!("Cannot delete your own account");
        }

        let users = self.list_users().await?;
        let target = users
            .iter()
            .find(|u| u.id == user_id)
            .ok_or_else(|| anyhow::anyhow!("User not found"))?;

        if target.is_admin {
            anyhow::bail!("Cannot delete admin accounts");
        }

        self.db.delete_user_by_id(user_id).await?;
        Ok(())
    }

    /// Find a user by username, or auto-create one for OIDC-authenticated users.
    ///
    /// Unlike the heartbeat auto-creation path (controlled by `allow_auto_create_user`),
    /// OIDC users are always provisioned automatically because their identity has already
    /// been verified by a trusted external Identity Provider (IdP).
    pub async fn find_or_create_oidc_user(&self, username: &str) -> anyhow::Result<User> {
        use entity::users;

        // Try to find an existing user first.
        if let Some(db_user) = users::Entity::find()
            .filter(users::Column::Username.eq(username))
            .one(self.db.orm_db())
            .await?
        {
            let table_tokens = load_live_tokens_or_empty(&self.db, db_user.id).await;
            let (_, tokens) =
                resolve_tokens_and_primary(&db_user.config_token.clone(), table_tokens);
            return Ok(User {
                tokens,
                db_user,
            });
        }

        // User not found – auto-provision a local account backed by the IdP identity.
        let random_password = uuid::Uuid::new_v4().to_string();
        let hashed_password =
            task::spawn_blocking(move || password_auth::generate_hash(&random_password))
                .await
                .map_err(|e| anyhow::anyhow!("Failed to hash password: {e}"))?;
        let db_user = self
            .db
            .create_user_and_join_users_group(username, hashed_password)
            .await?;
        tracing::info!("Auto-provisioned OIDC user '{username}'");
        let table_tokens = load_live_tokens_or_empty(&self.db, db_user.id).await;
        let (_, tokens) = resolve_tokens_and_primary(&db_user.config_token.clone(), table_tokens);
        Ok(User {
            tokens,
            db_user,
        })
    }

    pub async fn change_password(
        &self,
        id: <User as AuthUser>::Id,
        req: &ChangePassword,
    ) -> anyhow::Result<()> {
        let hashed_password = db::hash_web_login_password(req.new_password.as_str());

        use entity::users;

        let mut user = users::Entity::find_by_id(id)
            .one(self.db.orm_db())
            .await?
            .ok_or(anyhow::anyhow!("User not found"))?
            .into_active_model();
        user.password = Set(hashed_password.clone());

        entity::users::Entity::update(user)
            .exec(self.db.orm_db())
            .await?;

        Ok(())
    }
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    Sqlx(#[from] sea_orm::DbErr),

    #[error(transparent)]
    TaskJoin(#[from] task::JoinError),
}

impl AuthnBackend for Backend {
    type User = User;
    type Credentials = Credentials;
    type Error = Error;

    async fn authenticate(
        &self,
        creds: Self::Credentials,
    ) -> Result<Option<Self::User>, Self::Error> {
        let user = entity::users::Entity::find()
            .filter(entity::users::Column::Username.eq(creds.username.clone()))
            .one(self.db.orm_db())
            .await?;
        let Some(db_user) = user else {
            return Ok(None);
        };

        let password = creds.password.clone();
        let stored_hash = db_user.password.clone();
        let matched = task::spawn_blocking(move || {
            db::verify_web_login_password(&password, &stored_hash)
        })
        .await?;

        if !matched {
            return Ok(None);
        }

        // Upgrade legacy argon2(md5(password)) hashes to argon2(password).
        if db::web_login_password_needs_upgrade(&creds.password, &db_user.password) {
            let new_hash = db::hash_web_login_password(&creds.password);
            if let Err(e) = self
                .db
                .set_user_password_by_username(&db_user.username, new_hash)
                .await
            {
                tracing::warn!(
                    "Failed to upgrade password hash for user {}: {:?}",
                    db_user.username,
                    e
                );
            }
        }

        let table_tokens = load_live_tokens_or_empty(&self.db, db_user.id).await;
        let (_, tokens) = resolve_tokens_and_primary(&db_user.config_token.clone(), table_tokens);

        Ok(Some(User {
            tokens,
            db_user,
        }))
    }

    async fn get_user(&self, user_id: &UserId<Self>) -> Result<Option<Self::User>, Self::Error> {
        let user = entity::users::Entity::find()
            .filter(entity::users::Column::Id.eq(*user_id))
            .one(self.db.orm_db())
            .await?;

        if let Some(u) = user {
            let table_tokens = load_live_tokens_or_empty(&self.db, u.id).await;
            let (_, tokens) = resolve_tokens_and_primary(&u.config_token.clone(), table_tokens);
            Ok(Some(User {
                tokens,
                db_user: u,
            }))
        } else {
            Ok(None)
        }
    }
}

#[derive(Debug, Clone, Eq, PartialEq, Hash, FromQueryResult)]
pub struct Permission {
    pub name: String,
}

impl From<&str> for Permission {
    fn from(name: &str) -> Self {
        Permission {
            name: name.to_string(),
        }
    }
}

impl AuthzBackend for Backend {
    type Permission = Permission;

    async fn get_group_permissions(
        &self,
        user: &Self::User,
    ) -> Result<HashSet<Self::Permission>, Self::Error> {
        let permissions = entity::users::Entity::find()
            .filter(entity::users::Column::Id.eq(user.db_user.id))
            .column_as(entity::permissions::Column::Name, "name")
            .join(
                JoinType::LeftJoin,
                entity::users::Relation::UsersGroups.def(),
            )
            .join(
                JoinType::LeftJoin,
                entity::users_groups::Relation::Groups.def(),
            )
            .join(
                JoinType::LeftJoin,
                entity::groups::Relation::GroupsPermissions.def(),
            )
            .join(
                JoinType::LeftJoin,
                entity::groups_permissions::Relation::Permissions.def(),
            )
            .into_model::<Self::Permission>()
            .all(self.db.orm_db())
            .await?;

        Ok(permissions.into_iter().collect())
    }
}

// We use a type alias for convenience.
//
// Note that we've supplied our concrete backend here.
pub type AuthSession = axum_login::AuthSession<Backend>;
