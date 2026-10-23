use sea_orm_migration::prelude::*;

pub struct Migration;

impl MigrationName for Migration {
    fn name(&self) -> &str {
        "m20261022_000013_flow_history"
    }
}

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        // 每行 = 一次采样里「某条隧道上的某个 IP/网段」的流量。
        //
        // - ip 默认是**掩码后的网段**（控制面只收掩码，见决策 D8）；prefix_len
        //   记录聚合位数，0 表示精确 IP（需要节点侧开启 allow_exact_ip 且调用方
        //   显式请求 EXACT 才会出现，本采样器不会请求）。
        // - bytes/packets 是流条目自创建以来的累计值，但流条目会在 TTL / 上限
        //   淘汰后重新计数，所以**不能**像 peer_conn_history 那样靠差分算速率；
        //   rate_bytes_per_sec 是节点侧 1s 固定窗口算好的速率，直接存。
        // - flow_count 是掩码合并了多少个精确 IP（掩码视图下有值，精确视图为 1），
        //   没有它就无法区分「一个 /24 里 14 台机器」和「一台机器」。
        manager
            .get_connection()
            .execute_unprepared(
                r#"
                CREATE TABLE flow_history (
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

                -- 时序查询主索引：按 (用户, 设备, 实例, 隧道, IP, 方向) 取时间区间
                CREATE INDEX idx_flow_history_series
                    ON flow_history(user_id, machine_id, instance_id, conn_id, ip, direction, sampled_at);

                -- 按时间清理用
                CREATE INDEX idx_flow_history_sampled_at
                    ON flow_history(sampled_at);
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
                DROP INDEX IF EXISTS idx_flow_history_series;
                DROP INDEX IF EXISTS idx_flow_history_sampled_at;
                DROP TABLE flow_history;
                "#,
            )
            .await?;
        Ok(())
    }
}
