#![cfg_attr(
    not(any(feature = "public-ipv6-provider", feature = "tun")),
    allow(dead_code)
)]

#[cfg(any(
    all(target_os = "macos", not(feature = "macos-ne")),
    target_os = "freebsd"
))]
mod darwin;
#[cfg(all(target_os = "linux", feature = "linux-netlink"))]
mod netlink;
#[cfg(all(target_os = "linux", feature = "linux-netlink"))]
mod netlink_wire;
#[cfg(target_os = "windows")]
mod win;
#[cfg(target_os = "windows")]
mod windows;

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

use async_trait::async_trait;
use cidr::{Ipv4Inet, Ipv6Inet};
#[cfg(any(
    all(target_os = "macos", not(feature = "macos-ne")),
    target_os = "freebsd"
))]
use tokio::process::Command;

use super::error::Error;

/// Cost used when the caller does not pass an explicit route metric.
/// Prefix `/0` (default route) uses `default_route` so TUN can win over a
/// typical LAN default; more-specific CIDRs keep `specific` so they do not
/// steal on-link or host routes.
pub(crate) fn implicit_route_metric(cidr_prefix: u8, specific: u32, default_route: u32) -> u32 {
    if cidr_prefix == 0 {
        default_route
    } else {
        specific
    }
}

/// Public IPv6 provider default (`::/0`) metric. Must be higher than
/// [`EXIT_IPV6_DEFAULT_METRIC`] so an exit-node default wins while both exist.
pub(crate) const PUBLIC_IPV6_DEFAULT_METRIC: i32 = 5;
/// Exit-node IPv6 default (`::/0`) metric. Kept distinct from
/// [`PUBLIC_IPV6_DEFAULT_METRIC`] so netlink deletes can target one writer.
pub(crate) const EXIT_IPV6_DEFAULT_METRIC: i32 = 1;

/// Physical (non-TUN) default route used to pin underlay host routes so peer
/// tunnels / STUN stay off the exit-node TUN default.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PhysicalDefaultRoute {
    pub ifindex: u32,
    pub ifname: String,
    pub gateway: Option<IpAddr>,
}

/// Treat "already present" as success for **add** ops so reconcile can stop retrying.
/// Does **not** treat NotFound / missing-interface as success (that would fake an install).
pub(crate) fn route_add_already_satisfied(err: &Error) -> bool {
    match err {
        Error::NotFound => false,
        Error::IOError(io_err) => io_error_already_exists(io_err),
        Error::AnyhowError(anyhow_err) => {
            if let Some(io_err) = anyhow_err.downcast_ref::<std::io::Error>() {
                return io_error_already_exists(io_err);
            }
            error_text_already_exists(&anyhow_err.to_string())
        }
        Error::ShellCommandError(text) => error_text_already_exists(text),
        _ => false,
    }
}

/// Treat "already gone" as success for **remove** ops so reconcile can stop retrying.
pub(crate) fn route_remove_already_satisfied(err: &Error) -> bool {
    match err {
        Error::NotFound => true,
        Error::IOError(io_err) => io_error_already_gone(io_err),
        Error::AnyhowError(anyhow_err) => {
            if let Some(io_err) = anyhow_err.downcast_ref::<std::io::Error>() {
                return io_error_already_gone(io_err);
            }
            error_text_already_gone(&anyhow_err.to_string())
        }
        Error::ShellCommandError(text) => error_text_already_gone(text),
        _ => false,
    }
}

fn io_error_already_exists(err: &std::io::Error) -> bool {
    if matches!(err.kind(), std::io::ErrorKind::AlreadyExists) {
        return true;
    }
    match err.raw_os_error() {
        #[cfg(windows)]
        Some(183 | 5010) => true,
        #[cfg(unix)]
        Some(17) => true,
        _ => error_text_already_exists(&err.to_string()),
    }
}

fn io_error_already_gone(err: &std::io::Error) -> bool {
    if matches!(err.kind(), std::io::ErrorKind::NotFound) {
        return true;
    }
    match err.raw_os_error() {
        #[cfg(windows)]
        Some(1168 | 2 | 3) => true,
        #[cfg(unix)]
        Some(2 | 3) => true,
        _ => error_text_already_gone(&err.to_string()),
    }
}

fn error_text_already_exists(text: &str) -> bool {
    // Match localized Win32 text (e.g. Chinese "对象已存在") before ascii lowercasing.
    if text.contains("对象已存在") {
        return true;
    }
    let text = text.to_ascii_lowercase();
    text.contains("(code: 183)")
        || text.contains("(code: 5010)")
        || text.contains("file exists")
        || text.contains("already exists")
        || text.contains("object already exists")
}

fn error_text_already_gone(text: &str) -> bool {
    let text = text.to_ascii_lowercase();
    text.contains("(code: 1168)")
        || text.contains("not in table")
        || text.contains("not found")
        || text.contains("cannot find")
        || text.contains("no such process")
        || text.contains("no such file")
}

#[async_trait]
pub trait IfConfiguerTrait: Send + Sync {
    async fn add_ipv4_route(
        &self,
        _name: &str,
        _address: Ipv4Addr,
        _cidr_prefix: u8,
        _cost: Option<i32>,
    ) -> Result<(), Error> {
        Ok(())
    }
    /// Remove an IPv4 route matching destination / prefix / interface.
    ///
    /// `metric`:
    /// - `Some(priority)` — **best-effort** exact priority match. Honored on
    ///   Linux netlink; **ignored** on Windows / Darwin where the platform API
    ///   cannot filter by metric (those backends delete by dest+iface only).
    /// - `None` — delete the first matching dest/oif (any metric).
    async fn remove_ipv4_route(
        &self,
        _name: &str,
        _address: Ipv4Addr,
        _cidr_prefix: u8,
        _metric: Option<u32>,
    ) -> Result<(), Error> {
        Ok(())
    }
    async fn add_ipv4_ip(
        &self,
        _name: &str,
        _address: Ipv4Addr,
        _cidr_prefix: u8,
    ) -> Result<(), Error> {
        Ok(())
    }
    async fn add_ipv6_route(
        &self,
        _name: &str,
        _address: Ipv6Addr,
        _cidr_prefix: u8,
        _cost: Option<i32>,
    ) -> Result<(), Error> {
        Ok(())
    }
    /// Remove an IPv6 route. See [`Self::remove_ipv4_route`] for `metric`
    /// semantics (exact priority on Linux; dest+iface only on Windows/Darwin).
    async fn remove_ipv6_route(
        &self,
        _name: &str,
        _address: Ipv6Addr,
        _cidr_prefix: u8,
        _metric: Option<u32>,
    ) -> Result<(), Error> {
        Ok(())
    }
    async fn add_ipv6_ip(
        &self,
        _name: &str,
        _address: Ipv6Addr,
        _cidr_prefix: u8,
    ) -> Result<(), Error> {
        Ok(())
    }
    async fn set_link_status(&self, _name: &str, _up: bool) -> Result<(), Error> {
        Ok(())
    }
    async fn remove_ip(&self, _name: &str, _ip: Option<Ipv4Inet>) -> Result<(), Error> {
        Ok(())
    }
    async fn remove_ipv6(&self, _name: &str, _ip: Option<Ipv6Inet>) -> Result<(), Error> {
        Ok(())
    }
    async fn wait_interface_show(&self, _name: &str) -> Result<(), Error> {
        return Ok(());
    }
    async fn set_mtu(&self, _name: &str, _mtu: u32) -> Result<(), Error> {
        Ok(())
    }

    /// Metric used for non-exit (more-specific or peer-advertised `/0`) routes.
    fn specific_route_metric(&self) -> i32 {
        9000
    }

    /// Metric used when `add_*_route` is called with `cost: None` for a `/0`.
    fn default_route_metric(&self) -> i32 {
        1
    }

    /// Resolve the kernel priority that `add_*_route` would install for `cost`.
    fn resolve_route_metric(&self, cidr_prefix: u8, cost: Option<i32>) -> u32 {
        cost.map(|v| v as u32).unwrap_or_else(|| {
            implicit_route_metric(
                cidr_prefix,
                self.specific_route_metric() as u32,
                self.default_route_metric() as u32,
            )
        })
    }

    /// Find the best IPv4 default route that is **not** on `exclude_ifname` (TUN).
    async fn find_ipv4_physical_default(
        &self,
        _exclude_ifname: &str,
    ) -> Result<Option<PhysicalDefaultRoute>, Error> {
        Ok(None)
    }

    async fn find_ipv6_physical_default(
        &self,
        _exclude_ifname: &str,
    ) -> Result<Option<PhysicalDefaultRoute>, Error> {
        Ok(None)
    }

    /// Host `/32` via the captured physical default (never on TUN).
    async fn add_ipv4_host_route(
        &self,
        _dest: Ipv4Addr,
        _via: &PhysicalDefaultRoute,
        _cost: Option<i32>,
    ) -> Result<(), Error> {
        Ok(())
    }

    async fn remove_ipv4_host_route(
        &self,
        _dest: Ipv4Addr,
        _via: &PhysicalDefaultRoute,
    ) -> Result<(), Error> {
        Ok(())
    }

    async fn add_ipv6_host_route(
        &self,
        _dest: Ipv6Addr,
        _via: &PhysicalDefaultRoute,
        _cost: Option<i32>,
    ) -> Result<(), Error> {
        Ok(())
    }

    async fn remove_ipv6_host_route(
        &self,
        _dest: Ipv6Addr,
        _via: &PhysicalDefaultRoute,
    ) -> Result<(), Error> {
        Ok(())
    }
}

#[cfg(any(
    all(target_os = "macos", not(feature = "macos-ne")),
    target_os = "freebsd"
))]
fn cidr_to_subnet_mask(prefix_length: u8) -> Ipv4Addr {
    if prefix_length > 32 {
        panic!("Invalid CIDR prefix length");
    }

    let subnet_mask: u32 = (!0u32)
        .checked_shl(32 - u32::from(prefix_length))
        .unwrap_or(0);
    Ipv4Addr::new(
        ((subnet_mask >> 24) & 0xFF) as u8,
        ((subnet_mask >> 16) & 0xFF) as u8,
        ((subnet_mask >> 8) & 0xFF) as u8,
        (subnet_mask & 0xFF) as u8,
    )
}

#[cfg(any(
    all(target_os = "macos", not(feature = "macos-ne")),
    target_os = "freebsd"
))]
async fn run_shell_cmd(cmd: &str) -> Result<(), Error> {
    let cmd_out: std::process::Output;
    let stdout: String;
    let stderr: String;
    #[cfg(target_os = "windows")]
    {
        const CREATE_NO_WINDOW: u32 = 0x08000000;
        cmd_out = Command::new("cmd")
            .stdin(std::process::Stdio::null())
            .arg("/C")
            .arg(cmd)
            .creation_flags(CREATE_NO_WINDOW)
            .output()
            .await?;
        stdout = crate::utils::string::utf8_or_gbk_to_string(cmd_out.stdout.as_slice());
        stderr = crate::utils::string::utf8_or_gbk_to_string(cmd_out.stderr.as_slice());
    };

    #[cfg(not(target_os = "windows"))]
    {
        cmd_out = Command::new("sh").arg("-c").arg(cmd).output().await?;
        stdout = String::from_utf8_lossy(cmd_out.stdout.as_slice()).to_string();
        stderr = String::from_utf8_lossy(cmd_out.stderr.as_slice()).to_string();
    };

    let ec = cmd_out.status.code();
    let succ = cmd_out.status.success();
    tracing::info!(?cmd, ?ec, ?succ, ?stdout, ?stderr, "run shell cmd");

    if !cmd_out.status.success() {
        return Err(Error::ShellCommandError(stdout + &stderr));
    }
    Ok(())
}

pub struct DummyIfConfiger {}
#[async_trait]
impl IfConfiguerTrait for DummyIfConfiger {}

#[cfg(all(target_os = "linux", feature = "linux-netlink"))]
pub type IfConfiger = netlink::NetlinkIfConfiger;
#[cfg(all(target_os = "linux", not(feature = "linux-netlink")))]
pub type IfConfiger = DummyIfConfiger;

#[cfg(any(
    all(target_os = "macos", not(feature = "macos-ne")),
    target_os = "freebsd"
))]
pub type IfConfiger = darwin::MacIfConfiger;

#[cfg(target_os = "windows")]
pub type IfConfiger = windows::WindowsIfConfiger;

#[cfg(not(any(
    all(target_os = "macos", not(feature = "macos-ne")),
    target_os = "linux",
    target_os = "windows",
    target_os = "freebsd",
)))]
pub type IfConfiger = DummyIfConfiger;

#[cfg(target_os = "windows")]
pub use windows::RegistryManager;

#[cfg(all(target_os = "linux", feature = "linux-netlink"))]
pub(crate) use netlink_wire::RouteMessage;
#[cfg(all(
    target_os = "linux",
    feature = "linux-netlink",
    feature = "public-ipv6-provider"
))]
pub(crate) use netlink_wire::RouteType;

#[cfg(all(target_os = "linux", feature = "linux-netlink"))]
pub(crate) fn list_ipv6_route_messages() -> Result<Vec<RouteMessage>, Error> {
    netlink::NetlinkIfConfiger::list_ipv6_route_messages()
}

#[cfg(all(target_os = "linux", feature = "linux-netlink"))]
pub(crate) fn get_interface_index(name: &str) -> Result<u32, Error> {
    netlink::NetlinkIfConfiger::get_interface_index(name)
}

#[cfg(all(target_os = "linux", feature = "linux-netlink"))]
pub(crate) fn add_ipv6_ndp_proxy(name: &str, address: Ipv6Addr) -> Result<(), Error> {
    netlink::NetlinkIfConfiger::add_ipv6_ndp_proxy(name, address)
}

#[cfg(all(target_os = "linux", feature = "linux-netlink"))]
pub(crate) fn remove_ipv6_ndp_proxy(name: &str, address: Ipv6Addr) -> Result<(), Error> {
    netlink::NetlinkIfConfiger::remove_ipv6_ndp_proxy(name, address)
}

#[cfg(all(target_os = "linux", feature = "linux-netlink"))]
pub(crate) fn list_ipv6_ndp_proxy(
    name: &str,
) -> Result<std::collections::BTreeSet<Ipv6Addr>, Error> {
    netlink::NetlinkIfConfiger::list_ipv6_ndp_proxy(name)
}

#[cfg(test)]
mod tests {
    use super::implicit_route_metric;

    #[test]
    fn default_prefix_uses_default_route_metric() {
        assert_eq!(implicit_route_metric(0, 9000, 1), 1);
        assert_eq!(implicit_route_metric(0, 65535, 50), 50);
    }

    #[test]
    fn specific_prefix_keeps_high_metric() {
        assert_eq!(implicit_route_metric(24, 9000, 1), 9000);
        assert_eq!(implicit_route_metric(32, 65535, 50), 65535);
    }

    #[test]
    fn add_and_remove_idempotency_are_separated() {
        use super::{Error, route_add_already_satisfied, route_remove_already_satisfied};

        assert!(!route_add_already_satisfied(&Error::NotFound));
        assert!(route_remove_already_satisfied(&Error::NotFound));

        #[cfg(unix)]
        {
            assert!(route_add_already_satisfied(&Error::IOError(
                std::io::Error::from_raw_os_error(17)
            )));
            assert!(!route_add_already_satisfied(&Error::IOError(
                std::io::Error::from_raw_os_error(2)
            )));
            assert!(route_remove_already_satisfied(&Error::IOError(
                std::io::Error::from_raw_os_error(2)
            )));
        }
        #[cfg(windows)]
        {
            assert!(route_add_already_satisfied(&Error::IOError(
                std::io::Error::from_raw_os_error(183)
            )));
            assert!(!route_add_already_satisfied(&Error::IOError(
                std::io::Error::from_raw_os_error(2)
            )));
            assert!(route_remove_already_satisfied(&Error::IOError(
                std::io::Error::from_raw_os_error(1168)
            )));
        }
        assert!(route_add_already_satisfied(&Error::AnyhowError(
            anyhow::anyhow!("Failed to add route: already exists (code: 183)")
        )));
        // Exact shape from Windows CreateIpForwardEntry2 + FormatMessageW (zh-CN).
        assert!(route_add_already_satisfied(&Error::AnyhowError(
            anyhow::anyhow!("Failed to add host route: 对象已存在。 (code: 5010)")
        )));
        #[cfg(windows)]
        assert!(route_add_already_satisfied(&Error::IOError(
            std::io::Error::from_raw_os_error(5010)
        )));
        assert!(!route_add_already_satisfied(&Error::ShellCommandError(
            "route: not in table".to_string()
        )));
        assert!(route_remove_already_satisfied(&Error::ShellCommandError(
            "route: not in table".to_string()
        )));
        assert!(!route_add_already_satisfied(&Error::AnyhowError(
            anyhow::anyhow!("access denied (code: 5)")
        )));
        assert!(!route_remove_already_satisfied(&Error::AnyhowError(
            anyhow::anyhow!("access denied (code: 5)")
        )));
    }
}
