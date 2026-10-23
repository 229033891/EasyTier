//! 隧道级 IP 流量历史查询接口。
//!
//! 数据由 [`crate::peer_history`] 采样器在同一个 `CollectNetworkInfo` 里顺带写入
//! （不额外发 RPC），这里只读并按桶聚合：
//! - `rate_bytes_per_sec` 取桶内**均值**：节点侧已经算好了固定窗口速率，
//!   这里不做差分（流条目 TTL 淘汰后会重新计数，差分不可靠）
//! - `bytes` / `packets` 是流条目生命周期内的累计值，取桶内最大值，仅作参考
//! - `ip` 默认是掩码网段；`prefix_len > 0` 时前端应显示成 `10.0.0.0/24`
//! - `bucket_seconds` 与 peer 历史共用同一套算法，保证最多约 720 个点
//!
//! 返回前按窗口内总流量排序并截断到 [`MAX_SERIES`] 条曲线：一个开了流量统计
//! 的节点可能有上千个流条目，全量下发会把浏览器拖死。

use std::collections::HashMap;

use axum::extract::{Path, Query, State};
use axum::{Json, Router, routing::get};
use axum_login::AuthUser;
use sea_orm::{ConnectionTrait as _, DbBackend, Statement, Value};

use crate::db::UserIdInDb;

use super::peer_history::bucket_seconds;
use super::users::AuthSession;
use super::{AppState, AppStateInner, HttpHandleError, other_error};

/// 最多返回的曲线数（每条曲线 = 一条隧道上的一个 IP/方向）
const MAX_SERIES: usize = 40;
/// 时间跨度上限：7 天（与采样器默认保留期一致）
const MAX_HOURS: i64 = 24 * 7;

#[derive(Debug, serde::Deserialize)]
struct FlowHistoryQuery {
    hours: Option<i64>,
}

#[derive(Clone, Debug, serde::Serialize)]
struct FlowHistoryPoint {
    /// 桶起始时间（unix 秒）
    t: i64,
    /// 桶内平均速率（字节/秒）
    rate_bytes_per_sec: f64,
    /// 桶内累计计数器最大值（流条目生命周期内）
    bytes: i64,
    /// 桶内 `flow_count` 最大值（掩码视图下合并了几个精确 IP）
    flow_count: i64,
    /// 桶内是否包含采样估算值
    sampled: bool,
    /// 桶内样本数
    samples: i64,
}

#[derive(Clone, Debug, serde::Serialize)]
struct FlowHistorySeries {
    conn_id: String,
    peer_id: i64,
    /// 对端主机名（同窗口内从 peer_conn_history 关联；拿不到时为空串）
    peer_hostname: String,
    ip: String,
    prefix_len: i32,
    /// 0 = tx（目的 IP），1 = rx（源 IP）
    direction: i32,
    /// 最近一次采样时间（unix 秒）
    last_seen: i64,
    points: Vec<FlowHistoryPoint>,
}

#[derive(Debug, serde::Serialize)]
struct FlowHistoryResponse {
    /// 聚合桶大小（秒）
    bucket_seconds: i64,
    /// 查询窗口起点（unix 秒）
    from: i64,
    /// 查询窗口终点（unix 秒）
    to: i64,
    /// 是否因 MAX_SERIES 截断
    truncated: bool,
    flows: Vec<FlowHistorySeries>,
}

/// 按 (conn_id, peer_id, ip, prefix_len, direction) + 时间桶聚合。
/// 绑定参数依次为：bucket、bucket、user_id、machine_id、instance_id、from、to。
///
/// `rate_bytes_per_sec` 取均值而不是差分：节点侧的 1s 窗口是唯一真相（D11）。
///
/// 上界 `sampled_at <= ?` 是必需的：节点时钟超前时写入的行会「来自未来」，
/// 没有上界的话它们会落进**每一次**查询的任何窗口，把曲线整体抬高（F-2）。
const SERIES_SQL: &str = r#"
    SELECT conn_id,
           peer_id,
           ip,
           prefix_len,
           direction,
           (sampled_at / ?) * ? AS bucket_ts,
           AVG(rate_bytes_per_sec) AS rate_bytes_per_sec,
           MAX(bytes) AS bytes,
           MAX(flow_count) AS flow_count,
           MAX(sampled) AS sampled,
           COUNT(*) AS samples
    FROM flow_history
    WHERE user_id = ? AND machine_id = ? AND instance_id = ? AND sampled_at >= ?
          AND sampled_at <= ?
    GROUP BY conn_id, peer_id, ip, prefix_len, direction, bucket_ts
    ORDER BY conn_id, peer_id, ip, prefix_len, direction, bucket_ts
"#;

/// 每个 peer 最近一次采样里的主机名（用于给曲线打标签）。
/// 绑定参数依次为：user_id、machine_id、instance_id、from、to。
///
/// 用窗口函数取「每个 peer 采样时间最新、同秒再按 id 最大」的那一行，
/// 避免 `GROUP BY peer_id` + 裸选 `peer_hostname` 在非 SQLite / 多行同秒时取错主机名。
///
/// 与 [`SERIES_SQL`] 同因加上界：时钟超前的行不该被当成「最新主机名」（F-2）。
const PEER_HOSTNAME_SQL: &str = r#"
    SELECT peer_id, peer_hostname, sampled_at AS last_seen
    FROM (
        SELECT peer_id,
               peer_hostname,
               sampled_at,
               ROW_NUMBER() OVER (
                   PARTITION BY peer_id
                   ORDER BY sampled_at DESC, id DESC
               ) AS rn
        FROM peer_conn_history
        WHERE user_id = ? AND machine_id = ? AND instance_id = ? AND sampled_at >= ?
              AND sampled_at <= ?
    )
    WHERE rn = 1
"#;

pub struct FlowHistoryApi;

impl FlowHistoryApi {
    fn get_user_id(auth_session: &AuthSession) -> Result<UserIdInDb, HttpHandleError> {
        let Some(user_id) = auth_session.user.as_ref().map(|x| x.id()) else {
            return Err((
                axum::http::StatusCode::UNAUTHORIZED,
                other_error("No user id found".to_string()).into(),
            ));
        };
        Ok(user_id)
    }

    async fn handle_flow_history(
        auth_session: AuthSession,
        State(client_mgr): AppState,
        Path((machine_id, inst_id)): Path<(uuid::Uuid, uuid::Uuid)>,
        Query(params): Query<FlowHistoryQuery>,
    ) -> Result<Json<FlowHistoryResponse>, HttpHandleError> {
        let user_id = Self::get_user_id(&auth_session)?;

        let hours = params.hours.unwrap_or(24).clamp(1, MAX_HOURS);
        let bucket = bucket_seconds(hours);
        let now = chrono::Utc::now().timestamp();
        let from = now - hours * 3600;

        let db = client_mgr.db().orm_db();
        let machine_id = machine_id.to_string();
        let inst_id = inst_id.to_string();

        let rows = db
            .query_all(Statement::from_sql_and_values(
                DbBackend::Sqlite,
                SERIES_SQL,
                vec![
                    Value::from(bucket),
                    Value::from(bucket),
                    Value::from(user_id),
                    Value::from(machine_id.clone()),
                    Value::from(inst_id.clone()),
                    Value::from(from),
                    Value::from(now),
                ],
            ))
            .await
            .map_err(super::convert_db_error)?;

        // 系列键：conn_id + peer_id + ip + prefix_len + direction。peer_id 必不可少：
        // 同一 conn_id 在不同 peer 上（或历史数据里 conn_id 缺失/复用时）必须分开成
        // 不同曲线；prefix_len 则区分精确 IP 与不同掩码级别的网段，否则会被错误合并。
        type SeriesKey = (String, i64, String, i32, i32);
        let mut series: Vec<FlowHistorySeries> = Vec::new();
        let mut index: HashMap<SeriesKey, usize> = HashMap::new();
        // 排序口径：窗口内总字节数 = Σ(桶内平均速率 × 桶秒数)。
        // 不能用 Σ MAX(bytes)：cumulative 计数在多个桶里重复累加，等于按“流活了多久”排序。
        let mut totals: Vec<f64> = Vec::new();

        for row in rows {
            let conn_id: String = row
                .try_get("", "conn_id")
                .map_err(super::convert_db_error)?;
            let peer_id: i64 = row
                .try_get("", "peer_id")
                .map_err(super::convert_db_error)?;
            let ip: String = row.try_get("", "ip").map_err(super::convert_db_error)?;
            let prefix_len: i32 = row.try_get("", "prefix_len").unwrap_or(0);
            let direction: i32 = row.try_get("", "direction").unwrap_or(0);
            let bytes: i64 = row.try_get("", "bytes").unwrap_or(0);

            let point = FlowHistoryPoint {
                t: row
                    .try_get("", "bucket_ts")
                    .map_err(super::convert_db_error)?,
                rate_bytes_per_sec: row.try_get("", "rate_bytes_per_sec").unwrap_or(0.0),
                bytes,
                flow_count: row.try_get("", "flow_count").unwrap_or(1),
                sampled: row.try_get("", "sampled").unwrap_or(false),
                samples: row.try_get("", "samples").unwrap_or(0),
            };

            let key: SeriesKey = (conn_id.clone(), peer_id, ip.clone(), prefix_len, direction);
            let idx = match index.get(&key) {
                Some(idx) => *idx,
                None => {
                    series.push(FlowHistorySeries {
                        conn_id,
                        peer_id,
                        peer_hostname: String::new(),
                        ip,
                        prefix_len,
                        direction,
                        last_seen: 0,
                        points: Vec::new(),
                    });
                    totals.push(0.0);
                    index.insert(key, series.len() - 1);
                    series.len() - 1
                }
            };
            totals[idx] += point.rate_bytes_per_sec * bucket as f64;
            series[idx].last_seen = series[idx].last_seen.max(point.t);
            series[idx].points.push(point);
        }

        // 主机名：同窗口内 peer_conn_history 的最近一次采样
        if !series.is_empty() {
            let meta_rows = db
                .query_all(Statement::from_sql_and_values(
                    DbBackend::Sqlite,
                    PEER_HOSTNAME_SQL,
                    vec![
                        Value::from(user_id),
                        Value::from(machine_id),
                        Value::from(inst_id),
                        Value::from(from),
                        Value::from(now),
                    ],
                ))
                .await
                .map_err(super::convert_db_error)?;
            let mut names: HashMap<i64, String> = HashMap::new();
            for row in meta_rows {
                let peer_id: i64 = row
                    .try_get("", "peer_id")
                    .map_err(super::convert_db_error)?;
                names.insert(
                    peer_id,
                    row.try_get("", "peer_hostname").unwrap_or_default(),
                );
            }
            for item in series.iter_mut() {
                if let Some(name) = names.get(&item.peer_id) {
                    item.peer_hostname = name.clone();
                }
            }
        }

        // 总流量降序，截断到 MAX_SERIES，保证响应体可控
        let mut order: Vec<usize> = (0..series.len()).collect();
        order.sort_by(|&a, &b| {
            totals[b]
                .partial_cmp(&totals[a])
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        let truncated = order.len() > MAX_SERIES;
        order.truncate(MAX_SERIES);
        let kept: Vec<FlowHistorySeries> = order.iter().map(|&i| series[i].clone()).collect();

        Ok(Json(FlowHistoryResponse {
            bucket_seconds: bucket,
            from,
            to: now,
            truncated,
            flows: kept,
        }))
    }

    pub fn build_route() -> Router<AppStateInner> {
        // 与 peer-history 一样避开 /networks/{inst-id}/ 前缀，防止 matchit 歧义。
        Router::new().route(
            "/api/v1/machines/{machine-id}/flow-history/{inst-id}",
            get(Self::handle_flow_history),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::{Db, entity::flow_history};
    use sea_orm::{EntityTrait as _, Set};

    /// 与 60s 边界对齐，桶边界才会是 T0 / T0+60 / ...
    const T0: i64 = 1_700_000_040;

    #[allow(clippy::too_many_arguments)]
    fn row(
        conn_id: &str,
        peer_id: i64,
        ip: &str,
        direction: i32,
        bytes: i64,
        rate: i64,
        flow_count: i32,
        sampled_at: i64,
    ) -> flow_history::ActiveModel {
        flow_history::ActiveModel {
            user_id: Set(1),
            machine_id: Set("m-1".to_string()),
            instance_id: Set("i-1".to_string()),
            conn_id: Set(conn_id.to_string()),
            peer_id: Set(peer_id),
            ip: Set(ip.to_string()),
            prefix_len: Set(24),
            direction: Set(direction),
            bytes: Set(bytes),
            packets: Set(1),
            rate_bytes_per_sec: Set(rate),
            flow_count: Set(flow_count),
            sampled: Set(false),
            sampled_at: Set(sampled_at),
            ..Default::default()
        }
    }

    async fn seed(db: &Db) {
        let rows = vec![
            // 隧道 42 上收流量：两条采样
            row("42", 7, "10.0.0.0", 1, 1_000, 100, 14, T0),
            row("42", 7, "10.0.0.0", 1, 2_000, 300, 15, T0 + 60),
            // 同一隧道出流量
            row("42", 7, "10.0.2.0", 0, 500, 50, 1, T0),
            // 另一条隧道（同 peer 的第二条链路）
            row("7", 7, "10.0.9.0", 1, 1, 1, 2, T0),
        ];
        flow_history::Entity::insert_many(rows)
            .exec(db.orm_db())
            .await
            .expect("seed flow_history");
    }

    async fn run_series(
        db: &Db,
        bucket: i64,
        to: i64,
    ) -> Vec<(i64, String, i32, i64, f64, i64, i64)> {
        let rows = db
            .orm_db()
            .query_all(Statement::from_sql_and_values(
                DbBackend::Sqlite,
                SERIES_SQL,
                vec![
                    Value::from(bucket),
                    Value::from(bucket),
                    Value::from(1),
                    Value::from("m-1"),
                    Value::from("i-1"),
                    Value::from(T0),
                    Value::from(to),
                ],
            ))
            .await
            .expect("run flow series query");

        rows.iter()
            .map(|r| {
                (
                    r.try_get::<String>("", "conn_id").unwrap(),
                    r.try_get::<String>("", "ip").unwrap(),
                    r.try_get::<i32>("", "direction").unwrap(),
                    r.try_get::<i64>("", "bytes").unwrap(),
                    r.try_get::<f64>("", "rate_bytes_per_sec").unwrap(),
                    r.try_get::<i64>("", "flow_count").unwrap(),
                    r.try_get::<i64>("", "samples").unwrap(),
                )
            })
            .collect()
    }

    #[test]
    fn bucket_seconds_is_shared_with_peer_history() {
        // 与 peer 历史同一口径：1h 最少 60s，24h 是 120s
        assert_eq!(bucket_seconds(1), 60);
        assert_eq!(bucket_seconds(24), 120);
    }

    #[tokio::test]
    async fn series_keeps_tunnels_apart_and_averages_rate() {
        let db = Db::memory_db().await;
        seed(&db).await;

        let rows = run_series(&db, 120, T0 + 600).await;
        // 3 条系列 × 1 个桶（两条 60s 采样落进同一 120s 桶）
        assert_eq!(rows.len(), 3);

        let rx = rows
            .iter()
            .find(|r| r.0 == "42" && r.1 == "10.0.0.0" && r.2 == 1)
            .expect("tunnel 42 rx series");
        assert!((rx.4 - 200.0).abs() < 1e-6, "rate 取桶内均值 (100+300)/2");
        assert_eq!(rx.3, 2_000, "累计值取桶内 MAX");
        assert_eq!(rx.5, 15, "flow_count 取桶内 MAX");
        assert_eq!(rx.6, 2, "两条采样合进一个桶");

        // 同 peer 的另一条隧道不能被合并进 conn "42"
        assert!(rows.iter().any(|r| r.0 == "7" && r.1 == "10.0.9.0"));
    }

    #[tokio::test]
    async fn series_filters_by_user_machine_and_window() {
        let db = Db::memory_db().await;
        seed(&db).await;

        let outside = db
            .orm_db()
            .query_all(Statement::from_sql_and_values(
                DbBackend::Sqlite,
                SERIES_SQL,
                vec![
                    Value::from(60),
                    Value::from(60),
                    Value::from(1),
                    Value::from("m-1"),
                    Value::from("i-1"),
                    Value::from(T0 + 600),
                    Value::from(T0 + 1200),
                ],
            ))
            .await
            .expect("run flow series query");
        assert!(outside.is_empty(), "窗口外不返回");
    }

    /// F-2：时钟超前的节点写入的行「来自未来」，没有上界时会落进每一次查询。
    #[tokio::test]
    async fn series_ignores_rows_dated_in_the_future() {
        let db = Db::memory_db().await;
        seed(&db).await;
        flow_history::Entity::insert(row("42", 7, "10.0.0.0", 1, 999, 999, 1, T0 + 1_000_000))
            .exec(db.orm_db())
            .await
            .expect("seed a future-dated row");

        let rows = run_series(&db, 120, T0 + 600).await;

        assert_eq!(rows.len(), 3, "只有窗口内的 3 条系列");
        assert!(
            rows.iter().all(|r| r.3 != 999),
            "未来行泄漏进了窗口: {rows:?}"
        );
    }
}
