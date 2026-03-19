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
pub(crate) struct ContainerListEntry {
    #[serde(rename = "Id")]
    pub(crate) id: String,
    #[serde(rename = "Ports")]
    pub(crate) ports: Vec<PortEntry>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct PortEntry {
    #[serde(rename = "PublicPort")]
    pub(crate) public_port: Option<u16>,
    #[serde(rename = "PrivatePort")]
    pub(crate) private_port: u16,
    #[serde(rename = "Type")]
    pub(crate) port_type: String,
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

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE_CONTAINERS_JSON: &str = r#"[
        {
            "Id": "abc123",
            "Ports": [
                {"PrivatePort": 80, "PublicPort": 8080, "Type": "tcp"},
                {"PrivatePort": 443, "Type": "tcp"}
            ]
        },
        {
            "Id": "def456",
            "Ports": [
                {"PrivatePort": 6379, "PublicPort": 6379, "Type": "tcp"},
                {"PrivatePort": 9999, "PublicPort": 9999, "Type": "udp"}
            ]
        },
        {
            "Id": "ghi789",
            "Ports": []
        }
    ]"#;

    #[test]
    fn deserialize_container_list() {
        let containers: Vec<ContainerListEntry> =
            serde_json::from_str(SAMPLE_CONTAINERS_JSON).unwrap();
        assert_eq!(containers.len(), 3);
        assert_eq!(containers[0].id, "abc123");
        assert_eq!(containers[0].ports.len(), 2);
    }

    #[test]
    fn port_with_public_port() {
        let containers: Vec<ContainerListEntry> =
            serde_json::from_str(SAMPLE_CONTAINERS_JSON).unwrap();
        let port = &containers[0].ports[0];
        assert_eq!(port.public_port, Some(8080));
        assert_eq!(port.private_port, 80);
        assert_eq!(port.port_type, "tcp");
    }

    #[test]
    fn port_without_public_port() {
        let containers: Vec<ContainerListEntry> =
            serde_json::from_str(SAMPLE_CONTAINERS_JSON).unwrap();
        let port = &containers[0].ports[1];
        assert!(port.public_port.is_none());
        assert_eq!(port.private_port, 443);
    }

    #[test]
    fn filter_tcp_ports_with_public() {
        let containers: Vec<ContainerListEntry> =
            serde_json::from_str(SAMPLE_CONTAINERS_JSON).unwrap();

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

        // abc123: 8080->80 (tcp, has public) -- 443 filtered (no public)
        assert_eq!(desired.get("abc123").unwrap(), &[(8080, 80)]);
        // def456: 6379->6379 (tcp) -- 9999 filtered (udp)
        assert_eq!(desired.get("def456").unwrap(), &[(6379, 6379)]);
        // ghi789: no ports at all
        assert!(!desired.contains_key("ghi789"));
    }

    #[test]
    fn empty_containers_json() {
        let containers: Vec<ContainerListEntry> = serde_json::from_str("[]").unwrap();
        assert!(containers.is_empty());
    }

    #[test]
    fn multiple_tcp_ports_one_container() {
        let json = r#"[{
            "Id": "multi",
            "Ports": [
                {"PrivatePort": 80, "PublicPort": 8080, "Type": "tcp"},
                {"PrivatePort": 443, "PublicPort": 8443, "Type": "tcp"}
            ]
        }]"#;
        let containers: Vec<ContainerListEntry> = serde_json::from_str(json).unwrap();
        let ports: Vec<_> = containers[0]
            .ports
            .iter()
            .filter(|p| p.public_port.is_some() && p.port_type == "tcp")
            .collect();
        assert_eq!(ports.len(), 2);
    }
}
