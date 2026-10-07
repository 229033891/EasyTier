use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

use arc_swap::ArcSwapOption;
use crossbeam::atomic::AtomicCell;
use dashmap::{DashMap, DashSet};
use parking_lot::{Mutex, RwLock};

use tokio::{select, sync::mpsc};

use tracing::Instrument;

use super::conn_bond::{
    BondCandidate, BondConfig, bond_member_index, conn_diversity_class, flow_key_from_payload,
    pick_bond_set,
};
use super::conn_select::{ConnMetrics, ConnSelectConfig, pick_default_conn, score_conn};
use super::peer_conn::{PeerConn, PeerConnId};
use crate::peers::{
    PacketRecvChan,
    context::{ArcPeerContext, PeerEvent},
    util::shrink_dashmap,
};
use crate::{
    config::PeerId,
    packet::ZCPacket,
    peers::error::Error,
    proto::{core_peer::peer::PeerConnInfo, peer_rpc::PeerIdentityType},
};
use tokio_util::task::AbortOnDropHandle;

type ArcPeerConn = Arc<PeerConn>;
type ConnMap = Arc<DashMap<PeerConnId, ArcPeerConn>>;

pub struct Peer {
    pub peer_node_id: PeerId,
    conns: ConnMap,
    context: ArcPeerContext,

    packet_recv_chan: PacketRecvChan,

    close_event_sender: mpsc::Sender<PeerConnId>,
    #[allow(dead_code)]
    close_event_listener: AbortOnDropHandle<()>,

    shutdown_notifier: Arc<tokio::sync::Notify>,

    default_conn: Arc<ArcSwapOption<PeerConn>>,
    /// Cached bond members when `peer_link_bond_count > 1` (cleared with default_conn).
    bond_conns: Arc<ArcSwapOption<Vec<ArcPeerConn>>>,
    default_conn_update_lock: Arc<Mutex<()>>,
    /// Survives periodic default_conn cache clears for hysteresis (P1.7).
    last_default_conn_id: AtomicCell<Option<PeerConnId>>,
    better_streak: AtomicU32,
    peer_identity_type: Arc<AtomicCell<Option<PeerIdentityType>>>,
    peer_public_key: Arc<RwLock<Option<Vec<u8>>>>,
    #[allow(dead_code)]
    default_conn_clear_task: AbortOnDropHandle<()>,
}

impl Peer {
    pub(crate) fn new(
        peer_node_id: PeerId,
        packet_recv_chan: PacketRecvChan,
        context: ArcPeerContext,
    ) -> Self {
        let conns: ConnMap = Arc::new(DashMap::new());
        let (close_event_sender, mut close_event_receiver) = mpsc::channel(10);
        let shutdown_notifier = Arc::new(tokio::sync::Notify::new());
        let peer_identity_type = Arc::new(AtomicCell::new(None));
        let peer_identity_type_copy = peer_identity_type.clone();
        let peer_public_key = Arc::new(RwLock::new(None));
        let peer_public_key_copy = peer_public_key.clone();
        let default_conn = Arc::new(ArcSwapOption::empty());
        let bond_conns = Arc::new(ArcSwapOption::empty());
        let default_conn_update_lock = Arc::new(Mutex::new(()));
        let last_default_conn_id = AtomicCell::new(None);
        let better_streak = AtomicU32::new(0);

        let conns_copy = conns.clone();
        let shutdown_notifier_copy = shutdown_notifier.clone();
        let context_copy = context.clone();
        let default_conn_copy = default_conn.clone();
        let bond_conns_copy_close = bond_conns.clone();
        let default_conn_update_lock_copy = default_conn_update_lock.clone();
        let close_event_listener = AbortOnDropHandle::new(tokio::spawn(
            async move {
                loop {
                    select! {
                        ret = close_event_receiver.recv() => {
                            if ret.is_none() {
                                break;
                            }
                            let ret = ret.unwrap();
                            tracing::warn!(
                                ?peer_node_id,
                                ?ret,
                                "notified that peer conn is closed",
                            );

                            let removed_conn = {
                                let _update_guard = default_conn_update_lock_copy.lock();
                                let removed_conn = conns_copy.remove(&ret);
                                if let Some((_, conn)) = removed_conn.as_ref() {
                                    let cached_conn = default_conn_copy.load();
                                    if cached_conn
                                        .as_ref()
                                        .is_some_and(|cached| Arc::ptr_eq(cached, conn))
                                    {
                                        default_conn_copy.store(None);
                                    }
                                    // Drop bond cache if the closed conn was a member.
                                    if bond_conns_copy_close
                                        .load()
                                        .as_ref()
                                        .is_some_and(|members| {
                                            members.iter().any(|m| Arc::ptr_eq(m, conn))
                                        })
                                    {
                                        bond_conns_copy_close.store(None);
                                    }
                                }
                                removed_conn
                            };

                            if let Some((_, conn)) = removed_conn {
                                context_copy.issue_event(PeerEvent::PeerConnRemoved(
                                    conn.get_conn_info(),
                                ));
                                shrink_dashmap(&conns_copy, Some(4));
                                if conns_copy.is_empty() {
                                    peer_identity_type_copy.store(None);
                                    *peer_public_key_copy.write() = None;
                                }
                            }
                        }

                        _ = shutdown_notifier_copy.notified() => {
                            close_event_receiver.close();
                            tracing::warn!(?peer_node_id, "peer close event listener notified");
                        }
                    }
                }
                tracing::info!("peer {} close event listener exit", peer_node_id);
            }
            .instrument(tracing::info_span!(
                "peer_close_event_listener",
                ?peer_node_id,
            )),
        ));

        let conns_copy = conns.clone();
        let default_conn_copy = default_conn.clone();
        let bond_conns_copy = bond_conns.clone();
        let default_conn_clear_task = AbortOnDropHandle::new(tokio::spawn(async move {
            loop {
                crate::foundation::time::sleep(std::time::Duration::from_secs(5)).await;
                if conns_copy.len() > 1 {
                    default_conn_copy.store(None);
                    bond_conns_copy.store(None);
                }
            }
        }));

        Peer {
            peer_node_id,
            conns,
            packet_recv_chan,
            context,

            close_event_sender,
            close_event_listener,

            shutdown_notifier,
            default_conn,
            bond_conns,
            default_conn_update_lock,
            last_default_conn_id,
            better_streak,
            peer_identity_type,
            peer_public_key,
            default_conn_clear_task,
        }
    }

    pub async fn add_peer_conn(&self, mut conn: PeerConn) -> Result<(), Error> {
        let conn_identity_type = conn.get_peer_identity_type();
        let peer_identity_type = self.peer_identity_type.load();
        if let Some(peer_identity_type) = peer_identity_type {
            if peer_identity_type != conn_identity_type {
                return Err(Error::SecretKeyError(format!(
                    "peer identity type mismatch. peer: {:?}, conn: {:?}",
                    peer_identity_type, conn_identity_type
                )));
            }
        } else {
            self.peer_identity_type.store(Some(conn_identity_type));
        }

        let close_notifier = conn.get_close_notifier();
        let conn_info = conn.get_conn_info();
        let conn_pubkey = conn_info.noise_remote_static_pubkey.clone();
        {
            let mut peer_pubkey = self.peer_public_key.write();
            if let Some(existing_pubkey) = peer_pubkey.as_ref() {
                if existing_pubkey != &conn_pubkey {
                    return Err(Error::SecretKeyError(format!(
                        "peer public key mismatch. peer_id: {}, existing_len: {}, new_len: {}",
                        self.peer_node_id,
                        existing_pubkey.len(),
                        conn_pubkey.len()
                    )));
                }
            } else {
                *peer_pubkey = Some(conn_pubkey);
            }
        }

        conn.start_recv_loop(self.packet_recv_chan.clone()).await;
        conn.start_pingpong();
        self.conns.insert(conn.get_conn_id(), Arc::new(conn));

        let close_event_sender = self.close_event_sender.clone();
        tokio::spawn(async move {
            let conn_id = close_notifier.get_conn_id();
            if let Some(mut waiter) = close_notifier.get_waiter().await {
                let _ = waiter.recv().await;
            }
            if let Err(e) = close_event_sender.send(conn_id).await {
                tracing::warn!(?conn_id, "failed to send close event: {}", e);
            }
        });

        self.context
            .issue_event(PeerEvent::PeerConnAdded(conn_info));
        Ok(())
    }

    fn select_conn(&self) -> Option<ArcPeerConn> {
        let _update_guard = self.default_conn_update_lock.lock();
        if let Some(conn) = self.default_conn.load_full() {
            return Some(conn);
        }

        let cfg = ConnSelectConfig::from_flags(&self.context.flags());
        let mut scored = Vec::with_capacity(self.conns.len());
        let mut by_id = std::collections::HashMap::with_capacity(self.conns.len());
        for entry in self.conns.iter() {
            let conn = entry.value().clone();
            // Closing tunnels must not win default_conn on stale metrics.
            if conn.is_closed() {
                continue;
            }
            let stats = conn.get_stats();
            let metrics = ConnMetrics {
                conn_id: conn.get_conn_id(),
                latency_us: stats.latency_us,
                loss_rate: conn.loss_rate(),
                jitter_us: stats.jitter_us,
                is_hole_punched: conn.is_hole_punched(),
            };
            scored.push(score_conn(&metrics, cfg));
            by_id.insert(conn.get_conn_id(), conn);
        }

        let last = self.last_default_conn_id.load();
        let streak = self.better_streak.load(Ordering::Relaxed);
        let (picked_id, next_streak) = pick_default_conn(&scored, last, streak, cfg)?;
        self.better_streak.store(next_streak, Ordering::Relaxed);
        self.last_default_conn_id.store(Some(picked_id));

        let selected = by_id.get(&picked_id).cloned();
        if let Some(conn) = selected.as_ref() {
            self.default_conn.store(Some(conn.clone()));
        }
        selected
    }

    pub async fn send_msg(&self, msg: ZCPacket) -> Result<(), Error> {
        let bond = BondConfig::from_flags(&self.context.flags());
        if bond.enabled() {
            let members = self.select_bond_conns(bond);
            if let Some(members) = members
                && !members.is_empty()
            {
                // Prefer precomputed key (plaintext before encrypt); payload hash
                // is only a fallback for paths that never set bond_flow_key.
                let key = msg
                    .bond_flow_key()
                    .unwrap_or_else(|| flow_key_from_payload(msg.payload()));
                let idx = bond_member_index(key, members.len());
                // On member failure drop the cached set so the next packet rebuilds
                // without a half-dead member (matches single-path cache-clear semantics).
                return members[idx].send_msg(msg).await.map_err(|e| {
                    self.bond_conns.store(None);
                    e
                });
            }
            // Fall through to single-path select if bond set empty.
        }

        let default_conn = self.default_conn.load();
        if let Some(conn) = default_conn.as_ref() {
            conn.send_msg(msg).await?;
            return Ok(());
        }
        drop(default_conn);

        let Some(conn) = self.select_conn() else {
            return Err(Error::PeerNoConnectionError(self.peer_node_id));
        };
        conn.send_msg(msg).await?;

        Ok(())
    }

    /// Build / refresh the bond member cache (diversity-first, replica-fill).
    fn select_bond_conns(&self, bond: BondConfig) -> Option<Arc<Vec<ArcPeerConn>>> {
        if let Some(cached) = self.bond_conns.load_full()
            && !cached.is_empty()
            && cached.iter().all(|c| !c.is_closed())
        {
            return Some(cached);
        }

        let _update_guard = self.default_conn_update_lock.lock();
        // Re-check under lock.
        if let Some(cached) = self.bond_conns.load_full()
            && !cached.is_empty()
            && cached.iter().all(|c| !c.is_closed())
        {
            return Some(cached);
        }

        let cfg = ConnSelectConfig::from_flags(&self.context.flags());
        let mut candidates = Vec::with_capacity(self.conns.len());
        let mut by_id = std::collections::HashMap::with_capacity(self.conns.len());
        for entry in self.conns.iter() {
            let conn = entry.value().clone();
            if conn.is_closed() {
                continue;
            }
            let stats = conn.get_stats();
            let metrics = ConnMetrics {
                conn_id: conn.get_conn_id(),
                latency_us: stats.latency_us,
                loss_rate: conn.loss_rate(),
                jitter_us: stats.jitter_us,
                is_hole_punched: conn.is_hole_punched(),
            };
            candidates.push(BondCandidate {
                scored: score_conn(&metrics, cfg),
                class: conn_diversity_class(&conn),
            });
            by_id.insert(conn.get_conn_id(), conn);
        }

        let ids = pick_bond_set(&candidates, bond, cfg);
        if ids.is_empty() {
            self.bond_conns.store(None);
            return None;
        }
        let mut members: Vec<ArcPeerConn> = ids
            .into_iter()
            .filter_map(|id| by_id.get(&id).cloned())
            .collect();
        if members.is_empty() {
            self.bond_conns.store(None);
            return None;
        }
        // Stable order so 5s cache rebuild keeps the same flow→member mapping
        // when the member set is unchanged (conn_id is stable).
        members.sort_by_key(|c| c.get_conn_id());
        let members = Arc::new(members);
        self.bond_conns.store(Some(members.clone()));
        Some(members)
    }

    pub async fn close_peer_conn(&self, conn_id: &PeerConnId) -> Result<(), Error> {
        let has_key = self.conns.contains_key(conn_id);
        if !has_key {
            return Err(Error::NotFound);
        }
        self.close_event_sender.send(*conn_id).await.unwrap();
        Ok(())
    }

    pub async fn list_peer_conns(&self) -> Vec<PeerConnInfo> {
        let mut conns = vec![];
        for conn in self.conns.iter() {
            // do not lock here, otherwise it will cause dashmap deadlock
            conns.push(conn.clone());
        }

        let cfg = ConnSelectConfig::from_flags(&self.context.flags());
        let mut live: Vec<(uuid::Uuid, ArcPeerConn, PeerConnInfo)> = Vec::new();
        for conn in conns {
            let info = conn.get_conn_info();
            if info.is_closed {
                let conn_id = info.conn_id.parse().ok();
                if let Some(conn_id) = conn_id {
                    let _ = self.close_peer_conn(&conn_id).await;
                }
                continue;
            }
            let Ok(conn_id) = info.conn_id.parse::<uuid::Uuid>() else {
                continue;
            };
            live.push((conn_id, conn, info));
        }

        // Attach the same quality score/fuse flags used by select_conn so the
        // status surface can explain why a path is (or is not) default_conn.
        let scored: Vec<_> = live
            .iter()
            .map(|(conn_id, conn, _)| {
                let stats = conn.get_stats();
                score_conn(
                    &ConnMetrics {
                        conn_id: *conn_id,
                        latency_us: stats.latency_us,
                        loss_rate: conn.loss_rate(),
                        jitter_us: stats.jitter_us,
                        is_hole_punched: conn.is_hole_punched(),
                    },
                    cfg,
                )
            })
            .collect();
        let by_id: std::collections::HashMap<_, _> =
            scored.iter().map(|s| (s.conn_id, *s)).collect();

        // Bond membership for status: conn ids currently in the bond send set.
        let bond_members: std::collections::HashSet<PeerConnId> = self
            .bond_conns
            .load_full()
            .map(|members| members.iter().map(|c| c.get_conn_id()).collect())
            .unwrap_or_default();

        live.into_iter()
            .map(|(conn_id, conn, mut info)| {
                if let Some(s) = by_id.get(&conn_id) {
                    info.quality_score = s.score as f32;
                    info.quality_fused = s.fused;
                    info.unverified_hole_punch = s.unverified_hole_punch;
                }
                if bond_members.contains(&conn_id) {
                    info.in_bond_set = true;
                    info.bond_class = conn_diversity_class(&conn).scheme;
                }
                info
            })
            .collect()
    }

    pub fn has_live_conns(&self) -> bool {
        self.conns.iter().any(|entry| !entry.value().is_closed())
    }

    pub fn live_conn_count(&self) -> usize {
        self.conns
            .iter()
            .filter(|entry| !entry.value().is_closed())
            .count()
    }

    pub fn has_directly_connected_conn(&self) -> bool {
        self.conns
            .iter()
            .any(|entry| !entry.value().is_closed() && !entry.value().is_hole_punched())
    }

    #[cfg(test)]
    pub(crate) fn has_direct_attached_conn(&self) -> bool {
        self.conns.iter().any(|entry| {
            let conn = entry.value();
            !conn.is_closed() && !conn.is_hole_punched() && conn.is_attached()
        })
    }

    pub fn get_directly_connections(&self) -> DashSet<uuid::Uuid> {
        self.conns
            .iter()
            .filter(|entry| !(entry.value()).is_hole_punched())
            .map(|entry| (entry.value()).get_conn_id())
            .collect()
    }

    pub fn get_default_conn_id(&self) -> PeerConnId {
        self.default_conn
            .load()
            .as_ref()
            .map(|conn| conn.get_conn_id())
            .unwrap_or_default()
    }

    pub fn get_peer_identity_type(&self) -> Option<PeerIdentityType> {
        self.peer_identity_type.load()
    }

    pub fn get_peer_public_key(&self) -> Option<Vec<u8>> {
        self.peer_public_key.read().clone()
    }
}

// pritn on drop
impl Drop for Peer {
    fn drop(&mut self) {
        self.conns.retain(|_, conn| {
            self.context
                .issue_event(PeerEvent::PeerConnRemoved(conn.get_conn_info()));
            false
        });
        self.shutdown_notifier.notify_one();
        tracing::info!("peer {} drop", self.peer_node_id);
    }
}

#[cfg(test)]
mod tests {
    use super::super::conn_select::{
        ConnMetrics, ConnSelectConfig, conn_quality_score, pick_default_conn, score_conn,
    };
    use uuid::Uuid;

    fn id(n: u8) -> Uuid {
        Uuid::from_bytes([n; 16])
    }

    #[test]
    fn measured_relay_precedes_unverified_hole_punch_path() {
        let cfg = ConnSelectConfig::default();
        let scored = [
            score_conn(
                &ConnMetrics {
                    conn_id: id(1),
                    latency_us: 0,
                    loss_rate: 0.0,
                    jitter_us: 0,
                    is_hole_punched: true,
                },
                cfg,
            ),
            score_conn(
                &ConnMetrics {
                    conn_id: id(2),
                    latency_us: 20_000,
                    loss_rate: 0.0,
                    jitter_us: 0,
                    is_hole_punched: false,
                },
                cfg,
            ),
        ];
        let (picked, _) = pick_default_conn(&scored, None, 0, cfg).unwrap();
        assert_eq!(picked, id(2));
    }

    #[test]
    fn unverified_regular_connection_keeps_existing_priority() {
        // latency 0 on a non-hole-punched path is still a valid measured-or-idle RTT;
        // quality score treats it as best RTT (not deferred).
        let cfg = ConnSelectConfig::default();
        let a = conn_quality_score(0, 0.0, 0, cfg);
        let b = conn_quality_score(20_000, 0.0, 0, cfg);
        assert!(a < b);
    }

    #[test]
    fn verified_lower_quality_path_can_be_preferred() {
        let cfg = ConnSelectConfig::default();
        let a = conn_quality_score(5_000, 0.0, 0, cfg);
        let b = conn_quality_score(20_000, 0.0, 0, cfg);
        assert!(a < b);
    }
}
