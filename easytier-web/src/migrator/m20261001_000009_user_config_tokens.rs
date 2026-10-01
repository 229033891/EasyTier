use sea_orm_migration::prelude::*;
use sea_orm::{DbBackend, Statement};

pub struct Migration;

impl MigrationName for Migration {
    fn name(&self) -> &str {
        "m20261001_000009_user_config_tokens"
    }
}

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let db = manager.get_connection();
        db.execute_unprepared(
            r#"
            -- Multiple config-server URL tokens per user.
            -- Client URL: udp://host:port/<token>  (e.g. .../admin)
            CREATE TABLE IF NOT EXISTS user_config_tokens (
                id INTEGER PRIMARY KEY AUTOINCREMENT NOT NULL,
                user_id INTEGER NOT NULL,
                token TEXT NOT NULL,
                label TEXT NOT NULL DEFAULT '',
                create_time TEXT NOT NULL,
                update_time TEXT NOT NULL,
                CONSTRAINT fk_user_config_tokens_user_id_to_users_id
                    FOREIGN KEY (user_id) REFERENCES users(id)
                    ON DELETE CASCADE
                    ON UPDATE CASCADE
            );
            "#,
        )
        .await?;
        db.execute_unprepared(
            "CREATE UNIQUE INDEX IF NOT EXISTS idx_user_config_tokens_token ON user_config_tokens(token);",
        )
        .await?;
        db.execute_unprepared(
            "CREATE INDEX IF NOT EXISTS idx_user_config_tokens_user_id ON user_config_tokens(user_id);",
        )
        .await?;

        // Seed from existing users.config_token (compat with prior single-token design).
        // Timestamps are generated on the Rust side as RFC3339 with timezone so they
        // parse back into the DateTimeWithTimeZone entity fields; bare
        // datetime('now') ('YYYY-MM-DD HH:MM:SS', no offset) does not.
        let now = chrono::Local::now().fixed_offset().to_rfc3339();
        let seed = Statement::from_sql_and_values(
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
              AND NOT EXISTS (
                  SELECT 1 FROM user_config_tokens t WHERE t.token = users.config_token
              );"#,
            vec![now.clone().into(), now.into()],
        );
        db.execute(seed).await?;

        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let db = manager.get_connection();
        db.execute_unprepared("DROP TABLE IF EXISTS user_config_tokens;")
            .await?;
        Ok(())
    }
}
