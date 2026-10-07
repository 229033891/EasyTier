use std::net::IpAddr;
use std::process::Command;

use std::io;
use winreg::RegKey;

use crate::common::ifcfg::RegistryManager;

use super::{OSConfig, SystemConfig};

pub fn is_windows_10_or_better() -> io::Result<bool> {
    let hklm = winreg::enums::HKEY_LOCAL_MACHINE;
    let key_path = "SOFTWARE\\Microsoft\\Windows NT\\CurrentVersion";
    let key = winreg::RegKey::predef(hklm).open_subkey(key_path)?;

    // check CurrentMajorVersionNumber, which only exists on Windows 10 and later
    let value_name = "CurrentMajorVersionNumber";
    key.get_raw_value(value_name).map(|_| true)
}

// 假设 interface_guid 是你的网络接口 GUID
pub struct InterfaceControl {
    interface_guid: String,
    /// 最近一次写入的 nameservers（v4），close() 只回滚自己写过的值，
    /// 避免误删用户自有或其它实例的 DNS 配置。
    last_nameservers_v4: std::sync::Mutex<Option<Vec<String>>>,
    last_nameservers_v6: std::sync::Mutex<Option<Vec<String>>>,
}

fn store_nameservers(slot: &std::sync::Mutex<Option<Vec<String>>>, values: Vec<String>) {
    if let Ok(mut guard) = slot.lock() {
        *guard = Some(values);
    }
}

fn take_nameservers(slot: &std::sync::Mutex<Option<Vec<String>>>) -> Option<Vec<String>> {
    slot.lock().ok().and_then(|mut guard| guard.take())
}

impl InterfaceControl {
    // 构造函数
    pub fn new(interface_guid: &str) -> Self {
        InterfaceControl {
            interface_guid: interface_guid.to_string(),
            last_nameservers_v4: std::sync::Mutex::new(None),
            last_nameservers_v6: std::sync::Mutex::new(None),
        }
    }

    // 删除注册表值（模拟 delValue）
    fn delete_value(key: &RegKey, value_name: &str) -> io::Result<()> {
        match key.delete_value(value_name) {
            Ok(_) => Ok(()),
            Err(e) => {
                if matches!(e.kind(), io::ErrorKind::NotFound) {
                    Ok(()) // 忽略不存在的值
                } else {
                    Err(e)
                }
            }
        }
    }

    pub fn set_primary_dns(&self, resolvers: &[IpAddr], domains: &[String]) -> io::Result<()> {
        let (ipsv4, ipsv6): (Vec<String>, Vec<String>) = resolvers
            .iter()
            .map(|ip| ip.to_string())
            .partition(|ip| ip.contains('.'));

        let dom_strs: Vec<String> = domains
            .iter()
            .map(|d| d.trim_end_matches('.').to_string())
            .collect();

        // IPv4 处理
        if let Ok(key4) = RegistryManager::open_interface_key(
            &self.interface_guid,
            RegistryManager::IPV4_TCPIP_INTERFACE_PREFIX,
        ) {
            if ipsv4.is_empty() {
                Self::delete_value(&key4, "NameServer")?;
            } else {
                key4.set_value("NameServer", &ipsv4.join(","))?;
            }

            if dom_strs.is_empty() {
                Self::delete_value(&key4, "SearchList")?;
            } else {
                key4.set_value("SearchList", &dom_strs.join(","))?;
            }

            // 禁用 LLMNR（通过 DisableMulticast）
            key4.set_value("EnableMulticast", &0u32)?;
        }
        store_nameservers(&self.last_nameservers_v4, ipsv4);

        // IPv6 处理
        if let Ok(key6) = RegistryManager::open_interface_key(
            &self.interface_guid,
            RegistryManager::IPV6_TCPIP_INTERFACE_PREFIX,
        ) {
            if ipsv6.is_empty() {
                Self::delete_value(&key6, "NameServer")?;
            } else {
                key6.set_value("NameServer", &ipsv6.join(","))?;
            }

            if dom_strs.is_empty() {
                Self::delete_value(&key6, "SearchList")?;
            } else {
                key6.set_value("SearchList", &dom_strs.join(","))?;
            }
            key6.set_value("EnableMulticast", &0u32)?;
        }
        store_nameservers(&self.last_nameservers_v6, ipsv6);

        Ok(())
    }

    /// 只回滚本实例写过的值：当前注册表仍等于写入值时才删除，
    /// 否则说明已被用户或其它实例改动，直接放过。
    fn revert_if_ours(&self, prefix: &str, expected: Option<Vec<String>>) {
        let Some(expected) = expected else {
            return;
        };
        let Ok(key) = RegistryManager::open_interface_key(&self.interface_guid, prefix) else {
            return;
        };
        let current: io::Result<String> = key.get_value("NameServer");
        if current
            .map(|value| value == expected.join(","))
            .unwrap_or(false)
        {
            let _ = Self::delete_value(&key, "NameServer");
            let _ = Self::delete_value(&key, "SearchList");
            let _ = Self::delete_value(&key, "EnableMulticast");
        }
    }

    pub fn close(&self) -> io::Result<()> {
        self.revert_if_ours(
            RegistryManager::IPV4_TCPIP_INTERFACE_PREFIX,
            take_nameservers(&self.last_nameservers_v4),
        );
        self.revert_if_ours(
            RegistryManager::IPV6_TCPIP_INTERFACE_PREFIX,
            take_nameservers(&self.last_nameservers_v6),
        );
        // 刷新缓存 best-effort：关闭路径上绝不 panic，失败直接忽略
        let _ = Command::new("ipconfig").arg("/flushdns").output();
        Ok(())
    }

    fn flush_dns(&self) -> io::Result<()> {
        // 刷新 DNS 缓存
        let output = Command::new("ipconfig")
            .arg("/flushdns")
            .output()
            .expect("failed to execute process");
        if !output.status.success() {
            return Err(io::Error::other("Failed to flush DNS cache"));
        }
        Ok(())
    }

    // re-register DNS
    pub fn re_register_dns(&self) -> io::Result<()> {
        // ipconfig /registerdns
        let output = Command::new("ipconfig")
            .arg("/registerdns")
            .output()
            .expect("failed to execute process");
        if !output.status.success() {
            return Err(io::Error::other("Failed to register DNS"));
        }
        Ok(())
    }
}

pub struct WindowsDNSManager {
    interface_control: InterfaceControl,
}

impl WindowsDNSManager {
    pub fn new(tun_dev_name: &str) -> io::Result<Self> {
        let interface_guid = RegistryManager::find_interface_guid(tun_dev_name)?;
        Ok(WindowsDNSManager {
            interface_control: InterfaceControl::new(&interface_guid),
        })
    }

    pub fn set_primary_dns(&self, resolvers: &[IpAddr], domains: &[String]) -> io::Result<()> {
        self.interface_control.set_primary_dns(resolvers, domains)?;
        self.interface_control.flush_dns()?;
        Ok(())
    }
}

impl SystemConfig for WindowsDNSManager {
    fn set_dns(&self, cfg: &OSConfig) -> io::Result<()> {
        self.set_primary_dns(
            &cfg.nameservers
                .iter()
                .map(|s| s.parse::<IpAddr>().unwrap())
                .collect::<Vec<_>>(),
            &cfg.match_domains,
        )?;
        Ok(())
    }

    fn close(&self) -> io::Result<()> {
        self.interface_control.close()
    }
}

#[cfg(test)]
mod tests {
    use cidr::Ipv4Inet;

    #[cfg(target_os = "windows")]
    #[tokio::test]
    async fn test_windows_set_primary_server() {
        use std::{net::Ipv4Addr, str::FromStr as _, time::Duration};

        use tokio_util::sync::CancellationToken;

        use crate::instance::dns_server::{
            MAGIC_DNS_FAKE_IP,
            runner::DnsRunner,
            tests::{check_dns_record, prepare_env},
        };

        let tun_ip = Ipv4Inet::from_str("10.144.144.10/24").unwrap();
        let (global_ctx, core_instance, virtual_nic) = prepare_env("test1", tun_ip).await;
        let tun_name = virtual_nic.ifname().await.unwrap();

        println!("dev_name: {}", tun_name);
        let fake_ip = Ipv4Addr::from_str(MAGIC_DNS_FAKE_IP).unwrap();
        let mut dns_runner = DnsRunner::new(
            core_instance.packet_plane(),
            global_ctx,
            Some(tun_name.clone()),
            tun_ip,
            fake_ip,
        );

        let cancel_token = CancellationToken::new();
        let cancel_token_clone = cancel_token.clone();
        let t = tokio::spawn(async move {
            dns_runner.run(cancel_token_clone).await;
        });

        // windows is slow to add a ip address, wait for a longer time for dns server ready ,with ping
        let now = std::time::Instant::now();
        while now.elapsed() < Duration::from_secs(15) {
            tokio::time::sleep(Duration::from_secs(1)).await;
            if let Ok(o) = tokio::process::Command::new("ping")
                .arg("-n")
                .arg("1")
                .arg("-w")
                .arg("100")
                .arg(fake_ip.to_string())
                .output()
                .await
                && o.status.success()
            {
                break;
            }
        }

        check_dns_record(&fake_ip, "test1.et.net", "10.144.144.10").await;

        let dns_mgr = super::WindowsDNSManager::new(&tun_name).unwrap();
        println!("dev_name: {}", tun_name);
        println!("guid: {}", dns_mgr.interface_control.interface_guid);

        dns_mgr
            .interface_control
            .set_primary_dns(
                &[MAGIC_DNS_FAKE_IP.parse().unwrap()],
                &[".et.net.".to_string()],
            )
            .unwrap();
        dns_mgr.interface_control.flush_dns().unwrap();

        tracing::info!("check dns record with nslookup");

        // nslookup should return 10.144.144.10
        let ret = tokio::process::Command::new("nslookup")
            .arg("test1.et.net")
            .output()
            .await
            .expect("failed to execute process");
        assert!(ret.status.success());
        let output = String::from_utf8_lossy(&ret.stdout);
        println!("nslookup output: {}", output);
        assert!(output.contains("10.144.144.10"));

        cancel_token.cancel();
        let _ = t.await;
    }
}
