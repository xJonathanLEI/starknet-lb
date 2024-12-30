use std::{collections::HashMap, iter::once, sync::Arc};

use anyhow::Result;
use arc_swap::ArcSwap;
use rand::{thread_rng, Rng};
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};
use url::Url;

use crate::{
    cli::UpstreamSpec,
    head::ChainHead,
    shutdown::{FinishSignal, ShutdownHandle},
    upstream_resolver::UpstreamResolver,
    upstream_tracker::UpstreamTracker,
};

pub struct UpstreamStoreManager {
    event_sender: UnboundedSender<UpstreamStoreManagerEvent>,
    event_receiver: UnboundedReceiver<UpstreamStoreManagerEvent>,
    store: UpstreamStore,
    tracker_handles: HashMap<UpstreamId, ShutdownHandle>,
}

#[derive(Clone)]
pub struct UpstreamStore {
    upstreams: Arc<ArcSwap<Vec<Upstream>>>,
}

pub enum UpstreamStoreManagerEvent {
    NewHead,
    UpstreamResolved(UpstreamResolvedEvent),
}

pub struct UpstreamResolvedEvent {
    pub addition: Vec<UpstreamId>,
    pub removal: Vec<UpstreamId>,
}

#[derive(Debug, Clone)]
pub struct Upstream {
    pub id: UpstreamId,
    pub head: Arc<ArcSwap<Option<ChainHead>>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct UpstreamId {
    pub endpoint: Url,
}

impl UpstreamStoreManager {
    pub fn new() -> Self {
        let (head_sender, head_receiver) = tokio::sync::mpsc::unbounded_channel();

        Self {
            event_sender: head_sender,
            event_receiver: head_receiver,
            store: UpstreamStore {
                upstreams: Arc::new(ArcSwap::from_pointee(vec![])),
            },
            tracker_handles: HashMap::new(),
        }
    }

    pub fn start(
        mut self,
        upstream_specs: Vec<UpstreamSpec>,
    ) -> Result<(UpstreamStore, ShutdownHandle)> {
        let store = self.store.clone();

        let resolver = UpstreamResolver::new(upstream_specs, self.event_sender.clone());
        let resolver_handle = resolver.start();

        let (shutdown_handle, finish_handle) = ShutdownHandle::new();
        let cancellation_token = shutdown_handle.cancellation_token();

        // TODO: implement a way to detect readiness, as currently the app starts up with no
        //       available upstreams and will fail first few requests.
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = cancellation_token.cancelled() => {
                        self.shutdown(resolver_handle, finish_handle).await;
                        break;
                    }
                    Some(event) = self.event_receiver.recv() => {
                        self.handle_event(event).await;
                    },
                }
            }
        });

        Ok((store, shutdown_handle))
    }

    async fn handle_event(&mut self, event: UpstreamStoreManagerEvent) {
        match event {
            UpstreamStoreManagerEvent::NewHead => {
                // TODO: use this as a tick to build cache.
            }
            UpstreamStoreManagerEvent::UpstreamResolved(event) => {
                let mut updated_upstreams = vec![];

                for upstream_id in event.addition {
                    println!("Adding a new upstream: {}", upstream_id.endpoint);

                    let upstream_head = Arc::new(ArcSwap::from_pointee(None));
                    let endpoint = upstream_id.endpoint.clone();

                    updated_upstreams.push(Upstream {
                        id: upstream_id.clone(),
                        head: upstream_head.clone(),
                    });
                    self.tracker_handles.insert(
                        upstream_id,
                        UpstreamTracker::new(upstream_head, endpoint, self.event_sender.clone())
                            .start(),
                    );
                }

                for existing_upstream in self.store.upstreams.load().iter() {
                    if !event
                        .removal
                        .iter()
                        .any(|to_remove| &existing_upstream.id == to_remove)
                    {
                        updated_upstreams.push(existing_upstream.to_owned());
                    }
                }

                self.store.upstreams.swap(Arc::new(updated_upstreams));

                for upstream_id in event.removal {
                    println!("Removing upstream: {}", upstream_id.endpoint);

                    // We don't wait for trackers to finish shutting down, so it might be the case
                    // that when `UpstreamStoreManager::shutdown()` resolves, some removed trackers
                    // would still be running.
                    //
                    // If this matters in the future, add a new event type that reports tracker
                    // shutdown progress so it can be tracked here in `UpstreamStoreManager`.
                    match self.tracker_handles.remove(&upstream_id) {
                        Some(handle) => {
                            tokio::spawn(handle.shutdown());
                        }
                        None => {
                            // This should never happen and indicates a bug. However, it's probably
                            // not worth crashing the entire load balancer just for this.
                            eprintln!("Warning: tracker task handle not found");
                        }
                    }
                }
            }
        }
    }

    async fn shutdown(self, resolver_handler: ShutdownHandle, finish_handle: FinishSignal) {
        // Wait for all background tasks to finish
        futures_util::future::join_all(
            self.tracker_handles
                .into_values()
                .map(|handle| handle.shutdown())
                .chain(once(resolver_handler.shutdown())),
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

        let upstreams = self.upstreams.load();

        let available = if let Some(max_head) = upstreams
            .iter()
            .filter_map(|upstream| *upstream.head.load_full())
            .max()
        {
            upstreams
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
            Some(chosen.id.endpoint.clone())
        }
    }
}

impl From<Url> for UpstreamId {
    fn from(value: Url) -> Self {
        Self { endpoint: value }
    }
}
