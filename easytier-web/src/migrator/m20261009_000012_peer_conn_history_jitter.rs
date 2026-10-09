use sea_orm_migration::prelude::*;

pub struct Migration;

impl MigrationName for Migration {
    fn name(&self) -> &str {
        "m20261009_000012_peer_conn_history_jitter"
    }
}

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        // -1 = unknown (same convention as latency_us / loss_rate for history samples)
        manager
            .get_connection()
            .execute_unprepared(
                r#"
                ALTER TABLE peer_conn_history
                    ADD COLUMN jitter_us INTEGER NOT NULL DEFAULT -1;
                "#,
            )
            .await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        // SQLite may not support DROP COLUMN; rebuild without jitter_us.
        manager
            .get_connection()
            .execute_unprepared(
                r#"
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
                    sampled_at INTEGER NOT NULL,
                    CONSTRAINT fk_peer_conn_history_user_id_to_users_id
                        FOREIGN KEY (user_id) REFERENCES users(id)
                        ON DELETE CASCADE
                        ON UPDATE CASCADE
                );

                INSERT INTO peer_conn_history_old (
                    id, user_id, machine_id, instance_id, peer_id, peer_hostname,
                    remote_addr, tunnel_type, latency_us, loss_rate, rx_bytes,
                    tx_bytes, conn_count, sampled_at
                )
                SELECT
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
                "#,
            )
            .await?;
        Ok(())
    }
}
