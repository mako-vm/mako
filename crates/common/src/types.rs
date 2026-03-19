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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vm_state_serde_lowercase() {
        assert_eq!(
            serde_json::to_string(&VmState::Running).unwrap(),
            "\"running\""
        );
        assert_eq!(
            serde_json::to_string(&VmState::Stopped).unwrap(),
            "\"stopped\""
        );
        assert_eq!(
            serde_json::to_string(&VmState::Starting).unwrap(),
            "\"starting\""
        );
        assert_eq!(
            serde_json::to_string(&VmState::Stopping).unwrap(),
            "\"stopping\""
        );
        assert_eq!(serde_json::to_string(&VmState::Error).unwrap(), "\"error\"");
    }

    #[test]
    fn vm_state_deserialize() {
        let state: VmState = serde_json::from_str("\"running\"").unwrap();
        assert_eq!(state, VmState::Running);
    }

    #[test]
    fn vm_config_default_reasonable() {
        let config = VmConfig::default();
        assert!(config.cpu_count >= 1);
        assert_eq!(config.memory_bytes, 4 * 1024 * 1024 * 1024);
        assert_eq!(config.disk_size_bytes, 64 * 1024 * 1024 * 1024);
        assert!(!config.shared_directories.is_empty());
        assert_eq!(config.shared_directories[0].mount_tag, "home");
        assert!(!config.shared_directories[0].read_only);
    }

    #[test]
    fn shared_directory_serde_round_trip() {
        let sd = SharedDirectory {
            host_path: Some(std::path::PathBuf::from("/Users/test")),
            mount_tag: "data".into(),
            read_only: true,
        };
        let json = serde_json::to_string(&sd).unwrap();
        let parsed: SharedDirectory = serde_json::from_str(&json).unwrap();
        assert_eq!(
            parsed.host_path,
            Some(std::path::PathBuf::from("/Users/test"))
        );
        assert_eq!(parsed.mount_tag, "data");
        assert!(parsed.read_only);
    }

    #[test]
    fn shared_directory_null_host_path() {
        let sd = SharedDirectory {
            host_path: None,
            mount_tag: "empty".into(),
            read_only: false,
        };
        let json = serde_json::to_string(&sd).unwrap();
        assert!(json.contains("null"));
        let parsed: SharedDirectory = serde_json::from_str(&json).unwrap();
        assert!(parsed.host_path.is_none());
    }

    #[test]
    fn vm_config_serde_round_trip() {
        let config = VmConfig::default();
        let json = serde_json::to_string_pretty(&config).unwrap();
        let parsed: VmConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.cpu_count, config.cpu_count);
        assert_eq!(parsed.memory_bytes, config.memory_bytes);
        assert_eq!(parsed.rosetta, config.rosetta);
    }
}
