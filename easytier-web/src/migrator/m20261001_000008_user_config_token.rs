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
        // Existing rows keep username as token for compatibility; new users get random tokens.
        db.execute_unprepared(
            r#"
            ALTER TABLE users ADD COLUMN config_token TEXT NOT NULL DEFAULT '';
            UPDATE users SET config_token = username WHERE config_token = '' OR config_token IS NULL;
            CREATE UNIQUE INDEX IF NOT EXISTS idx_users_config_token ON users(config_token);
            "#,
        )
        .await?;

        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let db = manager.get_connection();
        db.execute_unprepared(
            r#"
            DROP INDEX IF EXISTS idx_users_config_token;
            -- SQLite cannot DROP COLUMN on older versions; leave column in place on down.
            "#,
        )
        .await?;
        Ok(())
    }
}
