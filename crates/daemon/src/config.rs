//! Configuration management for fakenotifyd.
//!
//! Uses figment to merge configuration from multiple sources:
//! 1. Default values
//! 2. Config file (TOML)
//! 3. Environment variables
//! 4. Command-line arguments

use figment::{
    providers::{Env, Format, Serialized, Toml},
    Figment,
};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Main configuration structure
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    /// Daemon configuration
    #[serde(default)]
    pub daemon: DaemonConfig,

    /// Watch paths configured at startup
    #[serde(default)]
    pub watch: Vec<WatchConfig>,
}

/// Daemon-specific configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DaemonConfig {
    /// Unix socket path
    #[serde(default = "default_socket_path")]
    pub socket: PathBuf,

    /// Log level (trace, debug, info, warn, error)
    #[serde(default = "default_log_level")]
    pub log_level: String,

    /// Maximum number of concurrent clients
    #[serde(default = "default_max_clients")]
    pub max_clients: usize,

    /// Enable metrics/stats collection
    #[serde(default)]
    pub enable_stats: bool,
}

/// Watch path configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WatchConfig {
    /// Path to watch
    pub path: PathBuf,

    /// Polling interval in seconds
    #[serde(default = "default_poll_interval")]
    pub poll_interval: u64,

    /// Whether to watch recursively
    #[serde(default = "default_recursive")]
    pub recursive: bool,
}

fn default_socket_path() -> PathBuf {
    fakenotify_protocol::get_socket_path_with_xdg_fallback()
}

fn default_log_level() -> String {
    "info".to_string()
}

fn default_max_clients() -> usize {
    100
}

fn default_poll_interval() -> u64 {
    5
}

fn default_recursive() -> bool {
    true
}

impl Default for Config {
    fn default() -> Self {
        Self {
            daemon: DaemonConfig::default(),
            watch: Vec::new(),
        }
    }
}

impl Default for DaemonConfig {
    fn default() -> Self {
        Self {
            socket: default_socket_path(),
            log_level: default_log_level(),
            max_clients: default_max_clients(),
            enable_stats: false,
        }
    }
}

impl Config {
    /// Load configuration from all sources
    pub fn load(config_file: Option<&PathBuf>) -> Result<Self, figment::Error> {
        let mut figment = Figment::new().merge(Serialized::defaults(Config::default()));

        // Add config file if provided
        if let Some(path) = config_file {
            figment = figment.merge(Toml::file(path));
        } else {
            // Try default config locations
            let default_paths = [
                PathBuf::from("/etc/fakenotify/config.toml"),
                dirs::config_dir()
                    .unwrap_or_default()
                    .join("fakenotify/config.toml"),
            ];

            for path in &default_paths {
                if path.exists() {
                    figment = figment.merge(Toml::file(path));
                    break;
                }
            }
        }

        // Environment variables (FAKENOTIFYD_ prefix)
        figment = figment.merge(Env::prefixed("FAKENOTIFYD_").split("_"));

        figment.extract()
    }

    /// Override socket path from CLI
    pub fn with_socket(mut self, socket: Option<PathBuf>) -> Self {
        if let Some(s) = socket {
            self.daemon.socket = s;
        }
        self
    }

    /// Override log level from CLI
    pub fn with_log_level(mut self, log_level: Option<String>) -> Self {
        if let Some(level) = log_level {
            self.daemon.log_level = level;
        }
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = Config::default();
        assert_eq!(config.daemon.log_level, "info");
        assert_eq!(config.daemon.max_clients, 100);
        assert!(config.watch.is_empty());
    }

    #[test]
    fn test_config_override_socket() {
        let config = Config::default().with_socket(Some(PathBuf::from("/tmp/test.sock")));
        assert_eq!(config.daemon.socket, PathBuf::from("/tmp/test.sock"));
    }

    #[test]
    fn test_config_override_log_level() {
        let config = Config::default().with_log_level(Some("debug".to_string()));
        assert_eq!(config.daemon.log_level, "debug");
    }
}
