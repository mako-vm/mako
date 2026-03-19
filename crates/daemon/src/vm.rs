use crate::ffi::{SharedDirConfig, VmFfiConfig, VmHandle};
use anyhow::Result;
use mako_common::config::MakoConfig;
use mako_common::types::VmState;
use std::io::{BufRead, BufReader};
use std::os::fd::FromRawFd;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::info;

pub struct VmManager {
    config: MakoConfig,
    state: Arc<RwLock<VmState>>,
    handle: Arc<RwLock<Option<VmHandle>>>,
    handle_arc: Arc<RwLock<Option<Arc<VmHandle>>>>,
    vm_ip: Arc<RwLock<Option<String>>>,
}

#[allow(dead_code)]
impl VmManager {
    pub fn new(config: MakoConfig) -> Result<Self> {
        Ok(Self {
            config,
            state: Arc::new(RwLock::new(VmState::Stopped)),
            handle: Arc::new(RwLock::new(None)),
            handle_arc: Arc::new(RwLock::new(None)),
            vm_ip: Arc::new(RwLock::new(None)),
        })
    }

    /// Boot the VM and return the handle immediately (before waiting for guest init).
    /// The caller should set up vsock listeners, then call `wait_for_ready()`.
    pub async fn start_and_get_handle(&self) -> Result<Arc<VmHandle>> {
        {
            let state = self.state.read().await;
            if *state == VmState::Running {
                anyhow::bail!("VM is already running");
            }
        }
        *self.state.write().await = VmState::Starting;

        let kernel_path = self.config.kernel_path.to_string_lossy().into_owned();
        let initrd_path = self.config.initrd_path.as_ref().and_then(|p| {
            if p.exists() {
                Some(p.to_string_lossy().into_owned())
            } else {
                None
            }
        });
        let rootfs_path = self.config.rootfs_path.to_string_lossy().into_owned();

        if !std::path::Path::new(&kernel_path).exists() {
            anyhow::bail!(
                "Kernel not found at {}. Run 'cd vm-image && make' to build the VM image.",
                kernel_path
            );
        }
        if !std::path::Path::new(&rootfs_path).exists() {
            anyhow::bail!(
                "Rootfs not found at {}. Run 'cd vm-image && make' to build the VM image.",
                rootfs_path
            );
        }

        info!(
            cpus = self.config.vm.cpu_count,
            memory_mb = self.config.vm.memory_bytes / (1024 * 1024),
            kernel = %kernel_path,
            initrd = ?initrd_path,
            rootfs = %rootfs_path,
            "booting VM"
        );

        let shared_directories = self
            .config
            .vm
            .shared_directories
            .iter()
            .filter_map(|sd| {
                sd.host_path.as_ref().map(|p| SharedDirConfig {
                    tag: sd.mount_tag.clone(),
                    host_path: p.to_string_lossy().into_owned(),
                    read_only: sd.read_only,
                })
            })
            .collect();

        let ffi_config = VmFfiConfig {
            cpu_count: self.config.vm.cpu_count,
            memory_bytes: self.config.vm.memory_bytes,
            kernel_path,
            initrd_path,
            rootfs_path,
            rosetta: self.config.vm.rosetta,
            vsock_control_port: self.config.vsock_control_port,
            vsock_docker_port: self.config.vsock_docker_port,
            shared_directories,
        };

        let vm_handle = tokio::task::spawn_blocking(move || -> Result<VmHandle> {
            let handle = VmHandle::create(&ffi_config)?;
            handle.configure()?;
            info!("VM configured, starting...");
            handle.start()?;
            info!("VM started");
            Ok(handle)
        })
        .await??;

        let handle = Arc::new(vm_handle);
        *self.handle.write().await = None; // handle is returned directly
        self.handle_arc.write().await.replace(handle.clone());
        Ok(handle)
    }

    /// Wait for the guest init to complete (reads serial until MAKO_VM_READY).
    pub async fn wait_for_ready(&self) -> Result<()> {
        let handle = self
            .handle_arc
            .read()
            .await
            .clone()
            .ok_or_else(|| anyhow::anyhow!("VM not started"))?;
        let vm_ip_writer = self.vm_ip.clone();

        tokio::task::spawn_blocking(move || -> Result<()> {
            let serial_fd = handle.serial_read_fd();
            if serial_fd < 0 {
                anyhow::bail!("no serial fd");
            }
            let file = unsafe { std::fs::File::from_raw_fd(serial_fd) };
            let reader = BufReader::new(file);
            let mut lines_iter = reader.lines();
            while let Some(Ok(line)) = lines_iter.next() {
                if !line.is_empty() {
                    info!(serial = %line);
                }

                if let Some(ip) = line.strip_prefix("MAKO_VM_IP=") {
                    let ip = ip.trim().to_string();
                    if !ip.is_empty() {
                        info!(ip = %ip, "discovered VM IP");
                        let rt = tokio::runtime::Handle::current();
                        rt.block_on(async {
                            *vm_ip_writer.write().await = Some(ip);
                        });
                    }
                }

                if line.contains("MAKO_VM_READY") {
                    info!("VM init complete, dockerd should be ready");
                    std::thread::spawn(move || {
                        for line in lines_iter.map_while(Result::ok) {
                            if !line.is_empty() {
                                info!(serial = %line);
                            }
                        }
                    });
                    break;
                }
            }
            Ok(())
        })
        .await??;

        *self.state.write().await = VmState::Running;
        info!("VM is running");
        Ok(())
    }

    pub async fn stop(&self) -> Result<()> {
        *self.state.write().await = VmState::Stopping;

        if let Some(h) = self.handle_arc.write().await.take() {
            tokio::task::spawn_blocking(move || h.stop()).await??;
        } else if let Some(h) = self.handle.write().await.take() {
            tokio::task::spawn_blocking(move || h.stop()).await??;
        }

        *self.vm_ip.write().await = None;
        *self.state.write().await = VmState::Stopped;
        info!("VM stopped");
        Ok(())
    }

    pub async fn state(&self) -> VmState {
        *self.state.read().await
    }

    pub async fn vm_ip(&self) -> Option<String> {
        self.vm_ip.read().await.clone()
    }

    pub fn vm_ip_ref(&self) -> Arc<RwLock<Option<String>>> {
        self.vm_ip.clone()
    }

    /// Take the VmHandle out wrapped in an Arc for sharing with the socket proxy.
    pub async fn take_handle_arc(&self) -> Option<Arc<VmHandle>> {
        self.handle.write().await.take().map(Arc::new)
    }
}
