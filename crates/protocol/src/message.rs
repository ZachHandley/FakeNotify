//! IPC message types for communication between the daemon and LD_PRELOAD library.
//!
//! These types are serialized using bincode for efficient wire format.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use thiserror::Error;

/// Error type for protocol operations.
#[derive(Debug, Error)]
pub enum ProtocolError {
    /// Serialization/deserialization error.
    #[error("serialization error: {0}")]
    Serialization(#[from] bincode::Error),

    /// IO error during communication.
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    /// Invalid message received.
    #[error("invalid message: {0}")]
    InvalidMessage(String),
}

/// Request messages sent from client (LD_PRELOAD) to daemon.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum Request {
    /// Register a new client connection.
    /// The daemon responds with a unique client ID.
    RegisterClient,

    /// Add a watch for filesystem events.
    AddWatch {
        /// Path to watch.
        path: PathBuf,
        /// Event mask (combination of EventMask flags).
        mask: u32,
    },

    /// Remove an existing watch.
    RemoveWatch {
        /// Watch descriptor to remove.
        wd: i32,
    },

    /// Keepalive ping.
    Ping,
}

/// Response messages sent from daemon to client (LD_PRELOAD).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum Response {
    /// Client registration successful.
    ClientRegistered {
        /// Unique client identifier.
        client_id: u64,
    },

    /// Watch added successfully.
    WatchAdded {
        /// Watch descriptor for the new watch.
        wd: i32,
    },

    /// Watch removed successfully.
    WatchRemoved,

    /// Error response.
    Error {
        /// Human-readable error message.
        message: String,
    },

    /// Pong response to a ping.
    Pong,
}

impl Request {
    /// Serialize this request to bytes using bincode.
    pub fn to_bytes(&self) -> Result<Vec<u8>, ProtocolError> {
        bincode::serialize(self).map_err(Into::into)
    }

    /// Deserialize a request from bytes.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, ProtocolError> {
        bincode::deserialize(bytes).map_err(Into::into)
    }
}

impl Response {
    /// Serialize this response to bytes using bincode.
    pub fn to_bytes(&self) -> Result<Vec<u8>, ProtocolError> {
        bincode::serialize(self).map_err(Into::into)
    }

    /// Deserialize a response from bytes.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, ProtocolError> {
        bincode::deserialize(bytes).map_err(Into::into)
    }

    /// Create an error response.
    #[must_use]
    pub fn error(message: impl Into<String>) -> Self {
        Self::Error {
            message: message.into(),
        }
    }
}

/// A length-prefixed message wrapper for framing.
///
/// Messages are sent as:
/// - 4 bytes: message length (u32, little-endian)
/// - N bytes: message payload
#[derive(Debug, Clone)]
pub struct FramedMessage;

impl FramedMessage {
    /// Maximum message size (1 MB).
    pub const MAX_SIZE: usize = 1024 * 1024;

    /// Frame a message with a length prefix.
    pub fn frame(payload: &[u8]) -> Vec<u8> {
        let len = payload.len() as u32;
        let mut buf = Vec::with_capacity(4 + payload.len());
        buf.extend_from_slice(&len.to_le_bytes());
        buf.extend_from_slice(payload);
        buf
    }

    /// Read the length prefix from a buffer.
    ///
    /// Returns `None` if the buffer is too small.
    #[must_use]
    pub fn read_length(buf: &[u8]) -> Option<u32> {
        if buf.len() < 4 {
            return None;
        }
        Some(u32::from_le_bytes(buf[0..4].try_into().ok()?))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_request_roundtrip() {
        let requests = vec![
            Request::RegisterClient,
            Request::AddWatch {
                path: PathBuf::from("/tmp/test"),
                mask: 0x100,
            },
            Request::RemoveWatch { wd: 42 },
            Request::Ping,
        ];

        for req in requests {
            let bytes = req.to_bytes().unwrap();
            let decoded = Request::from_bytes(&bytes).unwrap();
            assert_eq!(req, decoded);
        }
    }

    #[test]
    fn test_response_roundtrip() {
        let responses = vec![
            Response::ClientRegistered { client_id: 12345 },
            Response::WatchAdded { wd: 1 },
            Response::WatchRemoved,
            Response::Error {
                message: "test error".to_string(),
            },
            Response::Pong,
        ];

        for resp in responses {
            let bytes = resp.to_bytes().unwrap();
            let decoded = Response::from_bytes(&bytes).unwrap();
            assert_eq!(resp, decoded);
        }
    }

    #[test]
    fn test_framed_message() {
        let payload = b"hello world";
        let framed = FramedMessage::frame(payload);

        assert_eq!(framed.len(), 4 + payload.len());
        assert_eq!(FramedMessage::read_length(&framed), Some(payload.len() as u32));
        assert_eq!(&framed[4..], payload);
    }

    #[test]
    fn test_response_error_helper() {
        let resp = Response::error("something went wrong");
        match resp {
            Response::Error { message } => assert_eq!(message, "something went wrong"),
            _ => panic!("expected Error variant"),
        }
    }
}
