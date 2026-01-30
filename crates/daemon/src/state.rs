//! Shared state management for the daemon.
//!
//! This module manages:
//! - Connected clients
//! - Active watches
//! - Watch descriptor allocation

use fakenotify_protocol::EventMask;
use parking_lot::RwLock;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicI32, AtomicU64, Ordering};
use std::time::Instant;
use tokio::io::AsyncWriteExt;
use tokio::net::unix::OwnedWriteHalf;
use tokio::sync::Mutex;

/// Unique client identifier
pub type ClientId = u64;

/// Watch descriptor (matches inotify wd type)
pub type WatchDescriptor = i32;

/// Information about a connected client
pub struct Client {
    /// Unique client ID
    pub id: ClientId,
    /// Write half of the socket (for sending events)
    pub writer: Mutex<OwnedWriteHalf>,
    /// Watches owned by this client
    pub watches: RwLock<Vec<WatchDescriptor>>,
    /// Connection time
    pub connected_at: Instant,
}

impl Client {
    pub fn new(id: ClientId, writer: OwnedWriteHalf) -> Self {
        Self {
            id,
            writer: Mutex::new(writer),
            watches: RwLock::new(Vec::new()),
            connected_at: Instant::now(),
        }
    }

    /// Send raw event bytes to this client
    pub async fn send_event(&self, event_bytes: &[u8]) -> std::io::Result<()> {
        let mut writer = self.writer.lock().await;
        writer.write_all(event_bytes).await
    }

    /// Add a watch to this client's list
    pub fn add_watch(&self, wd: WatchDescriptor) {
        self.watches.write().push(wd);
    }

    /// Remove a watch from this client's list
    pub fn remove_watch(&self, wd: WatchDescriptor) {
        self.watches.write().retain(|&w| w != wd);
    }
}

/// Information about a watch
#[derive(Debug, Clone)]
pub struct WatchInfo {
    /// Watch descriptor
    pub wd: WatchDescriptor,
    /// Watched path
    pub path: PathBuf,
    /// Event mask
    pub mask: EventMask,
    /// Whether this is a recursive watch
    pub recursive: bool,
    /// Clients subscribed to this watch
    pub clients: Vec<ClientId>,
}

/// Shared daemon state
pub struct DaemonState {
    /// Connected clients, keyed by client ID
    clients: RwLock<HashMap<ClientId, Arc<Client>>>,

    /// Active watches, keyed by watch descriptor
    watches: RwLock<HashMap<WatchDescriptor, WatchInfo>>,

    /// Path to watch descriptor mapping (for deduplication)
    path_to_wd: RwLock<HashMap<PathBuf, WatchDescriptor>>,

    /// Next client ID
    next_client_id: AtomicU64,

    /// Next watch descriptor
    next_wd: AtomicI32,

    /// Daemon start time
    started_at: Instant,
}

impl DaemonState {
    pub fn new() -> Self {
        Self {
            clients: RwLock::new(HashMap::new()),
            watches: RwLock::new(HashMap::new()),
            path_to_wd: RwLock::new(HashMap::new()),
            next_client_id: AtomicU64::new(1),
            next_wd: AtomicI32::new(1),
            started_at: Instant::now(),
        }
    }

    /// Register a new client
    pub fn register_client(&self, writer: OwnedWriteHalf) -> Arc<Client> {
        let id = self.next_client_id.fetch_add(1, Ordering::Relaxed);
        let client = Arc::new(Client::new(id, writer));
        self.clients.write().insert(id, Arc::clone(&client));
        tracing::info!(client_id = id, "Client connected");
        client
    }

    /// Unregister a client and clean up its watches
    pub fn unregister_client(&self, client_id: ClientId) {
        // Get the client's watches before removing
        let watches_to_check = if let Some(client) = self.clients.read().get(&client_id) {
            client.watches.read().clone()
        } else {
            return;
        };

        // Remove client from each watch
        let mut watches = self.watches.write();
        let mut path_to_wd = self.path_to_wd.write();

        for wd in watches_to_check {
            if let Some(watch) = watches.get_mut(&wd) {
                watch.clients.retain(|&c| c != client_id);

                // If no clients are watching, remove the watch entirely
                if watch.clients.is_empty() {
                    let path = watch.path.clone();
                    watches.remove(&wd);
                    path_to_wd.remove(&path);
                    tracing::debug!(wd = wd, path = %path.display(), "Watch removed (no clients)");
                }
            }
        }

        // Remove the client
        self.clients.write().remove(&client_id);
        tracing::info!(client_id = client_id, "Client disconnected");
    }

    /// Get a client by ID
    pub fn get_client(&self, client_id: ClientId) -> Option<Arc<Client>> {
        self.clients.read().get(&client_id).cloned()
    }

    /// Add or update a watch
    ///
    /// Returns the watch descriptor for the path.
    /// If the path is already being watched, adds the client to the existing watch.
    pub fn add_watch(
        &self,
        client_id: ClientId,
        path: PathBuf,
        mask: EventMask,
        recursive: bool,
    ) -> WatchDescriptor {
        let mut watches = self.watches.write();
        let mut path_to_wd = self.path_to_wd.write();

        // Check if path is already being watched
        if let Some(&wd) = path_to_wd.get(&path) {
            if let Some(watch) = watches.get_mut(&wd) {
                // Add client to existing watch if not already present
                if !watch.clients.contains(&client_id) {
                    watch.clients.push(client_id);
                }
                // Merge masks
                watch.mask |= mask;
                tracing::debug!(wd = wd, path = %path.display(), "Client added to existing watch");

                // Add watch to client's list
                if let Some(client) = self.clients.read().get(&client_id) {
                    client.add_watch(wd);
                }

                return wd;
            }
        }

        // Create new watch
        let wd = self.next_wd.fetch_add(1, Ordering::Relaxed);
        let watch = WatchInfo {
            wd,
            path: path.clone(),
            mask,
            recursive,
            clients: vec![client_id],
        };

        watches.insert(wd, watch);
        path_to_wd.insert(path.clone(), wd);

        // Add watch to client's list
        if let Some(client) = self.clients.read().get(&client_id) {
            client.add_watch(wd);
        }

        tracing::info!(wd = wd, path = %path.display(), recursive = recursive, "Watch added");
        wd
    }

    /// Remove a watch for a specific client
    ///
    /// Returns true if the watch was removed, false if not found.
    pub fn remove_watch(&self, client_id: ClientId, wd: WatchDescriptor) -> bool {
        let mut watches = self.watches.write();
        let mut path_to_wd = self.path_to_wd.write();

        if let Some(watch) = watches.get_mut(&wd) {
            watch.clients.retain(|&c| c != client_id);

            // Remove watch from client's list
            if let Some(client) = self.clients.read().get(&client_id) {
                client.remove_watch(wd);
            }

            // If no clients are watching, remove the watch entirely
            if watch.clients.is_empty() {
                let path = watch.path.clone();
                watches.remove(&wd);
                path_to_wd.remove(&path);
                tracing::info!(wd = wd, path = %path.display(), "Watch removed");
            }

            return true;
        }

        false
    }

    /// Get all watched paths
    pub fn get_watched_paths(&self) -> Vec<PathBuf> {
        self.watches
            .read()
            .values()
            .map(|w| w.path.clone())
            .collect()
    }

    /// Get watch info by descriptor
    pub fn get_watch(&self, wd: WatchDescriptor) -> Option<WatchInfo> {
        self.watches.read().get(&wd).cloned()
    }

    /// Get watch descriptor for a path
    pub fn get_wd_for_path(&self, path: &PathBuf) -> Option<WatchDescriptor> {
        self.path_to_wd.read().get(path).copied()
    }

    /// Find the watch descriptor for a path or any of its parent directories
    pub fn find_watch_for_path(&self, path: &PathBuf) -> Option<WatchInfo> {
        let watches = self.watches.read();
        let path_to_wd = self.path_to_wd.read();

        // First check exact match
        if let Some(&wd) = path_to_wd.get(path) {
            return watches.get(&wd).cloned();
        }

        // Check parent directories for recursive watches
        let mut current = path.as_path();
        while let Some(parent) = current.parent() {
            if let Some(&wd) = path_to_wd.get(&parent.to_path_buf()) {
                if let Some(watch) = watches.get(&wd) {
                    if watch.recursive {
                        return Some(watch.clone());
                    }
                }
            }
            current = parent;
        }

        None
    }

    /// Get all clients watching a specific watch descriptor
    pub fn get_clients_for_watch(&self, wd: WatchDescriptor) -> Vec<Arc<Client>> {
        let watches = self.watches.read();
        let clients = self.clients.read();

        if let Some(watch) = watches.get(&wd) {
            watch
                .clients
                .iter()
                .filter_map(|&client_id| clients.get(&client_id).cloned())
                .collect()
        } else {
            Vec::new()
        }
    }

    /// Get daemon statistics
    pub fn stats(&self) -> DaemonStats {
        DaemonStats {
            uptime_secs: self.started_at.elapsed().as_secs(),
            total_clients: self.clients.read().len(),
            total_watches: self.watches.read().len(),
        }
    }
}

impl Default for DaemonState {
    fn default() -> Self {
        Self::new()
    }
}

/// Daemon statistics
#[derive(Debug, Clone)]
pub struct DaemonStats {
    pub uptime_secs: u64,
    pub total_clients: usize,
    pub total_watches: usize,
}

#[cfg(test)]
mod tests {
    use super::*;

    // Note: Most tests require tokio runtime and actual socket pairs
    // For unit tests, we test the basic state operations

    #[test]
    fn test_daemon_state_new() {
        let state = DaemonState::new();
        assert_eq!(state.clients.read().len(), 0);
        assert_eq!(state.watches.read().len(), 0);
    }
}
