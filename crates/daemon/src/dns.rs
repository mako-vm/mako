use anyhow::Result;
use std::io::{Read, Write};
use std::net::UdpSocket;
use std::sync::Arc;
use tokio::sync::watch;
use tracing::{debug, info, warn};

const LISTEN_ADDR: &str = "127.0.0.1";
const MAKO_DOMAIN: &str = ".mako.local";

pub struct DnsForwarder {
    socket_path: std::path::PathBuf,
    vm_ip: Arc<tokio::sync::RwLock<Option<String>>>,
}

impl DnsForwarder {
    pub fn new(
        socket_path: std::path::PathBuf,
        vm_ip: Arc<tokio::sync::RwLock<Option<String>>>,
    ) -> Self {
        Self { socket_path, vm_ip }
    }

    pub async fn run(self, mut shutdown_rx: watch::Receiver<bool>) {
        let port = find_available_port().unwrap_or(15353);
        let bind_addr = format!("{LISTEN_ADDR}:{port}");

        let socket = match UdpSocket::bind(&bind_addr) {
            Ok(s) => {
                info!(addr = %bind_addr, "DNS forwarder listening");
                s
            }
            Err(e) => {
                warn!(?e, addr = %bind_addr, "DNS forwarder: failed to bind");
                return;
            }
        };

        socket
            .set_read_timeout(Some(std::time::Duration::from_millis(500)))
            .ok();

        // Write resolver config for macOS
        if let Err(e) = install_resolver(port) {
            warn!(?e, "DNS forwarder: could not install resolver config");
            info!(
                "To enable DNS: sudo mkdir -p /etc/resolver && echo 'nameserver 127.0.0.1\\nport {}' | sudo tee /etc/resolver/mako.local",
                port
            );
        }

        let socket_path = self.socket_path.clone();
        let vm_ip = self.vm_ip.clone();

        let handle = std::thread::spawn(move || {
            let mut buf = [0u8; 512];
            loop {
                let (n, src) = match socket.recv_from(&mut buf) {
                    Ok(r) => r,
                    Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => continue,
                    Err(_) => break,
                };

                if let Some(response) = handle_dns_query(&buf[..n], &socket_path, &vm_ip) {
                    socket.send_to(&response, src).ok();
                }
            }
        });

        // Wait for shutdown
        loop {
            if shutdown_rx.changed().await.is_ok() && *shutdown_rx.borrow() {
                break;
            }
        }

        info!("DNS forwarder: shutting down");
        remove_resolver();
        drop(handle);
    }
}

fn handle_dns_query(
    query: &[u8],
    socket_path: &std::path::Path,
    vm_ip: &Arc<tokio::sync::RwLock<Option<String>>>,
) -> Option<Vec<u8>> {
    if query.len() < 12 {
        return None;
    }

    let name = parse_dns_name(query, 12)?;
    let name_lower = name.to_lowercase();

    if !name_lower.ends_with(MAKO_DOMAIN) && name_lower != MAKO_DOMAIN.trim_start_matches('.') {
        return None;
    }

    // Extract container name: <name>.mako.local -> <name>
    let container_name = name_lower.strip_suffix(MAKO_DOMAIN)?.to_string();

    if container_name.is_empty() {
        return None;
    }

    debug!(container = %container_name, "DNS query for container");

    // Special case: "vm.mako.local" returns the VM IP
    if container_name == "vm" {
        let ip = {
            let rt = tokio::runtime::Handle::try_current().ok()?;
            rt.block_on(async { vm_ip.read().await.clone() })?
        };
        return build_dns_response(query, &ip);
    }

    // Look up container IP via Docker API
    let ip = lookup_container_ip(socket_path, &container_name)?;
    build_dns_response(query, &ip)
}

fn lookup_container_ip(socket_path: &std::path::Path, name: &str) -> Option<String> {
    let mut stream = std::os::unix::net::UnixStream::connect(socket_path).ok()?;
    let req = format!(
        "GET /containers/{}/json HTTP/1.0\r\nHost: localhost\r\n\r\n",
        name
    );
    stream.write_all(req.as_bytes()).ok()?;
    stream.flush().ok()?;

    let mut response = Vec::new();
    stream.read_to_end(&mut response).ok()?;

    let response_str = String::from_utf8_lossy(&response);
    let body = response_str.split("\r\n\r\n").nth(1)?;

    let json: serde_json::Value = serde_json::from_str(body).ok()?;
    let networks = json.get("NetworkSettings")?.get("Networks")?.as_object()?;

    // Return the first network's IP
    for (_net_name, net_info) in networks {
        if let Some(ip) = net_info.get("IPAddress").and_then(|v| v.as_str()) {
            if !ip.is_empty() {
                return Some(ip.to_string());
            }
        }
    }
    None
}

fn parse_dns_name(packet: &[u8], offset: usize) -> Option<String> {
    let mut parts = Vec::new();
    let mut pos = offset;
    loop {
        if pos >= packet.len() {
            return None;
        }
        let len = packet[pos] as usize;
        if len == 0 {
            break;
        }
        if len >= 0xC0 {
            // Pointer, not supported in queries typically
            return None;
        }
        pos += 1;
        if pos + len > packet.len() {
            return None;
        }
        parts.push(String::from_utf8_lossy(&packet[pos..pos + len]).to_string());
        pos += len;
    }
    Some(parts.join("."))
}

fn build_dns_response(query: &[u8], ip: &str) -> Option<Vec<u8>> {
    let parts: Vec<u8> = ip.split('.').filter_map(|p| p.parse::<u8>().ok()).collect();
    if parts.len() != 4 {
        return None;
    }

    // Find end of question section
    let mut pos = 12;
    while pos < query.len() && query[pos] != 0 {
        let len = query[pos] as usize;
        pos += 1 + len;
    }
    pos += 1; // null terminator
    pos += 4; // QTYPE + QCLASS

    let question = &query[12..pos];

    let mut response = Vec::with_capacity(pos + 16);
    // Header
    response.extend_from_slice(&query[0..2]); // ID
    response.extend_from_slice(&[0x81, 0x80]); // Flags: response, recursion available
    response.extend_from_slice(&query[4..6]); // QDCOUNT
    response.extend_from_slice(&[0x00, 0x01]); // ANCOUNT = 1
    response.extend_from_slice(&[0x00, 0x00]); // NSCOUNT
    response.extend_from_slice(&[0x00, 0x00]); // ARCOUNT

    // Question section (echo back)
    response.extend_from_slice(question);

    // Answer: pointer to name in question, type A, class IN, TTL 5s, 4 bytes
    response.extend_from_slice(&[0xC0, 0x0C]); // Name pointer to offset 12
    response.extend_from_slice(&[0x00, 0x01]); // Type A
    response.extend_from_slice(&[0x00, 0x01]); // Class IN
    response.extend_from_slice(&[0x00, 0x00, 0x00, 0x05]); // TTL 5s
    response.extend_from_slice(&[0x00, 0x04]); // RDLENGTH 4
    response.extend_from_slice(&parts); // IP address

    Some(response)
}

fn find_available_port() -> Option<u16> {
    // Try the preferred port first, then fall back
    for port in [15353, 15354, 15355] {
        if UdpSocket::bind(format!("{LISTEN_ADDR}:{port}")).is_ok() {
            return Some(port);
        }
    }
    None
}

fn install_resolver(port: u16) -> Result<()> {
    let resolver_dir = "/etc/resolver";
    let resolver_file = format!("{resolver_dir}/mako.local");
    let content = format!("nameserver 127.0.0.1\nport {port}\n");

    // Try to create via sudo
    let status = std::process::Command::new("sudo")
        .args(["-n", "mkdir", "-p", resolver_dir])
        .output();

    if let Ok(output) = status {
        if output.status.success() {
            let status = std::process::Command::new("sudo")
                .args(["-n", "tee", &resolver_file])
                .stdin(std::process::Stdio::piped())
                .stdout(std::process::Stdio::null())
                .spawn()
                .and_then(|mut child| {
                    child
                        .stdin
                        .as_mut()
                        .unwrap()
                        .write_all(content.as_bytes())?;
                    child.wait()
                });

            if let Ok(s) = status {
                if s.success() {
                    info!(file = %resolver_file, "installed macOS resolver for mako.local");
                    return Ok(());
                }
            }
        }
    }

    anyhow::bail!("could not install resolver (needs sudo without password, or manual setup)")
}

fn remove_resolver() {
    let _ = std::process::Command::new("sudo")
        .args(["-n", "rm", "-f", "/etc/resolver/mako.local"])
        .output();
}
