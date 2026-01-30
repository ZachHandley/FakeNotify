//! Unix domain socket server for client connections.
//!
//! Handles client requests and manages client lifecycle.

use crate::state::{ClientId, DaemonState};
use fakenotify_protocol::{EventMask, FramedMessage, Request, Response};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::broadcast;

/// Socket server for handling client connections
pub struct Server {
    /// Path to the Unix socket
    socket_path: PathBuf,
    /// Shared daemon state
    state: Arc<DaemonState>,
    /// Shutdown signal receiver
    shutdown_rx: broadcast::Receiver<()>,
}

impl Server {
    /// Create a new server
    pub fn new(
        socket_path: PathBuf,
        state: Arc<DaemonState>,
        shutdown_rx: broadcast::Receiver<()>,
    ) -> Self {
        Self {
            socket_path,
            state,
            shutdown_rx,
        }
    }

    /// Run the server
    pub async fn run(mut self) -> color_eyre::Result<()> {
        // Remove existing socket file if present
        if self.socket_path.exists() {
            std::fs::remove_file(&self.socket_path)?;
        }

        // Create parent directory if needed
        if let Some(parent) = self.socket_path.parent() {
            if !parent.exists() {
                std::fs::create_dir_all(parent)?;
            }
        }

        // Bind the socket
        let listener = UnixListener::bind(&self.socket_path)?;
        tracing::info!(socket = %self.socket_path.display(), "Server listening");

        // Set socket permissions (allow all users to connect)
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let permissions = std::fs::Permissions::from_mode(0o666);
            std::fs::set_permissions(&self.socket_path, permissions)?;
        }

        loop {
            tokio::select! {
                accept_result = listener.accept() => {
                    match accept_result {
                        Ok((stream, _addr)) => {
                            let state = Arc::clone(&self.state);
                            let shutdown_rx = self.shutdown_rx.resubscribe();
                            tokio::spawn(async move {
                                if let Err(e) = handle_client(stream, state, shutdown_rx).await {
                                    tracing::error!(error = %e, "Client handler error");
                                }
                            });
                        }
                        Err(e) => {
                            tracing::error!(error = %e, "Accept error");
                        }
                    }
                }
                _ = self.shutdown_rx.recv() => {
                    tracing::info!("Server shutting down");
                    break;
                }
            }
        }

        // Clean up socket file
        if self.socket_path.exists() {
            let _ = std::fs::remove_file(&self.socket_path);
        }

        Ok(())
    }
}

/// Handle a single client connection
async fn handle_client(
    stream: UnixStream,
    state: Arc<DaemonState>,
    mut shutdown_rx: broadcast::Receiver<()>,
) -> color_eyre::Result<()> {
    let (read_half, write_half) = stream.into_split();

    // Register the client
    let client = state.register_client(write_half);
    let client_id = client.id;

    // Send registration response
    let response = Response::ClientRegistered { client_id };
    send_response(&client, &response).await?;

    // Read loop
    let mut reader = tokio::io::BufReader::new(read_half);
    let mut len_buf = [0u8; 4];

    loop {
        tokio::select! {
            read_result = reader.read_exact(&mut len_buf) => {
                match read_result {
                    Ok(_) => {
                        let len = u32::from_le_bytes(len_buf) as usize;

                        // Sanity check message size
                        if len > FramedMessage::MAX_SIZE {
                            tracing::warn!(client_id = client_id, len = len, "Message too large");
                            break;
                        }

                        // Read the message payload
                        let mut payload = vec![0u8; len];
                        if reader.read_exact(&mut payload).await.is_err() {
                            break;
                        }

                        // Parse and handle the request
                        match Request::from_bytes(&payload) {
                            Ok(request) => {
                                let response = handle_request(&state, client_id, request).await;
                                if let Err(e) = send_response(&client, &response).await {
                                    tracing::error!(
                                        client_id = client_id,
                                        error = %e,
                                        "Failed to send response"
                                    );
                                    break;
                                }
                            }
                            Err(e) => {
                                tracing::warn!(
                                    client_id = client_id,
                                    error = %e,
                                    "Invalid request"
                                );
                                let response = Response::Error {
                                    message: format!("Invalid request: {}", e),
                                };
                                let _ = send_response(&client, &response).await;
                            }
                        }
                    }
                    Err(_) => {
                        // Client disconnected
                        break;
                    }
                }
            }
            _ = shutdown_rx.recv() => {
                tracing::debug!(client_id = client_id, "Client handler received shutdown signal");
                break;
            }
        }
    }

    // Unregister the client
    state.unregister_client(client_id);

    Ok(())
}

/// Handle a single request
async fn handle_request(state: &DaemonState, client_id: ClientId, request: Request) -> Response {
    match request {
        Request::RegisterClient => {
            // Already registered during connection
            Response::ClientRegistered { client_id }
        }

        Request::AddWatch { path, mask } => {
            let event_mask = EventMask::from_bits_truncate(mask);

            // Validate path exists
            if !path.exists() {
                return Response::Error {
                    message: format!("Path does not exist: {}", path.display()),
                };
            }

            let wd = state.add_watch(client_id, path, event_mask, true);
            Response::WatchAdded { wd }
        }

        Request::RemoveWatch { wd } => {
            if state.remove_watch(client_id, wd) {
                Response::WatchRemoved
            } else {
                Response::Error {
                    message: format!("Watch descriptor {} not found", wd),
                }
            }
        }

        Request::Ping => Response::Pong,
    }
}

/// Send a response to a client
async fn send_response(
    client: &crate::state::Client,
    response: &Response,
) -> color_eyre::Result<()> {
    let payload = response.to_bytes()?;
    let framed = FramedMessage::frame(&payload);
    client.send_event(&framed).await?;
    Ok(())
}

/// Check if the daemon is running by attempting to connect to the socket
pub async fn is_daemon_running(socket_path: &Path) -> bool {
    UnixStream::connect(socket_path).await.is_ok()
}

/// Send a request to the daemon and receive a response
pub async fn send_daemon_request(
    socket_path: &Path,
    request: Request,
) -> color_eyre::Result<Response> {
    let mut stream = UnixStream::connect(socket_path).await?;

    // Read the initial ClientRegistered response
    let mut len_buf = [0u8; 4];
    stream.read_exact(&mut len_buf).await?;
    let len = u32::from_le_bytes(len_buf) as usize;
    let mut payload = vec![0u8; len];
    stream.read_exact(&mut payload).await?;
    let _ = Response::from_bytes(&payload)?;

    // Send our request
    let request_bytes = request.to_bytes()?;
    let framed = FramedMessage::frame(&request_bytes);
    stream.write_all(&framed).await?;

    // Read the response
    stream.read_exact(&mut len_buf).await?;
    let len = u32::from_le_bytes(len_buf) as usize;
    let mut payload = vec![0u8; len];
    stream.read_exact(&mut payload).await?;

    let response = Response::from_bytes(&payload)?;
    Ok(response)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_is_daemon_running_nonexistent() {
        let result = is_daemon_running(Path::new("/nonexistent/path.sock")).await;
        assert!(!result);
    }
}
