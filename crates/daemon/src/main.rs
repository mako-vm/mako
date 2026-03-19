mod ffi;
mod socket_proxy;
mod vm;

use anyhow::Result;
use mako_common::config::MakoConfig;

use tracing::{error, info};

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

    let rt = tokio::runtime::Runtime::new().expect("failed to create tokio runtime");

    rt.spawn(async {
        if let Err(e) = run_daemon().await {
            error!(?e, "daemon error");
            std::process::exit(1);
        }
    });

    // macOS main run loop -- services DispatchQueue.main for VZ operations
    unsafe { core_foundation::CFRunLoopRun() };
}

async fn run_daemon() -> Result<()> {
    let config = MakoConfig::load()?;

    info!("booting Linux VM...");
    let vm_manager = vm::VmManager::new(config.clone())?;

    // Start the VM, but setup the vsock listener before waiting for MAKO_VM_READY
    // so the guest agent can connect as soon as it starts.
    let vm_handle = vm_manager.start_and_get_handle().await?;

    // Start vsock listener immediately (before guest finishes booting)
    let vsock_port = config.vsock_docker_port;
    let vm_for_listen = vm_handle.clone();
    tokio::task::spawn_blocking(move || vm_for_listen.vsock_listen(vsock_port)).await??;
    info!(vsock_port, "vsock listener ready for guest connections");

    // Now wait for the VM to be fully ready (dockerd started, etc.)
    vm_manager.wait_for_ready().await?;

    info!("VM is ready, starting Docker socket proxy");

    let proxy = socket_proxy::DockerSocketProxy::new(
        config.docker_socket_path.clone(),
        vm_handle,
        config.vsock_docker_port,
    );

    info!(
        socket = %config.docker_socket_path.display(),
        "Docker is available. Set DOCKER_HOST=unix://{}",
        config.docker_socket_path.display()
    );

    proxy.run().await?;

    Ok(())
}

mod core_foundation {
    extern "C" {
        pub fn CFRunLoopRun();
    }
}
