//! Dedicated `users.config_token` column (was previously username-as-token).
//!
//! **Do not edit this migration after it may have been applied.** Further data
//! fixes belong in later migrations (see `m20261001_000010_repair_config_tokens`).

use sea_orm_migration::prelude::*;

pub struct Migration;

impl MigrationName for Migration {
    fn name(&self) -> &str {
        "m20261001_000008_user_config_token"
    }
}

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let db = manager.get_connection();

        // Dedicated config-server auth token (was previously username-as-token).
        // Each statement runs in its own exec: some drivers reject multi-statement strings.
        db.execute_unprepared("ALTER TABLE users ADD COLUMN config_token TEXT NOT NULL DEFAULT '';")
            .await?;

        // Only carry over usernames that are already valid tokens
        // ([A-Za-z0-9._-], 1..=128 chars); the rest get random tokens below
        // so every row stays authenticatable and later updates pass validation.
        db.execute_unprepared(
            r#"UPDATE users SET config_token = username
            WHERE (config_token = '' OR config_token IS NULL)
              AND username != ''
              AND length(username) <= 128
              AND username NOT GLOB '*[^A-Za-z0-9._-]*';"#,
        )
        .await?;

        // Rows with illegal usernames (or empty) get a legal random token.
        // hex() output is [0-9a-f], always valid in URL paths; 128-bit entropy
        // makes collisions negligible (UNIQUE index is the final guard).
        db.execute_unprepared(
            r#"UPDATE users SET config_token = 'et_' || hex(randomblob(16))
            WHERE config_token = '' OR config_token IS NULL;"#,
        )
        .await?;

        db.execute_unprepared(
            "CREATE UNIQUE INDEX IF NOT EXISTS idx_users_config_token ON users(config_token);",
        )
        .await?;

        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let db = manager.get_connection();
        db.execute_unprepared("DROP INDEX IF EXISTS idx_users_config_token;")
            .await?;
        // Modern SQLite supports DROP COLUMN; mirror `up` symmetrically.
        db.execute_unprepared("ALTER TABLE users DROP COLUMN config_token;")
            .await?;
        Ok(())
    }
}
