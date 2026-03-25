use anyhow::Result;
use std::os::fd::{FromRawFd, RawFd};
use std::path::PathBuf;
use std::sync::Arc;
use tokio::net::UnixListener;
use tokio::sync::watch;
use tracing::{debug, error, info};

use crate::ffi::VmHandle;

fn set_nonblocking(fd: RawFd) {
    unsafe {
        let flags = libc::fcntl(fd, libc::F_GETFL);
        if flags >= 0 {
            libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK);
        }
    }
}

/// Proxies Docker API traffic from a Unix socket on macOS to dockerd inside
/// the VM via vsock (guest-initiated connections).
pub struct DockerSocketProxy {
    socket_path: PathBuf,
    vm_handle: Arc<VmHandle>,
    vsock_port: u32,
}

impl DockerSocketProxy {
    pub fn new(socket_path: PathBuf, vm_handle: Arc<VmHandle>, vsock_port: u32) -> Self {
        Self {
            socket_path,
            vm_handle,
            vsock_port,
        }
    }

    pub async fn run(&self, mut shutdown_rx: watch::Receiver<bool>) -> Result<()> {
        if self.socket_path.exists() {
            std::fs::remove_file(&self.socket_path)?;
        }
        if let Some(parent) = self.socket_path.parent() {
            std::fs::create_dir_all(parent).ok();
        }

        let listener = UnixListener::bind(&self.socket_path)?;
        info!(
            path = %self.socket_path.display(),
            vsock_port = self.vsock_port,
            "docker socket proxy listening"
        );

        loop {
            tokio::select! {
                result = listener.accept() => {
                    let (client_stream, _) = result?;
                    debug!("accepted docker client connection, waiting for guest vsock...");
                    let vm_handle = self.vm_handle.clone();
                    let port = self.vsock_port;
                    tokio::spawn(async move {
                        if let Err(e) = proxy_via_vsock(client_stream, vm_handle, port).await {
                            error!("proxy error: {e:#}");
                        }
                    });
                }
                _ = shutdown_rx.changed() => {
                    if *shutdown_rx.borrow() {
                        info!("proxy shutting down");
                        break;
                    }
                }
            }
        }
        Ok(())
    }
}

async fn proxy_via_vsock(
    client: tokio::net::UnixStream,
    vm_handle: Arc<VmHandle>,
    port: u32,
) -> Result<()> {
    let fd = tokio::task::spawn_blocking(move || vm_handle.vsock_accept(port)).await??;
    info!(fd, "accepted guest vsock connection");

    // Set non-blocking for tokio async I/O
    set_nonblocking(fd);

    // Wrap the vsock fd as a UnixStream -- vsock is SOCK_STREAM so kqueue handles it fine
    let vsock_std = unsafe { std::os::unix::net::UnixStream::from_raw_fd(fd) };
    let mut vsock = tokio::net::UnixStream::from_std(vsock_std)?;

    let mut client = client;

    let result = tokio::io::copy_bidirectional(&mut client, &mut vsock).await;
    debug!(?result, "proxy session ended");

    Ok(())
}
