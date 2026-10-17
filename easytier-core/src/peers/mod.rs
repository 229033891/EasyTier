pub(crate) mod acl;
pub(crate) mod admission;
pub(crate) mod attached;
pub(crate) mod conn;
pub mod context;
pub mod credential_manager;
pub mod error;
pub mod foreign_network;
pub mod peer_center;
pub mod peer_manager;
pub(crate) mod peer_rpc;
pub(crate) mod pending_packets;
pub mod public_ipv6;
pub(crate) mod relay_peer_map;
pub(crate) mod route;
pub(crate) mod traffic_metrics;
mod util;
pub(crate) mod whitelist;

#[cfg(any(test, feature = "test-utils"))]
#[allow(dead_code)]
pub(crate) mod test_support;
#[cfg(test)]
mod tests;

/// criterion 基准（`easytier/benches/`）专用构造辅助，仅 `test-utils` feature 下编译。
#[cfg(feature = "test-utils")]
pub mod bench_support;

use crate::packet::ZCPacket;
use tokio::sync::mpsc::error::{SendError, TryRecvError, TrySendError};

use self::conn::peer_conn::PeerConnId;
use crate::config::PeerId;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum PeerConnectionOrigin {
    Network,
    Attached,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum PeerPacketIngress {
    Local,
    Peer {
        peer_id: PeerId,
        conn_id: PeerConnId,
        origin: PeerConnectionOrigin,
    },
}

impl PeerPacketIngress {
    #[cfg(test)]
    pub(crate) fn is_attached(self) -> bool {
        matches!(
            self,
            Self::Peer {
                origin: PeerConnectionOrigin::Attached,
                ..
            }
        )
    }
}

#[derive(Debug)]
pub(crate) struct PeerPacketEnvelope {
    packet: ZCPacket,
    ingress: PeerPacketIngress,
}

impl PeerPacketEnvelope {
    fn local(packet: ZCPacket) -> Self {
        Self {
            packet,
            ingress: PeerPacketIngress::Local,
        }
    }

    fn from_peer(packet: ZCPacket, ingress: PeerPacketIngress) -> Self {
        debug_assert!(matches!(ingress, PeerPacketIngress::Peer { .. }));
        Self { packet, ingress }
    }

    pub(crate) fn into_parts(self) -> (ZCPacket, PeerPacketIngress) {
        (self.packet, self.ingress)
    }
}

#[derive(Clone)]
pub struct PacketRecvChan(tokio::sync::mpsc::Sender<PeerPacketEnvelope>);

impl PacketRecvChan {
    /// Injects a packet produced by a local subsystem. Local injection never carries
    /// the privileges of an attached peer connection.
    pub async fn send(&self, packet: ZCPacket) -> Result<(), SendError<ZCPacket>> {
        self.0
            .send(PeerPacketEnvelope::local(packet))
            .await
            .map_err(|error| SendError(error.0.packet))
    }
}

pub struct PacketRecvChanReceiver(tokio::sync::mpsc::Receiver<PeerPacketEnvelope>);

impl PacketRecvChanReceiver {
    pub async fn recv(&mut self) -> Option<ZCPacket> {
        self.0.recv().await.map(|envelope| envelope.packet)
    }
}

pub fn create_packet_recv_chan() -> (PacketRecvChan, PacketRecvChanReceiver) {
    let (sender, receiver) = tokio::sync::mpsc::channel(128);
    (PacketRecvChan(sender), PacketRecvChanReceiver(receiver))
}

pub(crate) async fn send_peer_packet_to_chan(
    sender: &PacketRecvChan,
    packet: ZCPacket,
    ingress: PeerPacketIngress,
) -> Result<(), SendError<ZCPacket>> {
    let envelope = PeerPacketEnvelope::from_peer(packet, ingress);
    match sender.0.try_send(envelope) {
        Ok(()) => Ok(()),
        Err(TrySendError::Full(envelope)) => sender
            .0
            .send(envelope)
            .await
            .map_err(|error| SendError(error.0.packet)),
        Err(TrySendError::Closed(envelope)) => Err(SendError(envelope.packet)),
    }
}

/// Receive one packet envelope, or `None` once the channel is closed.
///
/// CRITICAL (CR-15): this used to return `Result<_, anyhow::Error>` where both
/// "channel closed" and "receive failed" collapsed into the same opaque error,
/// so the only thing a caller could do with either was `panic!` — which under
/// `panic = "abort"` turned an orderly shutdown into a process abort and hid
/// real failures behind the same crash.
///
/// `None` is the honest signal here: `mpsc::Receiver::recv` yields `None` only
/// when the channel is closed and every sender has been dropped, which is an
/// expected shutdown. There is no separate transport-error case to represent.
pub(crate) async fn recv_packet_envelope_from_chan(
    packet_recv_chan_receiver: &mut PacketRecvChanReceiver,
) -> Option<PeerPacketEnvelope> {
    match packet_recv_chan_receiver.0.try_recv() {
        Ok(packet) => Some(packet),
        Err(TryRecvError::Empty) => packet_recv_chan_receiver.0.recv().await,
        Err(TryRecvError::Disconnected) => None,
    }
}

pub async fn recv_packet_from_chan(
    packet_recv_chan_receiver: &mut PacketRecvChanReceiver,
) -> Result<ZCPacket, anyhow::Error> {
    recv_packet_envelope_from_chan(packet_recv_chan_receiver)
        .await
        .map(|envelope| envelope.packet)
        .ok_or_else(|| anyhow::anyhow!("recv_packet_from_chan failed"))
}

#[async_trait::async_trait]
#[auto_impl::auto_impl(Arc)]
pub trait PeerPacketFilter {
    async fn try_process_packet_from_peer(&self, zc_packet: ZCPacket) -> Option<ZCPacket> {
        Some(zc_packet)
    }
}

#[async_trait::async_trait]
#[auto_impl::auto_impl(Arc)]
pub trait NicPacketFilter {
    async fn try_process_packet_from_nic(&self, data: &mut ZCPacket) -> bool;

    fn id(&self) -> String {
        format!("{:p}", self)
    }
}

pub type BoxPeerPacketFilter = Box<dyn PeerPacketFilter + Send + Sync>;
pub type BoxNicPacketFilter = Box<dyn NicPacketFilter + Send + Sync>;
