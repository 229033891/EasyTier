use std::{
    any::Any,
    net::{IpAddr, Ipv6Addr},
    sync::{Arc, OnceLock},
};

use easytier_core::{
    host::packet::HostPacketReceiver,
    instance::{CorePacketPlane, UnderlayExcludeState, UnderlayExcludeViaMirror},
};
use tokio::{sync::Mutex, task::JoinSet};

use super::MagicDnsRuntime;
use crate::{
    common::{
        global_ctx::ArcGlobalCtx,
        ifcfg::{IfConfiger, IfConfiguerTrait, PhysicalDefaultRoute},
    },
    instance::virtual_nic::NicCtx,
};

struct NicCtxContainer {
    nic_ctx: Option<Box<dyn Any + Send>>,
    magic_dns: MagicDnsRuntime,
}

impl NicCtxContainer {
    fn new(nic_ctx: NicCtx, magic_dns: MagicDnsRuntime) -> Self {
        Self {
            nic_ctx: Some(Box::new(nic_ctx)),
            magic_dns,
        }
    }

    fn packet_drain(tasks: JoinSet<()>) -> Self {
        Self {
            nic_ctx: Some(Box::new(tasks)),
            magic_dns: MagicDnsRuntime::default(),
        }
    }

    async fn shutdown_tasks(&mut self) {
        let Some(boxed) = self.nic_ctx.take() else {
            return;
        };
        // `Box::<dyn Any + Send>::downcast` consumes the box, so the second
        // attempt must re-bind it from `Err` instead of reusing `boxed`.
        match boxed.downcast::<NicCtx>() {
            Ok(nic) => nic.shutdown().await,
            Err(boxed) => {
                if let Ok(mut tasks) = boxed.downcast::<JoinSet<()>>() {
                    tasks.abort_all();
                    while tasks.join_next().await.is_some() {}
                }
            }
        }
    }
}

#[derive(Clone)]
pub(super) struct TunNicState {
    nic_ctx: Arc<Mutex<Option<NicCtxContainer>>>,
    receiver: Arc<OnceLock<Arc<Mutex<HostPacketReceiver>>>>,
}

impl TunNicState {
    pub(super) fn empty() -> Self {
        Self {
            nic_ctx: Arc::new(Mutex::new(None)),
            receiver: Arc::new(OnceLock::new()),
        }
    }

    pub(super) fn install_receiver(&self, receiver: HostPacketReceiver) -> anyhow::Result<()> {
        self.receiver
            .set(Arc::new(Mutex::new(receiver)))
            .map_err(|_| anyhow::anyhow!("native packet receiver is already installed"))
    }

    pub(super) fn receiver(&self) -> Arc<Mutex<HostPacketReceiver>> {
        self.receiver
            .get()
            .expect("packet receiver must be installed before preparing TUN")
            .clone()
    }

    pub(super) async fn stop(&self) {
        let mut old = self.nic_ctx.lock().await.take();
        if let Some(container) = old.as_mut() {
            container.magic_dns.stop().await;
            // Join aborted NIC tasks so AsyncFd epoll DEL finishes before a
            // replacement TUN reuses the same fd number (A10).
            container.shutdown_tasks().await;
        }
        drop(old);
    }

    pub(super) async fn drain(&self) {
        self.stop().await;
        let receiver = self.receiver();
        let mut tasks = JoinSet::new();
        tasks.spawn(async move {
            let mut receiver = receiver.lock().await;
            while let Some(packet) = receiver.recv().await {
                tracing::trace!(?packet, "discarded packet without a native interface");
            }
        });
        self.nic_ctx
            .lock()
            .await
            .replace(NicCtxContainer::packet_drain(tasks));
    }

    pub(super) async fn install(&self, nic: NicCtx, magic_dns: MagicDnsRuntime) {
        self.stop().await;
        self.nic_ctx
            .lock()
            .await
            .replace(NicCtxContainer::new(nic, magic_dns));
    }
}

/// 清掉本实例在系统里留下的网络痕迹（禁用网络 / 停止实例时调用）。
///
/// Linux 关 TUN 即删接口、内核顺带清路由；Windows wintun 适配器是持久设备，
/// 会话结束后接口、IP、路由都会留下来（`route print` 里仍能看到指向已失效接口的
/// 条目），DNS 也还挂在该适配器上，因此必须显式清理。
/// 只清本实例装过 / 自己持有的条目，不碰其它实例。
pub(super) async fn cleanup_tun_leftovers(
    global_ctx: &ArcGlobalCtx,
    packet_plane: &Arc<CorePacketPlane>,
) {
    let Some(ifname) = global_ctx.get_tun_device_name() else {
        return;
    };
    // 与 updater 一致：路由操作走 netns 守卫，容器 / netns 场景下才落在正确的命名空间。
    let _guard = global_ctx.net_ns.guard();
    let ifcfg = IfConfiger {};

    // 1) proxy CIDR / 默认路由：以 `packet_plane` 的同步状态为准，只删本实例装过的条目。
    //    这里不能因为 installed 为空就提前返回：普通组网根本没有 proxy CIDR，
    //    但接口地址、underlay 排除路由同样要清。
    for entry in packet_plane.proxy_cidr_route_sync_status().installed {
        let Ok(cidr) = entry.parse::<cidr::Ipv4Cidr>() else {
            continue;
        };
        if let Err(error) = ifcfg
            .remove_ipv4_route(&ifname, cidr.first_address(), cidr.network_length(), None)
            .await
        {
            tracing::debug!(
                ?error,
                route = %entry,
                ifname = %ifname,
                "failed to remove proxy CIDR route on TUN stop",
            );
        }
    }

    // 2) public-ipv6 client peer leases（多为 /128）：Windows 亦可作 client，
    //    `run_public_ipv6_route_updater` 会装其它 peer 的 lease 路由，须按同步状态删掉。
    for route in packet_plane.public_ipv6_routes().await {
        if let Err(error) = ifcfg
            .remove_ipv6_route(&ifname, route.address(), route.network_length(), None)
            .await
        {
            tracing::debug!(
                ?error,
                route = %route,
                ifname = %ifname,
                "failed to remove public ipv6 peer route on TUN stop",
            );
        }
    }

    // 3) IPv6 exit / public-ipv6 本机默认路由 (::/0) 只保存在 updater 任务内部，
    //    同步状态里没有；按目的+接口 best-effort 删除（Windows 后端忽略 metric，
    //    一次删一条，循环几次即可覆盖 exit 与 public-ipv6 两条）。
    //    不存在的条目只会返回错误并忽略，不影响其它接口。
    for _ in 0..4 {
        let _ = ifcfg
            .remove_ipv6_route(&ifname, Ipv6Addr::UNSPECIFIED, 0, None)
            .await;
    }

    // 4) 接口自身的地址：Windows wintun 适配器是持久设备，会话结束后 IP 不会自动
    //    消失，Windows 会一直保留它的 on-link 子网路由（10.144.144.0/24 之类），
    //    该接口也继续参与源地址选择。下次启用会重新下发，删掉是安全的。
    #[cfg(target_os = "windows")]
    for (family, result) in [
        ("ipv4", ifcfg.remove_ip(&ifname, None).await),
        ("ipv6", ifcfg.remove_ipv6(&ifname, None).await),
    ] {
        if let Err(error) = result {
            tracing::debug!(
                ?error,
                family,
                ifname = %ifname,
                "failed to flush TUN addresses on stop",
            );
        }
    }

    // 5) exit-node 模式下钉在物理网卡上的 underlay 排除路由（/32、/128）：它们挂在
    //    物理接口上，不会随 TUN 关闭消失。updater 收尾那段清理要等事件总线 Closed，
    //    而任务自己持有 global_ctx，永远等不到，所以在这里补一次。
    cleanup_underlay_exclude_routes(packet_plane, &ifcfg).await;
}

/// 删除 exit-node 模式为保住 underlay 而钉在物理网卡上的主机路由。
///
/// 用安装器发布的「已安装集合 + 安装时 via 快照」（`packet_plane.underlay_exclude_state()`）
/// 推导目标条目，**而不是重新采集当前对端**：对端已断 / 集合清空时照样能卸载（否则
/// 停止/卸载后残留，见 docs/roadmap/underlay-exclude-onlink-filter.md §2.2 现网两例）。
/// 条目全部来自安装器自己的记账，不会误删用户或其它实例的路由。
async fn cleanup_underlay_exclude_routes(
    packet_plane: &Arc<CorePacketPlane>,
    ifcfg: &impl IfConfiguerTrait,
) {
    let state = packet_plane.underlay_exclude_state();
    if state.installed.is_empty() {
        return;
    }
    let remaining = remove_installed_underlay_excludes(ifcfg, state).await;
    // Publish the remainder verbatim: an all-success run clears `installed`
    // (the via snapshots may remain, which is harmless); a partial failure
    // keeps the failed entries accounted.
    // Note: the accounting only gets retried by a later cleanup **within the
    // same process run** (repeated stop / mobile restart reusing the plane).
    // Once the process is gone nothing retries it — the shipped one-time
    // cleanup (docker-compose-deploy.md §2.7 / roadmap §4.4) is the backstop.
    packet_plane.report_underlay_exclude_state(remaining);
}

/// Remove the given installed underlay-exclude host routes; return the state
/// for the entries that could **not** be removed.
///
/// Pure seam (state in → remaining state out) so tests can drive it with a
/// mock `IfConfiguerTrait` without constructing a `CorePacketPlane`.
///
/// via 必须用安装时快照：停止时网络可能已断，重新解析拿到 `None` 就跳过删除；
/// Windows 删除按 `ifindex + gateway` 定位路由，用新值删不掉旧条目（roadmap §4 F2）。
/// 删除失败（或无 via 快照）的条目**必须保留在返回状态里** —— 这里丢账就是 G2 的
/// 复发路径：失败变成永久残留且无任何重试机会（与 `virtual_nic.rs` updater 的
/// warn + 保留 installed 口径一致；`NotFound` / already-gone 视为已删除）。
async fn remove_installed_underlay_excludes(
    ifcfg: &impl IfConfiguerTrait,
    state: UnderlayExcludeState,
) -> UnderlayExcludeState {
    let UnderlayExcludeState {
        installed,
        ipv4_via,
        ipv6_via,
    } = state;
    if installed.is_empty() {
        return UnderlayExcludeState {
            installed,
            ipv4_via,
            ipv6_via,
        };
    }

    let via_from_mirror = |via: &Option<UnderlayExcludeViaMirror>| {
        via.as_ref().map(|m| PhysicalDefaultRoute {
            ifindex: m.ifindex,
            ifname: m.ifname.clone(),
            gateway: m.gateway,
        })
    };
    let ipv4_via_borrowed = via_from_mirror(&ipv4_via);
    let ipv6_via_borrowed = via_from_mirror(&ipv6_via);

    let mut remaining = std::collections::BTreeSet::new();
    for ip in installed {
        let result = match ip {
            IpAddr::V4(dest) => match ipv4_via_borrowed.as_ref() {
                Some(via) => ifcfg.remove_ipv4_host_route(dest, via).await,
                None => {
                    tracing::warn!(
                        %ip,
                        "no via snapshot for installed underlay exclude; keeping it accounted"
                    );
                    remaining.insert(ip);
                    continue;
                }
            },
            IpAddr::V6(dest) => match ipv6_via_borrowed.as_ref() {
                Some(via) => ifcfg.remove_ipv6_host_route(dest, via).await,
                None => {
                    tracing::warn!(
                        %ip,
                        "no via snapshot for installed underlay exclude; keeping it accounted"
                    );
                    remaining.insert(ip);
                    continue;
                }
            },
        };
        match result {
            Ok(()) => {}
            Err(error) if crate::common::ifcfg::route_remove_already_satisfied(&error) => {}
            Err(error) => {
                tracing::warn!(
                    ?error,
                    %ip,
                    "failed to remove underlay exclude route on stop; keeping it accounted"
                );
                remaining.insert(ip);
            }
        }
    }

    UnderlayExcludeState {
        installed: remaining,
        ipv4_via,
        ipv6_via,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::common::error::Error;
    use std::{collections::BTreeSet, net::Ipv4Addr};

    use async_trait::async_trait;

    /// Minimal stateful mock: records deletions, can be told to fail a set of
    /// destinations. Everything else falls back to the trait's no-op defaults
    /// (same shape as `DummyIfConfiger`).
    struct MockExcludeIfConfiger {
        removed: std::sync::Mutex<BTreeSet<IpAddr>>,
        fail: std::sync::Mutex<BTreeSet<IpAddr>>,
    }

    impl MockExcludeIfConfiger {
        fn new(fail: &[IpAddr]) -> Self {
            Self {
                removed: std::sync::Mutex::new(BTreeSet::new()),
                fail: std::sync::Mutex::new(fail.iter().copied().collect()),
            }
        }

        fn removed(&self) -> BTreeSet<IpAddr> {
            self.removed.lock().unwrap().clone()
        }
    }

    #[async_trait]
    impl IfConfiguerTrait for MockExcludeIfConfiger {
        async fn remove_ipv4_host_route(
            &self,
            dest: Ipv4Addr,
            _via: &PhysicalDefaultRoute,
        ) -> Result<(), Error> {
            let fail = self.fail.lock().unwrap();
            if fail.contains(&IpAddr::V4(dest)) {
                return Err(Error::AnyhowError(anyhow::anyhow!("mock delete failure")));
            }
            self.removed.lock().unwrap().insert(IpAddr::V4(dest));
            Ok(())
        }

        async fn remove_ipv6_host_route(
            &self,
            dest: std::net::Ipv6Addr,
            _via: &PhysicalDefaultRoute,
        ) -> Result<(), Error> {
            self.removed.lock().unwrap().insert(IpAddr::V6(dest));
            Ok(())
        }
    }

    fn v4_mirror() -> UnderlayExcludeViaMirror {
        UnderlayExcludeViaMirror {
            ifindex: 7,
            ifname: "ens256".to_string(),
            gateway: Some(IpAddr::V4("192.168.10.1".parse().unwrap())),
        }
    }

    fn state_with(
        ips: &[IpAddr],
        ipv4_via: Option<UnderlayExcludeViaMirror>,
    ) -> UnderlayExcludeState {
        UnderlayExcludeState {
            installed: ips.iter().copied().collect(),
            ipv4_via,
            ipv6_via: None,
        }
    }

    fn lan_ip(name: &str) -> IpAddr {
        IpAddr::V4(name.parse().unwrap())
    }

    #[test]
    fn all_removed_yields_empty_state() {
        let ip = lan_ip("43.154.13.211");
        let ifcfg = MockExcludeIfConfiger::new(&[]);
        let state = state_with(&[ip], Some(v4_mirror()));
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let remaining = rt.block_on(remove_installed_underlay_excludes(&ifcfg, state));
        assert!(remaining.installed.is_empty());
        assert!(ifcfg.removed().contains(&ip));
    }

    #[test]
    fn failed_removal_stays_accounted() {
        let ok_ip = lan_ip("43.154.13.211");
        let fail_ip = lan_ip("101.43.96.62");
        let ifcfg = MockExcludeIfConfiger::new(&[fail_ip]);
        let state = state_with(&[ok_ip, fail_ip], Some(v4_mirror()));
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let remaining = rt.block_on(remove_installed_underlay_excludes(&ifcfg, state));
        // 失败条目必须留在状态里（丢账 = G2 复发），成功条目不再出现。
        assert_eq!(remaining.installed, BTreeSet::from([fail_ip]));
        assert!(remaining.ipv4_via.is_some());
        assert!(ifcfg.removed().contains(&ok_ip));
        assert!(!ifcfg.removed().contains(&fail_ip));
    }

    #[test]
    fn missing_via_snapshot_stays_accounted() {
        let ip = lan_ip("124.222.29.94");
        let ifcfg = MockExcludeIfConfiger::new(&[]);
        let state = state_with(&[ip], None);
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let remaining = rt.block_on(remove_installed_underlay_excludes(&ifcfg, state));
        assert_eq!(remaining.installed, BTreeSet::from([ip]));
        assert!(ifcfg.removed().is_empty());
    }

    #[test]
    fn already_gone_counts_as_removed() {
        // Mock 对未登记失败的目标一律返回 Ok；这里用另一个 mock 变体模拟
        // 「路由已不存在」（NotFound）——应视为删除成功而不是失败。
        struct GoneIfConfiger;
        #[async_trait]
        impl IfConfiguerTrait for GoneIfConfiger {
            async fn remove_ipv4_host_route(
                &self,
                _dest: Ipv4Addr,
                _via: &PhysicalDefaultRoute,
            ) -> Result<(), Error> {
                Err(Error::NotFound)
            }
        }
        let ip = lan_ip("172.25.0.1");
        let state = state_with(&[ip], Some(v4_mirror()));
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let remaining = rt.block_on(remove_installed_underlay_excludes(&GoneIfConfiger, state));
        assert!(remaining.installed.is_empty());
    }
}
