use sea_orm_migration::prelude::*;

pub struct Migration;

impl MigrationName for Migration {
    fn name(&self) -> &str {
        "m20260930_000007_devices_networks_hardening"
    }
}

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let db = manager.get_connection();

        db.execute_unprepared(
            r#"
            -- Device archive (per user)
            CREATE TABLE IF NOT EXISTS devices (
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
            CREATE UNIQUE INDEX IF NOT EXISTS idx_devices_user_device
                ON devices(user_id, device_id);
            CREATE INDEX IF NOT EXISTS idx_devices_user_id ON devices(user_id);

            -- Network credential directory (per user, keyed by network_name)
            CREATE TABLE IF NOT EXISTS networks (
                id INTEGER PRIMARY KEY AUTOINCREMENT NOT NULL,
                user_id INTEGER NOT NULL,
                network_name TEXT NOT NULL,
                network_secret TEXT NOT NULL DEFAULT '',
                network_secret_digest TEXT NOT NULL DEFAULT '',
                create_time TEXT NOT NULL,
                update_time TEXT NOT NULL,
                CONSTRAINT fk_networks_user_id_to_users_id
                    FOREIGN KEY (user_id) REFERENCES users(id)
                    ON DELETE CASCADE
                    ON UPDATE CASCADE
            );
            CREATE UNIQUE INDEX IF NOT EXISTS idx_networks_user_name
                ON networks(user_id, network_name);
            CREATE INDEX IF NOT EXISTS idx_networks_user_id ON networks(user_id);

            -- Junction uniqueness
            CREATE UNIQUE INDEX IF NOT EXISTS idx_users_groups_user_group
                ON users_groups(user_id, group_id);
            CREATE UNIQUE INDEX IF NOT EXISTS idx_groups_permissions_group_perm
                ON groups_permissions(group_id, permission_id);

            -- Add network_name denormalized column on configs
            CREATE TABLE user_running_network_configs_new (
                id INTEGER PRIMARY KEY AUTOINCREMENT NOT NULL,
                user_id INTEGER NOT NULL,
                device_id TEXT NOT NULL,
                network_instance_id TEXT NOT NULL,
                network_config TEXT NOT NULL,
                network_name TEXT NOT NULL DEFAULT '',
                source TEXT NOT NULL DEFAULT 'user',
                disabled BOOLEAN NOT NULL DEFAULT FALSE,
                create_time TEXT NOT NULL,
                update_time TEXT NOT NULL,
                CONSTRAINT fk_user_running_network_configs_user_id_to_users_id
                    FOREIGN KEY (user_id) REFERENCES users(id)
                    ON DELETE CASCADE
                    ON UPDATE CASCADE
            );

            INSERT INTO user_running_network_configs_new (
                id, user_id, device_id, network_instance_id, network_config,
                network_name, source, disabled, create_time, update_time
            )
            SELECT
                id, user_id, device_id, network_instance_id, network_config,
                COALESCE(json_extract(network_config, '$.network_name'), ''),
                source, disabled, create_time, update_time
            FROM user_running_network_configs;

            DROP TABLE user_running_network_configs;
            ALTER TABLE user_running_network_configs_new RENAME TO user_running_network_configs;

            CREATE INDEX idx_user_running_network_configs_user_id
                ON user_running_network_configs(user_id);
            CREATE UNIQUE INDEX idx_user_running_network_configs_scope_inst
                ON user_running_network_configs(user_id, device_id, network_instance_id);
            CREATE INDEX idx_user_running_network_configs_user_name
                ON user_running_network_configs(user_id, network_name);

            -- Seed networks directory from existing configs (latest update_time wins)
            INSERT INTO networks (
                user_id, network_name, network_secret, network_secret_digest,
                create_time, update_time
            )
            SELECT
                user_id,
                network_name,
                network_secret,
                '',
                create_time,
                update_time
            FROM (
                SELECT
                    user_id,
                    network_name,
                    COALESCE(json_extract(network_config, '$.network_secret'), '') AS network_secret,
                    create_time,
                    update_time,
                    ROW_NUMBER() OVER (
                        PARTITION BY user_id, network_name
                        ORDER BY update_time DESC, id DESC
                    ) AS rn
                FROM user_running_network_configs
                WHERE network_name != ''
            )
            WHERE rn = 1;

            -- Rebuild peer_conn_history with FK to users
            CREATE TABLE peer_conn_history_new (
                id INTEGER PRIMARY KEY AUTOINCREMENT NOT NULL,
                user_id INTEGER NOT NULL,
                machine_id TEXT NOT NULL,
                instance_id TEXT NOT NULL,
                peer_id INTEGER NOT NULL,
                peer_hostname TEXT NOT NULL,
                remote_addr TEXT NOT NULL,
                tunnel_type TEXT NOT NULL,
                latency_us INTEGER NOT NULL,
                loss_rate REAL NOT NULL,
                rx_bytes INTEGER NOT NULL,
                tx_bytes INTEGER NOT NULL,
                conn_count INTEGER NOT NULL,
                sampled_at INTEGER NOT NULL,
                CONSTRAINT fk_peer_conn_history_user_id_to_users_id
                    FOREIGN KEY (user_id) REFERENCES users(id)
                    ON DELETE CASCADE
                    ON UPDATE CASCADE
            );

            INSERT INTO peer_conn_history_new (
                id, user_id, machine_id, instance_id, peer_id, peer_hostname,
                remote_addr, tunnel_type, latency_us, loss_rate, rx_bytes,
                tx_bytes, conn_count, sampled_at
            )
            SELECT
                id, user_id, machine_id, instance_id, peer_id, peer_hostname,
                remote_addr, tunnel_type, latency_us, loss_rate, rx_bytes,
                tx_bytes, conn_count, sampled_at
            FROM peer_conn_history
            WHERE user_id IN (SELECT id FROM users);

            DROP TABLE peer_conn_history;
            ALTER TABLE peer_conn_history_new RENAME TO peer_conn_history;

            CREATE INDEX idx_peer_conn_history_series
                ON peer_conn_history(user_id, machine_id, instance_id, peer_id, sampled_at);
            CREATE INDEX idx_peer_conn_history_sampled_at
                ON peer_conn_history(sampled_at);
            "#,
        )
        .await?;

        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let db = manager.get_connection();

        db.execute_unprepared(
            r#"
            DROP INDEX IF EXISTS idx_users_groups_user_group;
            DROP INDEX IF EXISTS idx_groups_permissions_group_perm;

            CREATE TABLE peer_conn_history_old (
                id INTEGER PRIMARY KEY AUTOINCREMENT NOT NULL,
                user_id INTEGER NOT NULL,
                machine_id TEXT NOT NULL,
                instance_id TEXT NOT NULL,
                peer_id INTEGER NOT NULL,
                peer_hostname TEXT NOT NULL,
                remote_addr TEXT NOT NULL,
                tunnel_type TEXT NOT NULL,
                latency_us INTEGER NOT NULL,
                loss_rate REAL NOT NULL,
                rx_bytes INTEGER NOT NULL,
                tx_bytes INTEGER NOT NULL,
                conn_count INTEGER NOT NULL,
                sampled_at INTEGER NOT NULL
            );
            INSERT INTO peer_conn_history_old SELECT
                id, user_id, machine_id, instance_id, peer_id, peer_hostname,
                remote_addr, tunnel_type, latency_us, loss_rate, rx_bytes,
                tx_bytes, conn_count, sampled_at
            FROM peer_conn_history;
            DROP TABLE peer_conn_history;
            ALTER TABLE peer_conn_history_old RENAME TO peer_conn_history;
            CREATE INDEX idx_peer_conn_history_series
                ON peer_conn_history(user_id, machine_id, instance_id, peer_id, sampled_at);
            CREATE INDEX idx_peer_conn_history_sampled_at
                ON peer_conn_history(sampled_at);

            CREATE TABLE user_running_network_configs_old (
                id INTEGER PRIMARY KEY AUTOINCREMENT NOT NULL,
                user_id INTEGER NOT NULL,
                device_id TEXT NOT NULL,
                network_instance_id TEXT NOT NULL,
                network_config TEXT NOT NULL,
                source TEXT NOT NULL DEFAULT 'user',
                disabled BOOLEAN NOT NULL DEFAULT FALSE,
                create_time TEXT NOT NULL,
                update_time TEXT NOT NULL,
                CONSTRAINT fk_user_running_network_configs_user_id_to_users_id
                    FOREIGN KEY (user_id) REFERENCES users(id)
                    ON DELETE CASCADE
                    ON UPDATE CASCADE
            );
            INSERT INTO user_running_network_configs_old (
                id, user_id, device_id, network_instance_id, network_config,
                source, disabled, create_time, update_time
            )
            SELECT
                id, user_id, device_id, network_instance_id, network_config,
                source, disabled, create_time, update_time
            FROM user_running_network_configs;
            DROP TABLE user_running_network_configs;
            ALTER TABLE user_running_network_configs_old RENAME TO user_running_network_configs;
            CREATE INDEX idx_user_running_network_configs_user_id
                ON user_running_network_configs(user_id);
            CREATE UNIQUE INDEX idx_user_running_network_configs_scope_inst
                ON user_running_network_configs(user_id, device_id, network_instance_id);

            DROP TABLE IF EXISTS networks;
            DROP TABLE IF EXISTS devices;
            "#,
        )
        .await?;

        Ok(())
    }
}
