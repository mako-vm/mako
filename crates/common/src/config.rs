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
        let path = config_file_path();
        if path.exists() {
            let contents = std::fs::read_to_string(&path)?;
            let config: MakoConfig = serde_json::from_str(&contents)?;
            Ok(config)
        } else {
            Ok(Self::default())
        }
    }

    pub fn save(&self) -> anyhow::Result<()> {
        let path = config_file_path();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let contents = serde_json::to_string_pretty(self)?;
        std::fs::write(&path, contents)?;
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
