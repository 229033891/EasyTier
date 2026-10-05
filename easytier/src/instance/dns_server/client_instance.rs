use std::{sync::Arc, time::Duration};

use easytier_core::gateway::magic_dns::{
    MagicDnsRoutePublisher, MagicDnsRouteSnapshot, run_magic_dns_route_publisher,
};
use easytier_core::instance::CorePacketPlane;
use tokio::task::JoinSet;

use crate::{
    common::global_ctx::ArcGlobalCtx,
    proto::{
        api::instance::Route,
        api::manage::DnsHostEntry,
        common::Void,
        magic_dns::{
            HandshakeRequest, MagicDnsServerRpc, MagicDnsServerRpcClientFactory, StaticDnsHost,
            UpdateDnsRecordRequest,
        },
        rpc::standalone::{RuntimeRpcClient, runtime_rpc_client},
        rpc_types::controller::BaseController,
    },
};

use super::{MAGIC_DNS_INSTANCE_ADDR, MAGIC_DNS_STATIC_HOSTS_CLIENT};

pub struct MagicDnsClientInstance {
    rpc_client: RuntimeRpcClient,
    rpc_stub: Option<Box<dyn MagicDnsServerRpc<Controller = BaseController> + Send>>,
    route_source: Arc<CorePacketPlane>,
    global_ctx: ArcGlobalCtx,
    tasks: JoinSet<()>,
}

struct RpcMagicDnsRoutePublisher {
    rpc_stub: Box<dyn MagicDnsServerRpc<Controller = BaseController> + Send>,
    global_ctx: ArcGlobalCtx,
    last_static_hosts: Option<Vec<DnsHostEntry>>,
}

fn current_static_hosts(global_ctx: &ArcGlobalCtx) -> Vec<DnsHostEntry> {
    global_ctx
        .config
        .get_dns_config()
        .map(|dns| dns.hosts)
        .unwrap_or_default()
}

#[async_trait::async_trait]
impl MagicDnsRoutePublisher for RpcMagicDnsRoutePublisher {
    async fn handshake(&mut self) -> anyhow::Result<()> {
        self.rpc_stub
            .handshake(BaseController::default(), HandshakeRequest::default())
            .await?;
        Ok(())
    }

    async fn heartbeat(&mut self) -> anyhow::Result<()> {
        self.rpc_stub
            .heartbeat(BaseController::default(), Void::default())
            .await?;
        // Hosts-only config edits do not bump route revision; push when hosts change.
        let hosts = current_static_hosts(&self.global_ctx);
        if self.last_static_hosts.as_ref() != Some(&hosts) {
            let request = UpdateDnsRecordRequest {
                routes: vec![],
                zone: String::new(),
                client: Some(MAGIC_DNS_STATIC_HOSTS_CLIENT.to_string()),
                static_hosts: hosts
                    .iter()
                    .map(|h| StaticDnsHost {
                        name: h.name.clone(),
                        ips: h.ips.clone(),
                        ttl_secs: h.ttl_secs,
                    })
                    .collect(),
            };
            self.rpc_stub
                .update_dns_record(BaseController::default(), request)
                .await?;
            self.last_static_hosts = Some(hosts);
        }
        Ok(())
    }

    async fn publish(&mut self, snapshot: &MagicDnsRouteSnapshot) -> anyhow::Result<()> {
        let hosts = current_static_hosts(&self.global_ctx);
        let request = UpdateDnsRecordRequest {
            routes: snapshot
                .routes
                .iter()
                .map(|route| Route {
                    hostname: route.hostname.clone(),
                    ipv4_addr: route.ipv4_addr,
                    ..Default::default()
                })
                .collect(),
            zone: snapshot.zone.clone(),
            // Always advertise the static-hosts channel so empty hosts clears prior entries.
            client: Some(MAGIC_DNS_STATIC_HOSTS_CLIENT.to_string()),
            static_hosts: hosts
                .iter()
                .map(|h| StaticDnsHost {
                    name: h.name.clone(),
                    ips: h.ips.clone(),
                    ttl_secs: h.ttl_secs,
                })
                .collect(),
        };
        tracing::debug!(
            "MagicDnsClientInstance::update_dns_task: update dns records: {:?}",
            request
        );
        self.rpc_stub
            .update_dns_record(BaseController::default(), request)
            .await?;
        self.last_static_hosts = Some(hosts);
        Ok(())
    }
}

impl MagicDnsClientInstance {
    pub(crate) async fn new(
        route_source: Arc<CorePacketPlane>,
        global_ctx: ArcGlobalCtx,
    ) -> Result<Self, anyhow::Error> {
        let mut rpc_client = runtime_rpc_client(MAGIC_DNS_INSTANCE_ADDR.parse().unwrap());
        let rpc_stub = rpc_client
            .scoped_client::<MagicDnsServerRpcClientFactory<BaseController>>("".to_string())
            .await?;
        Ok(MagicDnsClientInstance {
            rpc_client,
            rpc_stub: Some(rpc_stub),
            route_source,
            global_ctx,
            tasks: JoinSet::new(),
        })
    }

    async fn update_dns_task(
        route_source: Arc<CorePacketPlane>,
        global_ctx: ArcGlobalCtx,
        rpc_stub: Box<dyn MagicDnsServerRpc<Controller = BaseController> + Send>,
    ) -> Result<(), anyhow::Error> {
        let mut publisher = RpcMagicDnsRoutePublisher {
            rpc_stub,
            global_ctx,
            last_static_hosts: None,
        };
        run_magic_dns_route_publisher(
            route_source.as_ref(),
            &mut publisher,
            Duration::from_millis(500),
        )
        .await
    }

    pub async fn run_and_wait(&mut self) {
        let rpc_stub = self.rpc_stub.take().unwrap();
        let route_source = self.route_source.clone();
        let global_ctx = self.global_ctx.clone();
        self.tasks.spawn(async move {
            let ret = Self::update_dns_task(route_source, global_ctx, rpc_stub).await;
            if let Err(e) = ret {
                tracing::error!("MagicDnsServerInstanceData::run_and_wait: {:?}", e);
            }
        });

        tokio::select! {
            _ = self.tasks.join_next() => {
                tracing::warn!("MagicDnsServerInstanceData::run_and_wait: dns record update task exited");
            }
            _ = self.rpc_client.wait() => {
                tracing::warn!("MagicDnsServerInstanceData::run_and_wait: rpc client exited");
            }
        }
    }
}
