use anyhow::Result;
use serde::Deserialize;
use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::Arc;
use tokio::sync::watch;
use tracing::{debug, info, warn};

use crate::ffi::VmHandle;

#[derive(Debug, Deserialize)]
struct ContainerListEntry {
    #[serde(rename = "Id")]
    id: String,
    #[serde(rename = "Ports")]
    ports: Vec<PortEntry>,
}

#[derive(Debug, Deserialize)]
struct PortEntry {
    #[serde(rename = "PublicPort")]
    public_port: Option<u16>,
    #[serde(rename = "PrivatePort")]
    private_port: u16,
    #[serde(rename = "Type")]
    port_type: String,
}

struct ActiveForward {
    host_port: u16,
    shutdown_tx: watch::Sender<bool>,
}

pub struct PortForwarder {
    #[allow(dead_code)]
    vm_handle: Arc<VmHandle>,
    vm_ip: Arc<tokio::sync::RwLock<Option<String>>>,
    socket_path: std::path::PathBuf,
    forwards: tokio::sync::Mutex<HashMap<String, Vec<ActiveForward>>>,
}

impl PortForwarder {
    pub fn new(
        vm_handle: Arc<VmHandle>,
        vm_ip: Arc<tokio::sync::RwLock<Option<String>>>,
        socket_path: std::path::PathBuf,
    ) -> Arc<Self> {
        Arc::new(Self {
            vm_handle,
            vm_ip,
            socket_path,
            forwards: tokio::sync::Mutex::new(HashMap::new()),
        })
    }

    pub async fn run(self: Arc<Self>, mut shutdown_rx: watch::Receiver<bool>) {
        info!("port forwarder: starting, polling every 2s");

        let mut interval = tokio::time::interval(std::time::Duration::from_secs(2));

        loop {
            tokio::select! {
                _ = interval.tick() => {
                    if let Err(e) = self.sync_forwards().await {
                        debug!(?e, "port forward sync error");
                    }
                }
                _ = shutdown_rx.changed() => {
                    if *shutdown_rx.borrow() {
                        info!("port forwarder: shutting down");
                        self.stop_all().await;
                        break;
                    }
                }
            }
        }
    }

    async fn sync_forwards(&self) -> Result<()> {
        let containers = self.list_containers().await?;
        let mut desired: HashMap<String, Vec<(u16, u16)>> = HashMap::new();

        for c in &containers {
            let mut port_pairs = Vec::new();
            for p in &c.ports {
                if let Some(pub_port) = p.public_port {
                    if p.port_type == "tcp" {
                        port_pairs.push((pub_port, p.private_port));
                    }
                }
            }
            if !port_pairs.is_empty() {
                desired.insert(c.id.clone(), port_pairs);
            }
        }

        let mut forwards = self.forwards.lock().await;

        // Remove forwards for containers that no longer need them
        let stale_ids: Vec<String> = forwards
            .keys()
            .filter(|id| !desired.contains_key(*id))
            .cloned()
            .collect();
        for id in stale_ids {
            if let Some(fwds) = forwards.remove(&id) {
                for fwd in fwds {
                    info!(host_port = fwd.host_port, "port forward: removing");
                    let _ = fwd.shutdown_tx.send(true);
                }
            }
        }

        // Add forwards for new containers
        let vm_ip = self.vm_ip.read().await.clone();
        let Some(vm_ip) = vm_ip else { return Ok(()) };

        for (id, port_pairs) in &desired {
            if forwards.contains_key(id) {
                continue;
            }
            let mut active = Vec::new();
            for &(host_port, _container_port) in port_pairs {
                let (tx, rx) = watch::channel(false);
                let ip = vm_ip.clone();
                info!(host_port, "port forward: adding");

                // Docker binds PublicPort on 0.0.0.0 inside the VM, so we
                // connect to vm_ip:host_port (not the container's internal port).
                let vm_port = host_port;
                tokio::spawn(async move {
                    if let Err(e) = run_tcp_forward(host_port, ip, vm_port, rx).await {
                        warn!(host_port, ?e, "port forward error");
                    }
                });

                active.push(ActiveForward {
                    host_port,
                    shutdown_tx: tx,
                });
            }
            forwards.insert(id.clone(), active);
        }

        Ok(())
    }

    async fn stop_all(&self) {
        let mut forwards = self.forwards.lock().await;
        for (_, fwds) in forwards.drain() {
            for fwd in fwds {
                let _ = fwd.shutdown_tx.send(true);
            }
        }
    }

    async fn list_containers(&self) -> Result<Vec<ContainerListEntry>> {
        let socket_path = self.socket_path.clone();
        tokio::task::spawn_blocking(move || {
            let stream = std::os::unix::net::UnixStream::connect(&socket_path)?;
            let mut stream = stream;
            let req = "GET /containers/json HTTP/1.0\r\nHost: localhost\r\n\r\n";
            stream.write_all(req.as_bytes())?;
            stream.flush()?;

            let mut response = Vec::new();
            stream.read_to_end(&mut response)?;

            let response_str = String::from_utf8_lossy(&response);
            let body = response_str.split("\r\n\r\n").nth(1).unwrap_or("[]");

            let containers: Vec<ContainerListEntry> = serde_json::from_str(body)?;
            Ok(containers)
        })
        .await?
    }
}

async fn run_tcp_forward(
    host_port: u16,
    vm_ip: String,
    container_port: u16,
    mut shutdown_rx: watch::Receiver<bool>,
) -> Result<()> {
    let addr: SocketAddr = format!("127.0.0.1:{host_port}").parse()?;
    let listener = TcpListener::bind(addr)?;
    listener.set_nonblocking(true)?;

    info!(
        host_port,
        container_port, "port forward: listening on 127.0.0.1:{host_port}"
    );

    loop {
        tokio::select! {
            _ = shutdown_rx.changed() => {
                if *shutdown_rx.borrow() {
                    debug!(host_port, "port forward: stopping listener");
                    break;
                }
            }
            result = tokio::task::spawn_blocking({
                let listener = listener.try_clone()?;
                move || -> Option<TcpStream> {
                    listener.set_nonblocking(false).ok();
                    // Short timeout so we can check shutdown
                    listener.set_nonblocking(true).ok();
                    match listener.accept() {
                        Ok((stream, _)) => Some(stream),
                        Err(_) => None,
                    }
                }
            }) => {
                if let Ok(Some(client)) = result {
                    let vm_ip = vm_ip.clone();
                    std::thread::spawn(move || {
                        if let Err(e) = forward_connection(client, &vm_ip, container_port) {
                            debug!(?e, "port forward connection error");
                        }
                    });
                } else {
                    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                }
            }
        }
    }
    Ok(())
}

fn forward_connection(client: TcpStream, vm_ip: &str, container_port: u16) -> Result<()> {
    let upstream = TcpStream::connect(format!("{vm_ip}:{container_port}"))?;

    let mut client_r = client.try_clone()?;
    let mut upstream_w = upstream.try_clone()?;
    let mut upstream_r = upstream;
    let mut client_w = client;

    let t1 = std::thread::spawn(move || -> std::io::Result<()> {
        let mut buf = [0u8; 65536];
        loop {
            let n = client_r.read(&mut buf)?;
            if n == 0 {
                break;
            }
            upstream_w.write_all(&buf[..n])?;
        }
        upstream_w.shutdown(std::net::Shutdown::Write)?;
        Ok(())
    });

    let t2 = std::thread::spawn(move || -> std::io::Result<()> {
        let mut buf = [0u8; 65536];
        loop {
            let n = upstream_r.read(&mut buf)?;
            if n == 0 {
                break;
            }
            client_w.write_all(&buf[..n])?;
        }
        client_w.shutdown(std::net::Shutdown::Write)?;
        Ok(())
    });

    let _ = t1.join();
    let _ = t2.join();
    Ok(())
}
