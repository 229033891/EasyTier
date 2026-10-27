use std::{
    sync::{
        LazyLock,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use dashmap::DashMap;

use crate::{config::PeerId, tunnel::TunnelError};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("wait response error: {0}")]
    WaitRespError(String),
    #[error("secret key error: {0}")]
    SecretKeyError(String),
    /// Configured `peer_public_key` (admin pin) does not match the remote Noise key.
    /// Preference scheme failover must not treat this as "transport unreachable".
    #[error("pinned remote static pubkey mismatch")]
    PinnedRemotePubkeyMismatch,
    #[error("peer has no connection: {0}")]
    PeerNoConnectionError(PeerId),
    #[error("route error: {0:?}")]
    RouteError(Option<String>),
    #[error("not found")]
    NotFound,
    #[error(transparent)]
    Tunnel(#[from] TunnelError),
    #[error(transparent)]
    Other(#[from] anyhow::Error),
}

/// How often a sustained path-unavailable condition may emit WARN (per peer).
///
/// Shared by every rate-limited class (path unavailable, local congestion): the
/// classes differ in *what they mean*, not in how fast they may log.
pub const PATH_UNAVAILABLE_WARN_INTERVAL: Duration = Duration::from_secs(30);

static PATH_UNAVAILABLE_WARN_MS: LazyLock<DashMap<PeerId, AtomicU64>> = LazyLock::new(DashMap::new);

/// Congestion tokens live in their own map: a path-unavailable burst and a
/// local-congestion burst are different incidents, so neither may consume the
/// other's single WARN slot.
static CONGESTION_WARN_MS: LazyLock<DashMap<PeerId, AtomicU64>> = LazyLock::new(DashMap::new);

impl Error {
    /// Peer offline / no next-hop / tunnel already shutting down.
    /// Common during reconnect teardown; not an unexpected code defect.
    pub fn is_expected_path_unavailable(&self) -> bool {
        matches!(
            self,
            Self::RouteError(_)
                | Self::PeerNoConnectionError(_)
                | Self::Tunnel(TunnelError::Shutdown)
        )
    }

    /// Local egress ring full — we dropped the packet ourselves.
    ///
    /// Deliberately **not** part of [`Error::is_expected_path_unavailable`]: the
    /// two answer different questions. Path-unavailable means "this link cannot
    /// carry the packet, consider another one"; congestion means "this link is
    /// fine, our local ring is momentarily full". Callers that decide whether to
    /// failover must not treat a full ring as a dead path, and callers that log
    /// must rate-limit it instead of raising one `error!` per dropped packet.
    pub fn is_congestion(&self) -> bool {
        matches!(self, Self::Tunnel(TunnelError::BufferFull))
    }

    /// Wrong admin/peer pin — do not preference-failover to another scheme.
    pub fn is_pinned_remote_pubkey_mismatch(&self) -> bool {
        matches!(self, Self::PinnedRemotePubkeyMismatch)
    }
}

/// Walk an anyhow chain for [`Error::PinnedRemotePubkeyMismatch`].
pub fn anyhow_is_pinned_remote_pubkey_mismatch(err: &anyhow::Error) -> bool {
    err.chain().any(|cause| {
        cause
            .downcast_ref::<Error>()
            .is_some_and(Error::is_pinned_remote_pubkey_mismatch)
    })
}

fn now_unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Returns `true` at most once per peer per [`PATH_UNAVAILABLE_WARN_INTERVAL`].
///
/// Use with [`Error::is_expected_path_unavailable`]: first hit → WARN for ops
/// visibility; subsequent hits within the window → debug to avoid spam.
pub fn take_path_unavailable_warn_token(peer_id: PeerId) -> bool {
    take_path_unavailable_warn_token_at(peer_id, now_unix_ms())
}

fn take_path_unavailable_warn_token_at(peer_id: PeerId, now_ms: u64) -> bool {
    take_warn_token(&PATH_UNAVAILABLE_WARN_MS, peer_id, now_ms)
}

/// Same contract as [`take_path_unavailable_warn_token`], for
/// [`Error::is_congestion`].
///
/// Congestion is the one drop class that is *expected* under load (see
/// `PeerConn::send_msg`: a full ring is dropped instead of blocking the shared
/// ingress path), so it must never reach a per-packet `error!`. First hit per
/// peer per window → WARN for ops visibility; the rest → debug.
pub fn take_congestion_warn_token(peer_id: PeerId) -> bool {
    take_congestion_warn_token_at(peer_id, now_unix_ms())
}

fn take_congestion_warn_token_at(peer_id: PeerId, now_ms: u64) -> bool {
    take_warn_token(&CONGESTION_WARN_MS, peer_id, now_ms)
}

fn take_warn_token(map: &DashMap<PeerId, AtomicU64>, peer_id: PeerId, now_ms: u64) -> bool {
    let interval_ms = PATH_UNAVAILABLE_WARN_INTERVAL.as_millis() as u64;

    // Bound map growth when peers churn.
    if map.len() > 512 {
        let retain_ms = interval_ms.saturating_mul(2);
        map.retain(|_, stamp| now_ms.saturating_sub(stamp.load(Ordering::Relaxed)) < retain_ms);
    }

    let entry = map.entry(peer_id).or_insert_with(|| AtomicU64::new(0));
    let prev = entry.load(Ordering::Relaxed);
    if now_ms.saturating_sub(prev) < interval_ms {
        return false;
    }
    entry
        .compare_exchange(prev, now_ms, Ordering::Relaxed, Ordering::Relaxed)
        .is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_expected_path_unavailable() {
        assert!(Error::RouteError(None).is_expected_path_unavailable());
        assert!(Error::RouteError(Some("x".into())).is_expected_path_unavailable());
        assert!(Error::PeerNoConnectionError(42).is_expected_path_unavailable());
        assert!(Error::Tunnel(TunnelError::Shutdown).is_expected_path_unavailable());
    }

    #[test]
    fn rejects_unexpected_failures() {
        assert!(!Error::WaitRespError("timeout".into()).is_expected_path_unavailable());
        assert!(!Error::NotFound.is_expected_path_unavailable());
        assert!(!Error::Tunnel(TunnelError::BufferFull).is_expected_path_unavailable());
        assert!(!Error::SecretKeyError("bad".into()).is_expected_path_unavailable());
        assert!(!Error::PinnedRemotePubkeyMismatch.is_expected_path_unavailable());
    }

    #[test]
    fn detects_pinned_mismatch_through_anyhow() {
        assert!(Error::PinnedRemotePubkeyMismatch.is_pinned_remote_pubkey_mismatch());
        let wrapped = anyhow::Error::from(Error::PinnedRemotePubkeyMismatch);
        assert!(anyhow_is_pinned_remote_pubkey_mismatch(&wrapped));
        assert!(!anyhow_is_pinned_remote_pubkey_mismatch(&anyhow::anyhow!(
            "transport timeout"
        )));
    }

    #[test]
    fn warn_token_rate_limits_per_peer() {
        PATH_UNAVAILABLE_WARN_MS.clear();
        let peer_a = 1001;
        let peer_b = 1002;
        let t0 = 1_000_000u64;

        assert!(take_path_unavailable_warn_token_at(peer_a, t0));
        assert!(
            !take_path_unavailable_warn_token_at(peer_a, t0 + 1),
            "same peer within interval must be denied"
        );
        assert!(
            take_path_unavailable_warn_token_at(peer_b, t0 + 1),
            "other peer must get its own token"
        );

        let after = t0 + PATH_UNAVAILABLE_WARN_INTERVAL.as_millis() as u64;
        assert!(take_path_unavailable_warn_token_at(peer_a, after));
    }

    #[test]
    fn classifies_congestion_separately_from_a_dead_path() {
        assert!(Error::Tunnel(TunnelError::BufferFull).is_congestion());
        assert!(!Error::Tunnel(TunnelError::Shutdown).is_congestion());
        assert!(!Error::RouteError(None).is_congestion());
        assert!(!Error::NotFound.is_congestion());
    }

    #[test]
    fn congestion_warn_does_not_consume_the_path_unavailable_warn() {
        PATH_UNAVAILABLE_WARN_MS.clear();
        CONGESTION_WARN_MS.clear();
        let peer = 2001;
        let t0 = 2_000_000u64;

        assert!(take_congestion_warn_token_at(peer, t0));
        assert!(
            !take_congestion_warn_token_at(peer, t0 + 1),
            "same peer within interval must be denied"
        );
        assert!(
            take_path_unavailable_warn_token_at(peer, t0 + 1),
            "a congestion WARN must not use up the path-unavailable WARN"
        );
    }
}
