use std::{sync::Arc, time::Duration};

use anyhow::Result;
use arc_swap::ArcSwap;
use rand::{thread_rng, Rng};
use starknet::{
    core::types::{BlockId, BlockTag, MaybePendingBlockWithTxHashes},
    providers::{jsonrpc::HttpTransport, JsonRpcClient, Provider},
};
use tokio::{
    net::lookup_host,
    sync::mpsc::{UnboundedReceiver, UnboundedSender},
};
use tokio_util::sync::CancellationToken;
use url::Url;

use crate::{
    cli::UpstreamSpec,
    head::{ChainHead, ConfirmedHead, PendingHead},
    shutdown::{FinishSignal, ShutdownHandle},
};

pub struct UpstreamStoreManager {
    upstreams: Vec<UpstreamSpec>,
}

pub struct UpstreamStore {
    upstreams: Vec<Upstream>,
}

struct Upstream {
    endpoint: Url,
    head: Arc<ArcSwap<Option<ChainHead>>>,
}

// TODO: support block stream subscription via WebSocket instead of polling
struct UpstreamTracker {
    head: Arc<ArcSwap<Option<ChainHead>>>,
    client: JsonRpcClient<HttpTransport>,
    channel: UnboundedSender<NewHeadMessage>,
}

#[derive(Debug, Clone, Copy)]
struct NewHeadMessage;

// TODO: make this configurable for each upstream group.
const TRACKER_POLL_INTERVAL: Duration = Duration::from_secs(1);
const TRACKER_POLL_TIMEOUT: Duration = Duration::from_secs(2);

impl UpstreamStoreManager {
    pub fn new(upstream_specs: Vec<UpstreamSpec>) -> Self {
        Self {
            upstreams: upstream_specs,
        }
    }

    pub async fn start(self) -> Result<(UpstreamStore, ShutdownHandle)> {
        let (shutdown_handle, finish_sender) = ShutdownHandle::new();
        let cancellation_token = shutdown_handle.cancellation_token();

        let (head_sender, head_receiver) = tokio::sync::mpsc::unbounded_channel();

        // TODO: dynamically monitor DNS resolution instead of only checking at startup
        let mut resolved_upstreams: Vec<Upstream> = vec![];
        for spec in self.upstreams {
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

        let task_handles = resolved_upstreams
            .iter()
            .map(|upstream| {
                let (task_shutdown_handle, task_finish_handle) = ShutdownHandle::new();
                let task_cancellation_token = task_shutdown_handle.cancellation_token();

                let tracker = UpstreamTracker::new(
                    upstream.head.clone(),
                    upstream.endpoint.clone(),
                    head_sender.clone(),
                );

                tokio::spawn(async move {
                    tracker.run_once().await;
                    loop {
                        tokio::select! {
                            _ = task_cancellation_token.cancelled() => {
                                task_finish_handle.finish();
                                break;
                            },
                            _ = tokio::time::sleep(TRACKER_POLL_INTERVAL) => {
                                tracker.run_once().await;
                            },
                        }
                    }
                });

                task_shutdown_handle
            })
            .collect::<Vec<_>>();

        // TODO: implement a way to detect readiness, as currently the app starts up with no
        //       available upstreams and will fail first few requests.
        tokio::spawn(Self::run(
            head_receiver,
            task_handles,
            finish_sender,
            cancellation_token,
        ));

        Ok((
            UpstreamStore {
                upstreams: resolved_upstreams,
            },
            shutdown_handle,
        ))
    }

    async fn run(
        mut head_receiver: UnboundedReceiver<NewHeadMessage>,
        task_handles: Vec<ShutdownHandle>,
        finish_sender: FinishSignal,
        cancellation_token: CancellationToken,
    ) {
        loop {
            tokio::select! {
                _ = cancellation_token.cancelled() => {
                    Self::shutdown(task_handles, finish_sender).await;
                    break;
                }
                // TODO: use this as a tick to build cache.
                Some(_) = head_receiver.recv() => {},
            }
        }
    }

    async fn shutdown(task_handles: Vec<ShutdownHandle>, finish_sender: FinishSignal) {
        // Wait for all background tasks to finish
        futures_util::future::join_all(task_handles.into_iter().map(|handle| handle.shutdown()))
            .await;
        finish_sender.finish();
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

impl UpstreamTracker {
    fn new(
        head: Arc<ArcSwap<Option<ChainHead>>>,
        rpc_url: Url,
        channel: UnboundedSender<NewHeadMessage>,
    ) -> Self {
        Self {
            head,
            client: JsonRpcClient::new(HttpTransport::new_with_client(
                rpc_url,
                reqwest::ClientBuilder::new()
                    .timeout(TRACKER_POLL_TIMEOUT)
                    .build()
                    .unwrap(),
            )),
            channel,
        }
    }

    async fn run_once(&self) {
        if let Err(err) = self.run_inner().await {
            // TODO: log properly
            eprintln!("Tracker error: {}", err);
        }
    }

    async fn run_inner(&self) -> Result<()> {
        let new_head = match self
            .client
            .get_block_with_tx_hashes(BlockId::Tag(BlockTag::Pending))
            .await?
        {
            MaybePendingBlockWithTxHashes::Block(block_with_tx_hashes) => {
                ChainHead::Confirmed(ConfirmedHead {
                    height: block_with_tx_hashes.block_number,
                })
            }
            MaybePendingBlockWithTxHashes::PendingBlock(pending_block_with_tx_hashes) => {
                // TODO: maintain headers internally to be able to _actually_ check longest chain,
                //       as this pending block could be on an abandoned fork.
                let pending_height = match self
                    .client
                    .get_block_with_tx_hashes(BlockId::Hash(
                        pending_block_with_tx_hashes.parent_hash,
                    ))
                    .await?
                {
                    MaybePendingBlockWithTxHashes::Block(block_with_tx_hashes) => {
                        block_with_tx_hashes.block_number + 1
                    }
                    MaybePendingBlockWithTxHashes::PendingBlock(_) => {
                        anyhow::bail!("unexpected pending block")
                    }
                };

                ChainHead::Pending(PendingHead {
                    height: pending_height,
                    tx_count: pending_block_with_tx_hashes.transactions.len(),
                })
            }
        };

        self.head.store(Arc::new(Some(new_head)));
        let _ = self.channel.send(NewHeadMessage);

        Ok(())
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
