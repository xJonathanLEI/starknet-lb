use std::{sync::Arc, time::Duration};

use anyhow::Result;
use arc_swap::ArcSwap;
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
    load_balancer::LoadBalancer,
    shutdown::{FinishSignal, ShutdownHandle},
};

pub struct UpstreamStore {
    upstreams: Vec<UpstreamSpec>,
}

// TODO: support block stream subscription via WebSocket instead of polling
struct UpstreamTracker {
    index: usize,
    client: JsonRpcClient<HttpTransport>,
    channel: UnboundedSender<NewHeadMessage>,
}

#[derive(Debug, Clone, Copy)]
struct NewHeadMessage {
    index: usize,
    head: ChainHead,
}

// TODO: make this configurable for each upstream group.
const TRACKER_POLL_INTERVAL: Duration = Duration::from_secs(1);
const TRACKER_POLL_TIMEOUT: Duration = Duration::from_secs(2);

impl UpstreamStore {
    pub fn new(upstream_specs: Vec<UpstreamSpec>) -> Self {
        Self {
            upstreams: upstream_specs,
        }
    }

    pub async fn start(self) -> Result<(LoadBalancer, ShutdownHandle)> {
        let (shutdown_handle, finish_sender) = ShutdownHandle::new();
        let cancellation_token = shutdown_handle.cancellation_token();

        let (head_sender, head_receiver) = tokio::sync::mpsc::unbounded_channel();

        // TODO: dynamically monitor DNS resolution instead of only checking at startup
        let mut resolved_upstreams = vec![];
        for spec in self.upstreams {
            match spec {
                UpstreamSpec::Raw(url) => resolved_upstreams.push(url),
                UpstreamSpec::Dns(dns_spec) => {
                    for name in lookup_host(dns_spec.host_port).await? {
                        resolved_upstreams
                            .push(Url::parse(&format!("http://{}{}", name, dns_spec.path))?)
                    }
                }
            }
        }

        println!("{} upstreams resolved:", resolved_upstreams.len());
        for upstream in resolved_upstreams.iter() {
            println!("- {}", upstream);
        }

        let task_handles = resolved_upstreams
            .iter()
            .enumerate()
            .map(|(ind, upstream)| {
                let (task_shutdown_handle, task_finish_handle) = ShutdownHandle::new();
                let task_cancellation_token = task_shutdown_handle.cancellation_token();

                let tracker = UpstreamTracker::new(ind, upstream.clone(), head_sender.clone());

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

        let available_upstreams = Arc::new(ArcSwap::from_pointee(Vec::<usize>::new()));

        // TODO: implement a way to detect readiness, as currently the app starts up with no
        //       available upstreams and will fail first few requests.
        tokio::spawn(Self::run(
            resolved_upstreams.len(),
            head_receiver,
            available_upstreams.clone(),
            task_handles,
            finish_sender,
            cancellation_token,
        ));

        Ok((
            LoadBalancer::new(resolved_upstreams, available_upstreams),
            shutdown_handle,
        ))
    }

    async fn run(
        upstream_count: usize,
        mut head_receiver: UnboundedReceiver<NewHeadMessage>,
        available_upstreams: Arc<ArcSwap<Vec<usize>>>,
        task_handles: Vec<ShutdownHandle>,
        finish_sender: FinishSignal,
        cancellation_token: CancellationToken,
    ) {
        let mut heads: Vec<Option<ChainHead>> = Vec::with_capacity(upstream_count);
        heads.resize(upstream_count, None);

        loop {
            tokio::select! {
                _ = cancellation_token.cancelled() => {
                    Self::shutdown(task_handles, finish_sender).await;
                    break;
                }
                Some(new_head) = head_receiver.recv() => {
                    heads[new_head.index] = Some(new_head.head);

                    // Only allow the absolute most up-to-date upstream
                    // TODO: implement policy for staleness tolerance (maybe even method-based)
                    let available = if let Some(max_head) = heads.iter().flatten().max() {
                        heads
                            .iter()
                            .enumerate()
                            .filter_map(|(ind, head)| match head {
                                Some(head) => {
                                    if head == max_head {
                                        Some(ind)
                                    } else {
                                        None
                                    }
                                }
                                None => None,
                            })
                            .collect::<Vec<_>>()
                    } else {
                        vec![]
                    };

                    available_upstreams.store(Arc::new(available));
                },
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

impl UpstreamTracker {
    fn new(index: usize, rpc_url: Url, channel: UnboundedSender<NewHeadMessage>) -> Self {
        Self {
            index,
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
        let head = match self
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

        let _ = self.channel.send(NewHeadMessage {
            index: self.index,
            head,
        });

        Ok(())
    }
}
