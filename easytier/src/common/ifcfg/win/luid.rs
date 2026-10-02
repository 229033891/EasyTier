//
// Port supporting code from wireguard-windows, as used by 3rd-party/wireguard-go, to Rust
// This file implements functionality similar to: wireguard-windows/tunnel/winipcfg/luid.go
//
// ATTENTION: NOT included are DNS() and SetDNS() - functions to query and set DNS servers for a network interface.
//

use super::types::*;
use cidr::Ipv4Inet;
use cidr::Ipv6Inet;
use std::net::{Ipv4Addr, Ipv6Addr};
use std::ptr;
use windows::Win32::Foundation::{ERROR_NOT_FOUND, NO_ERROR, WIN32_ERROR};
use windows::Win32::NetworkManagement::IpHelper::{
    ConvertInterfaceIndexToLuid, CreateIpForwardEntry2, CreateUnicastIpAddressEntry,
    DeleteIpForwardEntry2, DeleteUnicastIpAddressEntry, FreeMibTable, GetIpForwardEntry2,
    GetIpInterfaceEntry, GetUnicastIpAddressTable, InitializeIpForwardEntry,
    InitializeUnicastIpAddressEntry, MIB_IPFORWARD_ROW2, MIB_IPINTERFACE_ROW,
    MIB_UNICASTIPADDRESS_ROW, MIB_UNICASTIPADDRESS_TABLE, SetIpInterfaceEntry,
};
use windows::Win32::NetworkManagement::Ndis::NET_LUID_LH;
use windows::Win32::Networking::WinSock::{
    ADDRESS_FAMILY, AF_INET, AF_INET6, IpDadStatePreferred, SOCKADDR_INET,
};

pub struct InterfaceLuid {
    luid: NET_LUID_LH,
}

impl InterfaceLuid {
    /// Convert a local index for a network interface to the LUID for the interface.
    /// https://docs.microsoft.com/en-us/windows/desktop/api/netioapi/nf-netioapi-convertinterfaceindextoluid
    pub fn luid_from_index(interface_index: u32) -> Result<Self, WIN32_ERROR> {
        let mut interface_luid = NET_LUID_LH::default();

        let result = unsafe { ConvertInterfaceIndexToLuid(interface_index, &mut interface_luid) };

        if result == NO_ERROR {
            Ok(Self {
                luid: interface_luid,
            })
        } else {
            Err(result)
        }
    }

    /// Add a new unicast IP address to the interface.
    /// https://docs.microsoft.com/en-us/windows/desktop/api/netioapi/nf-netioapi-createunicastipaddressentry
    pub fn add_ipv4_address(&self, address: &Ipv4Inet) -> Result<(), WIN32_ERROR> {
        let mut row = MIB_UNICASTIPADDRESS_ROW::default();
        unsafe { InitializeUnicastIpAddressEntry(&mut row) };

        row.InterfaceLuid = self.luid;
        row.DadState = IpDadStatePreferred;
        row.ValidLifetime = 0xffffffff;
        row.PreferredLifetime = 0xffffffff;

        row.Address = SOCKADDR_INET {
            Ipv4: convert_ipv4addr_to_sockaddr(&address.address()),
        };
        row.OnLinkPrefixLength = address.network_length();

        let result = unsafe { CreateUnicastIpAddressEntry(&row) };

        if result == NO_ERROR {
            Ok(())
        } else {
            Err(result)
        }
    }

    /// Add a new unicast IPv6 address to the interface.
    pub fn add_ipv6_address(&self, address: &Ipv6Inet) -> Result<(), WIN32_ERROR> {
        let mut row = MIB_UNICASTIPADDRESS_ROW::default();
        unsafe { InitializeUnicastIpAddressEntry(&mut row) };

        row.InterfaceLuid = self.luid;
        row.DadState = IpDadStatePreferred;
        row.ValidLifetime = 0xffffffff;
        row.PreferredLifetime = 0xffffffff;

        row.Address = SOCKADDR_INET {
            Ipv6: convert_ipv6addr_to_sockaddr(&address.address()),
        };
        row.OnLinkPrefixLength = address.network_length();

        let result = unsafe { CreateUnicastIpAddressEntry(&row) };

        if result == NO_ERROR {
            Ok(())
        } else {
            Err(result)
        }
    }

    /// Delete an interface unicast IPv4 address.
    pub fn delete_ipv4_address(&self, address: &Ipv4Inet) -> Result<(), WIN32_ERROR> {
        let mut row = MIB_UNICASTIPADDRESS_ROW::default();
        unsafe { InitializeUnicastIpAddressEntry(&mut row) };

        row.InterfaceLuid = self.luid;
        row.DadState = IpDadStatePreferred;
        row.ValidLifetime = 0xffffffff;
        row.PreferredLifetime = 0xffffffff;

        row.Address = SOCKADDR_INET {
            Ipv4: convert_ipv4addr_to_sockaddr(&address.address()),
        };
        row.OnLinkPrefixLength = address.network_length();

        let result = unsafe { DeleteUnicastIpAddressEntry(&row) };

        if result == NO_ERROR {
            Ok(())
        } else {
            Err(result)
        }
    }

    /// Delete an interface unicast IPv6 address.
    pub fn delete_ipv6_address(&self, address: &Ipv6Inet) -> Result<(), WIN32_ERROR> {
        let mut row = MIB_UNICASTIPADDRESS_ROW::default();
        unsafe { InitializeUnicastIpAddressEntry(&mut row) };

        row.InterfaceLuid = self.luid;
        row.DadState = IpDadStatePreferred;
        row.ValidLifetime = 0xffffffff;
        row.PreferredLifetime = 0xffffffff;

        row.Address = SOCKADDR_INET {
            Ipv6: convert_ipv6addr_to_sockaddr(&address.address()),
        };
        row.OnLinkPrefixLength = address.network_length();

        let result = unsafe { DeleteUnicastIpAddressEntry(&row) };

        if result == NO_ERROR {
            Ok(())
        } else {
            Err(result)
        }
    }

    /// Delete all interface unicast IP addresses for the given address family.
    pub fn flush_ip_addresses(&self, address_family: ADDRESS_FAMILY) -> Result<(), WIN32_ERROR> {
        let mut p_table: *mut MIB_UNICASTIPADDRESS_TABLE = ptr::null_mut();
        let result = unsafe { GetUnicastIpAddressTable(address_family, &mut p_table) };
        if result != NO_ERROR {
            return Err(result);
        }

        assert!(!p_table.is_null());
        let num_entries = unsafe { (*p_table).NumEntries };
        let x_table = unsafe { (*p_table).Table.as_ptr() };
        let self_value = unsafe { self.luid.Value };
        for i in 0..num_entries {
            let current_entry = unsafe { x_table.add(i as _) };
            if unsafe { (*current_entry).InterfaceLuid.Value } == self_value {
                unsafe { let _ = DeleteUnicastIpAddressEntry(current_entry); };
            }
        }

        unsafe { FreeMibTable(p_table as _) };

        Ok(())
    }

    /// Delete all interface unicast IPv4 addresses.
    pub fn flush_ipv4_addresses(&self) -> Result<(), WIN32_ERROR> {
        self.flush_ip_addresses(AF_INET)
    }

    /// Delete all interface unicast IPv6 addresses.
    pub fn flush_ipv6_addresses(&self) -> Result<(), WIN32_ERROR> {
        self.flush_ip_addresses(AF_INET6)
    }

    /// Add an IPv4 route to the interface.
    /// https://docs.microsoft.com/en-us/windows/desktop/api/netioapi/nf-netioapi-createipforwardentry2
    pub fn add_route_ipv4(
        &self,
        destination: &Ipv4Inet,
        next_hop: &Ipv4Addr,
        metric: u32,
    ) -> Result<(), WIN32_ERROR> {
        let mut row = MIB_IPFORWARD_ROW2::default();
        unsafe { InitializeIpForwardEntry(&mut row) };

        row.InterfaceLuid = self.luid;
        row.ValidLifetime = 0xffffffff;
        row.PreferredLifetime = 0xffffffff;

        row.DestinationPrefix.Prefix = SOCKADDR_INET {
            Ipv4: convert_ipv4addr_to_sockaddr(&destination.address()),
        };
        row.DestinationPrefix.PrefixLength = destination.network_length();

        row.NextHop = SOCKADDR_INET {
            Ipv4: convert_ipv4addr_to_sockaddr(next_hop),
        };

        row.Metric = metric;

        let result = unsafe { CreateIpForwardEntry2(&row) };

        if result == NO_ERROR {
            Ok(())
        } else {
            Err(result)
        }
    }

    /// Add an IPv6 route to the interface.
    pub fn add_route_ipv6(
        &self,
        destination: &Ipv6Inet,
        next_hop: &Ipv6Addr,
        metric: u32,
    ) -> Result<(), WIN32_ERROR> {
        let mut row = MIB_IPFORWARD_ROW2::default();
        unsafe { InitializeIpForwardEntry(&mut row) };

        row.InterfaceLuid = self.luid;
        row.ValidLifetime = 0xffffffff;
        row.PreferredLifetime = 0xffffffff;

        row.DestinationPrefix.Prefix = SOCKADDR_INET {
            Ipv6: convert_ipv6addr_to_sockaddr(&destination.address()),
        };
        row.DestinationPrefix.PrefixLength = destination.network_length();

        row.NextHop = SOCKADDR_INET {
            Ipv6: convert_ipv6addr_to_sockaddr(next_hop),
        };

        row.Metric = metric;

        let result = unsafe { CreateIpForwardEntry2(&row) };

        if result == NO_ERROR {
            Ok(())
        } else {
            Err(result)
        }
    }

    /// Add multiple IPv4 routes to the interface.
    pub fn add_routes_ipv4(
        &self,
        routes_data: impl IntoIterator<Item = RouteDataIpv4>,
    ) -> Result<(), WIN32_ERROR> {
        for rd in routes_data.into_iter() {
            self.add_route_ipv4(&rd.destination, &rd.next_hop, rd.metric)?;
        }
        Ok(())
    }

    /// Add multiple IPv6 routes to the interface.
    pub fn add_routes_ipv6(
        &self,
        routes_data: impl IntoIterator<Item = RouteDataIpv6>,
    ) -> Result<(), WIN32_ERROR> {
        for rd in routes_data.into_iter() {
            self.add_route_ipv6(&rd.destination, &rd.next_hop, rd.metric)?;
        }
        Ok(())
    }

    /// Delete an IPv4 route that matches the criteria.
    /// https://docs.microsoft.com/en-us/windows/desktop/api/netioapi/nf-netioapi-deleteipforwardentry2
    pub fn delete_route_ipv4(
        &self,
        destination: &Ipv4Inet,
        next_hop: &Ipv4Addr,
    ) -> Result<(), WIN32_ERROR> {
        let mut row = MIB_IPFORWARD_ROW2::default();
        unsafe { InitializeIpForwardEntry(&mut row) };

        row.InterfaceLuid = self.luid;

        row.DestinationPrefix.Prefix = SOCKADDR_INET {
            Ipv4: convert_ipv4addr_to_sockaddr(&destination.address()),
        };
        row.DestinationPrefix.PrefixLength = destination.network_length();

        row.NextHop = SOCKADDR_INET {
            Ipv4: convert_ipv4addr_to_sockaddr(next_hop),
        };

        let result = unsafe { GetIpForwardEntry2(&mut row) };
        if result != NO_ERROR {
            return Err(result);
        }

        let result = unsafe { DeleteIpForwardEntry2(&row) };
        if result == NO_ERROR {
            Ok(())
        } else {
            Err(result)
        }
    }

    /// Delete an IPv6 route that matches the criteria.
    pub fn delete_route_ipv6(
        &self,
        destination: &Ipv6Inet,
        next_hop: &Ipv6Addr,
    ) -> Result<(), WIN32_ERROR> {
        let mut row = MIB_IPFORWARD_ROW2::default();
        unsafe { InitializeIpForwardEntry(&mut row) };

        row.InterfaceLuid = self.luid;

        row.DestinationPrefix.Prefix = SOCKADDR_INET {
            Ipv6: convert_ipv6addr_to_sockaddr(&destination.address()),
        };
        row.DestinationPrefix.PrefixLength = destination.network_length();

        row.NextHop = SOCKADDR_INET {
            Ipv6: convert_ipv6addr_to_sockaddr(next_hop),
        };

        let result = unsafe { GetIpForwardEntry2(&mut row) };
        if result != NO_ERROR {
            return Err(result);
        }

        let result = unsafe { DeleteIpForwardEntry2(&row) };

        if result == NO_ERROR {
            Ok(())
        } else {
            Err(result)
        }
    }

    /// Sets MTU on the interface.
    pub fn set_iface_config(&self, mtu: u32) -> Result<(), WIN32_ERROR> {
        if let Err(e) = self.try_set_mtu(AF_INET, mtu) {
            tracing::warn!("Failed to set IPv4 MTU: {:?}", e);
        }
        if let Err(e) = self.try_set_mtu(AF_INET6, mtu) {
            tracing::warn!("Failed to set IPv6 MTU: {:?}", e);
        }
        Ok(())
    }

    fn try_set_mtu(&self, family: ADDRESS_FAMILY, mut mtu: u32) -> Result<(), WIN32_ERROR> {
        let mut row = MIB_IPINTERFACE_ROW {
            Family: family,
            InterfaceLuid: self.luid,
            ..Default::default()
        };

        let error = unsafe { GetIpInterfaceEntry(&mut row) };
        if error != NO_ERROR {
            if family == AF_INET6 && error == ERROR_NOT_FOUND {
                tracing::debug!(?family, "Couldn't set MTU, maybe IPv6 is disabled.");
            } else {
                tracing::warn!(?family, "Couldn't set MTU: {:?}", error);
            }
            return Err(error);
        }

        if family == AF_INET6 {
            // ipv6 mtu must be at least 1280
            mtu = 1280.max(mtu);
        }

        // https://stackoverflow.com/questions/54857292/setipinterfaceentry-returns-error-invalid-parameter
        row.SitePrefixLength = 0;

        row.NlMtu = mtu;

        let ret = unsafe { SetIpInterfaceEntry(&mut row) };
        if ret == NO_ERROR {
            Ok(())
        } else {
            Err(ret)
        }
    }
}
