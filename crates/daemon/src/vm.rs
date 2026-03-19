use crate::ffi::{SharedDirConfig, VmFfiConfig, VmHandle};
use anyhow::Result;
use mako_common::config::{mako_data_dir, MakoConfig};
use mako_common::types::VmState;
use std::io::{BufRead, BufReader};
use std::os::fd::FromRawFd;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{info, warn};

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

    /// Pause the VM, snapshot the rootfs, save VM state, then stop.
    /// On next start, the VM can restore from the saved state for near-instant boot.
    pub async fn stop_and_save(&self) -> Result<bool> {
        if !VmHandle::supports_save_restore() {
            warn!("VM save/restore not supported (requires macOS 14+), doing cold stop");
            self.stop().await?;
            return Ok(false);
        }

        let state_path = vm_state_path();
        let state_path_str = state_path.to_string_lossy().into_owned();
        let rootfs = self.config.rootfs_path.clone();
        let rootfs_snapshot = rootfs_snapshot_path();

        *self.state.write().await = VmState::Stopping;

        let handle = self.handle_arc.read().await.clone().or_else(|| None);

        if let Some(h) = handle {
            let saved = tokio::task::spawn_blocking(move || -> Result<bool> {
                info!("pausing VM for state save...");
                if let Err(e) = h.pause() {
                    warn!(error = %e, "pause failed, falling back to cold stop");
                    let _ = h.stop();
                    return Ok(false);
                }

                // Clone rootfs while VM is paused (APFS COW clone, near-instant).
                // The clone preserves the exact disk state at this point.
                // After process exit, the original rootfs may be dirtied by VZ cleanup.
                info!("cloning rootfs for snapshot...");
                let _ = std::fs::remove_file(&rootfs_snapshot);
                let status = std::process::Command::new("cp")
                    .args([
                        "-c",
                        &rootfs.to_string_lossy(),
                        &rootfs_snapshot.to_string_lossy(),
                    ])
                    .status();
                match status {
                    Ok(s) if s.success() => {
                        info!("rootfs snapshot created (APFS clone)");
                    }
                    _ => {
                        warn!("APFS clone failed, trying regular copy...");
                        if let Err(e) = std::fs::copy(&rootfs, &rootfs_snapshot) {
                            warn!(error = %e, "rootfs copy failed, falling back to cold stop");
                            let _ = h.force_stop();
                            return Ok(false);
                        }
                        info!("rootfs snapshot created (regular copy)");
                    }
                }

                info!("saving VM state to {}", state_path_str);
                if let Err(e) = h.save_state(&state_path_str) {
                    warn!(error = %e, "save state failed, falling back to cold stop");
                    let _ = h.force_stop();
                    let _ = std::fs::remove_file(&state_path_str);
                    let _ = std::fs::remove_file(&rootfs_snapshot);
                    return Ok(false);
                }
                info!("VM state saved (VM left paused, will be cleaned up on exit)");
                Ok(true)
            })
            .await??;

            // Clear the handle
            self.handle_arc.write().await.take();

            *self.vm_ip.write().await = None;
            *self.state.write().await = VmState::Stopped;

            if saved {
                info!("VM suspended to disk (fast resume available)");
            } else {
                info!("VM stopped (cold boot on next start)");
            }
            return Ok(saved);
        }

        // No handle, just mark stopped
        *self.vm_ip.write().await = None;
        *self.state.write().await = VmState::Stopped;
        Ok(false)
    }

    /// Try to restore from a saved VM state. Returns the handle on success.
    /// The caller should re-setup vsock listeners and skip waiting for MAKO_VM_READY
    /// since the guest is already fully initialized.
    pub async fn start_from_saved(&self) -> Result<Option<Arc<VmHandle>>> {
        let state_path = vm_state_path();
        let rootfs_snapshot = rootfs_snapshot_path();
        let rootfs = &self.config.rootfs_path;

        if !state_path.exists() || !rootfs_snapshot.exists() {
            // Clean up partial save artifacts
            let _ = std::fs::remove_file(&state_path);
            let _ = std::fs::remove_file(&rootfs_snapshot);
            return Ok(None);
        }

        if !VmHandle::supports_save_restore() {
            let _ = std::fs::remove_file(&state_path);
            let _ = std::fs::remove_file(&rootfs_snapshot);
            return Ok(None);
        }

        info!("found saved VM state, swapping in rootfs snapshot...");

        // Swap the rootfs snapshot into place (process exit may have dirtied the original)
        let _ = std::fs::rename(rootfs, rootfs.with_extension("img.dirty"));
        if let Err(e) = std::fs::rename(&rootfs_snapshot, rootfs) {
            warn!(error = %e, "failed to swap rootfs snapshot, recovering...");
            let _ = std::fs::rename(rootfs.with_extension("img.dirty"), rootfs);
            let _ = std::fs::remove_file(&state_path);
            return Ok(None);
        }

        *self.state.write().await = VmState::Starting;

        let ffi_config = self.build_ffi_config();
        let state_path_str = state_path.to_string_lossy().into_owned();

        let result = tokio::task::spawn_blocking(move || -> Result<VmHandle> {
            let handle = VmHandle::create(&ffi_config)?;
            handle.configure()?;
            handle.create_instance()?;
            info!("VM instance created (stopped state), restoring saved state...");
            handle.restore_state(&state_path_str)?;
            info!("state restored (paused state), resuming VM...");
            handle.resume()?;
            info!("VM resumed from saved state");
            Ok(handle)
        })
        .await?;

        match result {
            Ok(vm_handle) => {
                // Clean up after successful restore
                let _ = std::fs::remove_file(&state_path);
                let _ = std::fs::remove_file(rootfs.with_extension("img.dirty"));

                let handle = Arc::new(vm_handle);
                *self.handle.write().await = None;
                self.handle_arc.write().await.replace(handle.clone());
                *self.state.write().await = VmState::Running;
                info!("VM restored from saved state (fast boot)");
                Ok(Some(handle))
            }
            Err(e) => {
                warn!(error = %e, "restore failed, recovering rootfs and cold booting");
                // Restore the original rootfs
                let _ = std::fs::remove_file(rootfs);
                let _ = std::fs::rename(rootfs.with_extension("img.dirty"), rootfs);
                let _ = std::fs::remove_file(&state_path);
                *self.state.write().await = VmState::Stopped;
                Ok(None)
            }
        }
    }

    fn build_ffi_config(&self) -> VmFfiConfig {
        let kernel_path = self.config.kernel_path.to_string_lossy().into_owned();
        let initrd_path = self.config.initrd_path.as_ref().and_then(|p| {
            if p.exists() {
                Some(p.to_string_lossy().into_owned())
            } else {
                None
            }
        });
        let rootfs_path = self.config.rootfs_path.to_string_lossy().into_owned();

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

        VmFfiConfig {
            cpu_count: self.config.vm.cpu_count,
            memory_bytes: self.config.vm.memory_bytes,
            kernel_path,
            initrd_path,
            rootfs_path,
            rosetta: self.config.vm.rosetta,
            vsock_control_port: self.config.vsock_control_port,
            vsock_docker_port: self.config.vsock_docker_port,
            shared_directories,
        }
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

pub fn vm_state_path() -> std::path::PathBuf {
    mako_data_dir().join("vm-state")
}

pub fn rootfs_snapshot_path() -> std::path::PathBuf {
    mako_data_dir().join("rootfs-snapshot.img")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vm_state_path_is_under_mako_dir() {
        let path = vm_state_path();
        assert!(path.to_string_lossy().contains(".mako"));
        assert!(path.to_string_lossy().ends_with("vm-state"));
    }

    #[test]
    fn vm_state_path_is_deterministic() {
        assert_eq!(vm_state_path(), vm_state_path());
    }
}
