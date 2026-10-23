use sea_orm_migration::prelude::*;

pub struct Migration;

impl MigrationName for Migration {
    fn name(&self) -> &str {
        "m20261023_000014_flow_history_conn_id_text"
    }
}

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let db = manager.get_connection();

        // conn_id 曾经是 INTEGER（按 u64 解析 UUID 永远落 -1），现在改为 Text 存 UUID 字符串。
        // SQLite 不支持直接 ALTER COLUMN TYPE，需重建表。历史数据中的 conn_id 全为 -1，
        // 无语义，转为文本 "-1" 保留行结构即可。
        db.execute_unprepared(
            r#"
            CREATE TABLE flow_history_new (
                id INTEGER PRIMARY KEY AUTOINCREMENT NOT NULL,
                user_id INTEGER NOT NULL,
                machine_id TEXT NOT NULL,
                instance_id TEXT NOT NULL,
                conn_id TEXT NOT NULL,
                peer_id INTEGER NOT NULL,
                ip TEXT NOT NULL,
                prefix_len INTEGER NOT NULL,
                direction INTEGER NOT NULL,
                bytes INTEGER NOT NULL,
                packets INTEGER NOT NULL,
                rate_bytes_per_sec INTEGER NOT NULL,
                flow_count INTEGER NOT NULL,
                sampled INTEGER NOT NULL,
                sampled_at INTEGER NOT NULL
            );
            INSERT INTO flow_history_new (
                id, user_id, machine_id, instance_id, conn_id, peer_id, ip,
                prefix_len, direction, bytes, packets, rate_bytes_per_sec,
                flow_count, sampled, sampled_at
            )
            SELECT id, user_id, machine_id, instance_id, CAST(conn_id AS TEXT), peer_id, ip,
                   prefix_len, direction, bytes, packets, rate_bytes_per_sec,
                   flow_count, sampled, sampled_at
            FROM flow_history;
            DROP TABLE flow_history;
            ALTER TABLE flow_history_new RENAME TO flow_history;
            CREATE INDEX idx_flow_history_series
                ON flow_history(user_id, machine_id, instance_id, conn_id, ip, direction, sampled_at);
            CREATE INDEX idx_flow_history_sampled_at
                ON flow_history(sampled_at);
            "#,
        )
        .await?;

        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let db = manager.get_connection();

        db.execute_unprepared(
            r#"
            CREATE TABLE flow_history_new (
                id INTEGER PRIMARY KEY AUTOINCREMENT NOT NULL,
                user_id INTEGER NOT NULL,
                machine_id TEXT NOT NULL,
                instance_id TEXT NOT NULL,
                conn_id INTEGER NOT NULL,
                peer_id INTEGER NOT NULL,
                ip TEXT NOT NULL,
                prefix_len INTEGER NOT NULL,
                direction INTEGER NOT NULL,
                bytes INTEGER NOT NULL,
                packets INTEGER NOT NULL,
                rate_bytes_per_sec INTEGER NOT NULL,
                flow_count INTEGER NOT NULL,
                sampled INTEGER NOT NULL,
                sampled_at INTEGER NOT NULL
            );
            INSERT INTO flow_history_new (
                id, user_id, machine_id, instance_id, conn_id, peer_id, ip,
                prefix_len, direction, bytes, packets, rate_bytes_per_sec,
                flow_count, sampled, sampled_at
            )
            SELECT id, user_id, machine_id, instance_id, CAST(conn_id AS INTEGER), peer_id, ip,
                   prefix_len, direction, bytes, packets, rate_bytes_per_sec,
                   flow_count, sampled, sampled_at
            FROM flow_history;
            DROP TABLE flow_history;
            ALTER TABLE flow_history_new RENAME TO flow_history;
            CREATE INDEX idx_flow_history_series
                ON flow_history(user_id, machine_id, instance_id, conn_id, ip, direction, sampled_at);
            CREATE INDEX idx_flow_history_sampled_at
                ON flow_history(sampled_at);
            "#,
        )
        .await?;

        Ok(())
    }
}
