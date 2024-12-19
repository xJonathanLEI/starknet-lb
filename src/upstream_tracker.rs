use std::{sync::Arc, time::Duration};

use anyhow::Result;
use arc_swap::ArcSwap;
use starknet::{
    core::types::{BlockId, BlockTag, MaybePendingBlockWithTxHashes},
    providers::{jsonrpc::HttpTransport, JsonRpcClient, Provider},
};
use tokio::sync::mpsc::UnboundedSender;
use url::Url;

use crate::{
    head::{ChainHead, ConfirmedHead, PendingHead},
    shutdown::ShutdownHandle,
    upstream_store::UpstreamStoreManagerEvent,
};

// TODO: make this configurable for each upstream group.
const TRACKER_POLL_INTERVAL: Duration = Duration::from_secs(1);
const TRACKER_POLL_TIMEOUT: Duration = Duration::from_secs(2);

// TODO: support block stream subscription via WebSocket instead of polling
pub struct UpstreamTracker {
    head: Arc<ArcSwap<Option<ChainHead>>>,
    client: JsonRpcClient<HttpTransport>,
    channel: UnboundedSender<UpstreamStoreManagerEvent>,
}

impl UpstreamTracker {
    pub fn new(
        head: Arc<ArcSwap<Option<ChainHead>>>,
        rpc_url: Url,
        channel: UnboundedSender<UpstreamStoreManagerEvent>,
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

    pub fn start(self) -> ShutdownHandle {
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
                    _ = tokio::time::sleep(TRACKER_POLL_INTERVAL) => {
                        self.run_once().await;
                    },
                }
            }
        });

        shutdown_handle
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
        let _ = self.channel.send(UpstreamStoreManagerEvent::NewHead);

        Ok(())
    }
}
