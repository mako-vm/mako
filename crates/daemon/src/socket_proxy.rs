use anyhow::Result;
use std::io::{Read, Write};
use std::os::fd::{AsRawFd, FromRawFd};
use std::path::PathBuf;
use std::sync::Arc;
use tokio::net::UnixListener;
use tokio::sync::watch;
use tracing::{debug, error, info};

use crate::ffi::VmHandle;

fn set_blocking(fd: i32) {
    unsafe {
        let flags = libc::fcntl(fd, libc::F_GETFL);
        if flags >= 0 && (flags & libc::O_NONBLOCK) != 0 {
            libc::fcntl(fd, libc::F_SETFL, flags & !libc::O_NONBLOCK);
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
    // Accept the next guest-initiated vsock connection (blocks until one arrives)
    let fd = tokio::task::spawn_blocking(move || vm_handle.vsock_accept(port)).await??;
    info!(fd, "accepted guest vsock connection");

    // Check fd type and force blocking mode
    unsafe {
        let mut stat: libc::stat = std::mem::zeroed();
        libc::fstat(fd, &mut stat);
        debug!(fd, mode = format!("{:#o}", stat.st_mode), "vsock fd fstat");

        let mut sock_type: libc::c_int = 0;
        let mut len: libc::socklen_t = std::mem::size_of::<libc::c_int>() as libc::socklen_t;
        let r = libc::getsockopt(
            fd,
            libc::SOL_SOCKET,
            libc::SO_TYPE,
            &mut sock_type as *mut _ as *mut libc::c_void,
            &mut len,
        );
        debug!(fd, r, sock_type, "vsock fd SO_TYPE");
    }

    set_blocking(fd);

    let client_std = client.into_std()?;
    set_blocking(client_std.as_raw_fd());

    let vsock_fd = fd;
    let client_fd = client_std.as_raw_fd();

    let vsock_read = unsafe { std::fs::File::from_raw_fd(vsock_fd) };
    let vsock_write = vsock_read.try_clone()?;
    let client_read = client_std.try_clone()?;
    let mut client_write = client_std;

    let mut vw = vsock_write;
    let mut cr = client_read;
    let vsock_fd_c2v = vsock_fd;
    let c2v = std::thread::spawn(move || -> std::io::Result<u64> {
        let mut buf = [0u8; 65536];
        let mut total = 0u64;
        loop {
            let n = cr.read(&mut buf)?;
            if n == 0 {
                break;
            }
            total += n as u64;
            vw.write_all(&buf[..n])?;
        }
        unsafe {
            libc::shutdown(vsock_fd_c2v, libc::SHUT_WR);
        }
        Ok(total)
    });

    let mut vr = vsock_read;
    let client_fd_v2c = client_fd;
    let c2c = std::thread::spawn(move || -> std::io::Result<u64> {
        let mut buf = [0u8; 65536];
        let mut total = 0u64;
        loop {
            let n = vr.read(&mut buf)?;
            if n == 0 {
                break;
            }
            total += n as u64;
            client_write.write_all(&buf[..n])?;
        }
        unsafe {
            libc::shutdown(client_fd_v2c, libc::SHUT_WR);
        }
        Ok(total)
    });

    let r1 = c2v
        .join()
        .map_err(|_| anyhow::anyhow!("c2v thread panicked"));
    let r2 = c2c
        .join()
        .map_err(|_| anyhow::anyhow!("v2c thread panicked"));
    debug!(?r1, ?r2, "proxy session ended");

    Ok(())
}
