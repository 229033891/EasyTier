//! Process-wide MagicDNS OS wiring signal (dns-policy.md §11 B6 phase 2).
//!
//! `None` = unset (old clients / platforms that do not report).
//! `Some(true)` = OS successfully pointed at MagicDNS fake IP.
//! `Some(false)` = accept_dns path ran but OS wiring skipped or failed.

use std::sync::atomic::{AtomicU8, Ordering};

const UNSET: u8 = 0;
const WIRED_FALSE: u8 = 1;
const WIRED_TRUE: u8 = 2;

static MAGIC_DNS_OS_WIRED: AtomicU8 = AtomicU8::new(UNSET);

pub fn set_magic_dns_os_wired(wired: bool) {
    MAGIC_DNS_OS_WIRED.store(
        if wired { WIRED_TRUE } else { WIRED_FALSE },
        Ordering::Relaxed,
    );
}

pub fn clear_magic_dns_os_wired() {
    MAGIC_DNS_OS_WIRED.store(UNSET, Ordering::Relaxed);
}

pub fn get_magic_dns_os_wired() -> Option<bool> {
    match MAGIC_DNS_OS_WIRED.load(Ordering::Relaxed) {
        WIRED_TRUE => Some(true),
        WIRED_FALSE => Some(false),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wired_flag_round_trips() {
        clear_magic_dns_os_wired();
        assert_eq!(get_magic_dns_os_wired(), None);
        set_magic_dns_os_wired(true);
        assert_eq!(get_magic_dns_os_wired(), Some(true));
        set_magic_dns_os_wired(false);
        assert_eq!(get_magic_dns_os_wired(), Some(false));
        clear_magic_dns_os_wired();
        assert_eq!(get_magic_dns_os_wired(), None);
    }
}
