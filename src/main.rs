use std::{sync::Arc, time::Duration};

use anyhow::Result;
use axum::{extract::State, response::IntoResponse, routing::post, Router};
use clap::Parser;
use url::Url;

mod head;

mod load_balancer;
use load_balancer::LoadBalancer;

mod upstream_store;
use upstream_store::UpstreamStore;

mod shutdown;

/// 10 seconds.
const GRACEFUL_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Debug, Parser)]
struct Cli {
    /// Port to listen on.
    #[clap(long, default_value = "9546")]
    port: u16,
    /// Upstream JSON-RPC endpoints.
    #[clap(long = "upstream")]
    upstreams: Vec<Url>,
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    let store = UpstreamStore::new(cli.upstreams);
    let (load_balancer, store_shutdown) = store.start();

    let axum_app = Router::new()
        .route("/", post(proxy))
        .with_state(Arc::new(load_balancer));
    let axum_listener = tokio::net::TcpListener::bind(("0.0.0.0", cli.port)).await?;

    let mut sigterm_handle =
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
    let ctrl_c_handle = tokio::signal::ctrl_c();

    println!("Listening on 0.0.0.0:{}", cli.port);

    // TODO: axum graceful shutdown
    tokio::select! {
        _ = sigterm_handle.recv() => {},
        _ = ctrl_c_handle => {},
        _ = axum::serve(axum_listener, axum_app) => {},
    }

    tokio::select! {
        _ = tokio::time::sleep(GRACEFUL_SHUTDOWN_TIMEOUT) => {
            anyhow::bail!("timeout waiting for graceful shutdown");
        },
        _ = store_shutdown.shutdown() => {},
    }

    Ok(())
}

async fn proxy(State(lb): State<Arc<LoadBalancer>>, raw_body: String) -> impl IntoResponse {
    // TODO: make proxy semantic-aware instead of blindly sending raw text
    // TODO: proper error handling
    match lb
        .get_client()
        .expect("no available upstream")
        .send(raw_body)
        .await
    {
        Ok(value) => (
            [(axum::http::header::CONTENT_TYPE, "application/json")],
            value,
        ),
        Err(err) => (
            [(axum::http::header::CONTENT_TYPE, "text/plain")],
            err.to_string(),
        ),
    }
}
