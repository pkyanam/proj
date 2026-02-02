//! Unix socket IPC server for CLI communication

use anyhow::{Context, Result};
use proj_common::{IpcRequest, IpcResponse, ProcessStatus};
use std::path::Path;
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::Mutex;

use crate::process::ProcessManager;
use crate::proxy::RoutingTable;
use crate::registry::Registry;

/// Shared daemon state
pub struct DaemonState {
    pub registry: Registry,
    pub process_manager: ProcessManager,
    pub routing_table: RoutingTable,
}

impl DaemonState {
    pub async fn new(routing_table: RoutingTable) -> Result<Self> {
        Ok(Self {
            registry: Registry::new().await?,
            process_manager: ProcessManager::new(),
            routing_table,
        })
    }
}

/// Start the IPC server
pub async fn start_ipc_server(
    socket_path: &Path,
    state: Arc<Mutex<DaemonState>>,
) -> Result<()> {
    // Remove existing socket file if it exists
    if socket_path.exists() {
        tokio::fs::remove_file(socket_path)
            .await
            .context("Failed to remove existing socket")?;
    }

    // Create parent directory if needed
    if let Some(parent) = socket_path.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .context("Failed to create socket directory")?;
    }

    let listener = UnixListener::bind(socket_path).context("Failed to bind Unix socket")?;

    tracing::info!("IPC server listening on {:?}", socket_path);

    loop {
        match listener.accept().await {
            Ok((stream, _)) => {
                let state = state.clone();
                tokio::spawn(async move {
                    if let Err(e) = handle_connection(stream, state).await {
                        tracing::error!("Connection error: {}", e);
                    }
                });
            }
            Err(e) => {
                tracing::error!("Accept error: {}", e);
            }
        }
    }
}

/// Handle a single IPC connection
async fn handle_connection(stream: UnixStream, state: Arc<Mutex<DaemonState>>) -> Result<()> {
    let (reader, mut writer) = stream.into_split();
    let mut reader = BufReader::new(reader);
    let mut line = String::new();

    // Read one line (one JSON request)
    reader.read_line(&mut line).await?;

    if line.is_empty() {
        return Ok(());
    }

    // Parse request
    let request: IpcRequest = match serde_json::from_str(&line) {
        Ok(req) => req,
        Err(e) => {
            let response = IpcResponse::Error {
                message: format!("Invalid request: {}", e),
            };
            let json = serde_json::to_string(&response)?;
            writer.write_all(json.as_bytes()).await?;
            writer.write_all(b"\n").await?;
            return Ok(());
        }
    };

    // Handle request
    let response = handle_request(request, state).await;

    // Send response
    let json = serde_json::to_string(&response)?;
    writer.write_all(json.as_bytes()).await?;
    writer.write_all(b"\n").await?;

    Ok(())
}

/// Handle an IPC request
async fn handle_request(
    request: IpcRequest,
    state: Arc<Mutex<DaemonState>>,
) -> IpcResponse {
    match request {
        IpcRequest::CreateProject { name, root_dir } => {
            let mut state = state.lock().await;
            match state.registry.create(name, root_dir).await {
                Ok(project) => IpcResponse::Project(project),
                Err(e) => IpcResponse::Error {
                    message: e.to_string(),
                },
            }
        }

        IpcRequest::ListProjects => {
            let state = state.lock().await;
            let projects: Vec<_> = state.registry.list().into_iter().cloned().collect();
            IpcResponse::Projects(projects)
        }

        IpcRequest::GetProject { name } => {
            let state = state.lock().await;
            match state.registry.get(&name) {
                Some(project) => IpcResponse::Project(project.clone()),
                None => IpcResponse::Error {
                    message: format!("Project '{}' not found", name),
                },
            }
        }

        IpcRequest::RunCommand {
            project_name,
            command,
            args,
        } => {
            let mut state = state.lock().await;

            // Get project to find working directory
            let working_dir = match state.registry.get(&project_name) {
                Some(project) => project.root_dir.clone(),
                None => {
                    return IpcResponse::Error {
                        message: format!("Project '{}' not found", project_name),
                    };
                }
            };

            // Spawn the process
            match state
                .process_manager
                .spawn(project_name, &command, &args, &working_dir)
                .await
            {
                Ok(process) => IpcResponse::ProcessStarted { process },
                Err(e) => IpcResponse::Error {
                    message: e.to_string(),
                },
            }
        }

        IpcRequest::StopProcess {
            project_name: _,
            process_id,
        } => {
            let mut state = state.lock().await;
            match state.process_manager.stop(process_id) {
                Ok(()) => IpcResponse::Success {
                    message: Some(format!("Process {} stopped", process_id)),
                },
                Err(e) => IpcResponse::Error {
                    message: e.to_string(),
                },
            }
        }

        IpcRequest::ListProcesses { project_name } => {
            let state = state.lock().await;
            let processes: Vec<_> = match project_name {
                Some(name) => state
                    .process_manager
                    .list_for_project(&name)
                    .into_iter()
                    .cloned()
                    .collect(),
                None => state
                    .process_manager
                    .list()
                    .into_iter()
                    .cloned()
                    .collect(),
            };
            IpcResponse::Processes(processes)
        }

        IpcRequest::Status => {
            let state = state.lock().await;
            IpcResponse::Status {
                running: true,
                project_count: state.registry.count(),
                process_count: state.process_manager.running_count(),
            }
        }

        IpcRequest::Shutdown => {
            tracing::info!("Shutdown requested");
            // We'll handle this specially
            IpcResponse::Success {
                message: Some("Shutting down".to_string()),
            }
        }
    }
}

/// Process events from the process manager and update routing table
pub async fn process_event_handler(
    state: Arc<Mutex<DaemonState>>,
    mut event_rx: tokio::sync::mpsc::Receiver<crate::process::ProcessEvent>,
) {
    while let Some(event) = event_rx.recv().await {
        match event {
            crate::process::ProcessEvent::PortDetected { process_id, port } => {
                let mut state = state.lock().await;

                // Update process port
                state.process_manager.update_port(process_id, port);

                // Get project name for this process
                if let Some(info) = state.process_manager.get(process_id) {
                    let project_name = info.project_name.clone();

                    // Update routing table
                    {
                        let mut table = state.routing_table.write().await;
                        table.insert(project_name.clone(), port);
                    }

                    // Update project's port
                    if let Err(e) = state.registry.update_port(&project_name, Some(port)).await {
                        tracing::error!("Failed to update project port: {}", e);
                    }

                    tracing::info!(
                        "Routing {} -> 127.0.0.1:{}",
                        format!("{}.localhost", project_name),
                        port
                    );
                }
            }

            crate::process::ProcessEvent::Exited {
                process_id,
                exit_code,
            } => {
                let mut state = state.lock().await;

                // Get project name before updating status
                let project_name = state
                    .process_manager
                    .get(process_id)
                    .map(|p| p.project_name.clone());

                // Update process status
                let status = if exit_code == Some(0) {
                    ProcessStatus::Stopped
                } else {
                    ProcessStatus::Failed
                };
                state.process_manager.update_status(process_id, status);

                // Remove from routing table
                if let Some(name) = project_name {
                    let mut table = state.routing_table.write().await;
                    table.remove(&name);

                    tracing::info!(
                        "Process {} exited with code {:?}, removed routing for {}",
                        process_id,
                        exit_code,
                        name
                    );
                }
            }

            crate::process::ProcessEvent::Output { .. } => {
                // Output is already printed to stdout/stderr in process.rs
            }
        }
    }
}
