//! Socket path helpers for the FakeNotify IPC.

use std::path::PathBuf;

/// Default socket path for the FakeNotify daemon.
pub const DEFAULT_SOCKET_PATH: &str = "/run/fakenotify/fakenotify.sock";

/// Environment variable to override the socket path.
pub const SOCKET_ENV_VAR: &str = "FAKENOTIFY_SOCKET";

/// Get the socket path to use for IPC.
///
/// Checks the `FAKENOTIFY_SOCKET` environment variable first,
/// falling back to the default path `/run/fakenotify/fakenotify.sock`.
#[must_use]
pub fn get_socket_path() -> PathBuf {
    std::env::var(SOCKET_ENV_VAR)
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from(DEFAULT_SOCKET_PATH))
}

/// Get the socket path, using XDG_RUNTIME_DIR as fallback.
///
/// Resolution order:
/// 1. `FAKENOTIFY_SOCKET` environment variable
/// 2. `$XDG_RUNTIME_DIR/fakenotify.sock` (if XDG_RUNTIME_DIR is set)
/// 3. Default: `/run/fakenotify/fakenotify.sock`
///
/// This is useful for unprivileged users who cannot write to `/run`.
#[must_use]
pub fn get_socket_path_with_xdg_fallback() -> PathBuf {
    // Check explicit override first
    if let Ok(path) = std::env::var(SOCKET_ENV_VAR) {
        return PathBuf::from(path);
    }

    // Try XDG_RUNTIME_DIR (typically /run/user/<uid>)
    if let Ok(runtime_dir) = std::env::var("XDG_RUNTIME_DIR") {
        return PathBuf::from(runtime_dir).join("fakenotify.sock");
    }

    // Default
    PathBuf::from(DEFAULT_SOCKET_PATH)
}

#[cfg(test)]
mod tests {
    use super::*;

    // Note: In Rust 2024 edition, set_var and remove_var are unsafe because
    // they can cause data races in multi-threaded programs. For tests that
    // need to modify environment variables, we use unsafe blocks and run
    // with --test-threads=1 or accept that the tests are checking basic
    // functionality without actual env manipulation.

    #[test]
    fn test_default_socket_path_constant() {
        // Test that the default path constant is correct
        assert_eq!(DEFAULT_SOCKET_PATH, "/run/fakenotify/fakenotify.sock");
    }

    #[test]
    fn test_socket_env_var_constant() {
        // Test that the env var name is correct
        assert_eq!(SOCKET_ENV_VAR, "FAKENOTIFY_SOCKET");
    }

    #[test]
    fn test_get_socket_path_returns_path() {
        // Test that the function returns a valid PathBuf
        let path = get_socket_path();
        // Either it's the default or from env, but it should be a valid path
        assert!(!path.as_os_str().is_empty());
    }

    #[test]
    fn test_get_socket_path_with_xdg_fallback_returns_path() {
        // Test that the function returns a valid PathBuf
        let path = get_socket_path_with_xdg_fallback();
        assert!(!path.as_os_str().is_empty());
    }

    // Tests that require unsafe env manipulation - only run if explicitly requested
    #[test]
    #[ignore = "requires unsafe env manipulation, run with --ignored"]
    fn test_socket_path_override() {
        let custom_path = "/tmp/test-fakenotify.sock";

        // SAFETY: Test is run in isolation with --test-threads=1
        unsafe {
            std::env::set_var(SOCKET_ENV_VAR, custom_path);
        }

        let path = get_socket_path();
        assert_eq!(path, PathBuf::from(custom_path));

        // Clean up
        // SAFETY: Test is run in isolation with --test-threads=1
        unsafe {
            std::env::remove_var(SOCKET_ENV_VAR);
        }
    }

    #[test]
    #[ignore = "requires unsafe env manipulation, run with --ignored"]
    fn test_socket_path_xdg_fallback() {
        // SAFETY: Test is run in isolation with --test-threads=1
        unsafe {
            std::env::remove_var(SOCKET_ENV_VAR);
            std::env::set_var("XDG_RUNTIME_DIR", "/run/user/1000");
        }

        let path = get_socket_path_with_xdg_fallback();
        assert_eq!(path, PathBuf::from("/run/user/1000/fakenotify.sock"));

        // Clean up
        // SAFETY: Test is run in isolation with --test-threads=1
        unsafe {
            std::env::remove_var("XDG_RUNTIME_DIR");
        }
    }

    #[test]
    #[ignore = "requires unsafe env manipulation, run with --ignored"]
    fn test_socket_path_override_takes_precedence() {
        let custom_path = "/custom/path.sock";

        // SAFETY: Test is run in isolation with --test-threads=1
        unsafe {
            std::env::set_var(SOCKET_ENV_VAR, custom_path);
            std::env::set_var("XDG_RUNTIME_DIR", "/run/user/1000");
        }

        let path = get_socket_path_with_xdg_fallback();
        assert_eq!(path, PathBuf::from(custom_path));

        // Clean up
        // SAFETY: Test is run in isolation with --test-threads=1
        unsafe {
            std::env::remove_var(SOCKET_ENV_VAR);
            std::env::remove_var("XDG_RUNTIME_DIR");
        }
    }
}
