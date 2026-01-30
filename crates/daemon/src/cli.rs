//! Command-line interface for fakenotifyd.
//!
//! Provides commands for starting, stopping, and managing the daemon.

use clap::{Parser, Subcommand};
use std::path::PathBuf;

/// FakeNotify Daemon - NFS filesystem watcher that emulates inotify events
#[derive(Debug, Parser)]
#[command(name = "fakenotifyd")]
#[command(author, version, about, long_about = None)]
pub struct Cli {
    /// Configuration file path
    #[arg(short, long, global = true, env = "FAKENOTIFYD_CONFIG")]
    pub config: Option<PathBuf>,

    /// Log level (trace, debug, info, warn, error)
    #[arg(short, long, global = true, env = "FAKENOTIFYD_LOG_LEVEL")]
    pub log_level: Option<String>,

    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Start the daemon
    Start {
        /// Override socket path
        #[arg(short, long, env = "FAKENOTIFY_SOCKET")]
        socket: Option<PathBuf>,

        /// Run in background (daemonize)
        #[arg(short, long)]
        daemonize: bool,

        /// PID file path (only used with --daemonize)
        #[arg(long)]
        pid_file: Option<PathBuf>,
    },

    /// Stop the running daemon
    Stop {
        /// Override socket path
        #[arg(short, long, env = "FAKENOTIFY_SOCKET")]
        socket: Option<PathBuf>,
    },

    /// Show daemon status
    Status {
        /// Override socket path
        #[arg(short, long, env = "FAKENOTIFY_SOCKET")]
        socket: Option<PathBuf>,
    },

    /// Add a watch path at runtime
    Add {
        /// Path to watch
        path: PathBuf,

        /// Polling interval in seconds
        #[arg(short = 'i', long, default_value = "5")]
        poll_interval: u64,

        /// Watch recursively (default: true)
        #[arg(short, long, default_value = "true")]
        recursive: bool,

        /// Override socket path
        #[arg(short, long, env = "FAKENOTIFY_SOCKET")]
        socket: Option<PathBuf>,
    },

    /// Remove a watch path
    Remove {
        /// Path to stop watching
        path: PathBuf,

        /// Override socket path
        #[arg(short, long, env = "FAKENOTIFY_SOCKET")]
        socket: Option<PathBuf>,
    },

    /// List watched paths
    List {
        /// Override socket path
        #[arg(short, long, env = "FAKENOTIFY_SOCKET")]
        socket: Option<PathBuf>,
    },
}

impl Cli {
    /// Get the socket path from command arguments or default
    pub fn socket_path(&self) -> PathBuf {
        match &self.command {
            Command::Start { socket, .. }
            | Command::Stop { socket }
            | Command::Status { socket }
            | Command::Add { socket, .. }
            | Command::Remove { socket, .. }
            | Command::List { socket } => socket
                .clone()
                .unwrap_or_else(fakenotify_protocol::get_socket_path_with_xdg_fallback),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cli_parse_start() {
        let cli = Cli::parse_from(["fakenotifyd", "start"]);
        assert!(matches!(cli.command, Command::Start { .. }));
    }

    #[test]
    fn test_cli_parse_start_with_options() {
        let cli = Cli::parse_from([
            "fakenotifyd",
            "start",
            "--socket",
            "/tmp/test.sock",
            "--daemonize",
        ]);
        match cli.command {
            Command::Start {
                socket, daemonize, ..
            } => {
                assert_eq!(socket, Some(PathBuf::from("/tmp/test.sock")));
                assert!(daemonize);
            }
            _ => panic!("expected Start command"),
        }
    }

    #[test]
    fn test_cli_parse_add() {
        let cli = Cli::parse_from(["fakenotifyd", "add", "/mnt/media", "--poll-interval", "10"]);
        match cli.command {
            Command::Add {
                path,
                poll_interval,
                ..
            } => {
                assert_eq!(path, PathBuf::from("/mnt/media"));
                assert_eq!(poll_interval, 10);
            }
            _ => panic!("expected Add command"),
        }
    }
}
