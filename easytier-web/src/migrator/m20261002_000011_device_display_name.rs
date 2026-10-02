use sea_orm_migration::prelude::*;

pub struct Migration;

impl MigrationName for Migration {
    fn name(&self) -> &str {
        "m20261002_000011_device_display_name"
    }
}

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let db = manager.get_connection();
        db.execute_unprepared(
            r#"
            ALTER TABLE devices ADD COLUMN display_name TEXT NOT NULL DEFAULT '';
            "#,
        )
        .await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let db = manager.get_connection();
        // SQLite cannot DROP COLUMN on older versions; rebuild table without display_name.
        db.execute_unprepared(
            r#"
            CREATE TABLE devices_old AS SELECT
                id, user_id, device_id, hostname, last_easytier_version,
                last_client_url, last_seen_at, create_time, update_time
            FROM devices;

            DROP TABLE devices;

            CREATE TABLE devices (
                id INTEGER PRIMARY KEY AUTOINCREMENT NOT NULL,
                user_id INTEGER NOT NULL,
                device_id TEXT NOT NULL,
                hostname TEXT NOT NULL DEFAULT '',
                last_easytier_version TEXT NOT NULL DEFAULT '',
                last_client_url TEXT NOT NULL DEFAULT '',
                last_seen_at INTEGER NOT NULL DEFAULT 0,
                create_time TEXT NOT NULL,
                update_time TEXT NOT NULL,
                CONSTRAINT fk_devices_user_id_to_users_id
                    FOREIGN KEY (user_id) REFERENCES users(id)
                    ON DELETE CASCADE
                    ON UPDATE CASCADE
            );

            INSERT INTO devices (
                id, user_id, device_id, hostname, last_easytier_version,
                last_client_url, last_seen_at, create_time, update_time
            )
            SELECT
                id, user_id, device_id, hostname, last_easytier_version,
                last_client_url, last_seen_at, create_time, update_time
            FROM devices_old;

            DROP TABLE devices_old;

            CREATE UNIQUE INDEX IF NOT EXISTS idx_devices_user_device
                ON devices(user_id, device_id);
            CREATE INDEX IF NOT EXISTS idx_devices_user_id ON devices(user_id);
            "#,
        )
        .await?;
        Ok(())
    }
}
