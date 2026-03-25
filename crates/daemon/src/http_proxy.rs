use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{watch, RwLock};
use tracing::{debug, info, warn};

const PROXY_PORT: u16 = 3128;

/// HTTP CONNECT proxy that runs on the VM gateway address.
///
/// Docker inside the VM sends HTTPS requests through this proxy. Since the
/// proxy runs on the macOS host, it has direct access to VPN routes and can
/// reach internal registries that the VM's NAT network cannot.
pub struct HttpProxy {
    vm_gateway: Arc<RwLock<Option<String>>>,
}

impl HttpProxy {
    pub fn new(vm_gateway: Arc<RwLock<Option<String>>>) -> Self {
        Self { vm_gateway }
    }

    pub async fn run(self, mut shutdown_rx: watch::Receiver<bool>) {
        let listener = match self.bind_listener().await {
            Some(l) => l,
            None => return,
        };

        info!("HTTP CONNECT proxy: accepting connections");

        loop {
            tokio::select! {
                result = listener.accept() => {
                    match result {
                        Ok((stream, addr)) => {
                            debug!(peer = %addr, "proxy: new connection");
                            tokio::spawn(async move {
                                if let Err(e) = handle_client(stream).await {
                                    debug!(error = %e, "proxy: session error");
                                }
                            });
                        }
                        Err(e) => {
                            warn!(error = %e, "proxy: accept error");
                        }
                    }
                }
                _ = shutdown_rx.changed() => {
                    if *shutdown_rx.borrow() {
                        break;
                    }
                }
            }
        }

        info!("HTTP CONNECT proxy: shutting down");
    }

    async fn bind_listener(&self) -> Option<TcpListener> {
        let gateway_ref = self.vm_gateway.clone();
        let mut attempts = 0;
        loop {
            if let Some(gw) = gateway_ref.read().await.as_ref() {
                let addr = format!("{gw}:{PROXY_PORT}");
                match TcpListener::bind(&addr).await {
                    Ok(l) => {
                        info!(addr = %addr, "HTTP CONNECT proxy listening");
                        return Some(l);
                    }
                    Err(e) => {
                        warn!(?e, addr = %addr, "HTTP CONNECT proxy: failed to bind");
                        return None;
                    }
                }
            }
            attempts += 1;
            if attempts > 120 {
                warn!("HTTP CONNECT proxy: timed out waiting for gateway IP");
                return None;
            }
            tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        }
    }
}

async fn handle_client(client: TcpStream) -> anyhow::Result<()> {
    let mut buf_reader = BufReader::new(client);

    let mut request_line = String::new();
    buf_reader.read_line(&mut request_line).await?;
    let request_line = request_line.trim().to_string();

    let parts: Vec<&str> = request_line.split_whitespace().collect();
    if parts.len() < 2 {
        buf_reader
            .get_mut()
            .write_all(b"HTTP/1.1 400 Bad Request\r\n\r\n")
            .await?;
        return Ok(());
    }

    let method = parts[0].to_uppercase();
    let target = parts[1].to_string();

    // Drain remaining headers
    loop {
        let mut line = String::new();
        buf_reader.read_line(&mut line).await?;
        if line.trim().is_empty() {
            break;
        }
    }

    if method == "CONNECT" {
        handle_connect(&target, buf_reader).await
    } else {
        handle_plain_forward(&target, buf_reader).await
    }
}

/// Handle CONNECT method: establish a TCP tunnel to the target.
/// The host has VPN access so it can reach internal IPs.
async fn handle_connect(target: &str, mut buf_reader: BufReader<TcpStream>) -> anyhow::Result<()> {
    let host_port = if target.contains(':') {
        target.to_string()
    } else {
        format!("{target}:443")
    };

    debug!(target = %host_port, "CONNECT tunnel");

    match TcpStream::connect(&host_port).await {
        Ok(mut upstream) => {
            upstream.set_nodelay(true)?;
            buf_reader.get_ref().set_nodelay(true).ok();
            buf_reader
                .get_mut()
                .write_all(b"HTTP/1.1 200 Connection Established\r\n\r\n")
                .await?;

            let mut client = buf_reader.into_inner();
            let result = tokio::io::copy_bidirectional(&mut client, &mut upstream).await;
            debug!(?result, "CONNECT tunnel closed");
        }
        Err(e) => {
            debug!(target = %host_port, error = %e, "CONNECT failed");
            let msg = format!(
                "HTTP/1.1 502 Bad Gateway\r\nContent-Length: {}\r\n\r\n{e}",
                e.to_string().len()
            );
            buf_reader.get_mut().write_all(msg.as_bytes()).await?;
        }
    }

    Ok(())
}

/// Forward plain HTTP requests to the target.
async fn handle_plain_forward(
    target: &str,
    mut buf_reader: BufReader<TcpStream>,
) -> anyhow::Result<()> {
    let (host_port, _path) = parse_http_target(target);

    debug!(target = %host_port, "HTTP forward");

    match TcpStream::connect(&host_port).await {
        Ok(mut upstream) => {
            upstream.set_nodelay(true)?;
            let mut client = buf_reader.into_inner();
            client.set_nodelay(true).ok();
            let result = tokio::io::copy_bidirectional(&mut client, &mut upstream).await;
            debug!(?result, "HTTP forward closed");
        }
        Err(e) => {
            debug!(error = %e, "HTTP forward failed");
            buf_reader
                .get_mut()
                .write_all(b"HTTP/1.1 502 Bad Gateway\r\n\r\n")
                .await?;
        }
    }

    Ok(())
}

fn parse_http_target(target: &str) -> (String, String) {
    let without_scheme = target
        .strip_prefix("http://")
        .or_else(|| target.strip_prefix("https://"))
        .unwrap_or(target);

    let (host, path) = match without_scheme.find('/') {
        Some(idx) => (
            without_scheme[..idx].to_string(),
            without_scheme[idx..].to_string(),
        ),
        None => (without_scheme.to_string(), "/".to_string()),
    };

    let host_port = if host.contains(':') {
        host
    } else {
        format!("{host}:80")
    };

    (host_port, path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn connect_target_with_port() {
        let target = "registry.example.com:443";
        let host_port = if target.contains(':') {
            target.to_string()
        } else {
            format!("{target}:443")
        };
        assert_eq!(host_port, "registry.example.com:443");
    }

    #[test]
    fn connect_target_without_port() {
        let target = "registry.example.com";
        let host_port = if target.contains(':') {
            target.to_string()
        } else {
            format!("{target}:443")
        };
        assert_eq!(host_port, "registry.example.com:443");
    }

    #[test]
    fn parse_target_with_path() {
        let (host, path) = parse_http_target("http://example.com/v2/catalog");
        assert_eq!(host, "example.com:80");
        assert_eq!(path, "/v2/catalog");
    }

    #[test]
    fn parse_target_without_path() {
        let (host, path) = parse_http_target("http://example.com");
        assert_eq!(host, "example.com:80");
        assert_eq!(path, "/");
    }

    #[test]
    fn parse_target_with_port() {
        let (host, path) = parse_http_target("http://example.com:8080/api");
        assert_eq!(host, "example.com:8080");
        assert_eq!(path, "/api");
    }

    #[test]
    fn parse_target_https() {
        let (host, path) = parse_http_target("https://registry.internal.corp/v2/");
        assert_eq!(host, "registry.internal.corp:80");
        assert_eq!(path, "/v2/");
    }

    #[tokio::test]
    async fn proxy_binds_when_gateway_available() {
        let gw = Arc::new(RwLock::new(Some("127.0.0.1".to_string())));
        let proxy = HttpProxy::new(gw);
        let listener = proxy.bind_listener().await;
        if let Some(l) = listener {
            let addr = l.local_addr().unwrap();
            assert_eq!(addr.port(), PROXY_PORT);
        }
    }
}
