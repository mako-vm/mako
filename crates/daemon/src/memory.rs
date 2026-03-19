use std::io::{Read, Write};
use std::sync::Arc;
use tokio::sync::{watch, RwLock};
use tracing::{debug, info};

#[derive(Debug, Clone, Default)]
pub struct MemoryStats {
    pub vm_total_bytes: u64,
    pub vm_available_bytes: u64,
    pub containers_used_bytes: u64,
}

pub struct MemoryMonitor {
    socket_path: std::path::PathBuf,
    stats: Arc<RwLock<MemoryStats>>,
    vm_memory_bytes: u64,
}

impl MemoryMonitor {
    pub fn new(socket_path: std::path::PathBuf, vm_memory_bytes: u64) -> Self {
        Self {
            socket_path,
            stats: Arc::new(RwLock::new(MemoryStats {
                vm_total_bytes: vm_memory_bytes,
                ..Default::default()
            })),
            vm_memory_bytes,
        }
    }

    pub fn stats_ref(&self) -> Arc<RwLock<MemoryStats>> {
        self.stats.clone()
    }

    pub async fn run(self, mut shutdown_rx: watch::Receiver<bool>) {
        info!("memory monitor: starting");
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(5));

        loop {
            tokio::select! {
                _ = interval.tick() => {
                    if let Some(used) = self.fetch_container_memory_usage().await {
                        let mut stats = self.stats.write().await;
                        stats.vm_total_bytes = self.vm_memory_bytes;
                        stats.containers_used_bytes = used;
                        stats.vm_available_bytes = self.vm_memory_bytes.saturating_sub(used);
                        debug!(
                            total_mb = stats.vm_total_bytes / (1024 * 1024),
                            used_mb = stats.containers_used_bytes / (1024 * 1024),
                            avail_mb = stats.vm_available_bytes / (1024 * 1024),
                            "memory stats"
                        );
                    }
                }
                _ = shutdown_rx.changed() => {
                    if *shutdown_rx.borrow() {
                        info!("memory monitor: shutting down");
                        break;
                    }
                }
            }
        }
    }

    async fn fetch_container_memory_usage(&self) -> Option<u64> {
        let socket_path = self.socket_path.clone();
        tokio::task::spawn_blocking(move || {
            let mut stream = std::os::unix::net::UnixStream::connect(&socket_path).ok()?;
            let req = "GET /containers/json HTTP/1.0\r\nHost: localhost\r\n\r\n";
            stream.write_all(req.as_bytes()).ok()?;
            stream.flush().ok()?;

            let mut response = Vec::new();
            stream.read_to_end(&mut response).ok()?;

            let response_str = String::from_utf8_lossy(&response);
            let body = response_str.split("\r\n\r\n").nth(1)?;
            let containers: Vec<serde_json::Value> = serde_json::from_str(body).ok()?;

            let mut total_memory: u64 = 0;
            for container in &containers {
                if let Some(id) = container.get("Id").and_then(|v| v.as_str()) {
                    if let Some(mem) = fetch_container_stats(&socket_path, id) {
                        total_memory += mem;
                    }
                }
            }
            Some(total_memory)
        })
        .await
        .ok()?
    }
}

fn fetch_container_stats(socket_path: &std::path::Path, container_id: &str) -> Option<u64> {
    let mut stream = std::os::unix::net::UnixStream::connect(socket_path).ok()?;
    stream
        .set_read_timeout(Some(std::time::Duration::from_secs(2)))
        .ok()?;

    let req = format!(
        "GET /containers/{}/stats?stream=false HTTP/1.0\r\nHost: localhost\r\n\r\n",
        container_id
    );
    stream.write_all(req.as_bytes()).ok()?;
    stream.flush().ok()?;

    let mut response = Vec::new();
    stream.read_to_end(&mut response).ok()?;

    let response_str = String::from_utf8_lossy(&response);
    let body = response_str.split("\r\n\r\n").nth(1)?;
    let stats: serde_json::Value = serde_json::from_str(body).ok()?;

    parse_memory_usage(&stats)
}

pub(crate) fn parse_memory_usage(stats: &serde_json::Value) -> Option<u64> {
    stats.get("memory_stats")?.get("usage")?.as_u64()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_valid_memory_stats() {
        let json: serde_json::Value = serde_json::from_str(
            r#"{
                "memory_stats": {
                    "usage": 104857600,
                    "max_usage": 209715200,
                    "limit": 4294967296
                }
            }"#,
        )
        .unwrap();
        assert_eq!(parse_memory_usage(&json), Some(104857600));
    }

    #[test]
    fn parse_missing_memory_stats() {
        let json: serde_json::Value = serde_json::from_str(r#"{"cpu_stats": {}}"#).unwrap();
        assert_eq!(parse_memory_usage(&json), None);
    }

    #[test]
    fn parse_missing_usage_field() {
        let json: serde_json::Value =
            serde_json::from_str(r#"{"memory_stats": {"limit": 4294967296}}"#).unwrap();
        assert_eq!(parse_memory_usage(&json), None);
    }

    #[test]
    fn parse_zero_usage() {
        let json: serde_json::Value =
            serde_json::from_str(r#"{"memory_stats": {"usage": 0}}"#).unwrap();
        assert_eq!(parse_memory_usage(&json), Some(0));
    }

    #[test]
    fn parse_large_usage() {
        let json: serde_json::Value =
            serde_json::from_str(r#"{"memory_stats": {"usage": 17179869184}}"#).unwrap();
        // 16 GiB
        assert_eq!(parse_memory_usage(&json), Some(17179869184));
    }

    #[test]
    fn memory_stats_default() {
        let stats = MemoryStats::default();
        assert_eq!(stats.vm_total_bytes, 0);
        assert_eq!(stats.vm_available_bytes, 0);
        assert_eq!(stats.containers_used_bytes, 0);
    }

    #[test]
    fn memory_monitor_initial_stats() {
        let monitor = MemoryMonitor::new(
            std::path::PathBuf::from("/tmp/test.sock"),
            4 * 1024 * 1024 * 1024,
        );
        let stats = monitor.stats.blocking_read();
        assert_eq!(stats.vm_total_bytes, 4 * 1024 * 1024 * 1024);
        assert_eq!(stats.containers_used_bytes, 0);
    }
}
