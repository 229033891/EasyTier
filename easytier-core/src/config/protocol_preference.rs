//! Ordered scheme preference for `flags.default_protocol` (P-AUTO.L1).
//!
//! Storage remains a single string: comma-separated list (legacy single value =
//! length-1). Empty / all-invalid → `["udp", "tcp"]`.
//!
//! `udp` comes first because, among the default-enabled transports, it is the
//! only one that does not add its own reliability on top: `tcp` / `ws` / `wss` /
//! `quic` / `faketcp` all retransmit, so any inner TCP flow (RDP, SSH, HTTP)
//! becomes TCP-over-TCP and stalls under mild packet loss. The previous `tcp`
//! default also made DirectConnector prefer a peer's TCP listener over its UDP
//! one. `tcp` stays second so the "UDP is blocked, fall back to TCP" guarantee
//! still works. `wg` is likewise plain UDP (boringtun) but stays opt-in: it
//! needs both sides to advertise it and has a larger per-packet overhead.
//!
//! Direct connector uses the full preference list to **sort advertised listeners**.
//! Manual connector **rewrites** among `REWRITEABLE_SCHEMES` (tcp/udp/ws/wss/quic)
//! but only as failover *after* the configured URL (`preference_candidate_urls`);
//! `wg` / `faketcp` stay preference-sortable for Direct but are never URL-rewritten
//! (incompatible handshake / fingerprint).

use std::collections::HashSet;

use url::Url;

/// Schemes that participate in preference lists (Direct sort + config allowlist).
/// Mirrors dialable schemes in `connectivity::protocol::protocol_transport`
/// except `ring` (rendezvous-only, filtered out by both connectors).
pub const PREFERENCE_SCHEMES: &[&str] = &["tcp", "udp", "ws", "wss", "quic", "wg", "faketcp"];

/// Schemes safe to cross-rewrite on manual peer URLs.
/// WireGuard / FakeTCP keep a distinct transport fingerprint and must not be
/// dialed as plain tcp/udp/ws via scheme rewrite.
pub const REWRITEABLE_SCHEMES: &[&str] = &["tcp", "udp", "ws", "wss", "quic"];

/// Parse `default_protocol` into an ordered, de-duplicated preference list.
pub fn parse_protocol_preference(raw: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut seen = HashSet::new();
    for token in raw.split(',') {
        let scheme = token.trim().to_ascii_lowercase();
        if scheme.is_empty() {
            continue;
        }
        if !PREFERENCE_SCHEMES.contains(&scheme.as_str()) {
            tracing::warn!(%scheme, "ignoring unknown default_protocol scheme");
            continue;
        }
        if seen.insert(scheme.clone()) {
            out.push(scheme);
        }
    }
    if out.is_empty() {
        out.push("udp".to_owned());
        out.push("tcp".to_owned());
    }
    out
}

/// Normalize a raw `default_protocol` string to canonical CSV (lowercase, ordered unique).
pub fn normalize_default_protocol(raw: &str) -> String {
    parse_protocol_preference(raw).join(",")
}

/// Sort key for DirectConnector: higher = tried first (popped from list end).
/// Preference index 0 → highest rank; unknown schemes → 0 (tried last).
pub fn protocol_preference_sort_key(preference: &[String], scheme: &str) -> u32 {
    match preference.iter().position(|item| item == scheme) {
        Some(index) => (preference.len() - index) as u32,
        None => 0,
    }
}

fn is_rewriteable_scheme(scheme: &str) -> bool {
    REWRITEABLE_SCHEMES.contains(&scheme)
}

/// Effective listen/dial port of `url`: explicit first, else EasyTier / IETF default
/// for the **source** scheme (so `wss://host/et` → tcp keeps 443, not 11010).
///
/// Mirrors `connectivity::protocol::protocol_default_port` for preference schemes
/// (kept local to avoid config ↔ connectivity cycles).
fn source_effective_port(url: &Url) -> Option<u16> {
    if let Some(port) = url.port() {
        return Some(port);
    }
    match url.scheme() {
        "ws" => Some(80),
        "wss" => Some(443),
        "tcp" | "udp" => Some(11010),
        "wg" => Some(11011),
        "quic" => Some(11012),
        "faketcp" => Some(11013),
        _ => url.port_or_known_default(),
    }
}

/// Rewrite `url`'s scheme among rewriteable transports, preserving host/path
/// and the source effective port. Returns `None` for non-rewriteable source or target.
pub fn rewrite_url_scheme(url: &Url, scheme: &str) -> Option<Url> {
    if !is_rewriteable_scheme(url.scheme()) || !is_rewriteable_scheme(scheme) {
        return None;
    }
    // Same scheme: return the original URL as-is so implicit ports
    // (e.g. `tcp://host`) do not gain an explicit `:11010` duplicate that
    // would dial the same endpoint twice and misreport `active_url`.
    if url.scheme() == scheme {
        return Some(url.clone());
    }
    let port_to_keep = source_effective_port(url);
    let mut next = url.clone();
    // WHATWG "special" schemes (ws/wss/http/…) cannot `set_scheme` into
    // non-special ones (tcp/udp/quic) and vice versa — rebuild instead.
    if next.set_scheme(scheme).is_err() {
        let raw = url.as_str();
        let prefix = format!("{}:", url.scheme());
        let rest = raw.strip_prefix(&prefix)?;
        next = Url::parse(&format!("{scheme}:{rest}")).ok()?;
    }
    if let Some(port) = port_to_keep {
        // Always re-apply: scheme changes clear defaults; without this, wss→tcp
        // would dial EasyTier tcp default 11010 instead of 443.
        if next.set_port(Some(port)).is_err() {
            tracing::warn!(%url, %scheme, port, "rewrite_url_scheme: set_port failed");
            return None;
        }
    }
    Some(next)
}

/// Candidate URLs for manual reconnect: the **configured URL first**, then the
/// preference schemes as failover (P-AUTO.L1 "保底降级").
///
/// The operator's explicit scheme must win. Dialing a preference candidate first
/// silently overrode it: a configured `udp://` peer was dialed over TCP whenever
/// TCP was reachable (TCP-over-TCP for every inner TCP flow, e.g. RDP, which
/// collapses under mild packet loss), and a configured `wss://` peer was tried as
/// plain `tcp://` first, trading away the TLS camouflage it was chosen for.
/// The preference list stays useful as failover: `wss://` still falls back to
/// `tcp://` on the same port when the TLS handshake cannot complete.
///
/// Non-rewriteable peers (`wg` / `faketcp` / `ring` / unknown) are never rewritten.
pub fn preference_candidate_urls(url: &Url, preference: &[String]) -> Vec<Url> {
    let original_scheme = url.scheme();
    if !is_rewriteable_scheme(original_scheme) {
        // ring / wg / faketcp / unknown: dial the configured URL only.
        return vec![url.clone()];
    }

    let mut out = vec![url.clone()];
    let mut seen = HashSet::new();
    seen.insert(url.as_str().to_owned());
    for scheme in preference {
        if !is_rewriteable_scheme(scheme) {
            // Preference may still list wg/faketcp for Direct sort; skip for Manual rewrite.
            continue;
        }
        let Some(candidate) = rewrite_url_scheme(url, scheme) else {
            continue;
        };
        let key = candidate.as_str().to_owned();
        if seen.insert(key) {
            out.push(candidate);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_single_value_and_empty() {
        assert_eq!(parse_protocol_preference("tcp"), vec!["tcp"]);
        assert_eq!(parse_protocol_preference("UDP"), vec!["udp"]);
        assert_eq!(parse_protocol_preference(""), vec!["udp", "tcp"]);
        assert_eq!(parse_protocol_preference("  , , "), vec!["udp", "tcp"]);
    }

    #[test]
    fn parse_csv_preserves_order_and_dedupes() {
        assert_eq!(
            parse_protocol_preference("wss, tcp, quic, tcp, udp"),
            vec!["wss", "tcp", "quic", "udp"]
        );
    }

    #[test]
    fn parse_drops_unknown_schemes() {
        assert_eq!(
            parse_protocol_preference("wg,tcp,ring,wss,faketcp"),
            vec!["wg", "tcp", "wss", "faketcp"]
        );
        assert_eq!(parse_protocol_preference("ring"), vec!["udp", "tcp"]);
    }

    #[test]
    fn normalize_joins_csv() {
        assert_eq!(normalize_default_protocol("WSS, TCP"), "wss,tcp");
        assert_eq!(normalize_default_protocol(""), "udp,tcp");
    }

    #[test]
    fn sort_key_prefers_earlier_list_entries() {
        let pref = parse_protocol_preference("wss,tcp,udp");
        assert!(
            protocol_preference_sort_key(&pref, "wss") > protocol_preference_sort_key(&pref, "tcp")
        );
        assert!(
            protocol_preference_sort_key(&pref, "tcp") > protocol_preference_sort_key(&pref, "udp")
        );
        assert_eq!(protocol_preference_sort_key(&pref, "quic"), 0);
    }

    #[test]
    fn sort_key_includes_wg_and_faketcp_for_direct() {
        let pref = parse_protocol_preference("wg,faketcp,tcp");
        assert!(
            protocol_preference_sort_key(&pref, "wg")
                > protocol_preference_sort_key(&pref, "faketcp")
        );
        assert!(
            protocol_preference_sort_key(&pref, "faketcp")
                > protocol_preference_sort_key(&pref, "tcp")
        );
    }

    #[test]
    fn rewrite_keeps_explicit_port() {
        let url = Url::parse("udp://relay.example:2200/et").unwrap();
        let tcp = rewrite_url_scheme(&url, "tcp").unwrap();
        assert_eq!(tcp.scheme(), "tcp");
        assert_eq!(tcp.port(), Some(2200));
        assert_eq!(tcp.path(), "/et");
    }

    #[test]
    fn rewrite_keeps_implicit_wss_port_when_falling_to_tcp() {
        let url = Url::parse("wss://relay.example/et").unwrap();
        assert!(url.port().is_none());
        let tcp = rewrite_url_scheme(&url, "tcp").unwrap();
        assert_eq!(tcp.scheme(), "tcp");
        assert_eq!(tcp.port(), Some(443));
        assert_eq!(tcp.path(), "/et");
    }

    #[test]
    fn rewrite_rejects_wg_and_faketcp() {
        let wg = Url::parse("wg://10.0.0.2:11011").unwrap();
        assert!(rewrite_url_scheme(&wg, "tcp").is_none());
        let tcp = Url::parse("tcp://10.0.0.2:11010").unwrap();
        assert!(rewrite_url_scheme(&tcp, "wg").is_none());
        assert!(rewrite_url_scheme(&tcp, "faketcp").is_none());
    }

    #[test]
    fn candidates_put_configured_url_first() {
        let url = Url::parse("udp://10.0.0.2:2200").unwrap();
        let pref = parse_protocol_preference("wss,tcp");
        let candidates = preference_candidate_urls(&url, &pref);
        assert_eq!(
            candidates
                .iter()
                .map(|u| u.as_str().to_owned())
                .collect::<Vec<_>>(),
            vec![
                // The configured scheme wins; the preference list is failover only.
                "udp://10.0.0.2:2200".to_owned(),
                // wss is WHATWG-special → empty path serializes as `/`;
                // tcp/udp are non-special → no trailing slash (matches dial URL).
                "wss://10.0.0.2:2200/".to_owned(),
                "tcp://10.0.0.2:2200".to_owned(),
            ]
        );
    }

    #[test]
    fn candidates_keep_wss_camouflage_then_fall_back_to_tcp() {
        let url = Url::parse("wss://relay.example/et").unwrap();
        let pref = parse_protocol_preference("tcp,wss");
        let candidates = preference_candidate_urls(&url, &pref);
        // The configured TLS endpoint is tried first, unchanged...
        assert_eq!(candidates[0].scheme(), "wss");
        assert_eq!(candidates[0].port(), None); // implicit 443
        // ...and the downgrade keeps the effective port only as failover.
        assert_eq!(candidates[1].scheme(), "tcp");
        assert_eq!(candidates[1].port(), Some(443));
        assert_eq!(candidates.len(), 2);
    }

    #[test]
    fn ring_is_not_rewritten() {
        let url = Url::parse("ring://uuid").unwrap();
        let pref = parse_protocol_preference("tcp,udp");
        assert_eq!(preference_candidate_urls(&url, &pref), vec![url]);
    }

    #[test]
    fn wg_and_faketcp_peers_are_not_rewritten() {
        let wg = Url::parse("wg://10.0.0.2:11011").unwrap();
        let pref = parse_protocol_preference("tcp,udp,wg");
        assert_eq!(preference_candidate_urls(&wg, &pref), vec![wg]);

        let faketcp = Url::parse("faketcp://10.0.0.2:11013").unwrap();
        assert_eq!(preference_candidate_urls(&faketcp, &pref), vec![faketcp]);
    }

    #[test]
    fn candidates_skip_wg_faketcp_targets_in_preference() {
        let url = Url::parse("tcp://10.0.0.2:2200").unwrap();
        let pref = parse_protocol_preference("wg,tcp,faketcp,udp");
        let candidates = preference_candidate_urls(&url, &pref);
        assert_eq!(
            candidates
                .iter()
                .map(|u| u.scheme().to_owned())
                .collect::<Vec<_>>(),
            vec!["tcp".to_owned(), "udp".to_owned()]
        );
    }

    #[test]
    fn preference_schemes_cover_rewriteable() {
        for scheme in REWRITEABLE_SCHEMES {
            assert!(PREFERENCE_SCHEMES.contains(scheme));
        }
        assert!(PREFERENCE_SCHEMES.contains(&"wg"));
        assert!(PREFERENCE_SCHEMES.contains(&"faketcp"));
        assert!(!is_rewriteable_scheme("wg"));
        assert!(!is_rewriteable_scheme("faketcp"));
        assert!(!is_rewriteable_scheme("ring"));
    }
}
