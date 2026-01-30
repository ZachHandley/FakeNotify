//! FakeNotify Daemon
//!
//! A daemon that polls NFS filesystems and emits inotify-compatible events
//! to connected clients via a Unix domain socket.

mod cli;
mod config;
mod server;
mod state;
mod watcher;

use clap::Parser;
use cli::{Cli, Command};
use color_eyre::eyre::{bail, Result};
use config::Config;
use fakenotify_protocol::Request;
use server::{is_daemon_running, send_daemon_request, Server};
use state::DaemonState;
use std::sync::Arc;
use tokio::sync::broadcast;
use tracing_subscriber::{fmt, prelude::*, EnvFilter};

#[tokio::main]
async fn main() -> Result<()> {
    color_eyre::install()?;

    let cli = Cli::parse();

    // Load configuration
    let config = Config::load(cli.config.as_ref())?
        .with_socket(Some(cli.socket_path()))
        .with_log_level(cli.log_level.clone());

    // Set up logging based on command
    // Only set up logging for start command (daemon mode)
    match &cli.command {
        Command::Start { .. } => {
            init_logging(&config.daemon.log_level)?;
        }
        _ => {
            // For CLI commands, use minimal logging
            init_logging("warn")?;
        }
    }

    match cli.command {
        Command::Start {
            socket,
            daemonize,
            pid_file,
        } => {
            cmd_start(config, socket, daemonize, pid_file).await
        }
        Command::Stop { socket } => {
            cmd_stop(&config, socket).await
        }
        Command::Status { socket } => {
            cmd_status(&config, socket).await
        }
        Command::Add {
            path,
            poll_interval,
            recursive,
            socket,
        } => {
            cmd_add(&config, socket, path, poll_interval, recursive).await
        }
        Command::Remove { path, socket } => {
            cmd_remove(&config, socket, path).await
        }
        Command::List { socket } => {
            cmd_list(&config, socket).await
        }
    }
}

fn init_logging(level: &str) -> Result<()> {
    let filter = EnvFilter::try_from_default_env()
        .or_else(|_| EnvFilter::try_new(level))?;

    tracing_subscriber::registry()
        .with(fmt::layer().with_target(true))
        .with(filter)
        .init();

    Ok(())
}

async fn cmd_start(
    config: Config,
    socket_override: Option<std::path::PathBuf>,
    daemonize: bool,
    pid_file: Option<std::path::PathBuf>,
) -> Result<()> {
    let socket_path = socket_override.unwrap_or(config.daemon.socket.clone());

    // Check if already running
    if is_daemon_running(&socket_path).await {
        bail!("Daemon is already running at {}", socket_path.display());
    }

    if daemonize {
        // Fork to background
        #[cfg(unix)]
        {
            use std::process::Command;

            // Re-exec ourselves without --daemonize
            let exe = std::env::current_exe()?;
            let mut args: Vec<String> = std::env::args().collect();

            // Remove --daemonize flag
            args.retain(|arg| arg != "--daemonize" && arg != "-d");

            // Fork and exec
            let child = Command::new(&exe)
                .args(&args[1..])
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .spawn()?;

            // Write PID file if requested
            if let Some(pid_path) = pid_file {
                std::fs::write(&pid_path, child.id().to_string())?;
            }

            println!("Daemon started with PID {}", child.id());
            return Ok(());
        }

        #[cfg(not(unix))]
        {
            bail!("Daemonize is only supported on Unix systems");
        }
    }

    tracing::info!(
        version = env!("CARGO_PKG_VERSION"),
        socket = %socket_path.display(),
        "Starting fakenotifyd"
    );

    // Create shared state
    let state = Arc::new(DaemonState::new());

    // Create shutdown channel
    let (shutdown_tx, shutdown_rx) = broadcast::channel::<()>(1);

    // Set up signal handlers
    let shutdown_tx_clone = shutdown_tx.clone();
    tokio::spawn(async move {
        #[cfg(unix)]
        {
            use tokio::signal::unix::{signal, SignalKind};

            let mut sigterm = signal(SignalKind::terminate()).expect("Failed to set up SIGTERM");
            let mut sigint = signal(SignalKind::interrupt()).expect("Failed to set up SIGINT");
            let mut sighup = signal(SignalKind::hangup()).expect("Failed to set up SIGHUP");

            tokio::select! {
                _ = sigterm.recv() => {
                    tracing::info!("Received SIGTERM");
                }
                _ = sigint.recv() => {
                    tracing::info!("Received SIGINT");
                }
                _ = sighup.recv() => {
                    tracing::info!("Received SIGHUP (reload not implemented)");
                    return;
                }
            }

            let _ = shutdown_tx_clone.send(());
        }

        #[cfg(not(unix))]
        {
            tokio::signal::ctrl_c().await.expect("Failed to set up Ctrl+C");
            tracing::info!("Received Ctrl+C");
            let _ = shutdown_tx_clone.send(());
        }
    });

    // Start the file watcher
    let default_poll_interval = config
        .watch
        .first()
        .map(|w| w.poll_interval)
        .unwrap_or(5);

    let _watcher = watcher::start_watcher(
        Arc::clone(&state),
        config.watch.clone(),
        default_poll_interval,
    )
    .await?;

    // Start the socket server
    let server = Server::new(socket_path.clone(), Arc::clone(&state), shutdown_rx);
    server.run().await?;

    tracing::info!("Daemon stopped");
    Ok(())
}

async fn cmd_stop(config: &Config, socket_override: Option<std::path::PathBuf>) -> Result<()> {
    let socket_path = socket_override.unwrap_or_else(|| config.daemon.socket.clone());

    if !is_daemon_running(&socket_path).await {
        println!("Daemon is not running");
        return Ok(());
    }

    // Send ping to verify we can communicate
    match send_daemon_request(&socket_path, Request::Ping).await {
        Ok(_) => {
            // The daemon is running, we'd need a shutdown command
            // For now, we'll just report that it's running
            // A real implementation would send a shutdown command
            println!("Daemon is running at {}. Use kill or systemctl to stop.", socket_path.display());
            println!("(Shutdown command not implemented - use SIGTERM)");
        }
        Err(e) => {
            println!("Failed to communicate with daemon: {}", e);
        }
    }

    Ok(())
}

async fn cmd_status(config: &Config, socket_override: Option<std::path::PathBuf>) -> Result<()> {
    let socket_path = socket_override.unwrap_or_else(|| config.daemon.socket.clone());

    if !is_daemon_running(&socket_path).await {
        println!("Daemon is not running");
        return Ok(());
    }

    match send_daemon_request(&socket_path, Request::Ping).await {
        Ok(fakenotify_protocol::Response::Pong) => {
            println!("Daemon is running at {}", socket_path.display());
            println!("Status: OK");
        }
        Ok(resp) => {
            println!("Unexpected response: {:?}", resp);
        }
        Err(e) => {
            println!("Failed to communicate with daemon: {}", e);
        }
    }

    Ok(())
}

async fn cmd_add(
    config: &Config,
    socket_override: Option<std::path::PathBuf>,
    path: std::path::PathBuf,
    _poll_interval: u64,
    _recursive: bool,
) -> Result<()> {
    let socket_path = socket_override.unwrap_or_else(|| config.daemon.socket.clone());

    if !is_daemon_running(&socket_path).await {
        bail!("Daemon is not running");
    }

    // Resolve to absolute path
    let abs_path = std::fs::canonicalize(&path)?;

    let request = Request::AddWatch {
        path: abs_path.clone(),
        mask: fakenotify_protocol::EventMask::IN_ALL_EVENTS.bits(),
    };

    match send_daemon_request(&socket_path, request).await {
        Ok(fakenotify_protocol::Response::WatchAdded { wd }) => {
            println!("Watch added: wd={} path={}", wd, abs_path.display());
        }
        Ok(fakenotify_protocol::Response::Error { message }) => {
            bail!("Failed to add watch: {}", message);
        }
        Ok(resp) => {
            bail!("Unexpected response: {:?}", resp);
        }
        Err(e) => {
            bail!("Failed to communicate with daemon: {}", e);
        }
    }

    Ok(())
}

async fn cmd_remove(
    config: &Config,
    socket_override: Option<std::path::PathBuf>,
    path: std::path::PathBuf,
) -> Result<()> {
    let socket_path = socket_override.unwrap_or_else(|| config.daemon.socket.clone());

    if !is_daemon_running(&socket_path).await {
        bail!("Daemon is not running");
    }

    // For remove, we'd need to look up the wd for the path
    // This would require a ListWatches command or similar
    // For now, we just print a message
    println!(
        "Remove by path not fully implemented. Path: {}",
        path.display()
    );
    println!("Use the watch descriptor from 'list' command with the RemoveWatch request.");

    Ok(())
}

async fn cmd_list(config: &Config, socket_override: Option<std::path::PathBuf>) -> Result<()> {
    let socket_path = socket_override.unwrap_or_else(|| config.daemon.socket.clone());

    if !is_daemon_running(&socket_path).await {
        println!("Daemon is not running");
        return Ok(());
    }

    // We'd need a ListWatches command to implement this properly
    // For now, just verify the daemon is running
    match send_daemon_request(&socket_path, Request::Ping).await {
        Ok(fakenotify_protocol::Response::Pong) => {
            println!("Daemon is running at {}", socket_path.display());
            println!("(List watches command not yet implemented)");
        }
        Ok(resp) => {
            println!("Unexpected response: {:?}", resp);
        }
        Err(e) => {
            bail!("Failed to communicate with daemon: {}", e);
        }
    }

    Ok(())
}
