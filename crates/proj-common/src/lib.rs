//! Shared types and utilities for the proj system.

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use uuid::Uuid;

/// Project metadata stored in project.json
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Project {
    pub name: String,
    pub id: Uuid,
    pub created_at: DateTime<Utc>,
    pub root_dir: PathBuf,
    #[serde(default)]
    pub port: Option<u16>,
}

impl Project {
    pub fn new(name: String, root_dir: PathBuf) -> Self {
        Self {
            name,
            id: Uuid::new_v4(),
            created_at: Utc::now(),
            root_dir,
            port: None,
        }
    }
}

/// Process information for a running command
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProcessInfo {
    pub id: Uuid,
    pub project_name: String,
    pub pid: u32,
    pub command: String,
    pub started_at: DateTime<Utc>,
    #[serde(default)]
    pub port: Option<u16>,
    pub status: ProcessStatus,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum ProcessStatus {
    Running,
    Stopped,
    Failed,
}

/// Global configuration
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Config {
    #[serde(default = "default_proxy_port")]
    pub proxy_port: u16,
}

fn default_proxy_port() -> u16 {
    8080
}

/// IPC Request types from CLI to daemon
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum IpcRequest {
    /// Create a new project
    CreateProject { name: String, root_dir: PathBuf },
    /// List all projects
    ListProjects,
    /// Get a specific project
    GetProject { name: String },
    /// Run a command in project context
    RunCommand {
        project_name: String,
        command: String,
        args: Vec<String>,
    },
    /// Stop a process
    StopProcess { project_name: String, process_id: Uuid },
    /// List processes for a project
    ListProcesses { project_name: Option<String> },
    /// Get daemon status
    Status,
    /// Shutdown daemon
    Shutdown,
}

/// IPC Response types from daemon to CLI
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", content = "data", rename_all = "snake_case")]
pub enum IpcResponse {
    /// Success with optional message
    Success { message: Option<String> },
    /// Project data
    Project(Project),
    /// List of projects
    Projects(Vec<Project>),
    /// Process started
    ProcessStarted { process: ProcessInfo },
    /// List of processes
    Processes(Vec<ProcessInfo>),
    /// Daemon status
    Status {
        running: bool,
        project_count: usize,
        process_count: usize,
    },
    /// Error occurred
    Error { message: String },
}

/// Get the base directory for proj data (~/.proj)
pub fn proj_dir() -> Result<PathBuf> {
    let home = dirs::home_dir().context("Could not find home directory")?;
    Ok(home.join(".proj"))
}

/// Get the projects directory (~/.proj/projects)
pub fn projects_dir() -> Result<PathBuf> {
    Ok(proj_dir()?.join("projects"))
}

/// Get the path for a specific project
pub fn project_dir(name: &str) -> Result<PathBuf> {
    Ok(projects_dir()?.join(name))
}

/// Get the daemon socket path
pub fn socket_path() -> Result<PathBuf> {
    Ok(proj_dir()?.join("daemon.sock"))
}

/// Get the config file path
pub fn config_path() -> Result<PathBuf> {
    Ok(proj_dir()?.join("config.json"))
}

/// Get the daemon PID file path
pub fn pid_file_path() -> Result<PathBuf> {
    Ok(proj_dir()?.join("daemon.pid"))
}

/// Validate project name (alphanumeric, hyphens, underscores only)
pub fn validate_project_name(name: &str) -> Result<()> {
    if name.is_empty() {
        anyhow::bail!("Project name cannot be empty");
    }
    if name.len() > 64 {
        anyhow::bail!("Project name cannot exceed 64 characters");
    }
    if !name
        .chars()
        .all(|c| c.is_alphanumeric() || c == '-' || c == '_')
    {
        anyhow::bail!(
            "Project name can only contain alphanumeric characters, hyphens, and underscores"
        );
    }
    if name.starts_with('-') || name.starts_with('_') {
        anyhow::bail!("Project name cannot start with a hyphen or underscore");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_validate_project_name() {
        assert!(validate_project_name("my-app").is_ok());
        assert!(validate_project_name("my_app").is_ok());
        assert!(validate_project_name("myapp123").is_ok());
        assert!(validate_project_name("").is_err());
        assert!(validate_project_name("-myapp").is_err());
        assert!(validate_project_name("my app").is_err());
        assert!(validate_project_name("my.app").is_err());
    }
}
