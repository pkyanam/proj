//! proj CLI - Command-line interface for project management
//!
//! Ergonomic syntax:
//!   proj <project> run <cmd>   - Run command in project context
//!   proj <project> open        - Open browser with isolated profile
//!   proj <project> stop        - Stop project's processes
//!   proj <project>             - Show project info
//!   proj new <name>            - Create new project
//!   proj ls                    - List all projects
//!   proj                       - Show overview

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use proj_common::{
    pid_file_path, project_dir, projects_dir, socket_path, validate_project_name, IpcRequest,
    IpcResponse,
};
use std::path::PathBuf;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;

#[derive(Parser)]
#[command(name = "proj")]
#[command(about = "Project-scoped developer environment manager")]
#[command(version)]
#[command(after_help = "EXAMPLES:
    proj new my-app              Create a new project
    proj my-app run npm run dev  Run dev server in project context
    proj my-app open             Open browser with isolated profile
    proj my-app stop             Stop project's processes
    proj my-app                  Show project info
    proj ls                      List all projects with status
    proj                         Show daemon status overview")]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand)]
enum Commands {
    /// Create a new project (proj new <name>)
    New {
        /// Project name
        name: String,
        /// Project root directory (defaults to current directory)
        #[arg(short, long)]
        dir: Option<PathBuf>,
    },

    /// List all projects (alias: ls)
    #[command(alias = "ls")]
    List,

    /// Start the background daemon
    Daemon {
        /// Run in foreground (don't daemonize)
        #[arg(short, long)]
        foreground: bool,
    },

    /// Show daemon status
    Status,

    /// Run a command in project context (proj <project> run <cmd>)
    #[command(hide = true)]
    Run {
        #[arg(trailing_var_arg = true, required = true)]
        command: Vec<String>,
    },

    /// Open browser for project (proj <project> open)
    #[command(hide = true)]
    Open,

    /// Stop project's processes (proj <project> stop)
    #[command(hide = true)]
    Stop,

    /// Project-specific commands (proj <project> [action])
    #[command(external_subcommand)]
    Project(Vec<String>),
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        None => cmd_status().await,
        Some(Commands::New { name, dir }) => cmd_new(name, dir).await,
        Some(Commands::List) => cmd_list().await,
        Some(Commands::Daemon { foreground }) => cmd_daemon(foreground).await,
        Some(Commands::Status) => cmd_status().await,
        Some(Commands::Run { command }) => {
            // This shouldn't be reached directly, but handle it
            let project = detect_project_from_cwd()?;
            cmd_run(project, command).await
        }
        Some(Commands::Open) => {
            let project = detect_project_from_cwd()?;
            cmd_open(project).await
        }
        Some(Commands::Stop) => {
            let project = detect_project_from_cwd()?;
            cmd_stop(project).await
        }
        Some(Commands::Project(args)) => handle_project_command(args).await,
    }
}

/// Handle project-specific commands: proj <project> [action] [args...]
async fn handle_project_command(args: Vec<String>) -> Result<()> {
    if args.is_empty() {
        return cmd_status().await;
    }

    let project_name = &args[0];

    // Check if this might be a project name
    if args.len() == 1 {
        // Just "proj <name>" - show project info
        return cmd_project_info(project_name).await;
    }

    let action = &args[1];
    let rest = args[2..].to_vec();

    match action.as_str() {
        "run" => {
            if rest.is_empty() {
                anyhow::bail!("Usage: proj {} run <command>", project_name);
            }
            cmd_run(project_name.clone(), rest).await
        }
        "open" => cmd_open(project_name.clone()).await,
        "stop" => cmd_stop(project_name.clone()).await,
        "info" => cmd_project_info(project_name).await,
        _ => {
            // Assume it's a command to run: proj <project> npm run dev
            let mut command = vec![action.clone()];
            command.extend(rest);
            cmd_run(project_name.clone(), command).await
        }
    }
}

/// Show info about a specific project
async fn cmd_project_info(name: &str) -> Result<()> {
    let response = send_request(IpcRequest::GetProject {
        name: name.to_string(),
    })
    .await?;

    let project = match response {
        IpcResponse::Project(p) => p,
        IpcResponse::Error { message } => {
            anyhow::bail!("{}", message);
        }
        _ => anyhow::bail!("Unexpected response"),
    };

    // Get processes for this project
    let proc_response = send_request(IpcRequest::ListProcesses {
        project_name: Some(name.to_string()),
    })
    .await?;

    let processes = match proc_response {
        IpcResponse::Processes(p) => p,
        _ => vec![],
    };

    let running: Vec<_> = processes
        .iter()
        .filter(|p| p.status == proj_common::ProcessStatus::Running)
        .collect();

    println!("Project: {}", project.name);
    println!("  Root:    {}", project.root_dir.display());
    println!("  Created: {}", project.created_at.format("%Y-%m-%d %H:%M"));

    if let Some(proc) = running.first() {
        println!("  Status:  \x1b[32mrunning\x1b[0m");
        if let Some(port) = proc.port {
            println!("  Port:    {}", port);
            println!("  URL:     http://{}.localhost:8080", project.name);
        }
        println!("  PID:     {}", proc.pid);
        println!("  Command: {}", proc.command);
    } else {
        println!("  Status:  \x1b[90mstopped\x1b[0m");
    }

    println!();
    println!("Commands:");
    println!("  proj {} run <cmd>   Run a command", project.name);
    println!("  proj {} open        Open in browser", project.name);
    println!("  proj {} stop        Stop processes", project.name);

    Ok(())
}

/// Send a request to the daemon and get a response
async fn send_request(request: IpcRequest) -> Result<IpcResponse> {
    let socket = socket_path()?;

    // Auto-start daemon if not running
    if !socket.exists() {
        auto_start_daemon().await?;
    }

    let stream = UnixStream::connect(&socket)
        .await
        .context("Failed to connect to daemon. Try: proj daemon -f")?;

    let (reader, mut writer) = stream.into_split();

    // Send request
    let json = serde_json::to_string(&request)?;
    writer.write_all(json.as_bytes()).await?;
    writer.write_all(b"\n").await?;

    // Read response
    let mut reader = BufReader::new(reader);
    let mut line = String::new();
    reader.read_line(&mut line).await?;

    let response: IpcResponse =
        serde_json::from_str(&line).context("Invalid response from daemon")?;

    Ok(response)
}

/// Auto-start the daemon in the background
async fn auto_start_daemon() -> Result<()> {
    let daemon_path = std::env::current_exe()?
        .parent()
        .context("No parent directory")?
        .join("proj-daemon");

    if !daemon_path.exists() {
        anyhow::bail!(
            "Daemon binary not found. Please reinstall proj or run: cargo build --release"
        );
    }

    // Spawn detached
    std::process::Command::new(&daemon_path)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .context("Failed to start daemon")?;

    // Wait for daemon to be ready
    let socket = socket_path()?;
    for _ in 0..20 {
        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
        if socket.exists() {
            return Ok(());
        }
    }

    anyhow::bail!("Daemon failed to start. Try: proj daemon -f")
}

/// Create a new project
async fn cmd_new(name: String, dir: Option<PathBuf>) -> Result<()> {
    validate_project_name(&name)?;

    let root_dir = match dir {
        Some(d) => d.canonicalize().context("Invalid directory path")?,
        None => std::env::current_dir()?,
    };

    let response = send_request(IpcRequest::CreateProject {
        name: name.clone(),
        root_dir: root_dir.clone(),
    })
    .await?;

    match response {
        IpcResponse::Project(project) => {
            println!(
                "\x1b[32m✓\x1b[0m Created project \x1b[1m{}\x1b[0m",
                project.name
            );
            println!("  Root: {}", project.root_dir.display());
            println!();
            println!("Next steps:");
            println!(
                "  proj {} run <cmd>   Start a dev server",
                project.name
            );
            println!(
                "  proj {} open        Open in isolated browser",
                project.name
            );
        }
        IpcResponse::Error { message } => {
            anyhow::bail!("{}", message);
        }
        _ => {
            anyhow::bail!("Unexpected response from daemon");
        }
    }

    Ok(())
}

/// Run a command in project context
async fn cmd_run(project_name: String, command: Vec<String>) -> Result<()> {
    if command.is_empty() {
        anyhow::bail!("No command specified");
    }

    let cmd = command[0].clone();
    let args = command[1..].to_vec();

    println!(
        "\x1b[36m▶\x1b[0m Running in \x1b[1m{}\x1b[0m: {} {}",
        project_name,
        cmd,
        args.join(" ")
    );

    let response = send_request(IpcRequest::RunCommand {
        project_name: project_name.clone(),
        command: cmd,
        args,
    })
    .await?;

    match response {
        IpcResponse::ProcessStarted { process } => {
            println!("  PID: {}", process.pid);
            println!();
            println!(
                "\x1b[32m✓\x1b[0m Access at: \x1b[4mhttp://{}.localhost:8080\x1b[0m",
                project_name
            );
            println!("  Stop with: proj {} stop", project_name);
        }
        IpcResponse::Error { message } => {
            anyhow::bail!("{}", message);
        }
        _ => {
            anyhow::bail!("Unexpected response from daemon");
        }
    }

    Ok(())
}

/// Open browser for a project
async fn cmd_open(project_name: String) -> Result<()> {
    // Get project info to verify it exists
    let response = send_request(IpcRequest::GetProject {
        name: project_name.clone(),
    })
    .await?;

    let project = match response {
        IpcResponse::Project(p) => p,
        IpcResponse::Error { message } => {
            anyhow::bail!("{}", message);
        }
        _ => {
            anyhow::bail!("Unexpected response from daemon");
        }
    };

    // Chrome profile directory
    let chrome_dir = project_dir(&project.name)?.join("chrome");

    // URL to open
    let url = format!("http://{}.localhost:8080", project.name);

    println!(
        "\x1b[36m▶\x1b[0m Opening \x1b[4m{}\x1b[0m with isolated Chrome profile",
        url
    );

    // Open Chrome with isolated profile
    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("open")
            .args([
                "-na",
                "Google Chrome",
                "--args",
                &format!("--user-data-dir={}", chrome_dir.display()),
                &url,
            ])
            .spawn()
            .context("Failed to open Chrome. Is it installed?")?;
    }

    #[cfg(target_os = "linux")]
    {
        // Try different Chrome/Chromium variants
        let browsers = ["google-chrome", "chromium", "chromium-browser"];
        let mut opened = false;

        for browser in browsers {
            if std::process::Command::new(browser)
                .args([&format!("--user-data-dir={}", chrome_dir.display()), &url])
                .spawn()
                .is_ok()
            {
                opened = true;
                break;
            }
        }

        if !opened {
            anyhow::bail!("Failed to open Chrome/Chromium. Is it installed?");
        }
    }

    Ok(())
}

/// List all projects
async fn cmd_list() -> Result<()> {
    let response = send_request(IpcRequest::ListProjects).await?;

    match response {
        IpcResponse::Projects(projects) => {
            if projects.is_empty() {
                println!("No projects yet.");
                println!();
                println!("Create one with: proj new <name>");
                return Ok(());
            }

            // Also get processes to show status
            let proc_response =
                send_request(IpcRequest::ListProcesses { project_name: None }).await?;
            let processes = match proc_response {
                IpcResponse::Processes(p) => p,
                _ => vec![],
            };

            for project in projects {
                let proc = processes.iter().find(|p| {
                    p.project_name == project.name
                        && p.status == proj_common::ProcessStatus::Running
                });

                let (status_icon, status_color) = if proc.is_some() {
                    ("●", "\x1b[32m") // green
                } else {
                    ("○", "\x1b[90m") // gray
                };

                let port_str = proc
                    .and_then(|p| p.port)
                    .map(|p| format!(":{}", p))
                    .unwrap_or_default();

                println!(
                    "{}{}\x1b[0m \x1b[1m{}\x1b[0m{}",
                    status_color, status_icon, project.name, port_str
                );
                println!("    {}", project.root_dir.display());
            }
        }
        IpcResponse::Error { message } => {
            anyhow::bail!("{}", message);
        }
        _ => {
            anyhow::bail!("Unexpected response from daemon");
        }
    }

    Ok(())
}

/// Start or manage the daemon
async fn cmd_daemon(foreground: bool) -> Result<()> {
    let socket = socket_path()?;
    let pid_file = pid_file_path()?;

    // Check if daemon is already running
    if socket.exists() {
        // Try to connect to verify it's alive
        if UnixStream::connect(&socket).await.is_ok() {
            println!("\x1b[32m●\x1b[0m Daemon already running");
            return Ok(());
        } else {
            // Socket exists but daemon is dead, clean up
            let _ = tokio::fs::remove_file(&socket).await;
            if pid_file.exists() {
                let _ = tokio::fs::remove_file(&pid_file).await;
            }
        }
    }

    if foreground {
        println!("\x1b[36m▶\x1b[0m Starting daemon in foreground (Ctrl+C to stop)");
        println!();

        // Run daemon directly - exec into it
        let daemon_path = std::env::current_exe()?
            .parent()
            .context("No parent directory")?
            .join("proj-daemon");

        if !daemon_path.exists() {
            anyhow::bail!(
                "Daemon binary not found at {:?}. Build with: cargo build",
                daemon_path
            );
        }

        let status = std::process::Command::new(&daemon_path)
            .status()
            .context("Failed to start daemon")?;

        if !status.success() {
            anyhow::bail!("Daemon exited with error");
        }
    } else {
        // Spawn daemon in background
        let daemon_path = std::env::current_exe()?
            .parent()
            .context("No parent directory")?
            .join("proj-daemon");

        if !daemon_path.exists() {
            anyhow::bail!(
                "Daemon binary not found at {:?}. Build with: cargo build",
                daemon_path
            );
        }

        // Spawn detached
        std::process::Command::new(&daemon_path)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .context("Failed to start daemon")?;

        // Wait a bit and verify it started
        tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;

        if socket.exists() {
            println!(
                "\x1b[32m✓\x1b[0m Daemon started on \x1b[4mhttp://localhost:8080\x1b[0m"
            );
        } else {
            anyhow::bail!("Daemon failed to start. Try: proj daemon -f");
        }
    }

    Ok(())
}

/// Show daemon status
async fn cmd_status() -> Result<()> {
    let response = send_request(IpcRequest::Status).await?;

    match response {
        IpcResponse::Status {
            running: _,
            project_count,
            process_count,
        } => {
            println!(
                "\x1b[32m●\x1b[0m proj daemon running on \x1b[4mhttp://localhost:8080\x1b[0m"
            );
            println!(
                "  {} project{}, {} running",
                project_count,
                if project_count == 1 { "" } else { "s" },
                process_count
            );
            println!();
            println!("Commands:");
            println!("  proj new <name>         Create a project");
            println!("  proj <name> run <cmd>   Run command in project");
            println!("  proj <name> open        Open browser");
            println!("  proj ls                 List all projects");
        }
        IpcResponse::Error { message } => {
            anyhow::bail!("{}", message);
        }
        _ => {
            anyhow::bail!("Unexpected response from daemon");
        }
    }

    Ok(())
}

/// Stop a running process
async fn cmd_stop(project_name: String) -> Result<()> {
    // Get running process for project
    let response = send_request(IpcRequest::ListProcesses {
        project_name: Some(project_name.clone()),
    })
    .await?;

    let processes = match response {
        IpcResponse::Processes(p) => p,
        IpcResponse::Error { message } => {
            anyhow::bail!("{}", message);
        }
        _ => {
            anyhow::bail!("Unexpected response from daemon");
        }
    };

    let running: Vec<_> = processes
        .into_iter()
        .filter(|p| p.status == proj_common::ProcessStatus::Running)
        .collect();

    if running.is_empty() {
        println!("No running processes for project '{}'", project_name);
        return Ok(());
    }

    for proc in running {
        let response = send_request(IpcRequest::StopProcess {
            project_name: project_name.clone(),
            process_id: proc.id,
        })
        .await?;

        match response {
            IpcResponse::Success { .. } => {
                println!(
                    "\x1b[33m■\x1b[0m Stopped \x1b[1m{}\x1b[0m (PID: {})",
                    project_name, proc.pid
                );
            }
            IpcResponse::Error { message } => {
                eprintln!(
                    "\x1b[31m✗\x1b[0m Failed to stop process {}: {}",
                    proc.id, message
                );
            }
            _ => {}
        }
    }

    Ok(())
}

/// Try to detect project from current working directory
fn detect_project_from_cwd() -> Result<String> {
    let cwd = std::env::current_dir()?;

    // Check if any project.json files match our cwd
    let projects_path = projects_dir()?;
    if projects_path.exists() {
        if let Ok(entries) = std::fs::read_dir(&projects_path) {
            for entry in entries.flatten() {
                let project_file = entry.path().join("project.json");
                if project_file.exists() {
                    if let Ok(content) = std::fs::read_to_string(&project_file) {
                        if let Ok(project) =
                            serde_json::from_str::<proj_common::Project>(&content)
                        {
                            // Check if cwd is the project root or a subdirectory
                            if cwd.starts_with(&project.root_dir) {
                                return Ok(project.name);
                            }
                        }
                    }
                }
            }
        }
    }

    anyhow::bail!(
        "Not in a project directory. Specify project name:\n\
         \n\
         Usage: proj <project> <command>\n\
         \n\
         List projects: proj ls"
    )
}
