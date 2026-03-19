mod dns;
mod ffi;
mod memory;
mod port_forward;
mod socket_proxy;
mod vm;

use anyhow::Result;
use mako_common::config::MakoConfig;
use std::sync::Arc;
use tokio::sync::watch;
use tracing::{error, info, warn};

/// makod -- the Mako daemon.
///
/// Main thread runs Apple's CFRunLoop (required by Virtualization.framework).
/// All async logic runs on tokio worker threads.
fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive("makod=debug".parse().unwrap()),
        )
        .init();

    info!(version = env!("CARGO_PKG_VERSION"), "starting makod");

    write_pid_file();

    let rt = tokio::runtime::Runtime::new().expect("failed to create tokio runtime");

    rt.spawn(async {
        if let Err(e) = run_daemon().await {
            error!(?e, "daemon error");
            cleanup_pid_file();
            std::process::exit(1);
        }
        // run_daemon completed (shutdown finished), now safe to exit
        info!("daemon shutdown complete, exiting");
        unsafe { core_foundation::CFRunLoopStop(core_foundation::CFRunLoopGetMain()) };
    });

    unsafe { core_foundation::CFRunLoopRun() };
}

async fn run_daemon() -> Result<()> {
    let config = MakoConfig::load()?;
    let (shutdown_tx, shutdown_rx) = watch::channel(false);

    // Spawn signal handler -- only sends shutdown signal; cleanup happens in run_daemon()
    tokio::spawn(async move {
        wait_for_shutdown_signal().await;
        info!("shutdown signal received");
        let _ = shutdown_tx.send(true);
    });

    let vm_manager = Arc::new(vm::VmManager::new(config.clone())?);

    // Try fast restore from saved VM state first
    let vm_handle = if let Some(restored_handle) = vm_manager.start_from_saved().await? {
        info!("VM restored from saved state (fast boot)");

        let vsock_port = config.vsock_docker_port;
        let vm_for_listen = restored_handle.clone();
        tokio::task::spawn_blocking(move || vm_for_listen.vsock_listen(vsock_port)).await??;
        info!(vsock_port, "vsock listener ready for guest connections");

        restored_handle
    } else {
        info!("booting Linux VM (cold start)...");

        let handle = vm_manager.start_and_get_handle().await?;

        let vsock_port = config.vsock_docker_port;
        let vm_for_listen = handle.clone();
        tokio::task::spawn_blocking(move || vm_for_listen.vsock_listen(vsock_port)).await??;
        info!(vsock_port, "vsock listener ready for guest connections");

        vm_manager.wait_for_ready().await?;

        handle
    };

    info!("VM is ready, starting Docker socket proxy");

    let proxy = socket_proxy::DockerSocketProxy::new(
        config.docker_socket_path.clone(),
        vm_handle.clone(),
        config.vsock_docker_port,
    );

    info!(
        socket = %config.docker_socket_path.display(),
        "Docker is available. Set DOCKER_HOST=unix://{}",
        config.docker_socket_path.display()
    );

    // Start port forwarder (monitors containers and creates localhost listeners)
    let port_fwd = port_forward::PortForwarder::new(
        vm_handle,
        vm_manager.vm_ip_ref(),
        config.docker_socket_path.clone(),
    );
    let port_fwd_shutdown = shutdown_rx.clone();
    tokio::spawn(async move {
        port_fwd.run(port_fwd_shutdown).await;
    });

    // Start DNS forwarder (resolves *.mako.local to container IPs)
    let dns_fwd = dns::DnsForwarder::new(config.docker_socket_path.clone(), vm_manager.vm_ip_ref());
    let dns_shutdown = shutdown_rx.clone();
    tokio::spawn(async move {
        dns_fwd.run(dns_shutdown).await;
    });

    // Start memory monitor (tracks container memory usage)
    let mem_monitor =
        memory::MemoryMonitor::new(config.docker_socket_path.clone(), config.vm.memory_bytes);
    let _mem_stats = mem_monitor.stats_ref();
    let mem_shutdown = shutdown_rx.clone();
    tokio::spawn(async move {
        mem_monitor.run(mem_shutdown).await;
    });

    proxy.run(shutdown_rx).await?;

    // If we get here, shutdown was requested -- try to save state for fast resume
    info!("proxy stopped, saving VM state for fast resume...");
    match vm_manager.stop_and_save().await {
        Ok(true) => info!("VM state saved -- next start will be fast"),
        Ok(false) => info!("VM stopped without saving (cold boot on next start)"),
        Err(e) => {
            warn!(?e, "VM save/stop error, forcing stop");
            if let Err(e2) = vm_manager.stop().await {
                warn!(?e2, "VM stop also failed (may already be stopped)");
            }
        }
    }

    // Clean up socket and PID file
    if config.docker_socket_path.exists() {
        std::fs::remove_file(&config.docker_socket_path).ok();
        info!("removed docker socket");
    }
    cleanup_pid_file();
    info!("mako daemon stopped cleanly");

    Ok(())
}

async fn wait_for_shutdown_signal() {
    let ctrl_c = tokio::signal::ctrl_c();
    let mut sigterm =
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()).unwrap();
    tokio::select! {
        _ = ctrl_c => info!("received SIGINT"),
        _ = sigterm.recv() => info!("received SIGTERM"),
    }
}

fn pid_file_path() -> std::path::PathBuf {
    mako_common::config::mako_data_dir().join("makod.pid")
}

fn write_pid_file() {
    let path = pid_file_path();
    if let Some(parent) = path.parent() {
        if let Err(e) = std::fs::create_dir_all(parent) {
            eprintln!("warning: failed to create PID dir: {e}");
        }
    }
    match std::fs::write(&path, std::process::id().to_string()) {
        Ok(_) => eprintln!("PID file: {}", path.display()),
        Err(e) => eprintln!("warning: failed to write PID file {}: {e}", path.display()),
    }
}

fn cleanup_pid_file() {
    std::fs::remove_file(pid_file_path()).ok();
}

mod core_foundation {
    extern "C" {
        pub fn CFRunLoopRun();
        pub fn CFRunLoopGetMain() -> *mut std::ffi::c_void;
        pub fn CFRunLoopStop(rl: *mut std::ffi::c_void);
    }
}
