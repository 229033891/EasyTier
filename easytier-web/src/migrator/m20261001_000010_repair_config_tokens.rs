//! Repair pass for config-token data written by intermediate drafts of 008/009.
//!
//! Do **not** edit 008/009 after they may have been applied locally; fix forward here.
//!
//! Repairs:
//! 1. `user_config_tokens` timestamps that lack a timezone (bare `datetime('now')`)
//! 2. Tokens that fail current URL-path validation (charset / length / `revoked_` prefix)
//! 3. Re-seed any `users.config_token` missing from `user_config_tokens`

use sea_orm::{ConnectionTrait, DbBackend, FromQueryResult, Statement};
use sea_orm_migration::prelude::*;

use crate::db::{generate_config_token, validate_config_token_value};

pub struct Migration;

impl MigrationName for Migration {
    fn name(&self) -> &str {
        "m20261001_000010_repair_config_tokens"
    }
}

#[derive(Debug, FromQueryResult)]
struct TokenRow {
    id: i32,
    user_id: i32,
    token: String,
    create_time: String,
    update_time: String,
}

#[derive(Debug, FromQueryResult)]
struct UserTokenRow {
    id: i32,
    config_token: String,
}

fn needs_timezone_fix(value: &str) -> bool {
    let value = value.trim();
    if value.is_empty() {
        return true;
    }
    // SeaORM DateTimeWithTimeZone expects an offset; bare SQLite datetime fails.
    chrono::DateTime::parse_from_rfc3339(value).is_err()
}

fn to_rfc3339_utc(value: &str) -> String {
    let trimmed = value.trim();
    // Common SQLite `datetime('now')`: "YYYY-MM-DD HH:MM:SS"
    if let Ok(naive) = chrono::NaiveDateTime::parse_from_str(trimmed, "%Y-%m-%d %H:%M:%S") {
        return naive.and_utc().to_rfc3339();
    }
    if let Ok(naive) = chrono::NaiveDateTime::parse_from_str(trimmed, "%Y-%m-%dT%H:%M:%S") {
        return naive.and_utc().to_rfc3339();
    }
    // Last resort: stamp "now" so the row remains loadable.
    chrono::Utc::now().to_rfc3339()
}

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let db = manager.get_connection();

        // Table may be absent on very old DBs that somehow skipped 009; create is IF NOT EXISTS in 009.
        let table_exists = db
            .query_all(Statement::from_string(
                DbBackend::Sqlite,
                "SELECT name FROM sqlite_master WHERE type='table' AND name='user_config_tokens'"
                    .to_string(),
            ))
            .await?;
        if table_exists.is_empty() {
            return Ok(());
        }

        // --- 1) Normalize timestamps ---
        let token_rows = TokenRow::find_by_statement(Statement::from_string(
            DbBackend::Sqlite,
            "SELECT id, user_id, token, create_time, update_time FROM user_config_tokens"
                .to_string(),
        ))
        .all(db)
        .await?;

        for row in &token_rows {
            let mut create_time = row.create_time.clone();
            let mut update_time = row.update_time.clone();
            let mut changed = false;
            if needs_timezone_fix(&create_time) {
                create_time = to_rfc3339_utc(&create_time);
                changed = true;
            }
            if needs_timezone_fix(&update_time) {
                update_time = to_rfc3339_utc(&update_time);
                changed = true;
            }
            if changed {
                db.execute(Statement::from_sql_and_values(
                    DbBackend::Sqlite,
                    "UPDATE user_config_tokens SET create_time = ?, update_time = ? WHERE id = ?",
                    vec![create_time.into(), update_time.into(), row.id.into()],
                ))
                .await?;
            }
        }

        // --- 2) Replace invalid tokens with fresh et_* values ---
        let token_rows = TokenRow::find_by_statement(Statement::from_string(
            DbBackend::Sqlite,
            "SELECT id, user_id, token, create_time, update_time FROM user_config_tokens"
                .to_string(),
        ))
        .all(db)
        .await?;

        for row in token_rows {
            if validate_config_token_value(&row.token).is_ok() {
                continue;
            }
            let mut new_token = generate_config_token();
            // Ensure uniqueness against both tables.
            loop {
                let taken_tokens = db
                    .query_all(Statement::from_sql_and_values(
                        DbBackend::Sqlite,
                        "SELECT 1 AS x FROM user_config_tokens WHERE token = ? AND id != ? LIMIT 1",
                        vec![new_token.clone().into(), row.id.into()],
                    ))
                    .await?;
                let taken_users = db
                    .query_all(Statement::from_sql_and_values(
                        DbBackend::Sqlite,
                        "SELECT 1 AS x FROM users WHERE config_token = ? AND id != ? LIMIT 1",
                        vec![new_token.clone().into(), row.user_id.into()],
                    ))
                    .await?;
                if taken_tokens.is_empty() && taken_users.is_empty() {
                    break;
                }
                new_token = generate_config_token();
            }

            let old_token = row.token.clone();
            db.execute(Statement::from_sql_and_values(
                DbBackend::Sqlite,
                "UPDATE user_config_tokens SET token = ? WHERE id = ?",
                vec![new_token.clone().into(), row.id.into()],
            ))
            .await?;

            // Keep primary mirror in sync when it still pointed at the illegal value.
            db.execute(Statement::from_sql_and_values(
                DbBackend::Sqlite,
                "UPDATE users SET config_token = ? WHERE id = ? AND config_token = ?",
                vec![
                    new_token.clone().into(),
                    row.user_id.into(),
                    old_token.into(),
                ],
            ))
            .await?;
        }

        // --- 3) Fix invalid users.config_token that are not covered by a live row ---
        let users = UserTokenRow::find_by_statement(Statement::from_string(
            DbBackend::Sqlite,
            "SELECT id, config_token FROM users".to_string(),
        ))
        .all(db)
        .await?;

        for user in users {
            if validate_config_token_value(&user.config_token).is_ok() {
                continue;
            }
            // Prefer an existing valid token for this user.
            let existing = TokenRow::find_by_statement(Statement::from_sql_and_values(
                DbBackend::Sqlite,
                "SELECT id, user_id, token, create_time, update_time FROM user_config_tokens WHERE user_id = ? ORDER BY id DESC LIMIT 1",
                vec![user.id.into()],
            ))
            .all(db)
            .await?;

            if let Some(row) = existing
                .into_iter()
                .find(|r| validate_config_token_value(&r.token).is_ok())
            {
                db.execute(Statement::from_sql_and_values(
                    DbBackend::Sqlite,
                    "UPDATE users SET config_token = ? WHERE id = ?",
                    vec![row.token.into(), user.id.into()],
                ))
                .await?;
                continue;
            }

            let mut new_token = generate_config_token();
            loop {
                let taken = db
                    .query_all(Statement::from_sql_and_values(
                        DbBackend::Sqlite,
                        "SELECT 1 AS x FROM user_config_tokens WHERE token = ? UNION ALL SELECT 1 FROM users WHERE config_token = ? AND id != ? LIMIT 1",
                        vec![
                            new_token.clone().into(),
                            new_token.clone().into(),
                            user.id.into(),
                        ],
                    ))
                    .await?;
                if taken.is_empty() {
                    break;
                }
                new_token = generate_config_token();
            }

            let now = chrono::Utc::now().to_rfc3339();
            db.execute(Statement::from_sql_and_values(
                DbBackend::Sqlite,
                "UPDATE users SET config_token = ? WHERE id = ?",
                vec![new_token.clone().into(), user.id.into()],
            ))
            .await?;
            db.execute(Statement::from_sql_and_values(
                DbBackend::Sqlite,
                r#"INSERT INTO user_config_tokens (user_id, token, label, create_time, update_time)
                   SELECT ?, ?, 'repaired', ?, ?
                   WHERE NOT EXISTS (
                     SELECT 1 FROM user_config_tokens t WHERE t.token = ?
                   )"#,
                vec![
                    user.id.into(),
                    new_token.clone().into(),
                    now.clone().into(),
                    now.into(),
                    new_token.into(),
                ],
            ))
            .await?;
        }

        // --- 4) Re-seed any valid primary still missing from the multi-token table ---
        let now = chrono::Utc::now().to_rfc3339();
        db.execute(Statement::from_sql_and_values(
            DbBackend::Sqlite,
            r#"INSERT INTO user_config_tokens (user_id, token, label, create_time, update_time)
            SELECT
                id,
                config_token,
                'default',
                ?,
                ?
            FROM users
            WHERE config_token IS NOT NULL
              AND config_token != ''
              AND config_token NOT LIKE 'revoked_%'
              AND NOT EXISTS (
                  SELECT 1 FROM user_config_tokens t WHERE t.token = users.config_token
              );"#,
            vec![now.clone().into(), now.into()],
        ))
        .await?;

        Ok(())
    }

    async fn down(&self, _manager: &SchemaManager) -> Result<(), DbErr> {
        // Irreversible data repair.
        Ok(())
    }
}
