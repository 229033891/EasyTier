use sea_orm_migration::prelude::*;

pub struct Migration;

impl MigrationName for Migration {
    fn name(&self) -> &str {
        "m20261030_000015_device_local_edit_locked"
    }
}

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let db = manager.get_connection();
        // Machine-level admin switch: when 1, the console rejects client-side
        // edits/pushes of web-owned configs for this device. Default 0 keeps
        // pre-existing behaviour (client may edit and report web-owned rows).
        db.execute_unprepared(
            r#"
            ALTER TABLE devices ADD COLUMN local_edit_locked INTEGER NOT NULL DEFAULT 0;
            "#,
        )
        .await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let db = manager.get_connection();
        // SQLite cannot DROP COLUMN on older versions; rebuild table without the column.
        db.execute_unprepared(
            r#"
            CREATE TABLE devices_old AS SELECT
                id, user_id, device_id, hostname, display_name, last_easytier_version,
                last_client_url, last_seen_at, create_time, update_time
            FROM devices;

            DROP TABLE devices;

            CREATE TABLE devices (
                id INTEGER PRIMARY KEY AUTOINCREMENT NOT NULL,
                user_id INTEGER NOT NULL,
                device_id TEXT NOT NULL,
                hostname TEXT NOT NULL DEFAULT '',
                display_name TEXT NOT NULL DEFAULT '',
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
                id, user_id, device_id, hostname, display_name, last_easytier_version,
                last_client_url, last_seen_at, create_time, update_time
            )
            SELECT
                id, user_id, device_id, hostname, display_name, last_easytier_version,
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
