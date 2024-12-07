use std::sync::Arc;

use anyhow::Result;
use arc_swap::ArcSwap;
use rand::{thread_rng, Rng};
use reqwest::Client;
use url::Url;

pub struct LoadBalancer {
    clients: Vec<HttpClient>,
    available_upstreams: Arc<ArcSwap<Vec<usize>>>,
}

pub struct HttpClient {
    url: Url,
    client: Client,
}

impl LoadBalancer {
    pub fn new(upstreams: Vec<Url>, available_upstreams: Arc<ArcSwap<Vec<usize>>>) -> Self {
        Self {
            clients: upstreams.into_iter().map(HttpClient::new).collect(),
            available_upstreams,
        }
    }

    pub fn get_client(&self) -> Result<&HttpClient> {
        let mut rng = thread_rng();

        let snapshot = self.available_upstreams.load();
        if snapshot.is_empty() {
            anyhow::bail!("no available upstream");
        }

        Ok(&self.clients[snapshot[rng.gen_range(0..snapshot.len())]])
    }
}

impl HttpClient {
    fn new(url: Url) -> Self {
        Self {
            url,
            client: Client::new(),
        }
    }

    pub async fn send(&self, request: String) -> Result<String> {
        Ok(self
            .client
            .post(self.url.clone())
            .header("Content-Type", "application/json")
            .body(request)
            .send()
            .await?
            .text()
            .await?)
    }
}
