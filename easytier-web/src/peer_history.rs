//! 对端连接历史采样器。
//!
//! 每 `sample_interval`（默认 60s）遍历所有已授权设备，调用节点的
//! `CollectNetworkInfo`，把「本设备某个网络实例 ↔ 某个直连对端 peer」的
//! 延迟与累计流量按一行一条落库，并按 `retention_days`（默认 7 天）清理。
//!
//! 设计取舍：
//! - **只记直连对端**：`PeerRoutePair.peer` 为 `None` 说明只是路由可达、没有直连，
//!   没有 IP:端口和延迟可言，跳过。
//! - **rx/tx 存累计计数器**，不存速率。速率由查询侧对相邻采样做差分，
//!   这样连接重置（计数器归零）时只是少一个点，不会把库里的数据写脏。
//! - **拿不到的值写 -1**（延迟/丢包/抖动），查询侧按 NULL 处理，避免 0 污染折线。
//! - 单台设备采样失败（离线 / RPC 超时）只记日志，不影响其它设备。
//! - **优先按 heartbeat 的 running_network_instances 做定向 CollectNetworkInfo**，
//!   避免对停用实例做全量采集，减轻与状态页 / 心跳 reconcile 的隧道争用；
//!   尚无 heartbeat 时再回退全量。

use std::{sync::Arc, time::Duration};

use easytier::proto::api::instance::PeerRoutePair;
use easytier_core::management::remote_client::RemoteClientManager as _;
// DeleteMany::filter 来自 QueryFilter trait，必须显式引入，否则会解析到 Iterator::filter
use sea_orm::{ColumnTrait as _, EntityTrait as _, QueryFilter as _, Set};
use tokio::time::{MissedTickBehavior, interval};
use tracing::{debug, error, info};

use crate::client_manager::ClientManager;
use crate::db::{Db, entity::peer_conn_history};

/// 采样间隔：60s
pub const DEFAULT_SAMPLE_INTERVAL_SECS: u64 = 60;
/// 保留天数：7 天
pub const DEFAULT_RETENTION_DAYS: i64 = 7;
/// 清理任务的执行间隔（每 10 分钟扫一次）
const CLEANUP_INTERVAL: Duration = Duration::from_secs(600);
/// 单台设备一次采集的超时
const COLLECT_TIMEOUT: Duration = Duration::from_secs(20);

#[derive(Debug, Clone)]
pub struct PeerHistoryOptions {
    pub enabled: bool,
    pub sample_interval: Duration,
    pub retention_days: i64,
}

impl Default for PeerHistoryOptions {
    fn default() -> Self {
        Self {
            enabled: true,
            sample_interval: Duration::from_secs(DEFAULT_SAMPLE_INTERVAL_SECS),
            retention_days: DEFAULT_RETENTION_DAYS,
        }
    }
}

/// 一次采样里某个 peer 的汇总
#[derive(Debug, Clone, PartialEq)]
struct PeerSample {
    peer_id: i64,
    hostname: String,
    remote_addr: String,
    tunnel_type: String,
    latency_us: i64,
    loss_rate: f64,
    jitter_us: i64,
    rx_bytes: i64,
    tx_bytes: i64,
    conn_count: i32,
}

/// 把一次 `CollectNetworkInfo` 里某个实例的 peer 列表压成待落库的行
fn aggregate_instance(pairs: &[PeerRoutePair]) -> Vec<PeerSample> {
    let mut out = Vec::with_capacity(pairs.len());

    for pair in pairs {
        let Some(peer) = pair.peer.as_ref() else {
            // 非直连：只有路由没有连接
            continue;
        };
        if peer.conns.is_empty() {
            continue;
        }

        // 延迟/丢包/抖动优先取 default conn（就是实际跑流量的那条），拿不到再退化
        let latency_us = pair
            .get_latency_ms()
            .map(|ms| (ms * 1000.0).round() as i64)
            .unwrap_or(-1);
        let loss_rate = pair.get_loss_rate().unwrap_or(-1.0);
        let jitter_us = pair.get_jitter_us().map(|us| us as i64).unwrap_or(-1);
        let rx_bytes = pair.get_rx_bytes().unwrap_or(0) as i64;
        let tx_bytes = pair.get_tx_bytes().unwrap_or(0) as i64;

        // 隧道信息取 default conn，否则第一条带 tunnel 的连接
        let default_conn_id = peer.default_conn_id.as_ref().map(|id| id.to_string());
        let tunnel_conn = peer
            .conns
            .iter()
            .find(|c| default_conn_id.as_deref() == Some(c.conn_id.as_str()))
            .or_else(|| peer.conns.iter().find(|c| c.tunnel.is_some()));

        let remote_addr = tunnel_conn
            .and_then(|c| c.tunnel.as_ref())
            .and_then(|t| t.display_remote_addr())
            .unwrap_or_default();
        let tunnel_type = tunnel_conn
            .and_then(|c| c.tunnel.as_ref())
            .map(|t| t.display_tunnel_type())
            .unwrap_or_default();

        let hostname = pair
            .route
            .as_ref()
            .map(|r| r.hostname.clone())
            .unwrap_or_default();

        out.push(PeerSample {
            peer_id: i64::from(peer.peer_id),
            hostname,
            remote_addr,
            tunnel_type,
            latency_us,
            loss_rate,
            jitter_us,
            rx_bytes,
            tx_bytes,
            conn_count: peer.conns.len() as i32,
        });
    }

    out
}

/// 从最近一次 heartbeat 取运行中实例；无 heartbeat 返回 `None`（调用方回退全量）。
async fn running_inst_ids_from_heartbeat(
    client_mgr: &ClientManager,
    client_url: &url::Url,
) -> Option<Vec<uuid::Uuid>> {
    let req = client_mgr.get_heartbeat_requests(client_url).await?;
    Some(
        req.running_network_instances
            .into_iter()
            .map(uuid::Uuid::from)
            .collect(),
    )
}

/// 采集一轮，返回写入的行数
async fn sample_once(client_mgr: &Arc<ClientManager>, db: &Db) -> usize {
    let sessions = client_mgr.list_all_sessions().await;
    if sessions.is_empty() {
        return 0;
    }

    let now = chrono::Utc::now().timestamp();
    let mut rows: Vec<peer_conn_history::ActiveModel> = Vec::new();

    for token in sessions {
        let identify = (token.user_id, token.machine_id);
        // Prefer scoped collect: empty running list → skip RPC; missing heartbeat → full collect.
        let inst_ids = match running_inst_ids_from_heartbeat(client_mgr, &token.client_url).await {
            Some(ids) if ids.is_empty() => {
                debug!(
                    machine_id = %token.machine_id,
                    "peer history: no running instances, skip collect"
                );
                continue;
            }
            Some(ids) => Some(ids),
            None => None,
        };
        let collected = match tokio::time::timeout(
            COLLECT_TIMEOUT,
            client_mgr.handle_collect_network_info(identify, inst_ids),
        )
        .await
        {
            Ok(Ok(resp)) => resp,
            Ok(Err(e)) => {
                debug!(
                    machine_id = %token.machine_id,
                    "peer history: collect network info failed: {:?}", e
                );
                continue;
            }
            Err(_) => {
                debug!(
                    machine_id = %token.machine_id,
                    "peer history: collect network info timed out"
                );
                continue;
            }
        };

        let Some(info_map) = collected.info else {
            continue;
        };

        for (instance_id, info) in info_map.map.iter() {
            if !info.running {
                continue;
            }
            for sample in aggregate_instance(&info.peer_route_pairs) {
                rows.push(peer_conn_history::ActiveModel {
                    user_id: Set(token.user_id),
                    machine_id: Set(token.machine_id.to_string()),
                    instance_id: Set(instance_id.clone()),
                    peer_id: Set(sample.peer_id),
                    peer_hostname: Set(sample.hostname),
                    remote_addr: Set(sample.remote_addr),
                    tunnel_type: Set(sample.tunnel_type),
                    latency_us: Set(sample.latency_us),
                    loss_rate: Set(sample.loss_rate),
                    jitter_us: Set(sample.jitter_us),
                    rx_bytes: Set(sample.rx_bytes),
                    tx_bytes: Set(sample.tx_bytes),
                    conn_count: Set(sample.conn_count),
                    sampled_at: Set(now),
                    ..Default::default()
                });
            }
        }
    }

    if rows.is_empty() {
        return 0;
    }

    // 分块插入：一行 14 个绑定参数，整批一次插入在设备多时会顶到 SQLite 的变量上限
    const INSERT_CHUNK: usize = 200;
    let mut written = 0usize;
    for chunk in rows.chunks(INSERT_CHUNK) {
        let count = chunk.len();
        if let Err(e) = peer_conn_history::Entity::insert_many(chunk.to_vec())
            .exec(db.orm_db())
            .await
        {
            error!("peer history: insert {} rows failed: {:?}", count, e);
            continue;
        }
        written += count;
    }
    written
}

/// 删除超过保留期的行，返回删除条数
async fn cleanup_once(db: &Db, retention_days: i64) -> u64 {
    let cutoff = chrono::Utc::now().timestamp() - retention_days.max(1) * 86_400;
    match peer_conn_history::Entity::delete_many()
        .filter(peer_conn_history::Column::SampledAt.lt(cutoff))
        .exec(db.orm_db())
        .await
    {
        Ok(res) => res.rows_affected,
        Err(e) => {
            error!("peer history: cleanup failed: {:?}", e);
            0
        }
    }
}

/// 启动采样任务（后台常驻）。`enabled=false` 时只打一条日志并直接返回。
pub fn spawn_peer_history_sampler(
    client_mgr: Arc<ClientManager>,
    db: Db,
    opts: PeerHistoryOptions,
) -> Option<tokio_util::task::AbortOnDropHandle<()>> {
    if !opts.enabled {
        info!("peer history sampler disabled");
        return None;
    }

    info!(
        interval_secs = opts.sample_interval.as_secs(),
        retention_days = opts.retention_days,
        "peer history sampler started"
    );

    let task = tokio::spawn(async move {
        let mut ticker = interval(opts.sample_interval);
        ticker.set_missed_tick_behavior(MissedTickBehavior::Delay);
        let mut last_cleanup = tokio::time::Instant::now();

        loop {
            ticker.tick().await;

            let written = sample_once(&client_mgr, &db).await;
            if written > 0 {
                debug!("peer history: wrote {} rows", written);
            }

            if last_cleanup.elapsed() >= CLEANUP_INTERVAL {
                last_cleanup = tokio::time::Instant::now();
                let removed = cleanup_once(&db, opts.retention_days).await;
                if removed > 0 {
                    info!(
                        "peer history: removed {} rows older than {} days",
                        removed, opts.retention_days
                    );
                }
            }
        }
    });

    Some(tokio_util::task::AbortOnDropHandle::new(task))
}

#[cfg(test)]
mod tests {
    use super::*;
    use easytier::proto::{
        api::instance::{PeerConnInfo, PeerConnStats, PeerInfo, Route},
        common::TunnelInfo,
    };

    fn conn(id: &str, latency_us: u64, rx: u64, tx: u64) -> PeerConnInfo {
        conn_with_jitter(id, latency_us, 1_500, rx, tx)
    }

    fn conn_with_jitter(
        id: &str,
        latency_us: u64,
        jitter_us: u64,
        rx: u64,
        tx: u64,
    ) -> PeerConnInfo {
        PeerConnInfo {
            conn_id: id.to_string(),
            peer_id: 7,
            tunnel: Some(TunnelInfo {
                tunnel_type: "udp".to_string(),
                resolved_remote_addr: Some(easytier::proto::common::Url {
                    url: "udp://1.2.3.4:11010".to_string(),
                }),
                ..Default::default()
            }),
            stats: Some(PeerConnStats {
                latency_us,
                jitter_us,
                rx_bytes: rx,
                tx_bytes: tx,
                ..Default::default()
            }),
            loss_rate: 0.25,
            ..Default::default()
        }
    }

    fn pair(conns: Vec<PeerConnInfo>) -> PeerRoutePair {
        PeerRoutePair {
            route: Some(Route {
                peer_id: 7,
                hostname: "peer-b".to_string(),
                cost: 1,
                ..Default::default()
            }),
            peer: Some(PeerInfo {
                peer_id: 7,
                conns,
                ..Default::default()
            }),
        }
    }

    #[test]
    fn aggregates_direct_peer() {
        let pairs = vec![pair(vec![conn("c1", 12_500, 1000, 2000)])];
        let got = aggregate_instance(&pairs);
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].peer_id, 7);
        assert_eq!(got[0].hostname, "peer-b");
        assert_eq!(got[0].remote_addr, "1.2.3.4:11010");
        assert_eq!(got[0].tunnel_type, "udp");
        assert_eq!(got[0].latency_us, 12_500);
        assert_eq!(got[0].jitter_us, 1_500);
        assert_eq!(got[0].rx_bytes, 1000);
        assert_eq!(got[0].tx_bytes, 2000);
        assert_eq!(got[0].conn_count, 1);
        assert!((got[0].loss_rate - 0.25).abs() < 1e-6);
    }

    #[test]
    fn skips_non_direct_and_disconnected_peers() {
        let mut no_conns = pair(vec![]);
        no_conns.route.as_mut().unwrap().hostname = "relayed".to_string();
        let mut no_peer = pair(vec![conn("c1", 1, 1, 1)]);
        no_peer.peer = None;

        assert!(aggregate_instance(&[no_conns, no_peer]).is_empty());
    }

    #[test]
    fn sums_bytes_and_prefers_best_latency() {
        let pairs = vec![pair(vec![
            conn_with_jitter("c1", 30_000, 5_000, 100, 200),
            conn_with_jitter("c2", 9_000, 800, 1, 2),
        ])];
        let got = aggregate_instance(&pairs);
        assert_eq!(got[0].rx_bytes, 101);
        assert_eq!(got[0].tx_bytes, 202);
        assert_eq!(got[0].conn_count, 2);
        // 没有 default_conn_id 时取最小延迟 / 最小抖动
        assert_eq!(got[0].latency_us, 9_000);
        assert_eq!(got[0].jitter_us, 800);
    }
}
