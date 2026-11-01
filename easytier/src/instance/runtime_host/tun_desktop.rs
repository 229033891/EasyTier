use std::{sync::Arc, time::Duration};

use anyhow::Context as _;
use cidr::Ipv4Inet;
use easytier_core::{
    gateway::dhcp::{DhcpIpv4ApplyOutcome, DhcpIpv4ApplyPermit, DhcpIpv4Host},
    host::packet::HostPacketReceiver,
    instance::CorePacketPlane,
};
use futures::FutureExt as _;
use tokio::{
    sync::{Mutex, Notify, oneshot},
    task::JoinHandle,
};
use tokio_util::sync::CancellationToken;

use super::{MagicDnsRuntime, tun_common::TunNicState};
use crate::{
    common::{
        error::Error,
        global_ctx::{ArcGlobalCtx, GlobalCtxEvent},
    },
    instance::virtual_nic::NicCtx,
};

/// Number of consecutive TUN recreate failures after which the outage is
/// escalated to the UI/CLI (`GlobalCtxEvent::TunDeviceError`) instead of only
/// being logged once per attempt.
const MAX_FAST_RECREATE_ATTEMPTS: u32 = 5;

pub(super) struct NativeTunRuntime {
    global_ctx: ArcGlobalCtx,
    cancel: CancellationToken,
    nic: TunNicState,
    static_ip_task: Mutex<Option<JoinHandle<()>>>,
    packet_plane: Mutex<Option<Arc<CorePacketPlane>>>,
}

impl NativeTunRuntime {
    pub(super) fn new(global_ctx: ArcGlobalCtx, cancel: CancellationToken) -> Self {
        Self {
            global_ctx,
            cancel,
            nic: TunNicState::empty(),
            static_ip_task: Mutex::new(None),
            packet_plane: Mutex::new(None),
        }
    }

    pub(super) fn install_packet_receiver(
        &self,
        receiver: HostPacketReceiver,
    ) -> anyhow::Result<()> {
        self.nic.install_receiver(receiver)
    }

    fn report_static_ip_cancelled(output: &mut Option<oneshot::Sender<Result<(), Error>>>) {
        if let Some(output) = output.take() {
            let _ = output.send(Err(anyhow::anyhow!(
                "instance is closing; static IP setup cancelled"
            )
            .into()));
        }
    }

    /// Backoff between attempts to recreate a TUN device that failed to come up.
    ///
    /// The first retry happens after 1s, so a transient device loss (interface
    /// deleted or brought down by a netifd reload, `ip link del`, …) is a
    /// sub-second blip. The delay then doubles up to a 10s cap: a persistent
    /// failure settles at one attempt per 10s instead of the unbounded 1/s
    /// busy retry that used to spam the log (and the event bus) forever.
    fn recreate_backoff(failed_attempts: u32) -> Duration {
        const CAP: Duration = Duration::from_secs(10);
        let shift = failed_attempts.saturating_sub(1).min(4);
        Duration::from_secs(1u64 << shift).min(CAP)
    }

    async fn start_static_ip(&self, packet_plane: Arc<CorePacketPlane>) -> anyhow::Result<()> {
        let ipv4 = self.global_ctx.get_ipv4();
        let ipv6 = self.global_ctx.get_ipv6();
        if ipv4.is_none() && ipv6.is_none() {
            return Ok(());
        }

        let nic_state = self.nic.clone();
        let cancel = self.cancel.clone();
        let global_ctx = self.global_ctx.clone();
        let receiver = self.nic.receiver();
        let (output, first_round) = oneshot::channel();
        let task = tokio::spawn(async move {
            let mut output = Some(output);
            // Consecutive failed recreate attempts since the last successful
            // install. Drives the retry backoff and the escalated error report.
            let mut failed_attempts: u32 = 0;
            loop {
                if cancel.is_cancelled() {
                    Self::report_static_ip_cancelled(&mut output);
                    return;
                }
                // Release the previous NIC *before* creating the replacement.
                //
                // The previous NicCtx is installed in `nic_state` and keeps its
                // forwarding tasks alive; those tasks hold the TUN fd, so on
                // Linux the configured device name (default `tun0`) stays busy
                // and `tun::create` fails with EBUSY (os error 16) on every
                // retry — forever. The instance then keeps reporting "online"
                // while its data plane is dead. `drain()` stops the old NicCtx
                // (A10: abort + join, so the fd is really closed) and swallows
                // packets that arrive while no TUN exists, which is exactly
                // what `tun_mobile` already does before each attach.
                nic_state.drain().await;

                let closed = Arc::new(Notify::new());
                let mut nic = NicCtx::new(
                    global_ctx.clone(),
                    packet_plane.clone(),
                    receiver.clone(),
                    closed.clone(),
                );
                let result = tokio::select! {
                    biased;
                    _ = cancel.cancelled() => {
                        Self::report_static_ip_cancelled(&mut output);
                        return;
                    }
                    result = nic.run(ipv4, ipv6) => result,
                };
                if let Err(error) = result {
                    if let Some(output) = output.take() {
                        let _ = output.send(Err(error));
                        return;
                    }
                    // `run()` may have created the device and then failed on a
                    // later step (IP assignment, route updaters). Release that
                    // half-initialised NIC eagerly so the next attempt does not
                    // inherit the same name conflict.
                    nic.shutdown().await;
                    failed_attempts = failed_attempts.saturating_add(1);
                    let delay = Self::recreate_backoff(failed_attempts);
                    tracing::error!(
                        ?error,
                        failed_attempts,
                        ?delay,
                        "failed to create native interface; retrying"
                    );
                    if failed_attempts == MAX_FAST_RECREATE_ATTEMPTS {
                        // Keep retrying (a device that is only temporarily gone
                        // must not kill the whole instance), but stop reporting
                        // a "healthy" node: surface the prolonged outage with an
                        // actionable message instead of only a per-attempt log.
                        global_ctx.set_tun_device_error(format!(
                            "TUN device unavailable after {failed_attempts} consecutive \
                             recreate attempts: {error}; data plane is down, restart the \
                             instance if it does not recover"
                        ));
                    }
                    tokio::select! {
                        _ = cancel.cancelled() => return,
                        _ = tokio::time::sleep(delay) => {}
                    }
                    continue;
                }

                if failed_attempts > 0 {
                    tracing::info!(failed_attempts, "native interface recreated after failures");
                }
                failed_attempts = 0;

                let magic_dns = if let Some(ip) = ipv4 {
                    MagicDnsRuntime::start(
                        global_ctx.clone(),
                        packet_plane.clone(),
                        nic.ifname().await,
                        ip,
                    )
                } else {
                    MagicDnsRuntime::default()
                };
                nic_state.install(nic, magic_dns).await;
                if let Some(output) = output.take() {
                    let _ = output.send(Ok(()));
                }
                tokio::select! {
                    _ = cancel.cancelled() => return,
                    _ = closed.notified() => {}
                }
            }
        });
        self.static_ip_task.lock().await.replace(task);
        first_round
            .await
            .context("static IP setup task stopped")??;
        Ok(())
    }

    pub(super) async fn prepare(&self, packet_plane: Arc<CorePacketPlane>) -> anyhow::Result<()> {
        self.nic.drain().await;
        *self.packet_plane.lock().await = Some(packet_plane.clone());
        if !self.global_ctx.get_flags().no_tun {
            self.start_static_ip(packet_plane).await?;
        }
        Ok(())
    }

    pub(super) async fn shutdown(&self) {
        if let Some(task) = self.static_ip_task.lock().await.take() {
            let _ = task.await;
        }
        // Stop NicCtx (and Magic DNS) first so route updaters cannot reinstall
        // entries while cleanup_tun_leftovers is running.
        self.nic.stop().await;
        if let Some(packet_plane) = self.packet_plane.lock().await.clone() {
            super::tun_common::cleanup_tun_leftovers(&self.global_ctx, &packet_plane).await;
        }
    }

    pub(super) fn attach_fd(&self, _fd: i32) -> anyhow::Result<()> {
        anyhow::bail!("external TUN attachment is only supported on mobile Hosts")
    }

    pub(super) fn dhcp_host(
        &self,
        operation: Arc<Mutex<()>>,
        packet_plane: Arc<CorePacketPlane>,
    ) -> Arc<dyn DhcpIpv4Host> {
        Arc::new(NativeDhcpIpv4Host {
            global_ctx: self.global_ctx.clone(),
            operation,
            cancel: self.cancel.clone(),
            nic: self.nic.clone(),
            closed: Arc::new(Notify::new()),
            packet_plane,
        })
    }
}

struct NativeDhcpIpv4Host {
    global_ctx: ArcGlobalCtx,
    operation: Arc<Mutex<()>>,
    cancel: CancellationToken,
    nic: TunNicState,
    closed: Arc<Notify>,
    packet_plane: Arc<CorePacketPlane>,
}

impl NativeDhcpIpv4Host {
    fn ensure_open(&self) -> anyhow::Result<()> {
        if self.cancel.is_cancelled() {
            anyhow::bail!("instance is closing; DHCP update cancelled");
        }
        Ok(())
    }

    async fn apply(&self, next: Option<Ipv4Inet>) -> anyhow::Result<Option<Ipv4Inet>> {
        self.ensure_open()?;
        tokio::select! {
            _ = self.cancel.cancelled() => anyhow::bail!("instance is closing; DHCP update cancelled"),
            _ = self.nic.drain() => {}
        }
        self.ensure_open()?;

        let Some(ip) = next else {
            self.global_ctx.set_ipv4(None);
            return Ok(None);
        };
        if self.global_ctx.no_tun() {
            self.global_ctx.set_ipv4(Some(ip));
            return Ok(Some(ip));
        }

        let mut nic = NicCtx::new(
            self.global_ctx.clone(),
            self.packet_plane.clone(),
            self.nic.receiver(),
            self.closed.clone(),
        );
        tokio::select! {
            _ = self.cancel.cancelled() => anyhow::bail!("instance is closing; DHCP update cancelled"),
            result = nic.run(Some(ip), self.global_ctx.get_ipv6()) => result?,
        }
        let magic_dns = MagicDnsRuntime::start(
            self.global_ctx.clone(),
            self.packet_plane.clone(),
            nic.ifname().await,
            ip,
        );
        self.nic.install(nic, magic_dns).await;
        self.global_ctx.set_ipv4(Some(ip));
        Ok(Some(ip))
    }
}

#[async_trait::async_trait]
impl DhcpIpv4Host for NativeDhcpIpv4Host {
    fn take_interface_closed(&self) -> bool {
        self.closed.notified().now_or_never().is_some()
    }

    async fn apply_dhcp_ipv4(
        &self,
        _previous: Option<Ipv4Inet>,
        next: Option<Ipv4Inet>,
    ) -> DhcpIpv4ApplyOutcome {
        let permit = self.operation.clone().lock_owned().await;
        let outcome = match self.apply(next).await {
            Ok(actual) => DhcpIpv4ApplyOutcome::applied(actual),
            Err(error) => DhcpIpv4ApplyOutcome::failed(self.global_ctx.get_ipv4(), error),
        };
        outcome.with_permit(DhcpIpv4ApplyPermit::new(permit))
    }

    fn publish_dhcp_ipv4(
        &self,
        previous: Option<Ipv4Inet>,
        requested: Option<Ipv4Inet>,
        actual: Option<Ipv4Inet>,
    ) {
        let event = if requested.is_none() {
            GlobalCtxEvent::DhcpIpv4Conflicted(previous)
        } else {
            GlobalCtxEvent::DhcpIpv4Changed(previous, actual)
        };
        self.global_ctx.issue_event(event);
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    #[test]
    fn recreate_backoff_doubles_then_caps() {
        assert_eq!(
            NativeTunRuntime::recreate_backoff(1),
            Duration::from_secs(1)
        );
        assert_eq!(
            NativeTunRuntime::recreate_backoff(2),
            Duration::from_secs(2)
        );
        assert_eq!(
            NativeTunRuntime::recreate_backoff(3),
            Duration::from_secs(4)
        );
        assert_eq!(
            NativeTunRuntime::recreate_backoff(4),
            Duration::from_secs(8)
        );
        assert_eq!(
            NativeTunRuntime::recreate_backoff(5),
            Duration::from_secs(10)
        );
        assert_eq!(
            NativeTunRuntime::recreate_backoff(u32::MAX),
            Duration::from_secs(10)
        );
    }
}
