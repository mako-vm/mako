use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum VmState {
    Stopped,
    Starting,
    Running,
    Stopping,
    Error,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VmInfo {
    pub id: Uuid,
    pub state: VmState,
    pub cpu_count: u32,
    pub memory_bytes: u64,
    pub disk_size_bytes: u64,
    pub disk_used_bytes: u64,
    pub uptime_seconds: Option<u64>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VmConfig {
    pub cpu_count: u32,
    pub memory_bytes: u64,
    pub disk_size_bytes: u64,
    /// Directories on macOS to share into the VM via VirtioFS
    pub shared_directories: Vec<SharedDirectory>,
    /// Whether to enable Rosetta for x86 emulation on Apple Silicon
    pub rosetta: bool,
}

impl Default for VmConfig {
    fn default() -> Self {
        Self {
            cpu_count: num_cpus(),
            memory_bytes: 4 * 1024 * 1024 * 1024,     // 4 GiB
            disk_size_bytes: 64 * 1024 * 1024 * 1024, // 64 GiB sparse
            shared_directories: vec![SharedDirectory {
                host_path: home_dir(),
                mount_tag: "home".into(),
                read_only: false,
            }],
            rosetta: cfg!(target_arch = "aarch64"),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SharedDirectory {
    pub host_path: Option<std::path::PathBuf>,
    pub mount_tag: String,
    pub read_only: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DaemonStatus {
    pub vm: Option<VmInfo>,
    pub docker_socket: Option<String>,
    pub version: String,
}

fn num_cpus() -> u32 {
    std::thread::available_parallelism()
        .map(|n| n.get() as u32)
        .unwrap_or(2)
}

fn home_dir() -> Option<std::path::PathBuf> {
    std::env::var_os("HOME").map(std::path::PathBuf::from)
}
