use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

use super::{Error, IfConfiguerTrait, cidr_to_subnet_mask, run_shell_cmd};
use async_trait::async_trait;
use cidr::{Ipv4Inet, Ipv6Inet};
use tokio::process::Command;

pub struct MacIfConfiger {}
#[async_trait]
impl IfConfiguerTrait for MacIfConfiger {
    async fn add_ipv4_route(
        &self,
        name: &str,
        address: Ipv4Addr,
        cidr_prefix: u8,
        cost: Option<i32>,
    ) -> Result<(), Error> {
        run_shell_cmd(
            format!(
                "route -n add {} -netmask {} -interface {} -hopcount {}",
                address,
                cidr_to_subnet_mask(cidr_prefix),
                name,
                cost.unwrap_or(super::implicit_route_metric(cidr_prefix, 7, 1) as i32)
            )
            .as_str(),
        )
        .await
    }

    async fn remove_ipv4_route(
        &self,
        name: &str,
        address: Ipv4Addr,
        cidr_prefix: u8,
        _metric: Option<u32>,
    ) -> Result<(), Error> {
        // Darwin `route delete` cannot filter by hopcount reliably; dest+iface
        // is unique enough for our TUN defaults (exit vs peer share one iface).
        run_shell_cmd(
            format!(
                "route -n delete {} -netmask {} -interface {}",
                address,
                cidr_to_subnet_mask(cidr_prefix),
                name
            )
            .as_str(),
        )
        .await
    }

    async fn add_ipv4_ip(
        &self,
        name: &str,
        address: Ipv4Addr,
        cidr_prefix: u8,
    ) -> Result<(), Error> {
        run_shell_cmd(
            format!(
                "ifconfig {} {:?}/{:?} {:?} up",
                name, address, cidr_prefix, address,
            )
            .as_str(),
        )
        .await
    }

    async fn set_link_status(&self, name: &str, up: bool) -> Result<(), Error> {
        run_shell_cmd(format!("ifconfig {} {}", name, if up { "up" } else { "down" }).as_str())
            .await
    }

    async fn remove_ip(&self, name: &str, ip: Option<Ipv4Inet>) -> Result<(), Error> {
        if let Some(ip) = ip {
            run_shell_cmd(format!("ifconfig {} inet {} delete", name, ip.address()).as_str()).await
        } else {
            run_shell_cmd(format!("ifconfig {} inet delete", name).as_str()).await
        }
    }

    async fn set_mtu(&self, name: &str, mtu: u32) -> Result<(), Error> {
        run_shell_cmd(format!("ifconfig {} mtu {}", name, mtu).as_str()).await
    }

    async fn add_ipv6_ip(
        &self,
        name: &str,
        address: std::net::Ipv6Addr,
        cidr_prefix: u8,
    ) -> Result<(), Error> {
        run_shell_cmd(format!("ifconfig {} inet6 {}/{} add", name, address, cidr_prefix).as_str())
            .await
    }

    async fn remove_ipv6(&self, name: &str, ip: Option<Ipv6Inet>) -> Result<(), Error> {
        if let Some(ip) = ip {
            run_shell_cmd(format!("ifconfig {} inet6 {} delete", name, ip.address()).as_str()).await
        } else {
            // Remove all IPv6 addresses is more complex on macOS, just succeed
            Ok(())
        }
    }

    async fn add_ipv6_route(
        &self,
        name: &str,
        address: std::net::Ipv6Addr,
        cidr_prefix: u8,
        cost: Option<i32>,
    ) -> Result<(), Error> {
        let hopcount = cost.unwrap_or(super::implicit_route_metric(cidr_prefix, 7, 1) as i32);
        let cmd = format!(
            "route -n add -inet6 {}/{} -interface {} -hopcount {}",
            address, cidr_prefix, name, hopcount
        );
        run_shell_cmd(cmd.as_str()).await
    }

    async fn remove_ipv6_route(
        &self,
        name: &str,
        address: std::net::Ipv6Addr,
        cidr_prefix: u8,
        _metric: Option<u32>,
    ) -> Result<(), Error> {
        run_shell_cmd(
            format!(
                "route -n delete -inet6 {}/{} -interface {}",
                address, cidr_prefix, name
            )
            .as_str(),
        )
        .await
    }

    fn specific_route_metric(&self) -> i32 {
        7
    }

    fn default_route_metric(&self) -> i32 {
        1
    }

    async fn find_ipv4_physical_default(
        &self,
        exclude_ifname: &str,
    ) -> Result<Option<super::PhysicalDefaultRoute>, Error> {
        find_darwin_default("inet", exclude_ifname).await
    }

    async fn find_ipv6_physical_default(
        &self,
        exclude_ifname: &str,
    ) -> Result<Option<super::PhysicalDefaultRoute>, Error> {
        find_darwin_default("inet6", exclude_ifname).await
    }

    async fn add_ipv4_host_route(
        &self,
        dest: Ipv4Addr,
        via: &super::PhysicalDefaultRoute,
        cost: Option<i32>,
    ) -> Result<(), Error> {
        let hopcount = cost.unwrap_or(1);
        let cmd = match via.gateway {
            Some(gw) => format!("route -n add -host {} {} -hopcount {}", dest, gw, hopcount),
            None => format!(
                "route -n add -host {} -interface {} -hopcount {}",
                dest, via.ifname, hopcount
            ),
        };
        run_shell_cmd(cmd.as_str()).await
    }

    async fn remove_ipv4_host_route(
        &self,
        dest: Ipv4Addr,
        via: &super::PhysicalDefaultRoute,
    ) -> Result<(), Error> {
        let cmd = match via.gateway {
            Some(gw) => format!("route -n delete -host {} {}", dest, gw),
            None => format!("route -n delete -host {} -interface {}", dest, via.ifname),
        };
        run_shell_cmd(cmd.as_str()).await
    }

    async fn add_ipv6_host_route(
        &self,
        dest: Ipv6Addr,
        via: &super::PhysicalDefaultRoute,
        cost: Option<i32>,
    ) -> Result<(), Error> {
        let hopcount = cost.unwrap_or(1);
        let cmd = match via.gateway {
            Some(gw) => format!(
                "route -n add -inet6 -host {} {} -hopcount {}",
                dest, gw, hopcount
            ),
            None => format!(
                "route -n add -inet6 -host {} -interface {} -hopcount {}",
                dest, via.ifname, hopcount
            ),
        };
        run_shell_cmd(cmd.as_str()).await
    }

    async fn remove_ipv6_host_route(
        &self,
        dest: Ipv6Addr,
        via: &super::PhysicalDefaultRoute,
    ) -> Result<(), Error> {
        let cmd = match via.gateway {
            Some(gw) => format!("route -n delete -inet6 -host {} {}", dest, gw),
            None => format!(
                "route -n delete -inet6 -host {} -interface {}",
                dest, via.ifname
            ),
        };
        run_shell_cmd(cmd.as_str()).await
    }
}

async fn find_darwin_default(
    family: &str,
    exclude_ifname: &str,
) -> Result<Option<super::PhysicalDefaultRoute>, Error> {
    let output = if family == "inet6" {
        Command::new("route")
            .args(["-n", "get", "-inet6", "default"])
            .output()
            .await
    } else {
        Command::new("route")
            .args(["-n", "get", "default"])
            .output()
            .await
    }
    .map_err(Error::from)?;
    if !output.status.success() {
        return Ok(None);
    }
    let text = String::from_utf8_lossy(&output.stdout);
    let mut gateway = None;
    let mut ifname = None;
    for line in text.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("gateway:") {
            gateway = rest.trim().parse().ok();
        } else if let Some(rest) = line.strip_prefix("interface:") {
            ifname = Some(rest.trim().to_string());
        }
    }
    let Some(ifname) = ifname else {
        return Ok(None);
    };
    if ifname == exclude_ifname {
        return Ok(None);
    }
    let ifindex = {
        use network_interface::NetworkInterfaceConfig as _;
        network_interface::NetworkInterface::show()
            .ok()
            .and_then(|ifaces| {
                ifaces
                    .into_iter()
                    .find(|iface| iface.name == ifname)
                    .map(|iface| iface.index)
            })
            .unwrap_or(0)
    };
    Ok(Some(super::PhysicalDefaultRoute {
        ifindex,
        ifname,
        gateway,
    }))
}
