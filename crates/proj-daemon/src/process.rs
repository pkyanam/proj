//! Process management - spawning, monitoring, and port detection

use anyhow::{Context, Result};
use chrono::Utc;
use nix::sys::signal::{self, Signal};
use nix::unistd::Pid;
use proj_common::{ProcessInfo, ProcessStatus};
use std::collections::HashMap;
use std::process::Stdio;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::mpsc;
use uuid::Uuid;

/// Event from a managed process
#[derive(Debug, Clone)]
pub enum ProcessEvent {
    /// Process output (stdout or stderr)
    Output { process_id: Uuid, line: String, is_stderr: bool },
    /// Process exited
    Exited { process_id: Uuid, exit_code: Option<i32> },
    /// Port detected
    PortDetected { process_id: Uuid, port: u16 },
}

/// A managed child process
struct ManagedProcess {
    info: ProcessInfo,
    #[allow(dead_code)]
    child: Child,
}

/// Process manager handles spawning and monitoring processes
pub struct ProcessManager {
    processes: HashMap<Uuid, ManagedProcess>,
    event_tx: mpsc::Sender<ProcessEvent>,
    event_rx: Option<mpsc::Receiver<ProcessEvent>>,
}

impl ProcessManager {
    pub fn new() -> Self {
        let (event_tx, event_rx) = mpsc::channel(100);
        Self {
            processes: HashMap::new(),
            event_tx,
            event_rx: Some(event_rx),
        }
    }

    /// Take the event receiver (can only be called once)
    pub fn take_event_receiver(&mut self) -> Option<mpsc::Receiver<ProcessEvent>> {
        self.event_rx.take()
    }

    /// Spawn a new process for a project
    pub async fn spawn(
        &mut self,
        project_name: String,
        command: &str,
        args: &[String],
        working_dir: &std::path::Path,
    ) -> Result<ProcessInfo> {
        let process_id = Uuid::new_v4();

        // Build the command
        let mut cmd = Command::new(command);
        cmd.args(args)
            .current_dir(working_dir)
            .env("PROJECT_ID", &project_name)
            .env("PROJECT_HOST", format!("{}.localhost", project_name))
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);

        let mut child = cmd.spawn().context("Failed to spawn process")?;

        let pid = child.id().context("Failed to get process ID")? as u32;

        let info = ProcessInfo {
            id: process_id,
            project_name: project_name.clone(),
            pid,
            command: format!("{} {}", command, args.join(" ")),
            started_at: Utc::now(),
            port: None,
            status: ProcessStatus::Running,
        };

        // Capture stdout
        if let Some(stdout) = child.stdout.take() {
            let tx = self.event_tx.clone();
            let id = process_id;
            tokio::spawn(async move {
                let reader = BufReader::new(stdout);
                let mut lines = reader.lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    // Print to daemon stdout for visibility
                    println!("[{}] {}", id, line);
                    let _ = tx.send(ProcessEvent::Output {
                        process_id: id,
                        line,
                        is_stderr: false,
                    }).await;
                }
            });
        }

        // Capture stderr
        if let Some(stderr) = child.stderr.take() {
            let tx = self.event_tx.clone();
            let id = process_id;
            tokio::spawn(async move {
                let reader = BufReader::new(stderr);
                let mut lines = reader.lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    // Print to daemon stderr for visibility
                    eprintln!("[{}] {}", id, line);
                    let _ = tx.send(ProcessEvent::Output {
                        process_id: id,
                        line,
                        is_stderr: true,
                    }).await;
                }
            });
        }

        // Monitor for process exit
        let tx = self.event_tx.clone();
        let id = process_id;
        let mut child_for_wait = child;
        tokio::spawn(async move {
            let status = child_for_wait.wait().await;
            let exit_code = status.ok().and_then(|s| s.code());
            let _ = tx.send(ProcessEvent::Exited {
                process_id: id,
                exit_code,
            }).await;
        });

        // Start port detection
        self.start_port_detection(process_id, pid).await;

        // We can't store the child after spawning wait task, so create a dummy
        // In a real implementation, we'd use a different approach
        let dummy_child = Command::new("true").spawn()?;

        let managed = ManagedProcess {
            info: info.clone(),
            child: dummy_child,
        };
        self.processes.insert(process_id, managed);

        tracing::info!(
            "Spawned process {} (pid: {}) for project {}",
            process_id,
            pid,
            project_name
        );

        Ok(info)
    }

    /// Start port detection for a process
    async fn start_port_detection(&self, process_id: Uuid, pid: u32) {
        let tx = self.event_tx.clone();

        tokio::spawn(async move {
            // Give the process time to bind to a port
            tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;

            // Poll for port for up to 30 seconds
            for _ in 0..60 {
                if let Some(port) = detect_port(pid).await {
                    tracing::info!("Detected port {} for process {}", port, process_id);
                    let _ = tx.send(ProcessEvent::PortDetected {
                        process_id,
                        port,
                    }).await;
                    return;
                }
                tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;
            }
            tracing::debug!("No port detected for process {}", process_id);
        });
    }

    /// Stop a process
    pub fn stop(&mut self, process_id: Uuid) -> Result<()> {
        let managed = self
            .processes
            .get_mut(&process_id)
            .context("Process not found")?;

        // Send SIGTERM
        let pid = Pid::from_raw(managed.info.pid as i32);
        signal::kill(pid, Signal::SIGTERM).context("Failed to send SIGTERM")?;

        managed.info.status = ProcessStatus::Stopped;
        tracing::info!("Stopped process {}", process_id);
        Ok(())
    }

    /// Get process info
    pub fn get(&self, process_id: Uuid) -> Option<&ProcessInfo> {
        self.processes.get(&process_id).map(|m| &m.info)
    }

    /// Get mutable process info
    pub fn get_mut(&mut self, process_id: Uuid) -> Option<&mut ProcessInfo> {
        self.processes.get_mut(&process_id).map(|m| &mut m.info)
    }

    /// List all processes
    pub fn list(&self) -> Vec<&ProcessInfo> {
        self.processes.values().map(|m| &m.info).collect()
    }

    /// List processes for a specific project
    pub fn list_for_project(&self, project_name: &str) -> Vec<&ProcessInfo> {
        self.processes
            .values()
            .filter(|m| m.info.project_name == project_name)
            .map(|m| &m.info)
            .collect()
    }

    /// Get running process count
    pub fn running_count(&self) -> usize {
        self.processes
            .values()
            .filter(|m| m.info.status == ProcessStatus::Running)
            .count()
    }

    /// Update process status
    pub fn update_status(&mut self, process_id: Uuid, status: ProcessStatus) {
        if let Some(managed) = self.processes.get_mut(&process_id) {
            managed.info.status = status;
        }
    }

    /// Update process port
    pub fn update_port(&mut self, process_id: Uuid, port: u16) {
        if let Some(managed) = self.processes.get_mut(&process_id) {
            managed.info.port = Some(port);
        }
    }

    /// Find process by project name (returns the most recent running one)
    pub fn find_by_project(&self, project_name: &str) -> Option<&ProcessInfo> {
        self.processes
            .values()
            .filter(|m| m.info.project_name == project_name && m.info.status == ProcessStatus::Running)
            .map(|m| &m.info)
            .max_by_key(|p| p.started_at)
    }
}

/// Detect which port a process is listening on using lsof
async fn detect_port(pid: u32) -> Option<u16> {
    let output = tokio::process::Command::new("lsof")
        .args(["-i", "-P", "-n", "-a", "-p", &pid.to_string()])
        .output()
        .await
        .ok()?;

    let stdout = String::from_utf8_lossy(&output.stdout);

    // Parse lsof output to find LISTEN ports
    // Format: COMMAND PID USER FD TYPE DEVICE SIZE/OFF NODE NAME
    // Example: Python  93214 preetham    4u  IPv6 0x... 0t0  TCP *:3002 (LISTEN)
    for line in stdout.lines() {
        if line.contains("(LISTEN)") {
            // The line contains something like: TCP *:3002 (LISTEN)
            // Find the part before "(LISTEN)" and extract the port
            let parts: Vec<&str> = line.split_whitespace().collect();
            // Look for the NAME column which contains host:port
            for part in parts.iter().rev() {
                if *part == "(LISTEN)" {
                    continue;
                }
                // This should be host:port like "*:3002" or "127.0.0.1:3002"
                if let Some(port_str) = part.rsplit(':').next() {
                    if let Ok(port) = port_str.parse::<u16>() {
                        return Some(port);
                    }
                }
            }
        }
    }

    None
}
