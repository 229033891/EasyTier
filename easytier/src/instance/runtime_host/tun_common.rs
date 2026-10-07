use std::{
    any::Any,
    net::{IpAddr, Ipv6Addr},
    sync::{Arc, OnceLock},
};

use easytier_core::{host::packet::HostPacketReceiver, instance::CorePacketPlane};
use tokio::{sync::Mutex, task::JoinSet};

use super::MagicDnsRuntime;
use crate::{
    common::{
        global_ctx::ArcGlobalCtx,
        ifcfg::{IfConfiger, IfConfiguerTrait},
    },
    instance::virtual_nic::NicCtx,
};

struct NicCtxContainer {
    _nic_ctx: Option<Box<dyn Any + Send>>,
    magic_dns: MagicDnsRuntime,
}

impl NicCtxContainer {
    fn new(nic_ctx: NicCtx, magic_dns: MagicDnsRuntime) -> Self {
        Self {
            _nic_ctx: Some(Box::new(nic_ctx)),
            magic_dns,
        }
    }

    fn packet_drain(tasks: JoinSet<()>) -> Self {
        Self {
            _nic_ctx: Some(Box::new(tasks)),
            magic_dns: MagicDnsRuntime::default(),
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
        if let Some(nic) = old.as_mut() {
            nic.magic_dns.stop().await;
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
    cleanup_underlay_exclude_routes(packet_plane, &ifname, &ifcfg).await;
}

/// 删除 exit-node 模式为保住 underlay 而钉在物理网卡上的主机路由。
///
/// 用与 updater 相同的 desired 集合（`underlay_exclude_ips`）和同一条物理默认路由
/// 推导出目标条目，因此不会误删用户自己或其它实例的路由。
async fn cleanup_underlay_exclude_routes(
    packet_plane: &Arc<CorePacketPlane>,
    tun_ifname: &str,
    ifcfg: &impl IfConfiguerTrait,
) {
    let desired = packet_plane.underlay_exclude_ips().await;
    if desired.is_empty() {
        return;
    }

    let ipv4_via = ifcfg
        .find_ipv4_physical_default(tun_ifname)
        .await
        .ok()
        .flatten();
    let ipv6_via = ifcfg
        .find_ipv6_physical_default(tun_ifname)
        .await
        .ok()
        .flatten();

    for ip in desired {
        let result = match ip {
            IpAddr::V4(dest) => match ipv4_via.as_ref() {
                Some(via) => ifcfg.remove_ipv4_host_route(dest, via).await,
                None => continue,
            },
            IpAddr::V6(dest) => match ipv6_via.as_ref() {
                Some(via) => ifcfg.remove_ipv6_host_route(dest, via).await,
                None => continue,
            },
        };
        if let Err(error) = result
            && !crate::common::ifcfg::route_remove_already_satisfied(&error)
        {
            tracing::debug!(
                ?error,
                %ip,
                tun_ifname,
                "failed to remove underlay exclude route on stop",
            );
        }
    }
}
