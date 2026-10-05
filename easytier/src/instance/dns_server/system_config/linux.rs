use std::{
    fs::{self, OpenOptions},
    io::{self, Write},
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    process::Command,
};

use super::{OSConfig, SystemConfig};
use easytier_core::gateway::magic_dns::set_magic_dns_os_wired;

const EASYTIER_DROP_IN_HEADER: &str = "# Added by easytier\n";
const DEFAULT_DROP_IN_PATH: &str = "/etc/systemd/resolved.conf.d/easytier-magic-dns.conf";

/// systemd-resolved drop-in: `DNS=<fake_ip>` + `Domains=~<zone>` (mesh only).
pub struct LinuxResolvedConfigurator {
    drop_in_path: PathBuf,
    /// Set after the last `set_dns` / skip decision.
    wired: std::sync::atomic::AtomicBool,
}

impl Default for LinuxResolvedConfigurator {
    fn default() -> Self {
        Self::new()
    }
}

impl LinuxResolvedConfigurator {
    pub fn new() -> Self {
        Self::with_path(DEFAULT_DROP_IN_PATH)
    }

    pub fn with_path(path: impl Into<PathBuf>) -> Self {
        Self {
            drop_in_path: path.into(),
            wired: std::sync::atomic::AtomicBool::new(false),
        }
    }

    pub fn was_wired(&self) -> bool {
        self.wired.load(std::sync::atomic::Ordering::Relaxed)
    }

    fn resolved_usable() -> bool {
        Path::new("/run/systemd/resolve").exists()
            || Path::new("/etc/systemd/resolved.conf").exists()
    }

    fn build_drop_in(cfg: &OSConfig) -> io::Result<String> {
        let dns = cfg
            .nameservers
            .first()
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
            .ok_or_else(|| {
                io::Error::new(io::ErrorKind::InvalidInput, "missing MagicDNS nameserver")
            })?;

        let domain = cfg
            .match_domains
            .first()
            .or(cfg.search_domains.first())
            .map(|s| s.trim().trim_end_matches('.'))
            .filter(|s| !s.is_empty())
            .ok_or_else(|| {
                io::Error::new(io::ErrorKind::InvalidInput, "missing MagicDNS match domain")
            })?;

        Ok(format!(
            "{EASYTIER_DROP_IN_HEADER}\
             [Resolve]\n\
             DNS={dns}\n\
             Domains=~{domain}\n"
        ))
    }

    fn write_drop_in(&self, content: &str) -> io::Result<()> {
        if let Some(parent) = self.drop_in_path.parent() {
            fs::create_dir_all(parent)?;
        }
        let mut file = OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .open(&self.drop_in_path)?;
        file.set_permissions(fs::Permissions::from_mode(0o644))?;
        file.write_all(content.as_bytes())?;
        Ok(())
    }

    fn resolved_active() -> bool {
        Command::new("systemctl")
            .args(["is-active", "--quiet", "systemd-resolved"])
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    }

    /// Reload systemd-resolved and require it to be active afterwards.
    /// Returns false when reload fails or the unit is not active (do not claim wired).
    fn reload_resolved() -> bool {
        let reload_ok = Command::new("systemctl")
            .args(["reload", "systemd-resolved"])
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
        let _ = Command::new("resolvectl").args(["flush-caches"]).status();
        let active = Self::resolved_active();
        if !reload_ok || !active {
            tracing::warn!(
                reload_ok,
                active,
                "systemd-resolved reload/active check failed; MagicDNS OS wiring not confirmed"
            );
            return false;
        }
        true
    }

    fn remove_drop_in_if_ours(&self) -> io::Result<()> {
        match fs::read_to_string(&self.drop_in_path) {
            Ok(content) if content.starts_with(EASYTIER_DROP_IN_HEADER) => {
                fs::remove_file(&self.drop_in_path)?;
            }
            Ok(_) => {
                tracing::warn!(
                    path = %self.drop_in_path.display(),
                    "not removing MagicDNS drop-in (header mismatch)"
                );
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => return Err(e),
        }
        Ok(())
    }

    pub fn do_set_dns(&self, cfg: &OSConfig) -> io::Result<()> {
        if !Self::resolved_usable() {
            tracing::warn!(
                "systemd-resolved not detected; MagicDNS OS wiring skipped \
                 (see docs/current/magic-dns-manual-wiring.md for OpenWrt/dnsmasq)"
            );
            self.wired
                .store(false, std::sync::atomic::Ordering::Relaxed);
            set_magic_dns_os_wired(false);
            return Ok(());
        }

        let content = Self::build_drop_in(cfg)?;
        if let Err(e) = self.write_drop_in(&content) {
            // Best-effort: non-root / read-only /etc must not kill MagicDNS.
            tracing::warn!(
                path = %self.drop_in_path.display(),
                error = %e,
                "MagicDNS systemd-resolved drop-in write failed; continuing without OS wiring \
                 (see docs/current/magic-dns-manual-wiring.md)"
            );
            self.wired
                .store(false, std::sync::atomic::Ordering::Relaxed);
            set_magic_dns_os_wired(false);
            return Ok(());
        }
        if !Self::reload_resolved() {
            // Drop-in is on disk but resolved did not reload/activate — do not
            // report covered (B6). Leave the file for a later retry / manual fix.
            self.wired
                .store(false, std::sync::atomic::Ordering::Relaxed);
            set_magic_dns_os_wired(false);
            return Ok(());
        }
        self.wired.store(true, std::sync::atomic::Ordering::Relaxed);
        set_magic_dns_os_wired(true);
        tracing::info!(
            path = %self.drop_in_path.display(),
            "MagicDNS systemd-resolved drop-in installed"
        );
        Ok(())
    }

    pub fn do_close(&self) -> io::Result<()> {
        self.remove_drop_in_if_ours()?;
        if Self::resolved_usable() {
            let _ = Self::reload_resolved();
        }
        self.wired
            .store(false, std::sync::atomic::Ordering::Relaxed);
        set_magic_dns_os_wired(false);
        Ok(())
    }
}

impl SystemConfig for LinuxResolvedConfigurator {
    fn set_dns(&self, cfg: &OSConfig) -> io::Result<()> {
        self.do_set_dns(cfg)
    }

    fn close(&self) -> io::Result<()> {
        self.do_close()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn build_drop_in_uses_fake_ip_and_tilde_domain() {
        let cfg = OSConfig {
            nameservers: vec!["100.100.100.53".into()],
            search_domains: vec!["et.net.".into()],
            match_domains: vec!["et.net.".into()],
        };
        let content = LinuxResolvedConfigurator::build_drop_in(&cfg).unwrap();
        assert!(content.starts_with(EASYTIER_DROP_IN_HEADER));
        assert!(content.contains("DNS=100.100.100.53"));
        assert!(content.contains("Domains=~et.net"));
    }

    #[test]
    fn write_and_remove_drop_in_in_tempdir() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("easytier-magic-dns.conf");
        // Force "usable" path by writing under temp; do_set_dns checks host resolved paths.
        // Exercise write/remove helpers directly.
        let cfg = LinuxResolvedConfigurator::with_path(&path);
        let content = LinuxResolvedConfigurator::build_drop_in(&OSConfig {
            nameservers: vec!["100.100.100.53".into()],
            search_domains: vec![],
            match_domains: vec!["et.net.".into()],
        })
        .unwrap();
        cfg.write_drop_in(&content).unwrap();
        assert!(path.exists());
        let read = fs::read_to_string(&path).unwrap();
        assert!(read.starts_with(EASYTIER_DROP_IN_HEADER));
        cfg.remove_drop_in_if_ours().unwrap();
        assert!(!path.exists());
    }
}
