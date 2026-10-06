//! Connection path tier (P0.1): config model for fallback preference.
//!
//! Ordered fallback / relay endpoints are the existing peer URL list
//! (`[[peer]]` / `peer_urls` / `public_server_url`). This module owns the
//! `ConnectionPathTier` ↔ legacy flag projection (`disable_p2p`,
//! `prefer_peer_relay`, `p2p_only`).

use easytier_proto::common::{ConnectionPathTier, FlagsInConfig};

/// Normalize an optional / unspecified tier to a concrete policy value.
pub fn normalize_connection_path_tier(tier: ConnectionPathTier) -> ConnectionPathTier {
    match tier {
        ConnectionPathTier::Unspecified => ConnectionPathTier::DirectFirst,
        other => other,
    }
}

/// Infer tier from legacy path flags when `connection_path_tier` is unset.
///
/// `p2p_only` is orthogonal (anti-relay) and does not map to Prefer/RelayOnly.
pub fn infer_connection_path_tier(flags: &FlagsInConfig) -> ConnectionPathTier {
    if flags.disable_p2p {
        return ConnectionPathTier::RelayOnly;
    }
    if flags.prefer_peer_relay {
        return ConnectionPathTier::PreferRelay;
    }
    ConnectionPathTier::DirectFirst
}

/// Project tier onto legacy flags. Tier is the source of truth.
///
/// - Does not touch `disable_relay_data` / whitelist (orthogonal).
/// - Prefer/RelayOnly force `p2p_only = false` (incompatible with “no relay”).
pub fn apply_connection_path_tier(flags: &mut FlagsInConfig, tier: ConnectionPathTier) {
    let tier = normalize_connection_path_tier(tier);
    flags.connection_path_tier = tier.into();

    match tier {
        ConnectionPathTier::DirectFirst | ConnectionPathTier::Unspecified => {
            flags.disable_p2p = false;
            flags.prefer_peer_relay = false;
            // leave p2p_only as-is (compat-only orthogonal switch)
        }
        ConnectionPathTier::PreferRelay => {
            flags.disable_p2p = false;
            flags.prefer_peer_relay = true;
            flags.p2p_only = false;
        }
        ConnectionPathTier::RelayOnly => {
            flags.disable_p2p = true;
            flags.prefer_peer_relay = true;
            flags.p2p_only = false;
        }
    }
}

/// Resolve SoT tier after individual flag fields were applied from a form/API.
///
/// If `explicit` is set and not Unspecified, it wins and legacy path flags are
/// overwritten. Otherwise infer from flags and store the result.
pub fn resolve_connection_path_tier(
    flags: &mut FlagsInConfig,
    explicit: Option<ConnectionPathTier>,
) -> ConnectionPathTier {
    let tier = match explicit {
        Some(t) if t != ConnectionPathTier::Unspecified => t,
        _ => {
            let stored = ConnectionPathTier::try_from(flags.connection_path_tier)
                .unwrap_or(ConnectionPathTier::Unspecified);
            if stored != ConnectionPathTier::Unspecified {
                normalize_connection_path_tier(stored)
            } else {
                infer_connection_path_tier(flags)
            }
        }
    };
    apply_connection_path_tier(flags, tier);
    tier
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::toml::gen_default_flags;

    #[test]
    fn apply_relay_only_projects_legacy_flags() {
        let mut flags = gen_default_flags();
        flags.p2p_only = true;
        apply_connection_path_tier(&mut flags, ConnectionPathTier::RelayOnly);
        assert!(flags.disable_p2p);
        assert!(flags.prefer_peer_relay);
        assert!(!flags.p2p_only);
        assert_eq!(
            ConnectionPathTier::try_from(flags.connection_path_tier).unwrap(),
            ConnectionPathTier::RelayOnly
        );
    }

    #[test]
    fn infer_from_disable_p2p() {
        let mut flags = gen_default_flags();
        flags.disable_p2p = true;
        assert_eq!(
            infer_connection_path_tier(&flags),
            ConnectionPathTier::RelayOnly
        );
    }

    #[test]
    fn infer_prefer_relay() {
        let mut flags = gen_default_flags();
        flags.prefer_peer_relay = true;
        assert_eq!(
            infer_connection_path_tier(&flags),
            ConnectionPathTier::PreferRelay
        );
    }

    #[test]
    fn p2p_only_alone_stays_direct_first() {
        let mut flags = gen_default_flags();
        flags.p2p_only = true;
        assert_eq!(
            infer_connection_path_tier(&flags),
            ConnectionPathTier::DirectFirst
        );
    }

    #[test]
    fn explicit_tier_overrides_legacy_flags() {
        let mut flags = gen_default_flags();
        flags.disable_p2p = true;
        let tier = resolve_connection_path_tier(&mut flags, Some(ConnectionPathTier::DirectFirst));
        assert_eq!(tier, ConnectionPathTier::DirectFirst);
        assert!(!flags.disable_p2p);
        assert!(!flags.prefer_peer_relay);
    }

    #[test]
    fn absent_tier_infers_and_stores() {
        let mut flags = gen_default_flags();
        flags.prefer_peer_relay = true;
        flags.connection_path_tier = ConnectionPathTier::Unspecified.into();
        let tier = resolve_connection_path_tier(&mut flags, None);
        assert_eq!(tier, ConnectionPathTier::PreferRelay);
        assert_eq!(
            ConnectionPathTier::try_from(flags.connection_path_tier).unwrap(),
            ConnectionPathTier::PreferRelay
        );
    }
}
