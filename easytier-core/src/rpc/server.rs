use std::{
    pin::Pin,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
};

use bytes::Bytes;
use dashmap::DashMap;
use futures::StreamExt;
use prost::Message;
use tokio::task::JoinSet;

use crate::{
    foundation::{
        stats::{ArcRpcMetrics, RpcMetricLabels, RpcMetricsProvider},
        task::reap_joinset_background,
    },
    proto::{
        common::{self, RpcCompressionInfo, RpcPacket, RpcRequest, RpcResponse, TunnelInfo},
        rpc_types::error::Result,
    },
    rpc::packet::BuildRpcPacketArgs,
    tunnel::{
        Tunnel, ZCPacketStream,
        mpsc::{MpscTunnel, MpscTunnelSender},
        ring::create_ring_tunnel_pair,
    },
};

use super::{
    Transport,
    dispatch::dispatch_request,
    packet::{
        PacketMerger, accepted_compression_algo, build_rpc_packet, compress_packet,
        decompress_packet,
    },
    service_registry::ServiceRegistry,
};

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct PacketMergerKey {
    from_peer_id: crate::config::PeerId,
    transaction_id: i64,
}

pub struct Server {
    registry: Arc<ServiceRegistry>,
    mpsc: Mutex<Option<MpscTunnel<Box<dyn Tunnel>>>>,
    transport: Mutex<Transport>,
    tasks: Arc<Mutex<JoinSet<()>>>,
    handler_tasks: Arc<Mutex<JoinSet<()>>>,
    stopped: Arc<AtomicBool>,
    packet_mergers: Arc<DashMap<PacketMergerKey, PacketMerger>>,
    metrics: Option<ArcRpcMetrics>,
}

impl Default for Server {
    fn default() -> Self {
        Self::new()
    }
}

impl Server {
    pub fn new() -> Self {
        Server::new_with_registry(Arc::new(ServiceRegistry::new()))
    }

    pub fn new_with_registry(registry: Arc<ServiceRegistry>) -> Self {
        let (ring_a, ring_b) = create_ring_tunnel_pair();

        Self {
            registry,
            mpsc: Mutex::new(Some(MpscTunnel::new(ring_a, None))),
            transport: Mutex::new(MpscTunnel::new(ring_b, None)),
            tasks: Arc::new(Mutex::new(JoinSet::new())),
            handler_tasks: Arc::new(Mutex::new(JoinSet::new())),
            stopped: Arc::new(AtomicBool::new(false)),
            packet_mergers: Arc::new(DashMap::new()),
            metrics: None,
        }
    }

    pub fn new_with_registry_and_stats_manager<T>(
        registry: Arc<ServiceRegistry>,
        stats_manager: T,
    ) -> Self
    where
        T: RpcMetricsProvider,
    {
        let mut server = Self::new_with_registry(registry);
        server.metrics = stats_manager.into_rpc_metrics();
        server
    }

    pub fn new_with_registry_and_metrics(
        registry: Arc<ServiceRegistry>,
        metrics: ArcRpcMetrics,
    ) -> Self {
        let mut server = Self::new_with_registry(registry);
        server.metrics = Some(metrics);
        server
    }

    pub fn registry(&self) -> &ServiceRegistry {
        &self.registry
    }

    pub fn get_transport_sink(&self) -> MpscTunnelSender {
        self.transport.lock().unwrap().get_sink()
    }

    pub fn get_transport_stream(&self) -> Pin<Box<dyn ZCPacketStream>> {
        self.transport.lock().unwrap().get_stream()
    }

    pub fn run(&self) {
        self.run_with_tunnel_info(None);
    }

    pub(crate) fn run_with_tunnel_info(&self, tunnel_info: Option<TunnelInfo>) {
        self.stopped.store(false, Ordering::Relaxed);
        let handler_tasks = self.handler_tasks.clone();
        self.tasks.lock().unwrap().spawn(reap_joinset_background(
            Arc::downgrade(&handler_tasks),
            "rpc server handlers",
        ));

        let mpsc = self.mpsc.lock().unwrap().take().unwrap();

        let packet_merges = self.packet_mergers.clone();
        let reg = self.registry.clone();
        let tunnel_info = tunnel_info.or_else(|| mpsc.tunnel_info());
        let metrics = self.metrics.clone();
        let handler_tasks_weak = Arc::downgrade(&handler_tasks);
        let stopped = self.stopped.clone();
        self.tasks.lock().unwrap().spawn(async move {
            let mut mpsc = mpsc;
            let mut rx = mpsc.get_stream();

            while let Some(packet) = rx.next().await {
                let packet = match packet {
                    Err(err) => {
                        tracing::error!(?err, "Failed to receive packet");
                        continue;
                    }
                    Ok(packet) => packet,
                };

                // CRITICAL (CR-07): capture the sender identity from the **outer**
                // ZCPacket header *before* the payload is consumed.
                //
                // `RpcPacket.from_peer` (read further down) is a field the sender
                // wrote into its own payload — it is authenticated only in the
                // sense that the whole blob was encrypted, so an authenticated
                // peer can set it to any value. The outer
                // `peer_manager_header().from_peer_id` is different:
                // `PeerConn::after_received` drops any packet whose
                // `from_peer_id` differs from the authenticated peer id
                // *before* decrypting, so a packet that got this far is proven
                // to come from that peer.
                let verified_from_peer_id =
                    packet.peer_manager_header().map(|h| h.from_peer_id.get());
                let packet = match common::RpcPacket::decode(packet.payload()) {
                    Err(err) => {
                        tracing::error!(?err, "Failed to decode packet");
                        continue;
                    }
                    Ok(packet) => packet,
                };

                if !packet.is_request {
                    tracing::warn!(?packet, "Received non-request packet");
                    continue;
                }

                let key = PacketMergerKey {
                    // CRITICAL (CR-07): key the merger on the *verified* sender,
                    // not on `packet.from_peer`.
                    //
                    // Pieces of one RPC are keyed by `(from_peer, transaction_id)`
                    // and `PacketMerger::feed` only checks that all pieces agree on
                    // those two fields. Because `packet.from_peer` is chosen by the
                    // sender, an authenticated peer could set it to a third party's
                    // id and, if its `transaction_id` happened to collide with that
                    // party's in-flight request, have its own pieces spliced into
                    // the victim's request body — the merged packet would then be
                    // attributed to the victim. Keying on the transport-verified id
                    // means pieces from different connections can never share a
                    // merger entry.
                    from_peer_id: verified_from_peer_id.unwrap_or(packet.from_peer),
                    transaction_id: packet.transaction_id,
                };

                tracing::trace!(?key, ?packet, "Received request packet");

                let ret = packet_merges.entry(key.clone()).or_default().feed(packet);

                match ret {
                    Ok(Some(packet)) => {
                        packet_merges.remove(&key);
                        let Some(handler_tasks) = handler_tasks_weak.upgrade() else {
                            tracing::error!("rpc server handler task set is dropped");
                            return;
                        };
                        let mut handler_tasks = handler_tasks.lock().unwrap();
                        if stopped.load(Ordering::Relaxed) {
                            return;
                        }
                        handler_tasks.spawn(Self::handle_rpc(
                            mpsc.get_sink(),
                            packet,
                            reg.clone(),
                            tunnel_info.clone(),
                            metrics.clone(),
                            verified_from_peer_id,
                        ));
                    }
                    Ok(None) => {}
                    Err(err) => {
                        tracing::error!("Failed to feed packet to merger, {}", err);
                    }
                }
            }
        });

        let packet_mergers = self.packet_mergers.clone();
        self.tasks.lock().unwrap().spawn(async move {
            loop {
                crate::foundation::time::sleep(crate::foundation::time::Duration::from_secs(5))
                    .await;
                packet_mergers.retain(|_, v| v.last_updated().elapsed().as_secs() < 10);
                packet_mergers.shrink_to_fit();
            }
        });
    }

    async fn handle_rpc_request(
        packet: RpcPacket,
        descriptor: common::RpcDescriptor,
        reg: Arc<ServiceRegistry>,
        tunnel_info: Option<TunnelInfo>,
        verified_from_peer_id: Option<u32>,
    ) -> Result<Bytes> {
        let body = if let Some(compression_info) = packet.compression_info {
            decompress_packet(
                compression_info.algo.try_into().unwrap_or_default(),
                &packet.body,
            )
            .await?
        } else {
            packet.body
        };
        let rpc_request = RpcRequest::decode(Bytes::from(body))?;
        // CR-07: hand the transport-verified sender id down so handlers can bind
        // identity to the connection instead of trusting the request body.
        // Note this is deliberately NOT `packet.from_peer`: that field lives
        // inside the encrypted payload and is fully chosen by the sender, so an
        // authenticated peer could put any id there.
        dispatch_request(
            reg.as_ref(),
            descriptor,
            rpc_request,
            tunnel_info,
            verified_from_peer_id,
        )
        .await
    }

    async fn handle_rpc(
        sender: MpscTunnelSender,
        packet: RpcPacket,
        reg: Arc<ServiceRegistry>,
        tunnel_info: Option<TunnelInfo>,
        metrics: Option<ArcRpcMetrics>,
        verified_from_peer_id: Option<u32>,
    ) {
        let from_peer = packet.from_peer;
        let to_peer = packet.to_peer;
        let transaction_id = packet.transaction_id;
        let trace_id = packet.trace_id;
        let Some(desc) = packet.descriptor.clone() else {
            tracing::warn!("received RPC request without a descriptor");
            return;
        };
        let method_name = reg.get_method_name(&desc).unwrap_or("<Nil>".to_owned());
        let labels = RpcMetricLabels {
            network_name: desc.domain_name.clone(),
            src_peer_id: from_peer,
            dst_peer_id: to_peer,
            service_name: desc.service_name.clone(),
            method_name,
        };

        if let Some(metrics) = &metrics {
            metrics.server_rx(&labels);
        }

        let mut resp_msg = RpcResponse::default();
        let now = std::time::Instant::now();

        let compression_info = packet.compression_info;
        let resp_bytes = Self::handle_rpc_request(
            packet,
            desc.clone(),
            reg,
            tunnel_info,
            verified_from_peer_id,
        )
        .await;

        match &resp_bytes {
            Ok(r) => {
                resp_msg.response = r.clone().into();
                if let Some(metrics) = &metrics {
                    metrics.server_tx(&labels, now.elapsed().as_millis() as u64);
                }
            }
            Err(err) => {
                resp_msg.error = Some(err.into());
                if let Some(metrics) = &metrics {
                    metrics.server_error(
                        &labels,
                        Some(format!("{:?}", err)),
                        now.elapsed().as_millis() as u64,
                    );
                }
            }
        };
        resp_msg.runtime_us = now.elapsed().as_micros() as u64;

        let (compressed_resp, algo) = compress_packet(
            compression_info.unwrap_or_default().accepted_algo(),
            &resp_msg.encode_to_vec(),
        )
        .await
        .unwrap();

        let packets = build_rpc_packet(BuildRpcPacketArgs {
            from_peer: to_peer,
            to_peer: from_peer,
            rpc_desc: desc,
            transaction_id,
            is_req: false,
            content: &compressed_resp,
            trace_id,
            compression_info: RpcCompressionInfo {
                algo: algo.into(),
                accepted_algo: accepted_compression_algo().into(),
            },
        });
        for packet in packets {
            if let Err(err) = sender.send(packet).await {
                tracing::error!(?err, "Failed to send response packet");
            }
        }
    }

    pub fn close(&self) {
        self.transport.lock().unwrap().close();
    }

    pub async fn stop(&self) {
        self.stopped.store(true, Ordering::Relaxed);
        self.close();
        let (mut tasks, mut handler_tasks) = {
            let mut task_slot = self.tasks.lock().unwrap();
            let mut handler_task_slot = self.handler_tasks.lock().unwrap();
            (
                std::mem::replace(&mut *task_slot, JoinSet::new()),
                std::mem::replace(&mut *handler_task_slot, JoinSet::new()),
            )
        };
        tasks.abort_all();
        handler_tasks.abort_all();
        while tasks.join_next().await.is_some() {}
        while handler_tasks.join_next().await.is_some() {}
    }
}

#[cfg(any(test, feature = "test-utils"))]
mod test_utils {
    use super::Server;

    impl Server {
        #[doc(hidden)]
        pub fn inflight_count(&self) -> usize {
            self.packet_mergers.len()
        }
    }
}
