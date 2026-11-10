use std::{path::PathBuf, sync::Arc};

use anyhow::Context;
use tracing_subscriber::EnvFilter;

mod api;
mod cache;
mod config;
mod delete;
mod emoji;
mod fetch;
mod files;
mod games;
mod gifs;
mod routes;
mod sounds;

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
    let state = Arc::new(routes::AppState::new(config)?);
    if state.config.discord_games {
        tokio::spawn(games::refresh(state.clone()));
    }
    tokio::spawn(delete::worker(state.clone()));
    let app = routes::router(state);

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

/// Locks `m`, recovering from poisoning: one panicked request mustn't
/// take every later one down with it.
fn lock<T>(m: &std::sync::Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}
