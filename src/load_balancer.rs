use anyhow::Result;
use reqwest::Client;

use crate::upstream_store::UpstreamStore;

pub struct LoadBalancer {
    store: UpstreamStore,
    http_client: Client,
}

impl LoadBalancer {
    pub fn new(store: UpstreamStore) -> Self {
        Self {
            store,
            http_client: Client::new(),
        }
    }

    pub async fn route(&self, request: String) -> Result<String> {
        match self.store.get_upstream() {
            Some(endpoint) => Ok(self
                .http_client
                .post(endpoint)
                .header("Content-Type", "application/json")
                .body(request)
                .send()
                .await?
                .text()
                .await?),
            None => Err(anyhow::anyhow!("no available upstream")),
        }
    }
}
