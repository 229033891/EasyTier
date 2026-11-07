//! Prune installed/desired proxy CIDRs against local physical + virtual networks.
//!
//! See `docs/roadmap/lan-direct-underlay-guard.md` L1 and
//! `docs/roadmap/open-bugs-wave1-impl.md` §2.1.

use std::collections::BTreeSet;
use std::net::Ipv4Addr;

use cidr::Ipv4Cidr;

/// Subtract `holes` from `base`, returning zero or more residual prefixes.
///
/// `/0` is never subtracted (callers must skip it before calling).
pub fn subtract_ipv4(base: Ipv4Cidr, holes: &[Ipv4Cidr]) -> Vec<Ipv4Cidr> {
    let mut parts = vec![base];
    for hole in holes {
        let mut next = Vec::new();
        for part in parts {
            next.extend(subtract_one(part, *hole));
        }
        parts = next;
        if parts.is_empty() {
            break;
        }
    }
    parts
}

fn subtract_one(base: Ipv4Cidr, hole: Ipv4Cidr) -> Vec<Ipv4Cidr> {
    if !overlaps(base, hole) {
        return vec![base];
    }
    if hole_covers_base(base, hole) {
        return Vec::new();
    }
    // A /32 that overlaps a hole is always fully covered above; keep the host
    // if we somehow still reach here (cannot safely split past /32).
    if base.network_length() >= 32 {
        return vec![base];
    }
    let (lo, hi) = split_prefix(base);
    let mut out = subtract_one(lo, hole);
    out.extend(subtract_one(hi, hole));
    out
}

fn overlaps(a: Ipv4Cidr, b: Ipv4Cidr) -> bool {
    a.contains(&b.first_address())
        || a.contains(&b.last_address())
        || b.contains(&a.first_address())
        || b.contains(&a.last_address())
}

fn hole_covers_base(base: Ipv4Cidr, hole: Ipv4Cidr) -> bool {
    hole.network_length() <= base.network_length()
        && hole.contains(&base.first_address())
        && hole.contains(&base.last_address())
}

fn split_prefix(base: Ipv4Cidr) -> (Ipv4Cidr, Ipv4Cidr) {
    let len = base.network_length() + 1;
    let first = u32::from(base.first_address());
    let bit = 1u32 << (32 - len);
    let lo = Ipv4Cidr::new(Ipv4Addr::from(first), len).expect("split lo");
    let hi = Ipv4Cidr::new(Ipv4Addr::from(first | bit), len).expect("split hi");
    (lo, hi)
}

/// Apply L1 prune rules to a desired proxy-CIDR set.
///
/// - `/0` is never pruned.
/// - Overlapping non-default prefixes are subtracted by `holes`.
/// - Empty residuals are dropped (caller may warn).
pub fn prune_proxy_cidrs(raw: BTreeSet<Ipv4Cidr>, holes: &[Ipv4Cidr]) -> BTreeSet<Ipv4Cidr> {
    if holes.is_empty() {
        return raw;
    }
    let mut out = BTreeSet::new();
    for cidr in raw {
        if cidr.network_length() == 0 {
            out.insert(cidr);
            continue;
        }
        for part in subtract_ipv4(cidr, holes) {
            out.insert(part);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn c(s: &str) -> Ipv4Cidr {
        s.parse().unwrap()
    }

    #[test]
    fn subtract_slash22_minus_slash24() {
        let parts = subtract_ipv4(c("192.168.8.0/22"), &[c("192.168.11.0/24")]);
        let set: BTreeSet<_> = parts.into_iter().collect();
        assert!(set.contains(&c("192.168.8.0/23")));
        assert!(set.contains(&c("192.168.10.0/24")));
        assert_eq!(set.len(), 2);
    }

    #[test]
    fn subtract_full_cover_yields_empty() {
        assert!(subtract_ipv4(c("10.0.0.0/24"), &[c("10.0.0.0/16")]).is_empty());
    }

    #[test]
    fn subtract_no_overlap_keeps_base() {
        assert_eq!(
            subtract_ipv4(c("10.0.0.0/24"), &[c("192.168.0.0/24")]),
            vec![c("10.0.0.0/24")]
        );
    }

    #[test]
    fn prune_skips_default_route() {
        let mut raw = BTreeSet::new();
        raw.insert(c("0.0.0.0/0"));
        raw.insert(c("192.168.8.0/22"));
        let pruned = prune_proxy_cidrs(raw, &[c("192.168.11.0/24")]);
        assert!(pruned.contains(&c("0.0.0.0/0")));
        assert!(!pruned.contains(&c("192.168.8.0/22")));
        assert!(pruned.contains(&c("192.168.8.0/23")));
        assert!(pruned.contains(&c("192.168.10.0/24")));
    }
}
