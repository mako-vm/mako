use anyhow::Result;
use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, UdpSocket};
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::{watch, RwLock};
use tracing::{debug, info, warn};

const MAKO_DOMAIN: &str = ".mako.local";
const HOST_DNS_PORT: u16 = 15353;
const VM_DNS_PORT: u16 = 10053;
const DNS_CACHE_TTL_SECS: u64 = 30;
const DNS_CACHE_MAX_ENTRIES: usize = 512;

struct DnsCache {
    entries: HashMap<(String, u16), (Vec<u8>, Instant)>,
}

impl DnsCache {
    fn new() -> Self {
        Self {
            entries: HashMap::new(),
        }
    }

    fn get(&self, name: &str, qtype: u16) -> Option<&Vec<u8>> {
        let key = (name.to_lowercase(), qtype);
        if let Some((response, inserted)) = self.entries.get(&key) {
            if inserted.elapsed().as_secs() < DNS_CACHE_TTL_SECS {
                return Some(response);
            }
        }
        None
    }

    fn put(&mut self, name: &str, qtype: u16, response: Vec<u8>) {
        if self.entries.len() >= DNS_CACHE_MAX_ENTRIES {
            self.entries
                .retain(|_, (_, t)| t.elapsed().as_secs() < DNS_CACHE_TTL_SECS);
            if self.entries.len() >= DNS_CACHE_MAX_ENTRIES {
                self.entries.clear();
            }
        }
        self.entries
            .insert((name.to_lowercase(), qtype), (response, Instant::now()));
    }

    fn rewrite_txid(cached: &[u8], query: &[u8]) -> Option<Vec<u8>> {
        if cached.len() < 2 || query.len() < 2 {
            return None;
        }
        let mut out = cached.to_vec();
        out[0] = query[0];
        out[1] = query[1];
        Some(out)
    }
}

const QTYPE_A: u16 = 1;
const QTYPE_AAAA: u16 = 28;

pub struct DnsForwarder {
    socket_path: std::path::PathBuf,
    vm_ip: Arc<RwLock<Option<String>>>,
    vm_gateway: Arc<RwLock<Option<String>>>,
}

impl DnsForwarder {
    pub fn new(
        socket_path: std::path::PathBuf,
        vm_ip: Arc<RwLock<Option<String>>>,
        vm_gateway: Arc<RwLock<Option<String>>>,
    ) -> Self {
        Self {
            socket_path,
            vm_ip,
            vm_gateway,
        }
    }

    pub async fn run(self, mut shutdown_rx: watch::Receiver<bool>) {
        // Host-facing listener: only handles *.mako.local on loopback
        let host_bind = format!("127.0.0.1:{HOST_DNS_PORT}");
        let host_socket = match UdpSocket::bind(&host_bind) {
            Ok(s) => {
                info!(addr = %host_bind, "DNS forwarder (host) listening");
                Some(s)
            }
            Err(e) => {
                warn!(?e, addr = %host_bind, "DNS forwarder (host): failed to bind");
                None
            }
        };

        if host_socket.is_some() {
            if let Err(e) = install_resolver(HOST_DNS_PORT) {
                warn!(?e, "DNS forwarder: could not install resolver config");
                info!(
                    "To enable DNS: sudo mkdir -p /etc/resolver && \
                     echo 'nameserver 127.0.0.1\\nport {}' | sudo tee /etc/resolver/mako.local",
                    HOST_DNS_PORT
                );
            }
        }

        // VM-facing listener: full DNS proxy on the NAT gateway address.
        // Wait for the gateway IP to be discovered from the VM serial output.
        let vm_socket = self.bind_vm_listener().await;

        let socket_path = self.socket_path.clone();
        let vm_ip = self.vm_ip.clone();

        // Host listener thread (*.mako.local only)
        if let Some(sock) = host_socket {
            sock.set_read_timeout(Some(std::time::Duration::from_millis(500)))
                .ok();
            let sp = socket_path.clone();
            let vip = vm_ip.clone();
            std::thread::spawn(move || {
                dns_listener_loop(&sock, &sp, &vip, DnsMode::MakoLocalOnly);
            });
        }

        // VM listener thread (full proxy: .mako.local + system resolver + upstream forward)
        if let Some(sock) = vm_socket {
            sock.set_read_timeout(Some(std::time::Duration::from_millis(500)))
                .ok();
            std::thread::spawn(move || {
                dns_listener_loop(&sock, &socket_path, &vm_ip, DnsMode::FullProxy);
            });
        }

        loop {
            if shutdown_rx.changed().await.is_ok() && *shutdown_rx.borrow() {
                break;
            }
        }

        info!("DNS forwarder: shutting down");
        remove_resolver();
    }

    async fn bind_vm_listener(&self) -> Option<UdpSocket> {
        let gateway_ref = self.vm_gateway.clone();
        let mut attempts = 0;
        loop {
            if let Some(gw) = gateway_ref.read().await.as_ref() {
                // Bind to the high port; the VM uses iptables DNAT to redirect
                // port 53 -> VM_DNS_PORT so standard resolv.conf works.
                let addr = format!("{gw}:{VM_DNS_PORT}");
                match UdpSocket::bind(&addr) {
                    Ok(s) => {
                        info!(addr = %addr, "DNS proxy (VM-facing) listening");
                        return Some(s);
                    }
                    Err(e) => {
                        warn!(?e, addr = %addr, "DNS proxy: failed to bind");
                        return None;
                    }
                }
            }
            attempts += 1;
            if attempts > 120 {
                warn!("DNS proxy: timed out waiting for VM gateway IP (60s)");
                return None;
            }
            tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        }
    }
}

#[derive(Clone, Copy, PartialEq)]
enum DnsMode {
    MakoLocalOnly,
    FullProxy,
}

fn dns_listener_loop(
    socket: &UdpSocket,
    docker_socket: &std::path::Path,
    vm_ip: &Arc<RwLock<Option<String>>>,
    mode: DnsMode,
) {
    let mut buf = [0u8; 1024];
    let mut cache = DnsCache::new();
    loop {
        let (n, src) = match socket.recv_from(&mut buf) {
            Ok(r) => r,
            Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => continue,
            Err(_) => break,
        };

        let query = &buf[..n];

        // Try cache for non-mako.local queries in FullProxy mode
        if mode == DnsMode::FullProxy && n >= 12 {
            if let (Some(name), Some(qtype)) = (parse_dns_name(query, 12), parse_qtype(query)) {
                let name_lower = name.to_lowercase();
                if !name_lower.ends_with(MAKO_DOMAIN) {
                    if let Some(cached) = cache.get(&name_lower, qtype) {
                        if let Some(rewritten) = DnsCache::rewrite_txid(cached, query) {
                            debug!(name = %name_lower, "DNS cache hit");
                            socket.send_to(&rewritten, src).ok();
                            continue;
                        }
                    }
                }
            }
        }

        if let Some(response) = handle_dns_query(query, docker_socket, vm_ip, mode) {
            // Cache non-mako.local responses
            if mode == DnsMode::FullProxy && n >= 12 {
                if let (Some(name), Some(qtype)) = (parse_dns_name(query, 12), parse_qtype(query)) {
                    let name_lower = name.to_lowercase();
                    if !name_lower.ends_with(MAKO_DOMAIN) {
                        cache.put(&name_lower, qtype, response.clone());
                    }
                }
            }
            socket.send_to(&response, src).ok();
        }
    }
}

fn handle_dns_query(
    query: &[u8],
    socket_path: &std::path::Path,
    vm_ip: &Arc<RwLock<Option<String>>>,
    mode: DnsMode,
) -> Option<Vec<u8>> {
    if query.len() < 12 {
        return None;
    }

    let name = parse_dns_name(query, 12)?;
    let name_lower = name.to_lowercase();
    let qtype = parse_qtype(query)?;

    // .mako.local queries handled in both modes
    if name_lower.ends_with(MAKO_DOMAIN) || name_lower == MAKO_DOMAIN.trim_start_matches('.') {
        return handle_mako_local(query, &name_lower, socket_path, vm_ip);
    }

    if mode == DnsMode::MakoLocalOnly {
        return None;
    }

    debug!(name = %name_lower, qtype, "DNS proxy query");

    match qtype {
        QTYPE_A | QTYPE_AAAA => resolve_via_system(query, &name_lower, qtype),
        _ => forward_to_upstream(query),
    }
}

fn handle_mako_local(
    query: &[u8],
    name_lower: &str,
    socket_path: &std::path::Path,
    vm_ip: &Arc<RwLock<Option<String>>>,
) -> Option<Vec<u8>> {
    let container_name = name_lower.strip_suffix(MAKO_DOMAIN)?.to_string();
    if container_name.is_empty() {
        return None;
    }

    debug!(container = %container_name, "DNS query for container");

    if container_name == "vm" {
        let ip = {
            let rt = tokio::runtime::Handle::try_current().ok()?;
            rt.block_on(async { vm_ip.read().await.clone() })?
        };
        return build_a_response(query, &ip);
    }

    let ip = lookup_container_ip(socket_path, &container_name)?;
    build_a_response(query, &ip)
}

/// Use macOS system resolver (`getaddrinfo`) which respects VPN, split-DNS,
/// `/etc/hosts`, and mDNS configuration.
fn resolve_via_system(query: &[u8], name: &str, qtype: u16) -> Option<Vec<u8>> {
    use std::net::ToSocketAddrs;

    let lookup = format!("{name}:0");
    let addrs: Vec<SocketAddr> = match lookup.to_socket_addrs() {
        Ok(iter) => iter.collect(),
        Err(e) => {
            debug!(name, error = %e, "system resolver failed");
            return build_nxdomain_response(query);
        }
    };

    if addrs.is_empty() {
        return build_nxdomain_response(query);
    }

    match qtype {
        QTYPE_A => {
            let v4: Vec<Ipv4Addr> = addrs
                .iter()
                .filter_map(|a| match a.ip() {
                    IpAddr::V4(ip) => Some(ip),
                    _ => None,
                })
                .collect();
            if v4.is_empty() {
                return build_empty_response(query);
            }
            build_a_response_multi(query, &v4)
        }
        QTYPE_AAAA => {
            let v6: Vec<Ipv6Addr> = addrs
                .iter()
                .filter_map(|a| match a.ip() {
                    IpAddr::V6(ip) => Some(ip),
                    _ => None,
                })
                .collect();
            if v6.is_empty() {
                return build_empty_response(query);
            }
            build_aaaa_response_multi(query, &v6)
        }
        _ => None,
    }
}

/// Forward non-A/AAAA queries (MX, SRV, TXT, PTR …) to upstream DNS.
fn forward_to_upstream(query: &[u8]) -> Option<Vec<u8>> {
    let upstreams = get_upstream_resolvers();

    for server in &upstreams {
        let addr = format!("{server}:53");
        if let Some(resp) = forward_to_server(query, &addr) {
            return Some(resp);
        }
    }

    // Last resort: try Google public DNS
    if !upstreams.iter().any(|s| s == "8.8.8.8") {
        return forward_to_server(query, "8.8.8.8:53");
    }
    None
}

fn forward_to_server(query: &[u8], server: &str) -> Option<Vec<u8>> {
    let sock = UdpSocket::bind("0.0.0.0:0").ok()?;
    sock.set_read_timeout(Some(std::time::Duration::from_secs(3)))
        .ok();
    let addr: SocketAddr = server.parse().ok()?;
    sock.send_to(query, addr).ok()?;
    let mut buf = [0u8; 4096];
    let (n, _) = sock.recv_from(&mut buf).ok()?;
    Some(buf[..n].to_vec())
}

/// Parse nameservers from `scutil --dns`, preferring the default resolver (#1).
pub(crate) fn get_upstream_resolvers() -> Vec<String> {
    let output = match std::process::Command::new("scutil").arg("--dns").output() {
        Ok(o) if o.status.success() => o,
        _ => return vec!["8.8.8.8".to_string()],
    };

    let text = String::from_utf8_lossy(&output.stdout);
    let resolvers = parse_scutil_dns(&text);
    if resolvers.is_empty() {
        vec!["8.8.8.8".to_string()]
    } else {
        resolvers
    }
}

/// Extract nameserver IPs from scutil --dns output.
/// Prioritizes resolver #1 (the system default). Falls back to all resolvers
/// if #1 has none.
pub(crate) fn parse_scutil_dns(text: &str) -> Vec<String> {
    let mut default_resolvers = Vec::new();
    let mut all_resolvers = Vec::new();
    let mut in_default = false;

    for line in text.lines() {
        let trimmed = line.trim();

        if trimmed.starts_with("resolver #1") {
            in_default = true;
        } else if trimmed.starts_with("resolver #") {
            in_default = false;
        }

        if let Some(ip) = extract_nameserver_ip(trimmed) {
            if !all_resolvers.contains(&ip) {
                all_resolvers.push(ip.clone());
            }
            if in_default && !default_resolvers.contains(&ip) {
                default_resolvers.push(ip);
            }
        }
    }

    if default_resolvers.is_empty() {
        all_resolvers
    } else {
        default_resolvers
    }
}

fn extract_nameserver_ip(line: &str) -> Option<String> {
    // Format: "nameserver[0] : 10.0.0.1"
    let rest = line.strip_prefix("nameserver[")?;
    let after_bracket = rest.split(']').nth(1)?;
    let ip = after_bracket.trim().trim_start_matches(':').trim();
    if ip.is_empty() {
        return None;
    }
    // Validate it looks like an IP
    if ip.parse::<IpAddr>().is_ok() {
        Some(ip.to_string())
    } else {
        None
    }
}

// ── DNS packet helpers ──────────────────────────────────────────────

pub(crate) fn parse_dns_name(packet: &[u8], offset: usize) -> Option<String> {
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

fn parse_qtype(query: &[u8]) -> Option<u16> {
    let mut pos = 12;
    while pos < query.len() && query[pos] != 0 {
        let len = query[pos] as usize;
        pos += 1 + len;
    }
    pos += 1; // null terminator
    if pos + 2 > query.len() {
        return None;
    }
    Some(u16::from_be_bytes([query[pos], query[pos + 1]]))
}

fn question_end(query: &[u8]) -> Option<usize> {
    let mut pos = 12;
    while pos < query.len() && query[pos] != 0 {
        let len = query[pos] as usize;
        pos += 1 + len;
    }
    pos += 1; // null terminator
    pos += 4; // QTYPE + QCLASS
    if pos > query.len() {
        return None;
    }
    Some(pos)
}

fn build_a_response(query: &[u8], ip: &str) -> Option<Vec<u8>> {
    let addr: Ipv4Addr = ip.parse().ok()?;
    build_a_response_multi(query, &[addr])
}

fn build_a_response_multi(query: &[u8], addrs: &[Ipv4Addr]) -> Option<Vec<u8>> {
    let qend = question_end(query)?;
    let question = &query[12..qend];
    let ancount = addrs.len() as u16;

    let mut resp = Vec::with_capacity(qend + addrs.len() * 16);
    resp.extend_from_slice(&query[0..2]); // ID
    resp.extend_from_slice(&[0x81, 0x80]); // QR=1 RD=1 RA=1
    resp.extend_from_slice(&query[4..6]); // QDCOUNT
    resp.extend_from_slice(&ancount.to_be_bytes());
    resp.extend_from_slice(&[0x00, 0x00]); // NSCOUNT
    resp.extend_from_slice(&[0x00, 0x00]); // ARCOUNT
    resp.extend_from_slice(question);

    for addr in addrs {
        resp.extend_from_slice(&[0xC0, 0x0C]); // Name pointer
        resp.extend_from_slice(&[0x00, 0x01]); // Type A
        resp.extend_from_slice(&[0x00, 0x01]); // Class IN
        resp.extend_from_slice(&[0x00, 0x00, 0x00, 0x3C]); // TTL 60s
        resp.extend_from_slice(&[0x00, 0x04]); // RDLENGTH 4
        resp.extend_from_slice(&addr.octets());
    }
    Some(resp)
}

fn build_aaaa_response_multi(query: &[u8], addrs: &[Ipv6Addr]) -> Option<Vec<u8>> {
    let qend = question_end(query)?;
    let question = &query[12..qend];
    let ancount = addrs.len() as u16;

    let mut resp = Vec::with_capacity(qend + addrs.len() * 28);
    resp.extend_from_slice(&query[0..2]);
    resp.extend_from_slice(&[0x81, 0x80]);
    resp.extend_from_slice(&query[4..6]);
    resp.extend_from_slice(&ancount.to_be_bytes());
    resp.extend_from_slice(&[0x00, 0x00]);
    resp.extend_from_slice(&[0x00, 0x00]);
    resp.extend_from_slice(question);

    for addr in addrs {
        resp.extend_from_slice(&[0xC0, 0x0C]);
        resp.extend_from_slice(&[0x00, 0x1C]); // Type AAAA
        resp.extend_from_slice(&[0x00, 0x01]); // Class IN
        resp.extend_from_slice(&[0x00, 0x00, 0x00, 0x3C]); // TTL 60s
        resp.extend_from_slice(&[0x00, 0x10]); // RDLENGTH 16
        resp.extend_from_slice(&addr.octets());
    }
    Some(resp)
}

fn build_nxdomain_response(query: &[u8]) -> Option<Vec<u8>> {
    let qend = question_end(query)?;
    let question = &query[12..qend];

    let mut resp = Vec::with_capacity(qend);
    resp.extend_from_slice(&query[0..2]);
    resp.extend_from_slice(&[0x81, 0x83]); // QR=1 RD=1 RA=1 RCODE=NXDOMAIN
    resp.extend_from_slice(&query[4..6]);
    resp.extend_from_slice(&[0x00, 0x00]);
    resp.extend_from_slice(&[0x00, 0x00]);
    resp.extend_from_slice(&[0x00, 0x00]);
    resp.extend_from_slice(question);
    Some(resp)
}

fn build_empty_response(query: &[u8]) -> Option<Vec<u8>> {
    let qend = question_end(query)?;
    let question = &query[12..qend];

    let mut resp = Vec::with_capacity(qend);
    resp.extend_from_slice(&query[0..2]);
    resp.extend_from_slice(&[0x81, 0x80]); // NOERROR, 0 answers
    resp.extend_from_slice(&query[4..6]);
    resp.extend_from_slice(&[0x00, 0x00]);
    resp.extend_from_slice(&[0x00, 0x00]);
    resp.extend_from_slice(&[0x00, 0x00]);
    resp.extend_from_slice(question);
    Some(resp)
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

    for (_net_name, net_info) in networks {
        if let Some(ip) = net_info.get("IPAddress").and_then(|v| v.as_str()) {
            if !ip.is_empty() {
                return Some(ip.to_string());
            }
        }
    }
    None
}

fn install_resolver(port: u16) -> Result<()> {
    let resolver_dir = "/etc/resolver";
    let resolver_file = format!("{resolver_dir}/mako.local");
    let content = format!("nameserver 127.0.0.1\nport {port}\n");

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

// ── Tests ───────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn encode_dns_name(name: &str) -> Vec<u8> {
        let mut buf = Vec::new();
        for label in name.split('.') {
            buf.push(label.len() as u8);
            buf.extend_from_slice(label.as_bytes());
        }
        buf.push(0);
        buf
    }

    fn make_dns_query(name: &str) -> Vec<u8> {
        make_dns_query_typed(name, QTYPE_A)
    }

    fn make_dns_query_typed(name: &str, qtype: u16) -> Vec<u8> {
        let mut pkt = Vec::new();
        pkt.extend_from_slice(&[0xAB, 0xCD]); // ID
        pkt.extend_from_slice(&[0x01, 0x00]); // Flags
        pkt.extend_from_slice(&[0x00, 0x01]); // QDCOUNT
        pkt.extend_from_slice(&[0x00, 0x00]); // ANCOUNT
        pkt.extend_from_slice(&[0x00, 0x00]); // NSCOUNT
        pkt.extend_from_slice(&[0x00, 0x00]); // ARCOUNT
        pkt.extend_from_slice(&encode_dns_name(name));
        pkt.extend_from_slice(&qtype.to_be_bytes()); // QTYPE
        pkt.extend_from_slice(&[0x00, 0x01]); // QCLASS IN
        pkt
    }

    // -- parse_dns_name --

    #[test]
    fn parse_single_label() {
        let mut pkt = vec![0u8; 12];
        pkt.extend_from_slice(&encode_dns_name("test"));
        assert_eq!(parse_dns_name(&pkt, 12), Some("test".to_string()));
    }

    #[test]
    fn parse_multi_label() {
        let mut pkt = vec![0u8; 12];
        pkt.extend_from_slice(&encode_dns_name("nginx.mako.local"));
        assert_eq!(
            parse_dns_name(&pkt, 12),
            Some("nginx.mako.local".to_string())
        );
    }

    #[test]
    fn parse_empty_packet() {
        let pkt = vec![0u8; 12];
        assert_eq!(parse_dns_name(&pkt, 12), None);
    }

    #[test]
    fn parse_truncated_packet() {
        let mut pkt = vec![0u8; 12];
        pkt.push(10);
        pkt.extend_from_slice(b"abc");
        assert_eq!(parse_dns_name(&pkt, 12), None);
    }

    #[test]
    fn parse_pointer_returns_none() {
        let mut pkt = vec![0u8; 12];
        pkt.push(0xC0);
        pkt.push(0x00);
        assert_eq!(parse_dns_name(&pkt, 12), None);
    }

    // -- parse_qtype --

    #[test]
    fn parse_qtype_a() {
        let query = make_dns_query_typed("example.com", QTYPE_A);
        assert_eq!(parse_qtype(&query), Some(QTYPE_A));
    }

    #[test]
    fn parse_qtype_aaaa() {
        let query = make_dns_query_typed("example.com", QTYPE_AAAA);
        assert_eq!(parse_qtype(&query), Some(QTYPE_AAAA));
    }

    #[test]
    fn parse_qtype_mx() {
        let query = make_dns_query_typed("example.com", 15); // MX
        assert_eq!(parse_qtype(&query), Some(15));
    }

    // -- build_a_response --

    #[test]
    fn build_response_valid_ip() {
        let query = make_dns_query("nginx.mako.local");
        let response = build_a_response(&query, "172.17.0.2").unwrap();

        assert_eq!(response[0], 0xAB);
        assert_eq!(response[1], 0xCD);
        assert_eq!(response[2], 0x81);
        assert_eq!(response[3], 0x80);

        let len = response.len();
        assert_eq!(&response[len - 4..], &[172, 17, 0, 2]);
    }

    #[test]
    fn build_response_invalid_ip_returns_none() {
        let query = make_dns_query("test.mako.local");
        assert!(build_a_response(&query, "not-an-ip").is_none());
    }

    #[test]
    fn build_response_ipv6_returns_none() {
        let query = make_dns_query("test.mako.local");
        assert!(build_a_response(&query, "::1").is_none());
    }

    #[test]
    fn build_a_multi_response() {
        let query = make_dns_query("example.com");
        let addrs = vec!["1.2.3.4".parse().unwrap(), "5.6.7.8".parse().unwrap()];
        let response = build_a_response_multi(&query, &addrs).unwrap();
        // ANCOUNT should be 2
        assert_eq!(u16::from_be_bytes([response[6], response[7]]), 2);
    }

    #[test]
    fn build_aaaa_response() {
        let query = make_dns_query_typed("example.com", QTYPE_AAAA);
        let addrs = vec!["::1".parse::<Ipv6Addr>().unwrap()];
        let response = build_aaaa_response_multi(&query, &addrs).unwrap();
        assert_eq!(u16::from_be_bytes([response[6], response[7]]), 1);
        let len = response.len();
        assert_eq!(
            &response[len - 16..],
            &[0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1]
        );
    }

    // -- nxdomain / empty --

    #[test]
    fn build_nxdomain() {
        let query = make_dns_query("nonexistent.example.com");
        let response = build_nxdomain_response(&query).unwrap();
        assert_eq!(response[3] & 0x0F, 3); // RCODE=NXDOMAIN
        assert_eq!(u16::from_be_bytes([response[6], response[7]]), 0); // ANCOUNT=0
    }

    #[test]
    fn build_empty() {
        let query = make_dns_query("example.com");
        let response = build_empty_response(&query).unwrap();
        assert_eq!(response[3] & 0x0F, 0); // RCODE=NOERROR
        assert_eq!(u16::from_be_bytes([response[6], response[7]]), 0); // ANCOUNT=0
    }

    // -- domain matching --

    #[test]
    fn mako_domain_suffix_matching() {
        let name = "nginx.mako.local";
        assert!(name.ends_with(MAKO_DOMAIN));

        let name = "nginx.other.local";
        assert!(!name.ends_with(MAKO_DOMAIN));
    }

    #[test]
    fn extract_container_name() {
        let name = "myapp.mako.local";
        let container = name.strip_suffix(MAKO_DOMAIN).unwrap();
        assert_eq!(container, "myapp");
    }

    #[test]
    fn vm_is_special_case() {
        let name = "vm.mako.local";
        let container = name.strip_suffix(MAKO_DOMAIN).unwrap();
        assert_eq!(container, "vm");
    }

    // -- scutil parsing --

    #[test]
    fn parse_scutil_default_resolver() {
        let text = r#"
DNS configuration

resolver #1
  nameserver[0] : 10.0.0.1
  nameserver[1] : 10.0.0.2
  if_index : 6 (en0)
  flags    : Request A records

resolver #2
  domain   : corp.example.com
  nameserver[0] : 10.10.0.1
  flags    : Request A records
"#;
        let result = parse_scutil_dns(text);
        assert_eq!(result, vec!["10.0.0.1", "10.0.0.2"]);
    }

    #[test]
    fn parse_scutil_no_default() {
        let text = r#"
DNS configuration

resolver #2
  nameserver[0] : 172.16.0.1
  flags    : Request A records
"#;
        let result = parse_scutil_dns(text);
        assert_eq!(result, vec!["172.16.0.1"]);
    }

    #[test]
    fn parse_scutil_empty() {
        let result = parse_scutil_dns("");
        assert!(result.is_empty());
    }

    #[test]
    fn parse_scutil_ipv6_resolvers() {
        let text = r#"
resolver #1
  nameserver[0] : 2001:db8::1
  nameserver[1] : 10.0.0.1
"#;
        let result = parse_scutil_dns(text);
        assert_eq!(result, vec!["2001:db8::1", "10.0.0.1"]);
    }

    #[test]
    fn extract_nameserver_ip_valid() {
        assert_eq!(
            extract_nameserver_ip("nameserver[0] : 10.0.0.1"),
            Some("10.0.0.1".to_string())
        );
    }

    #[test]
    fn extract_nameserver_ip_ipv6() {
        assert_eq!(
            extract_nameserver_ip("nameserver[0] : 2001:db8::1"),
            Some("2001:db8::1".to_string())
        );
    }

    #[test]
    fn extract_nameserver_ip_invalid() {
        assert_eq!(extract_nameserver_ip("nameserver[0] : not-an-ip"), None);
    }

    #[test]
    fn extract_nameserver_ip_not_a_nameserver_line() {
        assert_eq!(extract_nameserver_ip("if_index : 6 (en0)"), None);
    }

    // -- full proxy query handling (unit-level) --

    #[test]
    fn mako_local_only_ignores_external() {
        let query = make_dns_query("google.com");
        let path = std::path::PathBuf::from("/nonexistent");
        let vm_ip = Arc::new(RwLock::new(None));
        let result = handle_dns_query(&query, &path, &vm_ip, DnsMode::MakoLocalOnly);
        assert!(result.is_none());
    }

    #[test]
    fn full_proxy_resolves_external_a_record() {
        let query = make_dns_query("localhost");
        let path = std::path::PathBuf::from("/nonexistent");
        let vm_ip = Arc::new(RwLock::new(None));
        let result = handle_dns_query(&query, &path, &vm_ip, DnsMode::FullProxy);
        // "localhost" should resolve to 127.0.0.1 via getaddrinfo
        assert!(result.is_some());
        let resp = result.unwrap();
        assert_eq!(resp[0], 0xAB);
        assert_eq!(resp[1], 0xCD);
    }

    #[test]
    fn full_proxy_still_handles_mako_local() {
        let query = make_dns_query("vm.mako.local");
        let path = std::path::PathBuf::from("/nonexistent");
        let rt = tokio::runtime::Runtime::new().unwrap();
        let vm_ip = Arc::new(RwLock::new(Some("192.168.64.5".to_string())));
        // enter() sets the runtime context without blocking, so the nested
        // block_on inside handle_mako_local works correctly.
        let _guard = rt.enter();
        let result = handle_dns_query(&query, &path, &vm_ip, DnsMode::FullProxy);
        assert!(result.is_some());
        let resp = result.unwrap();
        let len = resp.len();
        assert_eq!(&resp[len - 4..], &[192, 168, 64, 5]);
    }
}
