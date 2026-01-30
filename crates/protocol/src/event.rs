//! inotify event structures and mask constants.
//!
//! This module provides binary-compatible structures for inotify events
//! and the standard event mask constants.

use bitflags::bitflags;

bitflags! {
    /// inotify event mask flags.
    ///
    /// These match the kernel's inotify mask values exactly.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
    pub struct EventMask: u32 {
        /// File was accessed (e.g., read).
        const IN_ACCESS = 0x0000_0001;
        /// File was modified.
        const IN_MODIFY = 0x0000_0002;
        /// Metadata changed (e.g., permissions, timestamps).
        const IN_ATTRIB = 0x0000_0004;
        /// Writable file was closed.
        const IN_CLOSE_WRITE = 0x0000_0008;
        /// Unwritable file was closed.
        const IN_CLOSE_NOWRITE = 0x0000_0010;
        /// File was opened.
        const IN_OPEN = 0x0000_0020;
        /// File/directory moved out of watched directory.
        const IN_MOVED_FROM = 0x0000_0040;
        /// File/directory moved into watched directory.
        const IN_MOVED_TO = 0x0000_0080;
        /// File/directory created in watched directory.
        const IN_CREATE = 0x0000_0100;
        /// File/directory deleted from watched directory.
        const IN_DELETE = 0x0000_0200;
        /// Watched file/directory was deleted.
        const IN_DELETE_SELF = 0x0000_0400;
        /// Watched file/directory was moved.
        const IN_MOVE_SELF = 0x0000_0800;

        // Convenience combinations
        /// Close event (write or no-write).
        const IN_CLOSE = Self::IN_CLOSE_WRITE.bits() | Self::IN_CLOSE_NOWRITE.bits();
        /// Move event (from or to).
        const IN_MOVE = Self::IN_MOVED_FROM.bits() | Self::IN_MOVED_TO.bits();

        /// All events that can be watched.
        const IN_ALL_EVENTS = Self::IN_ACCESS.bits()
            | Self::IN_MODIFY.bits()
            | Self::IN_ATTRIB.bits()
            | Self::IN_CLOSE_WRITE.bits()
            | Self::IN_CLOSE_NOWRITE.bits()
            | Self::IN_OPEN.bits()
            | Self::IN_MOVED_FROM.bits()
            | Self::IN_MOVED_TO.bits()
            | Self::IN_CREATE.bits()
            | Self::IN_DELETE.bits()
            | Self::IN_DELETE_SELF.bits()
            | Self::IN_MOVE_SELF.bits();

        // Additional flags (for add_watch)
        /// Only watch pathname if it is a directory.
        const IN_ONLYDIR = 0x0100_0000;
        /// Don't follow symlinks.
        const IN_DONT_FOLLOW = 0x0200_0000;
        /// Add to existing watch mask rather than replacing.
        const IN_MASK_ADD = 0x2000_0000;
        /// Only send event once, then remove watch.
        const IN_ONESHOT = 0x8000_0000;

        // Event flags (set by kernel in returned events)
        /// Watch was removed (explicitly or automatically).
        const IN_IGNORED = 0x0000_8000;
        /// Subject of event is a directory.
        const IN_ISDIR = 0x4000_0000;
        /// Event queue overflowed.
        const IN_Q_OVERFLOW = 0x0000_4000;
        /// Filesystem containing watched object was unmounted.
        const IN_UNMOUNT = 0x0000_2000;
    }
}

/// Raw inotify event structure.
///
/// This is binary-compatible with the kernel's `struct inotify_event`.
/// The `name` field is variable-length and follows the struct in memory.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct InotifyEvent {
    /// Watch descriptor.
    pub wd: i32,
    /// Event mask.
    pub mask: u32,
    /// Unique cookie associating related events (for rename).
    pub cookie: u32,
    /// Length of the name field (including null terminator, if any).
    pub len: u32,
    // name: [u8; len] follows
}

impl InotifyEvent {
    /// Size of the fixed portion of the event structure.
    pub const HEADER_SIZE: usize = std::mem::size_of::<Self>();

    /// Create a new event with no name.
    #[must_use]
    pub const fn new(wd: i32, mask: u32, cookie: u32) -> Self {
        Self {
            wd,
            mask,
            cookie,
            len: 0,
        }
    }

    /// Create a new event with the given name length.
    #[must_use]
    pub const fn with_name_len(wd: i32, mask: u32, cookie: u32, name_len: u32) -> Self {
        Self {
            wd,
            mask,
            cookie,
            len: name_len,
        }
    }

    /// Calculate total size of this event including the name.
    #[must_use]
    pub const fn total_size(&self) -> usize {
        Self::HEADER_SIZE + self.len as usize
    }

    /// Serialize this event header to bytes.
    #[must_use]
    pub fn header_to_bytes(&self) -> [u8; Self::HEADER_SIZE] {
        let mut buf = [0u8; Self::HEADER_SIZE];
        buf[0..4].copy_from_slice(&self.wd.to_ne_bytes());
        buf[4..8].copy_from_slice(&self.mask.to_ne_bytes());
        buf[8..12].copy_from_slice(&self.cookie.to_ne_bytes());
        buf[12..16].copy_from_slice(&self.len.to_ne_bytes());
        buf
    }

    /// Serialize this event with the given name to a byte vector.
    ///
    /// The name is null-terminated and padded to the next 4-byte boundary
    /// (matching kernel behavior).
    #[must_use]
    pub fn to_bytes_with_name(&self, name: &[u8]) -> Vec<u8> {
        // Calculate padded name length (including null terminator, aligned to 4 bytes)
        let name_len_with_null = name.len() + 1;
        let padded_len = (name_len_with_null + 3) & !3;

        let event = Self {
            wd: self.wd,
            mask: self.mask,
            cookie: self.cookie,
            len: padded_len as u32,
        };

        let mut buf = Vec::with_capacity(Self::HEADER_SIZE + padded_len);
        buf.extend_from_slice(&event.header_to_bytes());
        buf.extend_from_slice(name);
        // Add null terminator and padding
        buf.resize(Self::HEADER_SIZE + padded_len, 0);
        buf
    }

    /// Parse an event header from bytes.
    ///
    /// Returns `None` if the buffer is too small.
    #[must_use]
    pub fn from_bytes(buf: &[u8]) -> Option<Self> {
        if buf.len() < Self::HEADER_SIZE {
            return None;
        }

        Some(Self {
            wd: i32::from_ne_bytes(buf[0..4].try_into().ok()?),
            mask: u32::from_ne_bytes(buf[4..8].try_into().ok()?),
            cookie: u32::from_ne_bytes(buf[8..12].try_into().ok()?),
            len: u32::from_ne_bytes(buf[12..16].try_into().ok()?),
        })
    }

    /// Get the event mask as an `EventMask` bitflags value.
    #[must_use]
    pub fn event_mask(&self) -> EventMask {
        EventMask::from_bits_truncate(self.mask)
    }
}

/// Calculate the total size of an inotify event with the given name.
///
/// The name length includes null terminator and is padded to 4-byte alignment.
#[must_use]
pub const fn event_size_with_name(name_len: usize) -> usize {
    let name_len_with_null = name_len + 1;
    let padded_len = (name_len_with_null + 3) & !3;
    InotifyEvent::HEADER_SIZE + padded_len
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_event_header_size() {
        // inotify_event header is always 16 bytes
        assert_eq!(InotifyEvent::HEADER_SIZE, 16);
    }

    #[test]
    fn test_event_roundtrip() {
        let event = InotifyEvent::new(42, EventMask::IN_CREATE.bits(), 0);
        let bytes = event.header_to_bytes();
        let parsed = InotifyEvent::from_bytes(&bytes).unwrap();
        assert_eq!(parsed.wd, 42);
        assert_eq!(parsed.mask, EventMask::IN_CREATE.bits());
        assert_eq!(parsed.cookie, 0);
        assert_eq!(parsed.len, 0);
    }

    #[test]
    fn test_event_with_name() {
        let event = InotifyEvent::new(1, EventMask::IN_CREATE.bits(), 0);
        let bytes = event.to_bytes_with_name(b"test.txt");

        // Header (16) + "test.txt" (8) + null (1) = 25, padded to 28
        assert_eq!(bytes.len(), 16 + 12);

        let parsed = InotifyEvent::from_bytes(&bytes).unwrap();
        assert_eq!(parsed.len, 12); // Padded length

        // Check name bytes
        let name_bytes = &bytes[16..16 + 8];
        assert_eq!(name_bytes, b"test.txt");
        assert_eq!(bytes[24], 0); // Null terminator
    }

    #[test]
    fn test_event_mask_all() {
        // Verify IN_ALL_EVENTS contains expected flags
        let all = EventMask::IN_ALL_EVENTS;
        assert!(all.contains(EventMask::IN_ACCESS));
        assert!(all.contains(EventMask::IN_MODIFY));
        assert!(all.contains(EventMask::IN_ATTRIB));
        assert!(all.contains(EventMask::IN_CLOSE_WRITE));
        assert!(all.contains(EventMask::IN_CLOSE_NOWRITE));
        assert!(all.contains(EventMask::IN_OPEN));
        assert!(all.contains(EventMask::IN_MOVED_FROM));
        assert!(all.contains(EventMask::IN_MOVED_TO));
        assert!(all.contains(EventMask::IN_CREATE));
        assert!(all.contains(EventMask::IN_DELETE));
        assert!(all.contains(EventMask::IN_DELETE_SELF));
        assert!(all.contains(EventMask::IN_MOVE_SELF));
    }

    #[test]
    fn test_event_size_calculation() {
        // Empty name: header only
        assert_eq!(event_size_with_name(0), 16 + 4); // null + padding

        // "a" -> 1 + 1 null = 2, padded to 4
        assert_eq!(event_size_with_name(1), 16 + 4);

        // "abc" -> 3 + 1 null = 4, no padding needed
        assert_eq!(event_size_with_name(3), 16 + 4);

        // "abcd" -> 4 + 1 null = 5, padded to 8
        assert_eq!(event_size_with_name(4), 16 + 8);
    }
}
