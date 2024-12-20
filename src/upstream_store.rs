use std::sync::Arc;

use anyhow::Result;
use arc_swap::ArcSwap;
use rand::{thread_rng, Rng};
use tokio::{
    net::lookup_host,
    sync::mpsc::{UnboundedReceiver, UnboundedSender},
};
use url::Url;

use crate::{
    cli::UpstreamSpec,
    head::ChainHead,
    shutdown::{FinishSignal, ShutdownHandle},
    upstream_tracker::UpstreamTracker,
};

pub struct UpstreamStoreManager {
    head_sender: UnboundedSender<UpstreamStoreManagerEvent>,
    head_receiver: UnboundedReceiver<UpstreamStoreManagerEvent>,
    task_handles: Vec<ShutdownHandle>,
}

pub struct UpstreamStore {
    upstreams: Vec<Upstream>,
}

pub enum UpstreamStoreManagerEvent {
    NewHead,
}

struct Upstream {
    endpoint: Url,
    head: Arc<ArcSwap<Option<ChainHead>>>,
}

impl UpstreamStoreManager {
    pub fn new() -> Self {
        let (head_sender, head_receiver) = tokio::sync::mpsc::unbounded_channel();

        Self {
            head_sender,
            head_receiver,
            task_handles: vec![],
        }
    }

    pub async fn start(
        mut self,
        upstream_specs: Vec<UpstreamSpec>,
    ) -> Result<(UpstreamStore, ShutdownHandle)> {
        // TODO: dynamically monitor DNS resolution instead of only checking at startup
        let mut resolved_upstreams: Vec<Upstream> = vec![];
        for spec in upstream_specs {
            match spec {
                UpstreamSpec::Raw(url) => resolved_upstreams.push(url.into()),
                UpstreamSpec::Dns(dns_spec) => {
                    for name in lookup_host(dns_spec.host_port).await? {
                        resolved_upstreams
                            .push(Url::parse(&format!("http://{}{}", name, dns_spec.path))?.into())
                    }
                }
            }
        }

        println!("{} upstreams resolved:", resolved_upstreams.len());
        for upstream in resolved_upstreams.iter() {
            println!("- {}", upstream.endpoint);
        }

        self.task_handles
            .extend(resolved_upstreams.iter().map(|upstream| {
                UpstreamTracker::new(
                    upstream.head.clone(),
                    upstream.endpoint.clone(),
                    self.head_sender.clone(),
                )
                .start()
            }));

        let (shutdown_handle, finish_handle) = ShutdownHandle::new();
        let cancellation_token = shutdown_handle.cancellation_token();

        // TODO: implement a way to detect readiness, as currently the app starts up with no
        //       available upstreams and will fail first few requests.
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = cancellation_token.cancelled() => {
                        self.shutdown(finish_handle).await;
                        break;
                    }
                    Some(event) = self.head_receiver.recv() => {
                        self.handle_event(event).await;
                    },
                }
            }
        });

        Ok((
            UpstreamStore {
                upstreams: resolved_upstreams,
            },
            shutdown_handle,
        ))
    }

    async fn handle_event(&self, _event: UpstreamStoreManagerEvent) {
        // TODO: use this as a tick to build cache.
    }

    async fn shutdown(self, finish_handle: FinishSignal) {
        // Wait for all background tasks to finish
        futures_util::future::join_all(
            self.task_handles
                .into_iter()
                .map(|handle| handle.shutdown()),
        )
        .await;
        finish_handle.finish();
    }
}

impl UpstreamStore {
    pub fn get_upstream(&self) -> Option<Url> {
        // TODO: make request context available to be able to route non-real-time-sensitive requests
        //       to lagging upstreams.
        // TODO: use the manager event loop (currently useless) to build cache for common request
        //       targets to avoid having to loop through all upstreams on every single request.

        let available = if let Some(max_head) = self
            .upstreams
            .iter()
            .filter_map(|upstream| *upstream.head.load_full())
            .max()
        {
            self.upstreams
                .iter()
                .filter(|upstream| {
                    upstream
                        .head
                        .load_full()
                        .is_some_and(|head| head >= max_head)
                })
                .collect::<Vec<_>>()
        } else {
            vec![]
        };

        if available.is_empty() {
            None
        } else {
            let mut rng = thread_rng();
            let chosen = &available[rng.gen_range(0..available.len())];
            Some(chosen.endpoint.clone())
        }
    }
}

impl From<Url> for Upstream {
    fn from(value: Url) -> Self {
        Self {
            endpoint: value,
            head: Arc::new(ArcSwap::from_pointee(None)),
        }
    }
}
