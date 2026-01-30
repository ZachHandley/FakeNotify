//! NFS filesystem watcher using polling.
//!
//! Uses the `notify` crate's `PollWatcher` which works on NFS filesystems
//! where inotify does not function.

use crate::config::WatchConfig;
use crate::state::DaemonState;
use fakenotify_protocol::{EventMask, FramedMessage, InotifyEvent};
use notify::{
    Config, EventKind, PollWatcher, RecursiveMode, Watcher,
    event::{CreateKind, ModifyKind, RemoveKind, RenameMode},
};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;
use tokio::sync::mpsc;

/// Cookie counter for rename events
static COOKIE_COUNTER: AtomicU32 = AtomicU32::new(1);

/// Generate a new unique cookie for rename events
fn next_cookie() -> u32 {
    COOKIE_COUNTER.fetch_add(1, Ordering::Relaxed)
}

/// Convert notify EventKind to inotify EventMask
fn notify_to_inotify_mask(kind: &EventKind, is_dir: bool) -> Option<EventMask> {
    let base_mask = match kind {
        EventKind::Create(create_kind) => match create_kind {
            CreateKind::File => EventMask::IN_CREATE,
            CreateKind::Folder => EventMask::IN_CREATE,
            CreateKind::Any => EventMask::IN_CREATE,
            _ => EventMask::IN_CREATE,
        },
        EventKind::Modify(modify_kind) => match modify_kind {
            ModifyKind::Data(_) => EventMask::IN_MODIFY,
            ModifyKind::Metadata(_) => EventMask::IN_ATTRIB,
            ModifyKind::Name(RenameMode::From) => EventMask::IN_MOVED_FROM,
            ModifyKind::Name(RenameMode::To) => EventMask::IN_MOVED_TO,
            ModifyKind::Name(RenameMode::Both) => EventMask::IN_MOVED_FROM | EventMask::IN_MOVED_TO,
            ModifyKind::Name(_) => EventMask::IN_MOVE,
            ModifyKind::Any => EventMask::IN_MODIFY,
            _ => EventMask::IN_MODIFY,
        },
        EventKind::Remove(remove_kind) => match remove_kind {
            RemoveKind::File => EventMask::IN_DELETE,
            RemoveKind::Folder => EventMask::IN_DELETE,
            RemoveKind::Any => EventMask::IN_DELETE,
            _ => EventMask::IN_DELETE,
        },
        EventKind::Access(_) => EventMask::IN_ACCESS,
        EventKind::Other => return None,
        EventKind::Any => EventMask::IN_ALL_EVENTS,
    };

    let mask = if is_dir {
        base_mask | EventMask::IN_ISDIR
    } else {
        base_mask
    };

    Some(mask)
}

/// Message sent from watcher to event dispatcher
#[derive(Debug)]
pub struct WatcherEvent {
    pub path: PathBuf,
    pub kind: EventKind,
    pub is_dir: bool,
}

/// Manages NFS watchers
pub struct WatcherManager {
    /// The poll watcher instance
    watcher: PollWatcher,
    /// Channel for receiving events
    event_rx: mpsc::UnboundedReceiver<WatcherEvent>,
    /// Currently watched paths and their intervals
    watched_paths: HashMap<PathBuf, WatchConfig>,
}

impl WatcherManager {
    /// Create a new watcher manager
    pub fn new(
        poll_interval_secs: u64,
    ) -> notify::Result<(Self, mpsc::UnboundedSender<WatcherEvent>)> {
        let (event_tx, event_rx) = mpsc::unbounded_channel();
        let event_tx_clone = event_tx.clone();

        let config = Config::default()
            .with_poll_interval(Duration::from_secs(poll_interval_secs))
            .with_compare_contents(false); // Use mtime, not content hashing

        let watcher = PollWatcher::new(
            move |res: Result<notify::Event, notify::Error>| match res {
                Ok(event) => {
                    for path in event.paths {
                        let is_dir = path.is_dir();
                        let _ = event_tx_clone.send(WatcherEvent {
                            path,
                            kind: event.kind.clone(),
                            is_dir,
                        });
                    }
                }
                Err(e) => {
                    tracing::error!(error = %e, "Watch error");
                }
            },
            config,
        )?;

        Ok((
            Self {
                watcher,
                event_rx,
                watched_paths: HashMap::new(),
            },
            event_tx,
        ))
    }

    /// Add a path to watch
    pub fn add_watch(&mut self, config: WatchConfig) -> notify::Result<()> {
        let recursive_mode = if config.recursive {
            RecursiveMode::Recursive
        } else {
            RecursiveMode::NonRecursive
        };

        self.watcher.watch(&config.path, recursive_mode)?;
        tracing::info!(
            path = %config.path.display(),
            poll_interval = config.poll_interval,
            recursive = config.recursive,
            "Added watch"
        );

        self.watched_paths.insert(config.path.clone(), config);
        Ok(())
    }

    /// Remove a watched path
    pub fn remove_watch(&mut self, path: &PathBuf) -> notify::Result<()> {
        self.watcher.unwatch(path)?;
        self.watched_paths.remove(path);
        tracing::info!(path = %path.display(), "Removed watch");
        Ok(())
    }

    /// Get the event receiver
    pub fn take_event_rx(&mut self) -> mpsc::UnboundedReceiver<WatcherEvent> {
        let (_, rx) = mpsc::unbounded_channel();
        std::mem::replace(&mut self.event_rx, rx)
    }
}

/// Event dispatcher - receives events from watcher and sends to clients
pub struct EventDispatcher {
    state: Arc<DaemonState>,
    event_rx: mpsc::UnboundedReceiver<WatcherEvent>,
    /// Track rename cookies for pairing MOVED_FROM/MOVED_TO
    pending_renames: HashMap<PathBuf, u32>,
}

impl EventDispatcher {
    pub fn new(state: Arc<DaemonState>, event_rx: mpsc::UnboundedReceiver<WatcherEvent>) -> Self {
        Self {
            state,
            event_rx,
            pending_renames: HashMap::new(),
        }
    }

    /// Run the event dispatcher loop
    pub async fn run(mut self) {
        tracing::info!("Event dispatcher started");

        while let Some(event) = self.event_rx.recv().await {
            if let Err(e) = self.handle_event(event).await {
                tracing::error!(error = %e, "Failed to dispatch event");
            }
        }

        tracing::info!("Event dispatcher stopped");
    }

    async fn handle_event(&mut self, event: WatcherEvent) -> color_eyre::Result<()> {
        // Find the watch for this path
        let watch = match self.state.find_watch_for_path(&event.path) {
            Some(w) => w,
            None => {
                tracing::trace!(path = %event.path.display(), "No watch found for path");
                return Ok(());
            }
        };

        // Convert to inotify mask
        let mask = match notify_to_inotify_mask(&event.kind, event.is_dir) {
            Some(m) => m,
            None => return Ok(()),
        };

        // Check if any client cares about this event type
        if !watch.mask.intersects(mask) {
            return Ok(());
        }

        // Determine cookie for rename events
        let cookie = if mask.intersects(EventMask::IN_MOVED_FROM) {
            let cookie = next_cookie();
            self.pending_renames.insert(event.path.clone(), cookie);
            cookie
        } else if mask.intersects(EventMask::IN_MOVED_TO) {
            // Try to find a matching MOVED_FROM event
            // For simplicity, we use a new cookie if no match found
            self.pending_renames
                .remove(&event.path)
                .unwrap_or_else(next_cookie)
        } else {
            0
        };

        // Get the filename relative to the watched directory
        let name = event
            .path
            .strip_prefix(&watch.path)
            .ok()
            .and_then(|p| p.to_str())
            .map(|s| s.to_string());

        // Create inotify event
        let inotify_event = InotifyEvent::new(watch.wd, mask.bits(), cookie);

        // Serialize the event
        let event_bytes = if let Some(ref name_str) = name {
            inotify_event.to_bytes_with_name(name_str.as_bytes())
        } else {
            inotify_event.header_to_bytes().to_vec()
        };

        // Frame the event for sending
        let framed = FramedMessage::frame(&event_bytes);

        // Send to all subscribed clients
        let clients = self.state.get_clients_for_watch(watch.wd);
        for client in clients {
            if let Err(e) = client.send_event(&framed).await {
                tracing::warn!(
                    client_id = client.id,
                    error = %e,
                    "Failed to send event to client"
                );
            }
        }

        tracing::debug!(
            wd = watch.wd,
            path = %event.path.display(),
            mask = ?mask,
            name = ?name,
            "Dispatched event"
        );

        Ok(())
    }
}

/// Start the watcher with initial configuration
pub async fn start_watcher(
    state: Arc<DaemonState>,
    initial_watches: Vec<WatchConfig>,
    default_poll_interval: u64,
) -> color_eyre::Result<WatcherManager> {
    let (mut watcher, _event_tx) = WatcherManager::new(default_poll_interval)?;

    // Add initial watches
    for watch_config in initial_watches {
        if let Err(e) = watcher.add_watch(watch_config.clone()) {
            tracing::error!(
                path = %watch_config.path.display(),
                error = %e,
                "Failed to add initial watch"
            );
        }
    }

    // Take the event receiver and start dispatcher
    let event_rx = watcher.take_event_rx();
    let dispatcher = EventDispatcher::new(state, event_rx);

    // Spawn dispatcher task
    tokio::spawn(dispatcher.run());

    Ok(watcher)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_notify_to_inotify_mask_create() {
        let mask = notify_to_inotify_mask(&EventKind::Create(CreateKind::File), false);
        assert!(mask.is_some());
        assert!(mask.unwrap().contains(EventMask::IN_CREATE));
        assert!(!mask.unwrap().contains(EventMask::IN_ISDIR));
    }

    #[test]
    fn test_notify_to_inotify_mask_create_dir() {
        let mask = notify_to_inotify_mask(&EventKind::Create(CreateKind::Folder), true);
        assert!(mask.is_some());
        assert!(mask.unwrap().contains(EventMask::IN_CREATE));
        assert!(mask.unwrap().contains(EventMask::IN_ISDIR));
    }

    #[test]
    fn test_notify_to_inotify_mask_modify() {
        let mask = notify_to_inotify_mask(
            &EventKind::Modify(ModifyKind::Data(notify::event::DataChange::Any)),
            false,
        );
        assert!(mask.is_some());
        assert!(mask.unwrap().contains(EventMask::IN_MODIFY));
    }

    #[test]
    fn test_notify_to_inotify_mask_delete() {
        let mask = notify_to_inotify_mask(&EventKind::Remove(RemoveKind::File), false);
        assert!(mask.is_some());
        assert!(mask.unwrap().contains(EventMask::IN_DELETE));
    }

    #[test]
    fn test_cookie_generation() {
        let c1 = next_cookie();
        let c2 = next_cookie();
        assert_ne!(c1, c2);
    }
}
