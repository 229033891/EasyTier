//
// Port supporting code for wireguard-nt from wireguard-windows v0.5.3 to Rust
// This file replicates parts of wireguard-windows/tunnel/winipcfg/types.go
//

use cidr::{Ipv4Inet, Ipv6Inet};
use std::net::{Ipv4Addr, Ipv6Addr};
use windows::Win32::Networking::WinSock::{
    AF_INET, AF_INET6, IN_ADDR, IN_ADDR_0, IN6_ADDR, IN6_ADDR_0, SOCKADDR_IN, SOCKADDR_IN6,
};

#[derive(Hash, Eq, PartialEq, Debug)]
pub struct RouteDataIpv4 {
    pub destination: Ipv4Inet,
    pub next_hop: Ipv4Addr,
    pub metric: u32,
}

#[derive(Hash, Eq, PartialEq, Debug)]
pub struct RouteDataIpv6 {
    pub destination: Ipv6Inet,
    pub next_hop: Ipv6Addr,
    pub metric: u32,
}

/// Convert std::net::Ipv4Addr to Windows IN_ADDR.
#[inline]
pub fn convert_ipv4addr_to_inaddr(ip: &Ipv4Addr) -> IN_ADDR {
    IN_ADDR {
        S_un: IN_ADDR_0 {
            S_addr: u32::from_ne_bytes(ip.octets()),
        },
    }
}

/// Convert std::net::Ipv6Addr to Windows IN6_ADDR.
#[inline]
pub fn convert_ipv6addr_to_inaddr(ip: &Ipv6Addr) -> IN6_ADDR {
    IN6_ADDR {
        u: IN6_ADDR_0 { Byte: ip.octets() },
    }
}

/// Convert std::net::Ipv4Addr to Windows SOCKADDR_IN.
pub fn convert_ipv4addr_to_sockaddr(ip: &Ipv4Addr) -> SOCKADDR_IN {
    SOCKADDR_IN {
        sin_family: AF_INET,
        sin_addr: convert_ipv4addr_to_inaddr(ip),
        ..Default::default()
    }
}

/// Convert std::net::Ipv6Addr to Windows SOCKADDR_IN6.
pub fn convert_ipv6addr_to_sockaddr(ip: &Ipv6Addr) -> SOCKADDR_IN6 {
    SOCKADDR_IN6 {
        sin6_family: AF_INET6,
        sin6_addr: convert_ipv6addr_to_inaddr(ip),
        ..Default::default()
    }
}
