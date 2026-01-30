//! FakeNotify Preload Library
//!
//! An LD_PRELOAD library that intercepts inotify syscalls and redirects them
//! to the fakenotifyd daemon. This allows applications to receive file system
//! events from network-mounted filesystems where kernel inotify doesn't work.
//!
//! # How it works
//!
//! 1. App calls `inotify_init()` -> We connect to daemon, return our socket fd
//! 2. App calls `inotify_add_watch(fd, path, mask)` -> We send AddWatch to daemon
//! 3. App calls `read(fd, ...)` -> Reads from our socket, gets inotify_event structs
//! 4. App thinks it's using real inotify
//!
//! # Safety
//!
//! This library is loaded into arbitrary processes via LD_PRELOAD.
//! We must be extremely careful about:
//! - No panics (use catch_unwind everywhere)
//! - Minimal allocations during init
//! - Thread safety (all state behind RwLock)
//! - No interference with app's own operations

use fakenotify_protocol::{FramedMessage, Request, Response, get_socket_path_with_xdg_fallback};
use parking_lot::RwLock;
use std::collections::HashSet;
use std::ffi::{CStr, c_char, c_int};
use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::Duration;

// ============================================================================
// Original function pointers (resolved via dlsym)
// ============================================================================

type InotifyInitFn = unsafe extern "C" fn() -> c_int;
type InotifyInit1Fn = unsafe extern "C" fn(c_int) -> c_int;
type InotifyAddWatchFn = unsafe extern "C" fn(c_int, *const c_char, u32) -> c_int;
type InotifyRmWatchFn = unsafe extern "C" fn(c_int, c_int) -> c_int;
type CloseFn = unsafe extern "C" fn(c_int) -> c_int;

static mut REAL_INOTIFY_INIT: Option<InotifyInitFn> = None;
static mut REAL_INOTIFY_INIT1: Option<InotifyInit1Fn> = None;
static mut REAL_INOTIFY_ADD_WATCH: Option<InotifyAddWatchFn> = None;
static mut REAL_INOTIFY_RM_WATCH: Option<InotifyRmWatchFn> = None;
static mut REAL_CLOSE: Option<CloseFn> = None;

// ============================================================================
// Global state
// ============================================================================

/// Set of file descriptors that are managed by us (daemon connections)
static MANAGED_FDS: RwLock<Option<HashSet<c_int>>> = RwLock::new(None);

/// Whether initialization has completed
static INITIALIZED: AtomicBool = AtomicBool::new(false);

// ============================================================================
// Initialization
// ============================================================================

/// Initialize the preload library
///
/// This runs automatically when the library is loaded via ctor.
/// We resolve the original libc functions via dlsym.
#[ctor::ctor]
fn init() {
    // Wrap everything in catch_unwind to prevent panics from propagating
    let _ = std::panic::catch_unwind(|| {
        // SAFETY: We're in initialization, before any threads are created.
        // These function pointers are only written here and read later.
        unsafe {
            REAL_INOTIFY_INIT = resolve_symbol(b"inotify_init\0");
            REAL_INOTIFY_INIT1 = resolve_symbol(b"inotify_init1\0");
            REAL_INOTIFY_ADD_WATCH = resolve_symbol(b"inotify_add_watch\0");
            REAL_INOTIFY_RM_WATCH = resolve_symbol(b"inotify_rm_watch\0");
            REAL_CLOSE = resolve_symbol(b"close\0");
        }

        // Initialize the managed FDs set
        *MANAGED_FDS.write() = Some(HashSet::new());

        INITIALIZED.store(true, Ordering::SeqCst);
    });
}

/// Resolve a symbol from the next library in the chain
///
/// # Safety
///
/// The returned function pointer must match the expected signature.
unsafe fn resolve_symbol<T>(name: &[u8]) -> Option<T> {
    // SAFETY: dlsym is safe to call with RTLD_NEXT and a valid C string
    let ptr = unsafe { libc::dlsym(libc::RTLD_NEXT, name.as_ptr() as *const c_char) };
    if ptr.is_null() {
        None
    } else {
        // SAFETY: Caller ensures the type T matches the actual function signature
        Some(unsafe { std::mem::transmute_copy(&ptr) })
    }
}

// ============================================================================
// Helper functions
// ============================================================================

/// Get the socket path from environment or use default with XDG fallback
fn get_socket_path() -> PathBuf {
    get_socket_path_with_xdg_fallback()
}

/// Check if a file descriptor is managed by us
fn is_managed_fd(fd: c_int) -> bool {
    MANAGED_FDS
        .read()
        .as_ref()
        .is_some_and(|set| set.contains(&fd))
}

/// Register a file descriptor as managed by us
fn register_fd(fd: c_int) {
    if let Some(ref mut set) = *MANAGED_FDS.write() {
        set.insert(fd);
    }
}

/// Unregister a file descriptor
fn unregister_fd(fd: c_int) {
    if let Some(ref mut set) = *MANAGED_FDS.write() {
        set.remove(&fd);
    }
}

/// Set errno
fn set_errno(err: c_int) {
    // SAFETY: __errno_location returns a valid pointer to the thread-local errno
    unsafe {
        *libc::__errno_location() = err;
    }
}

/// Connect to the daemon with retry logic
///
/// This blocks until connection succeeds (per user requirement).
fn connect_to_daemon() -> Option<UnixStream> {
    let socket_path = get_socket_path();
    let mut attempt = 0u32;

    loop {
        match UnixStream::connect(&socket_path) {
            Ok(stream) => {
                // Set reasonable timeouts
                let _ = stream.set_read_timeout(Some(Duration::from_secs(30)));
                let _ = stream.set_write_timeout(Some(Duration::from_secs(10)));
                return Some(stream);
            }
            Err(_) => {
                attempt = attempt.saturating_add(1);

                // Exponential backoff: 100ms, 200ms, 400ms, 800ms, 1s, 1s, 1s...
                let delay_ms = std::cmp::min(100 * (1 << std::cmp::min(attempt, 4)), 1000);
                thread::sleep(Duration::from_millis(delay_ms as u64));

                // After 60 seconds of trying, give up and return None
                // This prevents infinite blocking if daemon is truly unavailable
                if attempt > 60 {
                    return None;
                }
            }
        }
    }
}

/// Send a request and receive a response
fn send_request(stream: &mut UnixStream, request: &Request) -> Option<Response> {
    // Serialize the request
    let payload = request.to_bytes().ok()?;

    // Frame it with length prefix
    let framed = FramedMessage::frame(&payload);

    // Send it
    stream.write_all(&framed).ok()?;

    // Read the response length (4 bytes, little-endian)
    let mut len_buf = [0u8; 4];
    stream.read_exact(&mut len_buf).ok()?;
    let len = FramedMessage::read_length(&len_buf)? as usize;

    // Validate length
    if len > FramedMessage::MAX_SIZE {
        return None;
    }

    // Read the response payload
    let mut payload = vec![0u8; len];
    stream.read_exact(&mut payload).ok()?;

    // Deserialize the response
    Response::from_bytes(&payload).ok()
}

// ============================================================================
// Intercepted functions
// ============================================================================

/// Intercepted inotify_init()
///
/// Instead of creating a real inotify fd, we connect to the daemon
/// and return the socket fd.
///
/// # Safety
///
/// This function is called by libc as a replacement for inotify_init.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn inotify_init() -> c_int {
    // Wrap in catch_unwind to prevent panics
    std::panic::catch_unwind(|| inotify_init_impl(0)).unwrap_or_else(|_| {
        set_errno(libc::EIO);
        -1
    })
}

/// Intercepted inotify_init1()
///
/// Same as inotify_init but accepts flags (IN_NONBLOCK, IN_CLOEXEC).
///
/// # Safety
///
/// This function is called by libc as a replacement for inotify_init1.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn inotify_init1(flags: c_int) -> c_int {
    std::panic::catch_unwind(|| inotify_init_impl(flags)).unwrap_or_else(|_| {
        set_errno(libc::EIO);
        -1
    })
}

/// Implementation for both inotify_init and inotify_init1
fn inotify_init_impl(flags: c_int) -> c_int {
    // If not initialized, fall back to real inotify
    if !INITIALIZED.load(Ordering::SeqCst) {
        return call_real_inotify_init1(flags);
    }

    // Connect to daemon
    let mut stream = match connect_to_daemon() {
        Some(s) => s,
        None => {
            // Daemon unavailable, fall back to real inotify
            return call_real_inotify_init1(flags);
        }
    };

    // Register with daemon
    let response = match send_request(&mut stream, &Request::RegisterClient) {
        Some(r) => r,
        None => {
            set_errno(libc::EIO);
            return -1;
        }
    };

    // Check response
    match response {
        Response::ClientRegistered { .. } => {
            // Get the socket's file descriptor
            use std::os::unix::io::AsRawFd;
            let fd = stream.as_raw_fd();

            // Apply flags
            // SAFETY: fd is valid and fcntl is safe to call
            if flags & libc::O_NONBLOCK != 0 {
                let current = unsafe { libc::fcntl(fd, libc::F_GETFL) };
                unsafe { libc::fcntl(fd, libc::F_SETFL, current | libc::O_NONBLOCK) };
            }
            if flags & libc::O_CLOEXEC != 0 {
                unsafe { libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC) };
            }

            // Register this fd as managed by us
            register_fd(fd);

            // Leak the stream so the fd stays open
            // The fd will be closed when the app calls close()
            std::mem::forget(stream);

            fd
        }
        Response::Error { message } => {
            // Log error if possible, but don't panic
            let _ = message;
            set_errno(libc::EIO);
            -1
        }
        _ => {
            set_errno(libc::EIO);
            -1
        }
    }
}

/// Call the real inotify_init1 (or init if init1 unavailable)
fn call_real_inotify_init1(flags: c_int) -> c_int {
    // SAFETY: We're calling the original libc functions with valid arguments
    unsafe {
        if let Some(f) = REAL_INOTIFY_INIT1 {
            f(flags)
        } else if let Some(f) = REAL_INOTIFY_INIT {
            f()
        } else {
            set_errno(libc::ENOSYS);
            -1
        }
    }
}

/// Intercepted inotify_add_watch()
///
/// If the fd is one of ours, send AddWatch to daemon.
/// Otherwise, call the real inotify_add_watch.
///
/// # Safety
///
/// This function is called by libc as a replacement for inotify_add_watch.
/// The pathname must be a valid C string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn inotify_add_watch(fd: c_int, pathname: *const c_char, mask: u32) -> c_int {
    std::panic::catch_unwind(|| {
        // Check if this is our fd
        if !is_managed_fd(fd) {
            // Not ours, call real function
            // SAFETY: Passing through to original function
            unsafe {
                if let Some(f) = REAL_INOTIFY_ADD_WATCH {
                    return f(fd, pathname, mask);
                } else {
                    set_errno(libc::ENOSYS);
                    return -1;
                }
            }
        }

        // Convert pathname to Rust string
        // SAFETY: Caller guarantees pathname is a valid C string
        let path = match unsafe { CStr::from_ptr(pathname) }.to_str() {
            Ok(s) => PathBuf::from(s),
            Err(_) => {
                set_errno(libc::EINVAL);
                return -1;
            }
        };

        // Create a temporary stream from the fd
        // SAFETY: fd is a valid socket fd that we own
        use std::os::unix::io::FromRawFd;
        let mut stream = unsafe { UnixStream::from_raw_fd(fd) };

        // Send the request
        let result = send_request(&mut stream, &Request::AddWatch { path, mask });

        // Don't let stream drop close the fd
        std::mem::forget(stream);

        match result {
            Some(Response::WatchAdded { wd }) => wd,
            Some(Response::Error { .. }) => {
                set_errno(libc::EINVAL);
                -1
            }
            _ => {
                set_errno(libc::EIO);
                -1
            }
        }
    })
    .unwrap_or_else(|_| {
        set_errno(libc::EIO);
        -1
    })
}

/// Intercepted inotify_rm_watch()
///
/// If the fd is one of ours, send RemoveWatch to daemon.
/// Otherwise, call the real inotify_rm_watch.
///
/// # Safety
///
/// This function is called by libc as a replacement for inotify_rm_watch.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn inotify_rm_watch(fd: c_int, wd: c_int) -> c_int {
    std::panic::catch_unwind(|| {
        // Check if this is our fd
        if !is_managed_fd(fd) {
            // Not ours, call real function
            // SAFETY: Passing through to original function
            unsafe {
                if let Some(f) = REAL_INOTIFY_RM_WATCH {
                    return f(fd, wd);
                } else {
                    set_errno(libc::ENOSYS);
                    return -1;
                }
            }
        }

        // Create a temporary stream from the fd
        // SAFETY: fd is a valid socket fd that we own
        use std::os::unix::io::FromRawFd;
        let mut stream = unsafe { UnixStream::from_raw_fd(fd) };

        // Send the request
        let result = send_request(&mut stream, &Request::RemoveWatch { wd });

        // Don't let stream drop close the fd
        std::mem::forget(stream);

        match result {
            Some(Response::WatchRemoved) => 0,
            Some(Response::Error { .. }) => {
                set_errno(libc::EINVAL);
                -1
            }
            _ => {
                set_errno(libc::EIO);
                -1
            }
        }
    })
    .unwrap_or_else(|_| {
        set_errno(libc::EIO);
        -1
    })
}

/// Intercepted close()
///
/// If the fd is one of ours, clean up our state.
/// Always call the real close.
///
/// # Safety
///
/// This function is called by libc as a replacement for close.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn close(fd: c_int) -> c_int {
    std::panic::catch_unwind(|| {
        // Check if this is our fd and unregister it
        if is_managed_fd(fd) {
            // Just unregister - no need to send anything to daemon,
            // it will detect the disconnect
            unregister_fd(fd);
        }

        // Always call real close
        // SAFETY: Calling original close with valid fd
        unsafe {
            if let Some(f) = REAL_CLOSE {
                f(fd)
            } else {
                // Last resort: use syscall directly
                libc::syscall(libc::SYS_close, fd as libc::c_long) as c_int
            }
        }
    })
    .unwrap_or_else(|_| {
        // Even on panic, try to close the fd
        // SAFETY: syscall is the most direct way to close
        unsafe { libc::syscall(libc::SYS_close, fd as libc::c_long) as c_int }
    })
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_managed_fds() {
        // Initialize the set
        *MANAGED_FDS.write() = Some(HashSet::new());

        assert!(!is_managed_fd(42));

        register_fd(42);
        assert!(is_managed_fd(42));

        unregister_fd(42);
        assert!(!is_managed_fd(42));
    }

    #[test]
    fn test_socket_path_uses_xdg() {
        // SAFETY: Tests run serially and we restore the env vars
        unsafe {
            std::env::remove_var("FAKENOTIFY_SOCKET");
            std::env::set_var("XDG_RUNTIME_DIR", "/run/user/1000");
        }

        let path = get_socket_path();
        assert_eq!(path, PathBuf::from("/run/user/1000/fakenotify.sock"));

        // Clean up
        // SAFETY: Tests run serially
        unsafe {
            std::env::remove_var("XDG_RUNTIME_DIR");
        }
    }

    #[test]
    fn test_socket_path_env_override() {
        // SAFETY: Tests run serially and we restore the env vars
        unsafe {
            std::env::set_var("FAKENOTIFY_SOCKET", "/tmp/test.sock");
        }

        let path = get_socket_path();
        assert_eq!(path, PathBuf::from("/tmp/test.sock"));

        // Clean up
        // SAFETY: Tests run serially
        unsafe {
            std::env::remove_var("FAKENOTIFY_SOCKET");
        }
    }
}
