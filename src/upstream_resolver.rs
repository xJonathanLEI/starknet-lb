use std::time::Duration;

use anyhow::Result;
use tokio::{net::lookup_host, sync::mpsc::UnboundedSender};
use url::Url;

use crate::{
    cli::UpstreamSpec,
    shutdown::ShutdownHandle,
    upstream_store::{UpstreamId, UpstreamResolvedEvent, UpstreamStoreManagerEvent},
};

const RESOLVER_POLL_INTERVAL: Duration = Duration::from_secs(1);

pub struct UpstreamResolver {
    upstream_specs: Vec<UpstreamSpec>,
    resolved_upstreams: Vec<UpstreamId>,
    channel: UnboundedSender<UpstreamStoreManagerEvent>,
}

impl UpstreamResolver {
    pub fn new(
        upstream_specs: Vec<UpstreamSpec>,
        channel: UnboundedSender<UpstreamStoreManagerEvent>,
    ) -> Self {
        Self {
            upstream_specs,
            resolved_upstreams: vec![],
            channel,
        }
    }

    pub fn start(mut self) -> ShutdownHandle {
        let (shutdown_handle, finish_handle) = ShutdownHandle::new();
        let cancellation_token = shutdown_handle.cancellation_token();

        tokio::spawn(async move {
            self.run_once().await;
            loop {
                tokio::select! {
                    _ = cancellation_token.cancelled() => {
                        finish_handle.finish();
                        break;
                    },
                    _ = tokio::time::sleep(RESOLVER_POLL_INTERVAL) => {
                        self.run_once().await;
                    },
                }
            }
        });

        shutdown_handle
    }

    async fn run_once(&mut self) {
        if let Err(err) = self.run_inner().await {
            // TODO: log properly
            eprintln!("Resolver error: {}", err);
        }
    }

    async fn run_inner(&mut self) -> Result<()> {
        let mut latest_upstreams: Vec<UpstreamId> = vec![];
        for spec in self.upstream_specs.iter() {
            match spec {
                UpstreamSpec::Raw(url) => latest_upstreams.push(url.to_owned().into()),
                UpstreamSpec::Dns(dns_spec) => match lookup_host(&dns_spec.host_port).await {
                    Ok(resolution) => {
                        for name in resolution {
                            latest_upstreams.push(
                                Url::parse(&format!("http://{}{}", name, dns_spec.path))?.into(),
                            )
                        }
                    }
                    Err(_) => {
                        // This can happen in Kubernetes when the upstream headless service is still
                        // being deployed. It should be safe to ignore during deployment.
                        //
                        // Nevertheless, a warning is still printed for visibility.
                        eprintln!("Warning: cannot resolve DNS name: {}", dns_spec.host_port);
                    }
                },
            }
        }

        // The assumption here is that the upstream list is probably quite small, so using a `Vec`
        // should be faster than `HashSet` diffing. In any case the perf difference should
        // benegligible.
        let mut addition = vec![];
        let mut removal = vec![];
        let mut updated_list = vec![];
        while let Some(existing_upstream) = self.resolved_upstreams.pop() {
            if !latest_upstreams
                .iter()
                .any(|latest| existing_upstream.endpoint == latest.endpoint)
            {
                removal.push(existing_upstream);
            } else {
                updated_list.push(existing_upstream);
            }
        }
        while let Some(latest_upstream) = latest_upstreams.pop() {
            if !updated_list
                .iter()
                .any(|existing| existing == &latest_upstream)
            {
                addition.push(latest_upstream.clone());
                updated_list.push(latest_upstream);
            }
        }

        if !addition.is_empty() || !removal.is_empty() {
            let _ = self
                .channel
                .send(UpstreamStoreManagerEvent::UpstreamResolved(
                    UpstreamResolvedEvent { addition, removal },
                ));
        }

        self.resolved_upstreams = updated_list;

        Ok(())
    }
}
