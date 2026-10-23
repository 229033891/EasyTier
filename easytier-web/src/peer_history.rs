//! 对端连接历史采样器。
//!
//! 每 `sample_interval`（默认 60s）遍历所有已授权设备，调用节点的
//! `CollectNetworkInfo`，把「本设备某个网络实例 ↔ 某个直连对端 peer」的
//! 延迟与累计流量按一行一条落库，并按 `retention_days`（默认 7 天）清理。
//!
//! 同一次采集里还顺带落 `flow_history`（每条隧道上的 IP/网段流量）：
//! - 复用同一个 `CollectNetworkInfo` 响应，**不额外发 RPC**（见决策 D6）
//! - 只请求默认的 MASKED 视图，精确 IP 不出节点（见决策 D8）
//! - 节点没开 `flow_stats` 时 `flows` 为空，这里也就零写入
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

use easytier::proto::api::{instance::PeerRoutePair, manage::NetworkInstanceRunningInfo};
use easytier_core::management::remote_client::RemoteClientManager as _;
// DeleteMany::filter 来自 QueryFilter trait，必须显式引入，否则会解析到 Iterator::filter
use sea_orm::{ColumnTrait as _, EntityTrait as _, QueryFilter as _, Set};
use tokio::time::{MissedTickBehavior, interval};
use tracing::{debug, error, info};

use crate::client_manager::ClientManager;
use crate::db::{
    Db,
    entity::{flow_history, peer_conn_history},
};

/// 采样间隔：60s
pub const DEFAULT_SAMPLE_INTERVAL_SECS: u64 = 60;
/// 保留天数：7 天
pub const DEFAULT_RETENTION_DAYS: i64 = 7;
/// 清理任务的执行间隔（每 10 分钟扫一次）
const CLEANUP_INTERVAL: Duration = Duration::from_secs(600);
/// 单台设备一次采集的超时
const COLLECT_TIMEOUT: Duration = Duration::from_secs(20);
/// 单次采样写入 `flow_history` 的总行数上限（跨设备/实例）。
///
/// 节点侧已按 `FLOW_TOP_N_PER_CONN` 截断；这里再加一道表侧闸门，防止隧道数
/// 很多时单轮写入无界增长（§17.6 R1）。超出时保留字节最大的行并告警。
const MAX_FLOW_ROWS_PER_SAMPLE: usize = 4096;

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

/// 一次采样里某条隧道上的某个 IP/网段
#[derive(Debug, Clone, PartialEq)]
struct FlowSample {
    conn_id: String,
    peer_id: i64,
    ip: String,
    prefix_len: i32,
    direction: i32,
    bytes: i64,
    packets: i64,
    rate_bytes_per_sec: i64,
    flow_count: i32,
    sampled: bool,
}

/// 把一次 `CollectNetworkInfo` 里某个实例的 `PeerConnInfo.flows` 压成待落库的行。
///
/// 只保留有字节的条目：节点刚建流、还没跑流量时会出现 0 字节行，落库没有意义。
fn aggregate_flows(info: &NetworkInstanceRunningInfo) -> Vec<FlowSample> {
    let mut out = Vec::new();

    for pair in &info.peer_route_pairs {
        let Some(peer) = pair.peer.as_ref() else {
            continue;
        };
        for conn in &peer.conns {
            // PeerConnId = uuid::Uuid，线上是 to_string() 形如 "3f2a…-…"。
            // 按文本落库，与 machine_id 同策略；绝不尝试按整数解析。
            let conn_id = conn.conn_id.to_string();
            for flow in &conn.flows {
                if flow.bytes == 0 {
                    continue;
                }
                out.push(FlowSample {
                    // 同一条隧道可能有多个 flow，String 需要 clone 后再移动。
                    conn_id: conn_id.clone(),
                    // `flow.peer_id` 可能为 0（老节点/未填），回退到 peer 自身的 id。
                    peer_id: if flow.peer_id == 0 {
                        i64::from(peer.peer_id)
                    } else {
                        i64::from(flow.peer_id)
                    },
                    ip: flow.ip.clone(),
                    prefix_len: flow.prefix_len as i32,
                    direction: flow.direction as i32,
                    bytes: flow.bytes.min(i64::MAX as u64) as i64,
                    packets: flow.packets.min(i64::MAX as u64) as i64,
                    rate_bytes_per_sec: flow.rate_bytes_per_sec.min(i64::MAX as u64) as i64,
                    flow_count: flow.flow_count as i32,
                    sampled: flow.sampled,
                });
            }
        }
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

/// 采集一轮，返回写入的行数（peer_conn_history + flow_history）
async fn sample_once(client_mgr: &Arc<ClientManager>, db: &Db) -> usize {
    let sessions = client_mgr.list_all_sessions().await;
    if sessions.is_empty() {
        return 0;
    }

    let now = chrono::Utc::now().timestamp();
    let mut rows: Vec<peer_conn_history::ActiveModel> = Vec::new();
    let mut flow_rows: Vec<flow_history::ActiveModel> = Vec::new();

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
            // 与上面同一次采集：节点没开 flow_stats 时这里就是空向量。
            for flow in aggregate_flows(info) {
                flow_rows.push(flow_history::ActiveModel {
                    user_id: Set(token.user_id),
                    machine_id: Set(token.machine_id.to_string()),
                    instance_id: Set(instance_id.clone()),
                    conn_id: Set(flow.conn_id),
                    peer_id: Set(flow.peer_id),
                    ip: Set(flow.ip),
                    prefix_len: Set(flow.prefix_len),
                    direction: Set(flow.direction),
                    bytes: Set(flow.bytes),
                    packets: Set(flow.packets),
                    rate_bytes_per_sec: Set(flow.rate_bytes_per_sec),
                    flow_count: Set(flow.flow_count),
                    sampled: Set(flow.sampled),
                    sampled_at: Set(now),
                    ..Default::default()
                });
            }
        }
    }

    // 表侧限流：保留累计字节最大的行，其余丢弃并告警（§17.6 R1）。
    if flow_rows.len() > MAX_FLOW_ROWS_PER_SAMPLE {
        let dropped = flow_rows.len() - MAX_FLOW_ROWS_PER_SAMPLE;
        flow_rows.sort_by_key(|row| {
            let bytes = match &row.bytes {
                sea_orm::ActiveValue::Set(v) => *v,
                _ => 0,
            };
            std::cmp::Reverse(bytes)
        });
        flow_rows.truncate(MAX_FLOW_ROWS_PER_SAMPLE);
        tracing::warn!(
            dropped,
            kept = MAX_FLOW_ROWS_PER_SAMPLE,
            "flow history: per-sample row cap reached, dropped the least busy rows"
        );
    }

    let mut written = 0usize;

    // 分块插入：一行 14 个绑定参数，整批一次插入在设备多时会顶到 SQLite 的变量上限
    const INSERT_CHUNK: usize = 200;
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
    // flow_history 一行 15 个绑定参数，同样分块。
    for chunk in flow_rows.chunks(INSERT_CHUNK) {
        let count = chunk.len();
        if let Err(e) = flow_history::Entity::insert_many(chunk.to_vec())
            .exec(db.orm_db())
            .await
        {
            error!("flow history: insert {} rows failed: {:?}", count, e);
            continue;
        }
        written += count;
    }
    written
}

/// 删除超过保留期的行，返回删除条数
async fn cleanup_once(db: &Db, retention_days: i64) -> u64 {
    let cutoff = chrono::Utc::now().timestamp() - retention_days.max(1) * 86_400;
    let mut removed = 0u64;
    match peer_conn_history::Entity::delete_many()
        .filter(peer_conn_history::Column::SampledAt.lt(cutoff))
        .exec(db.orm_db())
        .await
    {
        Ok(res) => removed += res.rows_affected,
        Err(e) => {
            error!("peer history: cleanup failed: {:?}", e);
        }
    }
    // 流量明细与 peer 汇总同一保留期，避免两张表口径不一致。
    match flow_history::Entity::delete_many()
        .filter(flow_history::Column::SampledAt.lt(cutoff))
        .exec(db.orm_db())
        .await
    {
        Ok(res) => removed += res.rows_affected,
        Err(e) => {
            error!("flow history: cleanup failed: {:?}", e);
        }
    }
    removed
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
        api::instance::{PeerConnFlow, PeerConnInfo, PeerConnStats, PeerInfo, Route},
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
        // tunnel 地址以规范化形式返回（带 scheme），见 `Url::normalized_tunnel_display`
        assert_eq!(got[0].remote_addr, "udp://1.2.3.4:11010");
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

    fn flow(ip: &str, dir: u32, bytes: u64, rate: u64, prefix: u32, count: u32) -> PeerConnFlow {
        PeerConnFlow {
            ip: ip.to_string(),
            direction: dir,
            bytes,
            packets: 3,
            rate_bytes_per_sec: rate,
            prefix_len: prefix,
            flow_count: count,
            ..Default::default()
        }
    }

    #[test]
    fn aggregates_flows_per_tunnel_and_skips_empty() {
        let mut c1 = conn("42", 1_000, 10, 20);
        c1.flows = vec![
            // 掩码行：一个 /24 里 14 台机器
            flow("10.0.0.0", 1, 4_096, 512, 24, 14),
            // 节点刚建流、还没跑流量：不落库
            flow("10.0.1.0", 1, 0, 0, 24, 1),
            // 出站
            flow("10.0.2.0", 0, 2_048, 128, 24, 3),
        ];
        let mut c2 = conn("7", 1_000, 10, 20);
        c2.flows = vec![flow("fd00::", 1, 900, 9, 64, 2)];

        let info = NetworkInstanceRunningInfo {
            peer_route_pairs: vec![pair(vec![c1, c2])],
            ..Default::default()
        };
        let got = aggregate_flows(&info);

        assert_eq!(got.len(), 3, "0 字节条目被跳过");
        assert_eq!(got[0].conn_id, "42");
        assert_eq!(got[0].peer_id, 7, "flow.peer_id 未填时回退到 peer 自身");
        assert_eq!(got[0].ip, "10.0.0.0");
        assert_eq!(got[0].prefix_len, 24);
        assert_eq!(got[0].direction, 1);
        assert_eq!(got[0].bytes, 4_096);
        assert_eq!(got[0].rate_bytes_per_sec, 512);
        assert_eq!(got[0].flow_count, 14);
        assert_eq!(got[1].conn_id, "42");
        assert_eq!(got[1].direction, 0);
        assert_eq!(got[2].conn_id, "7");
        assert_eq!(got[2].prefix_len, 64);
    }

    #[test]
    fn aggregate_flows_tolerates_unparsable_conn_id_and_non_direct_peers() {
        let mut c = conn("not-a-number", 1_000, 10, 20);
        c.flows = vec![flow("10.0.0.0", 1, 1, 1, 24, 1)];
        let mut no_peer = pair(vec![c]);
        no_peer.peer = None;

        let info = NetworkInstanceRunningInfo {
            peer_route_pairs: vec![no_peer],
            ..Default::default()
        };
        assert!(aggregate_flows(&info).is_empty(), "非直连不落 flows");

        let c = {
            let mut c = conn("not-a-number", 1_000, 10, 20);
            c.flows = vec![flow("10.0.0.0", 1, 1, 1, 24, 1)];
            c
        };
        let info = NetworkInstanceRunningInfo {
            peer_route_pairs: vec![pair(vec![c])],
            ..Default::default()
        };
        let got = aggregate_flows(&info);
        assert_eq!(got.len(), 1);
        assert_eq!(
            got[0].conn_id, "not-a-number",
            "不可解析的 conn_id 原样落库"
        );
    }
}
