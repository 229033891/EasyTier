mod records;

#[cfg(feature = "proxy-packet")]
mod packet;

/// P3: wraps a resolver to feed observed DNS answers into the unified routing
/// table. Only compiled with the MagicDNS packet filter it plugs into.
#[cfg(feature = "proxy-packet")]
mod observe;

mod os_wired;

pub use records::{
    MagicDnsRecordSnapshot, MagicDnsRecordStore, MagicDnsRoute, MagicDnsRouteAdvertisement,
    MagicDnsRoutePublisher, MagicDnsRouteSnapshot, MagicDnsRouteSource,
    run_magic_dns_route_publisher,
};

pub use os_wired::{clear_magic_dns_os_wired, get_magic_dns_os_wired, set_magic_dns_os_wired};

#[cfg(feature = "proxy-packet")]
pub(crate) use observe::ObservingDnsResolver;
#[cfg(feature = "proxy-packet")]
pub(crate) use packet::magic_dns_packet_filter;
#[cfg(feature = "proxy-packet")]
pub use packet::{
    MagicDnsQuery, MagicDnsQueryResolver, MagicDnsResolverRegistration, process_magic_dns_packet,
};
