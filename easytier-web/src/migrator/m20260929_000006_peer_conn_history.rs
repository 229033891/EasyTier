use sea_orm_migration::prelude::*;

pub struct Migration;

impl MigrationName for Migration {
    fn name(&self) -> &str {
        "m20260929_000006_peer_conn_history"
    }
}

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        // sampled_at 存 unix 秒（INTEGER），方便范围过滤与按桶聚合，
        // 也避开 SQLite 解析 RFC3339 文本的各种坑。
        manager
            .get_connection()
            .execute_unprepared(
                r#"
                CREATE TABLE peer_conn_history (
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

                -- 时序查询主索引：按 (用户, 设备, 实例, peer) 取时间区间
                CREATE INDEX idx_peer_conn_history_series
                    ON peer_conn_history(user_id, machine_id, instance_id, peer_id, sampled_at);

                -- 按时间清理用
                CREATE INDEX idx_peer_conn_history_sampled_at
                    ON peer_conn_history(sampled_at);
                "#,
            )
            .await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared(
                r#"
                DROP INDEX IF EXISTS idx_peer_conn_history_series;
                DROP INDEX IF EXISTS idx_peer_conn_history_sampled_at;
                DROP TABLE peer_conn_history;
                "#,
            )
            .await?;
        Ok(())
    }
}
