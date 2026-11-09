use std::{
    pin::Pin,
    sync::Arc,
    time::{Duration, Instant},
};

use futures::SinkExt;
use tokio::{
    sync::mpsc::{Receiver, Sender, channel, error::TrySendError},
    task::JoinHandle,
};

use crate::{
    foundation::time::timeout,
    packet::ZCPacket,
    proto::common::TunnelInfo,
    tunnel::{
        Tunnel, TunnelError, ZCPacketSink, ZCPacketStream,
        egress_probe::{EGRESS_QUEUE_CAP, EgressProbe},
    },
};

#[derive(Clone)]
pub struct MpscTunnelSender {
    tx: Sender<ZCPacket>,
    // STALL PROBE (diagnostic only): see `tunnel::egress_probe`.
    probe: Arc<EgressProbe>,
}

impl MpscTunnelSender {
    /// Test-only: wrap a raw tokio sender (e.g. for unit-testing drain tasks
    /// that own a sink without a full [`MpscTunnel`] setup).
    #[cfg(test)]
    pub(crate) fn from_tokio_sender(sender: Sender<ZCPacket>) -> Self {
        Self {
            tx: sender,
            probe: Arc::new(EgressProbe::detached("test-sender")),
        }
    }

    pub async fn send(&self, item: ZCPacket) -> Result<(), TunnelError> {
        match self.tx.try_send(item) {
            Ok(()) => {
                self.probe.note_enqueue();
                Ok(())
            }
            Err(TrySendError::Full(item)) => {
                // STALL PROBE: the *unbounded* await of the egress path. A 40 s
                // `egress full` episode means this call sat here for 40 s, so
                // timing it separates "queue full because the drain task is
                // stuck" from "queue full because the whole process is".
                let started = Instant::now();
                let ret = self.tx.send(item).await.map_err(|_| TunnelError::Shutdown);
                let waited = started.elapsed();
                self.probe.note_enq_blocked(waited);
                if waited >= Duration::from_millis(20) {
                    tracing::warn!(
                        tunnel = %self.probe.name(),
                        waited_ms = waited.as_millis() as u64,
                        enq_full = self.probe.enq_full(),
                        "EGRESS PROBE: send blocked on a full egress queue"
                    );
                }
                if ret.is_ok() {
                    self.probe.note_enqueue();
                }
                ret
            }
            Err(TrySendError::Closed(_)) => Err(TunnelError::Shutdown),
        }
    }

    pub fn try_send(&self, item: ZCPacket) -> Result<(), TunnelError> {
        match self.tx.try_send(item) {
            Ok(()) => {
                self.probe.note_enqueue();
                Ok(())
            }
            Err(TrySendError::Full(_)) => {
                self.probe.note_enq_full();
                Err(TunnelError::BufferFull)
            }
            Err(TrySendError::Closed(_)) => Err(TunnelError::Shutdown),
        }
    }
}

pub struct MpscTunnel<T> {
    tx: Option<Sender<ZCPacket>>,
    tunnel: T,
    stream: Option<Pin<Box<dyn ZCPacketStream>>>,
    probe: Arc<EgressProbe>,
    task: JoinHandle<()>,
}

impl<T: Tunnel> MpscTunnel<T> {
    pub fn new(tunnel: T, send_timeout: Option<Duration>) -> Self {
        let (tx, mut rx) = channel(EGRESS_QUEUE_CAP as usize);

        // STALL PROBE (diagnostic only): name the tunnel so a jam can be
        // attributed to a peer without needing the conn's span context.
        let probe_name = tunnel
            .info()
            .and_then(|info| info.remote_addr.map(|url| url.to_string()))
            .unwrap_or_else(|| "unknown".to_owned());
        let probe = EgressProbe::register(probe_name);

        let (stream, mut sink) = tunnel.split();

        let task = {
            let probe = probe.clone();
            tokio::spawn(async move {
                loop {
                    if let Err(e) =
                        Self::forward_one_round(&mut rx, &mut sink, send_timeout, &probe).await
                    {
                        if matches!(e, TunnelError::Shutdown) {
                            tracing::debug!(?e, "forward error");
                        } else {
                            tracing::error!(?e, "forward error");
                        }
                        break;
                    }
                }
                rx.close();
                let close_ret = timeout(Duration::from_secs(5), sink.close()).await;
                match &close_ret {
                    Ok(Ok(())) => tracing::debug!(?close_ret, "mpsc close sink"),
                    _ => tracing::warn!(?close_ret, "mpsc close sink"),
                }
            })
        };

        Self {
            tx: Some(tx),
            tunnel,
            stream: Some(stream),
            probe,
            task,
        }
    }

    async fn forward_one_round(
        rx: &mut Receiver<ZCPacket>,
        sink: &mut Pin<Box<dyn ZCPacketSink>>,
        send_timeout_ms: Option<Duration>,
        probe: &EgressProbe,
    ) -> Result<(), TunnelError> {
        // STALL PROBE: this timestamp is what distinguishes "the drain task is
        // stuck inside feed/flush" (idle_ms keeps growing, max_feed_ms frozen)
        // from "the drain task is not being scheduled at all" (both silent).
        probe.note_poll();
        let item = rx.recv().await.ok_or(TunnelError::Shutdown)?;
        probe.note_dequeue();
        if let Some(timeout_ms) = send_timeout_ms {
            Self::forward_one_round_with_timeout(rx, sink, item, timeout_ms, probe).await
        } else {
            Self::forward_one_round_no_timeout(rx, sink, item, probe).await
        }
    }

    async fn forward_one_round_no_timeout(
        rx: &mut Receiver<ZCPacket>,
        sink: &mut Pin<Box<dyn ZCPacketSink>>,
        initial_item: ZCPacket,
        probe: &EgressProbe,
    ) -> Result<(), TunnelError> {
        probe.note_feed_begin();
        let started = Instant::now();
        let fed = sink.feed(initial_item).await;
        probe.note_feed(started.elapsed());
        fed?;

        while let Ok(item) = rx.try_recv() {
            probe.note_dequeue();
            probe.note_feed_begin();
            let started = Instant::now();
            let fed = sink.feed(item).await;
            probe.note_feed(started.elapsed());
            if let Err(e) = fed {
                if matches!(e, TunnelError::Shutdown) {
                    tracing::debug!(?e, "feed error");
                } else {
                    tracing::error!(?e, "feed error");
                }
                return Err(e);
            }
        }

        probe.note_feed_begin();
        let started = Instant::now();
        let flushed = sink.flush().await;
        probe.note_feed(started.elapsed());
        flushed
    }

    async fn forward_one_round_with_timeout(
        rx: &mut Receiver<ZCPacket>,
        sink: &mut Pin<Box<dyn ZCPacketSink>>,
        initial_item: ZCPacket,
        no_progress_timeout: Duration,
        probe: &EgressProbe,
    ) -> Result<(), TunnelError> {
        // Per-write no-progress budget (not a whole-batch wall clock). A slow
        // but progressing link can take longer than `no_progress_timeout` to
        // drain a full ring; only a stuck feed/flush tears the conn down.
        async fn feed_or_timeout(
            sink: &mut Pin<Box<dyn ZCPacketSink>>,
            item: ZCPacket,
            no_progress_timeout: Duration,
            probe: &EgressProbe,
        ) -> Result<(), TunnelError> {
            probe.note_feed_begin();
            let started = Instant::now();
            let ret = match timeout(no_progress_timeout, sink.feed(item)).await {
                Ok(ret) => ret,
                Err(e) => {
                    probe.note_feed(started.elapsed());
                    tracing::error!(?e, "mpsc feed no-progress timeout");
                    return Err(e.into());
                }
            };
            probe.note_feed(started.elapsed());
            ret
        }

        feed_or_timeout(sink, initial_item, no_progress_timeout, probe).await?;

        while let Ok(item) = rx.try_recv() {
            probe.note_dequeue();
            if let Err(e) = feed_or_timeout(sink, item, no_progress_timeout, probe).await {
                if matches!(e, TunnelError::Shutdown) {
                    tracing::debug!(?e, "feed error");
                } else {
                    tracing::error!(?e, "feed error");
                }
                return Err(e);
            }
        }

        probe.note_feed_begin();
        let flush_started = Instant::now();
        let flushed = timeout(no_progress_timeout, sink.flush()).await;
        probe.note_feed(flush_started.elapsed());
        match flushed {
            Ok(Ok(())) => Ok(()),
            Ok(Err(e)) => {
                if matches!(e, TunnelError::Shutdown) {
                    tracing::debug!(?e, "flush error");
                } else {
                    tracing::error!(?e, "flush error");
                }
                Err(e)
            }
            Err(e) => {
                tracing::error!(
                    ?e,
                    elapsed_ms = flush_started.elapsed().as_millis(),
                    "mpsc flush no-progress timeout"
                );
                Err(e.into())
            }
        }
    }

    pub fn get_stream(&mut self) -> Pin<Box<dyn ZCPacketStream>> {
        self.stream.take().unwrap()
    }

    pub fn get_sink(&self) -> MpscTunnelSender {
        MpscTunnelSender {
            tx: self.tx.as_ref().unwrap().clone(),
            probe: self.probe.clone(),
        }
    }

    pub fn close(&mut self) {
        self.tx.take();
        self.task.abort();
    }

    pub fn tunnel_info(&self) -> Option<TunnelInfo> {
        self.tunnel.info()
    }
}

impl<T> Drop for MpscTunnel<T> {
    fn drop(&mut self) {
        self.task.abort();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn sender_falls_back_to_waiting_when_channel_is_full() {
        let (tx, mut rx) = channel(1);
        let sender = MpscTunnelSender {
            tx,
            probe: Arc::new(EgressProbe::detached("test-sender")),
        };
        sender
            .send(ZCPacket::new_with_payload(b"first"))
            .await
            .unwrap();

        let blocked_send = sender.send(ZCPacket::new_with_payload(b"second"));
        tokio::pin!(blocked_send);
        assert!(
            tokio::time::timeout(Duration::from_millis(10), &mut blocked_send)
                .await
                .is_err()
        );

        assert_eq!(rx.recv().await.unwrap().payload(), b"first");
        tokio::time::timeout(Duration::from_secs(1), &mut blocked_send)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(rx.recv().await.unwrap().payload(), b"second");
    }
}
