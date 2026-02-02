//! proj-daemon - Background daemon for project management

mod ipc;
mod process;
mod proxy;
mod registry;

use anyhow::{Context, Result};
use proj_common::{pid_file_path, proj_dir, socket_path};
use std::sync::Arc;
use tokio::sync::Mutex;
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> Result<()> {
    // Initialize logging
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    tracing::info!("Starting proj-daemon");

    // Ensure proj directory exists
    let proj_path = proj_dir()?;
    tokio::fs::create_dir_all(&proj_path)
        .await
        .context("Failed to create proj directory")?;

    // Write PID file
    let pid = std::process::id();
    let pid_path = pid_file_path()?;
    tokio::fs::write(&pid_path, pid.to_string())
        .await
        .context("Failed to write PID file")?;

    tracing::info!("Daemon PID: {} (written to {:?})", pid, pid_path);

    // Create routing table for proxy
    let routing_table = proxy::new_routing_table();

    // Create shared daemon state
    let state = Arc::new(Mutex::new(
        ipc::DaemonState::new(routing_table.clone()).await?,
    ));

    // Take the event receiver from process manager
    let event_rx = {
        let mut s = state.lock().await;
        s.process_manager.take_event_receiver()
    };

    // Start event handler
    if let Some(rx) = event_rx {
        let state_clone = state.clone();
        tokio::spawn(async move {
            ipc::process_event_handler(state_clone, rx).await;
        });
    }

    // Get socket path
    let socket = socket_path()?;

    // Start IPC server and proxy in parallel
    let ipc_state = state.clone();
    let ipc_handle = tokio::spawn(async move {
        if let Err(e) = ipc::start_ipc_server(&socket, ipc_state).await {
            tracing::error!("IPC server error: {}", e);
        }
    });

    // Default proxy port
    let proxy_port = 8080;
    let proxy_handle = tokio::spawn(async move {
        if let Err(e) = proxy::start_proxy(proxy_port, routing_table).await {
            tracing::error!("Proxy error: {}", e);
        }
    });

    tracing::info!("Daemon ready");
    tracing::info!("  IPC socket: {:?}", socket_path()?);
    tracing::info!("  Proxy: http://127.0.0.1:{}", proxy_port);

    // Wait for shutdown signal
    tokio::select! {
        _ = tokio::signal::ctrl_c() => {
            tracing::info!("Received Ctrl+C, shutting down");
        }
        _ = ipc_handle => {
            tracing::error!("IPC server exited unexpectedly");
        }
        _ = proxy_handle => {
            tracing::error!("Proxy server exited unexpectedly");
        }
    }

    // Cleanup
    let pid_path = pid_file_path()?;
    if pid_path.exists() {
        let _ = tokio::fs::remove_file(&pid_path).await;
    }

    let socket = socket_path()?;
    if socket.exists() {
        let _ = tokio::fs::remove_file(&socket).await;
    }

    tracing::info!("Daemon stopped");
    Ok(())
}
