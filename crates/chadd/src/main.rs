use std::{path::PathBuf, sync::Arc};

use anyhow::Context;
use tracing_subscriber::EnvFilter;

mod api;
mod config;
mod routes;

use config::Config;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    let path: PathBuf = std::env::args_os()
        .nth(1)
        .or_else(|| std::env::var_os("CHADD_CONFIG"))
        .unwrap_or_else(|| "chadd.toml".into())
        .into();
    let config = Config::load(&path)?;
    let listen = config.listen;
    let app = routes::router(Arc::new(routes::AppState::new(config)?));

    let listener = tokio::net::TcpListener::bind(listen)
        .await
        .with_context(|| format!("binding {listen}"))?;
    tracing::info!(%listen, "chadd listening");
    axum::serve(listener, app)
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
        })
        .await?;
    Ok(())
}
