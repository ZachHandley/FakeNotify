//! FakeNotify Protocol - Shared types for IPC between daemon and LD_PRELOAD library.
//!
//! This crate provides:
//! - [`Request`] and [`Response`] message types for client-daemon communication
//! - [`InotifyEvent`] structure matching the kernel's binary format
//! - [`EventMask`] bitflags for inotify event masks
//! - Socket path helpers via [`get_socket_path`]
//!
//! # Wire Format
//!
//! Messages are serialized using [bincode](https://docs.rs/bincode) for efficiency.
//! Each message is length-prefixed with a 4-byte little-endian u32.
//!
//! # Example
//!
//! ```rust
//! use fakenotify_protocol::{Request, Response, EventMask};
//! use std::path::PathBuf;
//!
//! // Create a watch request
//! let request = Request::AddWatch {
//!     path: PathBuf::from("/tmp/watched"),
//!     mask: EventMask::IN_CREATE.bits() | EventMask::IN_DELETE.bits(),
//! };
//!
//! // Serialize for sending
//! let bytes = request.to_bytes().unwrap();
//!
//! // Deserialize on receive
//! let decoded = Request::from_bytes(&bytes).unwrap();
//! ```

mod event;
mod message;
mod socket;

// Re-export main types at crate root
pub use event::{EventMask, InotifyEvent, event_size_with_name};
pub use message::{FramedMessage, ProtocolError, Request, Response};
pub use socket::{
    DEFAULT_SOCKET_PATH, SOCKET_ENV_VAR, get_socket_path, get_socket_path_with_xdg_fallback,
};

/// Protocol version for compatibility checking.
///
/// Increment this when making breaking changes to the wire format.
pub const PROTOCOL_VERSION: u32 = 1;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_protocol_version_exists() {
        const {
            assert!(PROTOCOL_VERSION >= 1);
        }
    }

    #[test]
    fn test_reexports_accessible() {
        // Verify all re-exports are accessible
        let _ = Request::Ping;
        let _ = Response::Pong;
        let _ = EventMask::IN_CREATE;
        let _ = InotifyEvent::HEADER_SIZE;
        let _ = DEFAULT_SOCKET_PATH;
    }
}
