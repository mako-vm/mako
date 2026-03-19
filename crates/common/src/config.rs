use crate::types::VmConfig;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

const MAKO_DIR_NAME: &str = ".mako";
const CONFIG_FILE: &str = "config.json";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MakoConfig {
    pub vm: VmConfig,
    pub kernel_path: PathBuf,
    pub initrd_path: Option<PathBuf>,
    pub rootfs_path: PathBuf,
    /// Unix socket path for the Docker socket proxy on macOS
    pub docker_socket_path: PathBuf,
    /// Unix socket path for the makod control API
    pub daemon_socket_path: PathBuf,
    /// vsock port for the control channel between makod and mako-agent
    pub vsock_control_port: u32,
    /// vsock port for Docker socket forwarding
    pub vsock_docker_port: u32,
    /// TCP port that dockerd listens on inside the VM
    pub docker_tcp_port: u16,
}

impl Default for MakoConfig {
    fn default() -> Self {
        let mako_dir = mako_data_dir();
        Self {
            vm: VmConfig::default(),
            kernel_path: mako_dir.join("vmlinux"),
            initrd_path: Some(mako_dir.join("initramfs.img")),
            rootfs_path: mako_dir.join("rootfs.img"),
            docker_socket_path: mako_dir.join("docker.sock"),
            daemon_socket_path: mako_dir.join("makod.sock"),
            vsock_control_port: 2222,
            vsock_docker_port: 2375,
            docker_tcp_port: 2375,
        }
    }
}

impl MakoConfig {
    pub fn load() -> anyhow::Result<Self> {
        Self::load_from(&config_file_path())
    }

    pub fn load_from(path: &Path) -> anyhow::Result<Self> {
        if path.exists() {
            let contents = std::fs::read_to_string(path)?;
            let config: MakoConfig = serde_json::from_str(&contents)?;
            Ok(config)
        } else {
            Ok(Self::default())
        }
    }

    pub fn save(&self) -> anyhow::Result<()> {
        self.save_to(&config_file_path())
    }

    pub fn save_to(&self, path: &Path) -> anyhow::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let contents = serde_json::to_string_pretty(self)?;
        std::fs::write(path, contents)?;
        Ok(())
    }
}

pub fn mako_data_dir() -> PathBuf {
    let home = std::env::var("HOME").expect("HOME not set");
    Path::new(&home).join(MAKO_DIR_NAME)
}

fn config_file_path() -> PathBuf {
    mako_data_dir().join(CONFIG_FILE)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_has_sane_values() {
        let config = MakoConfig::default();
        assert!(config.vm.cpu_count > 0);
        assert!(config.vm.memory_bytes >= 1024 * 1024 * 1024); // at least 1 GiB
        assert!(config.vm.disk_size_bytes > 0);
        assert!(config
            .docker_socket_path
            .to_string_lossy()
            .ends_with("docker.sock"));
        assert_eq!(config.vsock_docker_port, 2375);
        assert!(!config.vm.shared_directories.is_empty());
    }

    #[test]
    fn config_json_round_trip() {
        let config = MakoConfig::default();
        let json = serde_json::to_string_pretty(&config).unwrap();
        let parsed: MakoConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(config.vm.cpu_count, parsed.vm.cpu_count);
        assert_eq!(config.vm.memory_bytes, parsed.vm.memory_bytes);
        assert_eq!(config.docker_socket_path, parsed.docker_socket_path);
        assert_eq!(config.vsock_docker_port, parsed.vsock_docker_port);
    }

    #[test]
    fn load_missing_file_returns_default() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nonexistent.json");
        let config = MakoConfig::load_from(&path).unwrap();
        assert!(config.vm.cpu_count > 0);
    }

    #[test]
    fn save_and_load_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");

        let mut config = MakoConfig::default();
        config.vm.cpu_count = 8;
        config.vm.memory_bytes = 8 * 1024 * 1024 * 1024;
        config.save_to(&path).unwrap();

        let loaded = MakoConfig::load_from(&path).unwrap();
        assert_eq!(loaded.vm.cpu_count, 8);
        assert_eq!(loaded.vm.memory_bytes, 8 * 1024 * 1024 * 1024);
    }

    #[test]
    fn load_valid_json_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        let config = MakoConfig::default();
        let json = serde_json::to_string_pretty(&config).unwrap();
        std::fs::write(&path, json).unwrap();

        let loaded = MakoConfig::load_from(&path).unwrap();
        assert_eq!(loaded.vm.cpu_count, config.vm.cpu_count);
    }

    #[test]
    fn mako_data_dir_under_home() {
        let dir = mako_data_dir();
        assert!(dir.to_string_lossy().contains(".mako"));
    }
}
