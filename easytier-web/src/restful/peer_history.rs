//! 对端连接历史（延迟 / 丢包 / 抖动 / 流量趋势）查询接口。
//!
//! 数据由 [`crate::peer_history`] 采样器写入，这里只读并按桶聚合：
//! - 延迟 / 丢包 / 抖动取桶内**均值**，全桶都拿不到值时为 `null`（前端断线）
//! - rx/tx 是**累计计数器**，取桶内最大值；速率由前端对相邻桶差分得到
//! - `bucket_seconds` 由请求的时间跨度算出，保证最多返回约 `MAX_POINTS` 个点

use axum::extract::{Path, Query, State};
use axum::{Json, Router, routing::get};
use axum_login::AuthUser;
use sea_orm::{ConnectionTrait as _, DbBackend, Statement, Value};

use crate::db::UserIdInDb;

use super::users::AuthSession;
use super::{AppState, AppStateInner, HttpHandleError, other_error};

/// 单条曲线最多返回的点数，超出则自动加大聚合桶
const MAX_POINTS: i64 = 720;
/// 时间跨度上限：7 天（与采样器默认保留期一致）
const MAX_HOURS: i64 = 24 * 7;

#[derive(Debug, serde::Deserialize)]
struct PeerConnHistoryQuery {
    hours: Option<i64>,
}

#[derive(Debug, serde::Serialize)]
struct PeerConnHistoryPoint {
    /// 桶起始时间（unix 秒）
    t: i64,
    latency_us: Option<i64>,
    loss_rate: Option<f64>,
    jitter_us: Option<i64>,
    /// 桶内累计计数器最大值
    rx_bytes: i64,
    tx_bytes: i64,
    /// 桶内样本数
    samples: i64,
}

#[derive(Debug, serde::Serialize)]
struct PeerConnHistorySeries {
    peer_id: i64,
    hostname: String,
    remote_addr: String,
    tunnel_type: String,
    /// 最近一次采样时间（unix 秒）
    last_seen: i64,
    points: Vec<PeerConnHistoryPoint>,
}

#[derive(Debug, serde::Serialize)]
struct PeerConnHistoryResponse {
    /// 聚合桶大小（秒）
    bucket_seconds: i64,
    /// 查询窗口起点（unix 秒）
    from: i64,
    /// 查询窗口终点（unix 秒）
    to: i64,
    peers: Vec<PeerConnHistorySeries>,
}

/// 按 peer + 时间桶聚合。
/// 绑定参数依次为：bucket、bucket、user_id、machine_id、instance_id、from。
///
/// - 延迟/丢包/抖动用 `CASE WHEN x >= 0` 把「拿不到」的 -1 排除掉，整桶都没有值时结果是 NULL，
///   前端据此断线，而不是把 0 当成真实延迟画进折线
/// - rx/tx 是累计计数器，取桶内 MAX；速率由前端对相邻桶差分
const SERIES_SQL: &str = r#"
    SELECT peer_id,
           (sampled_at / ?) * ? AS bucket_ts,
           CAST(AVG(CASE WHEN latency_us >= 0 THEN latency_us END) AS INTEGER) AS latency_us,
           AVG(CASE WHEN loss_rate >= 0 THEN loss_rate END) AS loss_rate,
           CAST(AVG(CASE WHEN jitter_us >= 0 THEN jitter_us END) AS INTEGER) AS jitter_us,
           MAX(rx_bytes) AS rx_bytes,
           MAX(tx_bytes) AS tx_bytes,
           COUNT(*) AS samples
    FROM peer_conn_history
    WHERE user_id = ? AND machine_id = ? AND instance_id = ? AND sampled_at >= ?
    GROUP BY peer_id, bucket_ts
    ORDER BY peer_id, bucket_ts
"#;

/// peer 的展示信息取最近一次采样。
/// 依赖 SQLite 的约定：聚合查询里出现 MAX() 时，裸列取最大值所在那一行的值。
const META_SQL: &str = r#"
    SELECT peer_id,
           peer_hostname,
           remote_addr,
           tunnel_type,
           MAX(sampled_at) AS last_seen
    FROM peer_conn_history
    WHERE user_id = ? AND machine_id = ? AND instance_id = ? AND sampled_at >= ?
    GROUP BY peer_id
"#;

/// 按时间跨度选桶大小：保证点数不超过 MAX_POINTS，且不小于一个采样间隔（60s）
fn bucket_seconds(hours: i64) -> i64 {
    let window = hours.clamp(1, MAX_HOURS) * 3600;
    // window 恒为正，用 (a + b - 1) / b 做向上取整。
    // 注意不能用 i64::div_ceil：1.95 上它仍属 unstable 的 int_roundings（只有无符号整数稳定了）。
    let buckets = (window + MAX_POINTS - 1) / MAX_POINTS;
    buckets.max(60)
}

pub struct PeerHistoryApi;

impl PeerHistoryApi {
    fn get_user_id(auth_session: &AuthSession) -> Result<UserIdInDb, HttpHandleError> {
        let Some(user_id) = auth_session.user.as_ref().map(|x| x.id()) else {
            return Err((
                axum::http::StatusCode::UNAUTHORIZED,
                other_error("No user id found".to_string()).into(),
            ));
        };
        Ok(user_id)
    }

    async fn handle_peer_conn_history(
        auth_session: AuthSession,
        State(client_mgr): AppState,
        Path((machine_id, inst_id)): Path<(uuid::Uuid, uuid::Uuid)>,
        Query(params): Query<PeerConnHistoryQuery>,
    ) -> Result<Json<PeerConnHistoryResponse>, HttpHandleError> {
        let user_id = Self::get_user_id(&auth_session)?;

        let hours = params.hours.unwrap_or(24).clamp(1, MAX_HOURS);
        let bucket = bucket_seconds(hours);
        let now = chrono::Utc::now().timestamp();
        let from = now - hours * 3600;

        let db = client_mgr.db().orm_db();
        let machine_id = machine_id.to_string();
        let inst_id = inst_id.to_string();

        // 每个 peer 一条曲线：桶内取延迟/丢包/抖动均值（拿不到值时保持 NULL）、累计计数器取最大值
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
                ],
            ))
            .await
            .map_err(super::convert_db_error)?;

        let mut series: Vec<PeerConnHistorySeries> = Vec::new();
        let mut index: std::collections::HashMap<i64, usize> = std::collections::HashMap::new();

        for row in rows {
            let peer_id: i64 = row
                .try_get("", "peer_id")
                .map_err(super::convert_db_error)?;
            let point = PeerConnHistoryPoint {
                t: row
                    .try_get("", "bucket_ts")
                    .map_err(super::convert_db_error)?,
                latency_us: row
                    .try_get("", "latency_us")
                    .map_err(super::convert_db_error)?,
                loss_rate: row
                    .try_get("", "loss_rate")
                    .map_err(super::convert_db_error)?,
                jitter_us: row
                    .try_get("", "jitter_us")
                    .map_err(super::convert_db_error)?,
                rx_bytes: row.try_get("", "rx_bytes").unwrap_or(0),
                tx_bytes: row.try_get("", "tx_bytes").unwrap_or(0),
                samples: row.try_get("", "samples").unwrap_or(0),
            };

            let idx = match index.get(&peer_id) {
                Some(idx) => *idx,
                None => {
                    series.push(PeerConnHistorySeries {
                        peer_id,
                        hostname: String::new(),
                        remote_addr: String::new(),
                        tunnel_type: String::new(),
                        last_seen: 0,
                        points: Vec::new(),
                    });
                    index.insert(peer_id, series.len() - 1);
                    series.len() - 1
                }
            };
            series[idx].points.push(point);
        }

        // peer 的展示信息取最近一次采样（SQLite 裸列 + MAX 聚合即取到最大值所在行）
        let meta_rows = db
            .query_all(Statement::from_sql_and_values(
                DbBackend::Sqlite,
                META_SQL,
                vec![
                    Value::from(user_id),
                    Value::from(machine_id),
                    Value::from(inst_id),
                    Value::from(from),
                ],
            ))
            .await
            .map_err(super::convert_db_error)?;

        for row in meta_rows {
            let peer_id: i64 = row
                .try_get("", "peer_id")
                .map_err(super::convert_db_error)?;
            let Some(&idx) = index.get(&peer_id) else {
                continue;
            };
            series[idx].hostname = row.try_get("", "peer_hostname").unwrap_or_default();
            series[idx].remote_addr = row.try_get("", "remote_addr").unwrap_or_default();
            series[idx].tunnel_type = row.try_get("", "tunnel_type").unwrap_or_default();
            series[idx].last_seen = row.try_get("", "last_seen").unwrap_or(0);
        }

        // 最近活跃的 peer 排前面
        series.sort_by_key(|a| std::cmp::Reverse(a.last_seen));

        Ok(Json(PeerConnHistoryResponse {
            bucket_seconds: bucket,
            from,
            to: now,
            peers: series,
        }))
    }

    pub fn build_route() -> Router<AppStateInner> {
        // 路径刻意避开 /networks/{inst-id}/... 前缀，避免与既有的
        // /networks/info/{inst-id} 这类同级路由在 matchit 里产生歧义。
        Router::new().route(
            "/api/v1/machines/{machine-id}/peer-history/{inst-id}",
            get(Self::handle_peer_conn_history),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::{Db, entity::peer_conn_history};
    use sea_orm::{EntityTrait as _, Set};

    /// 聚合桶行：(peer_id, bucket_ts, latency_us, loss_rate, jitter_us, rx_bytes, tx_bytes, samples)
    type SeriesRow = (
        i64,
        i64,
        Option<i64>,
        Option<f64>,
        Option<i64>,
        i64,
        i64,
        i64,
    );

    /// 与 60s 边界对齐，桶边界才会是 T0 / T0+60 / ...
    const T0: i64 = 1_700_000_040;

    #[allow(clippy::too_many_arguments)]
    fn row(
        peer_id: i64,
        remote_addr: &str,
        latency_us: i64,
        loss_rate: f64,
        jitter_us: i64,
        rx_bytes: i64,
        tx_bytes: i64,
        sampled_at: i64,
    ) -> peer_conn_history::ActiveModel {
        peer_conn_history::ActiveModel {
            user_id: Set(1),
            machine_id: Set("m-1".to_string()),
            instance_id: Set("i-1".to_string()),
            peer_id: Set(peer_id),
            peer_hostname: Set(format!("peer-{peer_id}")),
            remote_addr: Set(remote_addr.to_string()),
            tunnel_type: Set("udp".to_string()),
            latency_us: Set(latency_us),
            loss_rate: Set(loss_rate),
            jitter_us: Set(jitter_us),
            rx_bytes: Set(rx_bytes),
            tx_bytes: Set(tx_bytes),
            conn_count: Set(1),
            sampled_at: Set(sampled_at),
            ..Default::default()
        }
    }

    /// peer 7：6 条采样，第 3、4 条延迟/丢包/抖动拿不到（采样器写 -1）
    /// peer 9：计数器中途重置（连接重建），隧道地址也换过一次
    async fn seed(db: &Db) {
        let samples = [
            (10_000, 500, 1_000, 2_000),
            (20_000, 1_000, 1_600, 2_600),
            (-1, -1, 2_200, 3_200),
            (-1, -1, 2_800, 3_800),
            (30_000, 1_500, 3_400, 4_400),
            (40_000, 2_000, 4_000, 5_000),
        ];
        let mut rows: Vec<peer_conn_history::ActiveModel> = samples
            .iter()
            .enumerate()
            .map(|(i, (lat, jit, rx, tx))| {
                let loss = if *lat < 0 { -1.0 } else { 0.0 };
                row(
                    7,
                    "1.2.3.4:11010",
                    *lat,
                    loss,
                    *jit,
                    *rx,
                    *tx,
                    T0 + i as i64 * 60,
                )
            })
            .collect();

        rows.push(row(9, "5.6.7.8:11010", 8_000, 0.0, 400, 5_000, 5_000, T0));
        rows.push(row(
            9,
            "5.6.7.8:11010",
            8_000,
            0.0,
            400,
            5_500,
            5_500,
            T0 + 60,
        ));
        rows.push(row(9, "9.9.9.9:22022", 9_000, 0.0, 500, 100, 100, T0 + 120));

        peer_conn_history::Entity::insert_many(rows)
            .exec(db.orm_db())
            .await
            .expect("seed peer_conn_history");
    }

    async fn run_series(db: &Db, bucket: i64) -> Vec<SeriesRow> {
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
                ],
            ))
            .await
            .expect("run series query");

        rows.iter()
            .map(|r| {
                let peer_id: i64 = r.try_get("", "peer_id").unwrap();
                let bucket_ts: i64 = r.try_get("", "bucket_ts").unwrap();
                let latency_us: Option<i64> = r.try_get("", "latency_us").unwrap();
                let loss_rate: Option<f64> = r.try_get("", "loss_rate").unwrap();
                let jitter_us: Option<i64> = r.try_get("", "jitter_us").unwrap();
                let rx_bytes: i64 = r.try_get("", "rx_bytes").unwrap();
                let tx_bytes: i64 = r.try_get("", "tx_bytes").unwrap();
                let samples: i64 = r.try_get("", "samples").unwrap();
                (
                    peer_id, bucket_ts, latency_us, loss_rate, jitter_us, rx_bytes, tx_bytes,
                    samples,
                )
            })
            .collect()
    }

    fn find(rows: &[SeriesRow], peer: i64, ts: i64) -> SeriesRow {
        *rows
            .iter()
            .find(|r| r.0 == peer && r.1 == ts)
            .unwrap_or_else(|| panic!("no bucket for peer {peer} at {ts}"))
    }

    #[test]
    fn bucket_grows_with_window_and_respects_floor() {
        // 1 小时：3600 / 720 = 5s，但下限是一个采样间隔
        assert_eq!(bucket_seconds(1), 60);
        // 24 小时：86400 / 720 = 120s
        assert_eq!(bucket_seconds(24), 120);
        // 7 天：604800 / 720 = 840s
        assert_eq!(bucket_seconds(24 * 7), 840);
        // 超出上限按上限算
        assert_eq!(bucket_seconds(24 * 30), 840);
    }

    #[tokio::test]
    async fn series_buckets_by_peer_and_keeps_missing_latency_null() {
        let db = Db::memory_db().await;
        seed(&db).await;

        let rows = run_series(&db, 60).await;
        assert_eq!(rows.len(), 9, "peer7 六个桶 + peer9 三个桶");

        // 桶内只有一条采样时原样返回
        assert_eq!(find(&rows, 7, T0).2, Some(10_000));

        // 延迟/丢包/抖动全为 -1 的桶 -> NULL（前端断线），而不是被 0 拉低
        let missing = find(&rows, 7, T0 + 120);
        assert_eq!(missing.2, None);
        assert_eq!(missing.3, None);
        assert_eq!(missing.4, None);
        // 但累计计数器照常取 MAX
        assert_eq!(missing.5, 2_200);
        assert_eq!(missing.6, 3_200);

        assert_eq!(find(&rows, 7, T0 + 60).7, 1, "bucket=60 时每桶 1 条采样");
        assert_eq!(find(&rows, 9, T0).6, 5_000, "peer9 桶内 tx 取 MAX");
        assert_eq!(find(&rows, 7, T0).4, Some(500), "jitter 原样返回");
    }

    #[tokio::test]
    async fn wider_bucket_averages_latency_and_maxes_counters() {
        let db = Db::memory_db().await;
        seed(&db).await;

        let rows = run_series(&db, 120).await;
        assert_eq!(rows.len(), 5, "peer7 三个桶 + peer9 两个桶");

        // 两条采样 (10000, 20000) / jitter (500, 1000) 取均值
        let first = find(&rows, 7, T0);
        assert_eq!(first.2, Some(15_000));
        assert_eq!(first.4, Some(750));
        assert_eq!(first.7, 2);
        // 累计计数器取 MAX 而不是求和
        assert_eq!(first.5, 1_600);
        assert_eq!(first.6, 2_600);

        // 整桶都拿不到延迟 -> NULL，不会被 0 或相邻桶污染
        assert_eq!(find(&rows, 7, T0 + 120).2, None);
        assert_eq!(find(&rows, 7, T0 + 240).2, Some(35_000));
    }

    #[tokio::test]
    async fn meta_returns_latest_tunnel_address_per_peer() {
        let db = Db::memory_db().await;
        seed(&db).await;

        let rows = db
            .orm_db()
            .query_all(Statement::from_sql_and_values(
                DbBackend::Sqlite,
                META_SQL,
                vec![
                    Value::from(1),
                    Value::from("m-1"),
                    Value::from("i-1"),
                    Value::from(T0),
                ],
            ))
            .await
            .expect("run meta query");

        assert_eq!(rows.len(), 2, "两个 peer 各一行");

        let peer9 = rows
            .iter()
            .find(|r| r.try_get::<i64>("", "peer_id").unwrap() == 9)
            .expect("peer 9 row");
        // 隧道地址变过，应取最近一行的
        assert_eq!(
            peer9.try_get::<String>("", "remote_addr").unwrap(),
            "9.9.9.9:22022"
        );
        assert_eq!(peer9.try_get::<i64>("", "last_seen").unwrap(), T0 + 120);

        let peer7 = rows
            .iter()
            .find(|r| r.try_get::<i64>("", "peer_id").unwrap() == 7)
            .expect("peer 7 row");
        assert_eq!(peer7.try_get::<i64>("", "last_seen").unwrap(), T0 + 300);
    }
}
