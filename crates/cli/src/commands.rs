use colored::Colorize;
use mako_common::config::{mako_data_dir, MakoConfig};
use std::process::Command;

pub async fn start(cpus: Option<u32>, memory: Option<u32>, foreground: bool) -> anyhow::Result<()> {
    let mut config = MakoConfig::load()?;
    if let Some(c) = cpus {
        config.vm.cpu_count = c;
    }
    if let Some(m) = memory {
        config.vm.memory_bytes = (m as u64) * 1024 * 1024 * 1024;
    }
    config.save()?;

    // Check that VM image exists
    if !config.kernel_path.exists() || !config.rootfs_path.exists() {
        eprintln!(
            "{} VM image not found. Run {} first.",
            "Error:".red().bold(),
            "mako setup".cyan()
        );
        std::process::exit(1);
    }

    println!("{}", "Starting Mako...".green().bold());
    println!("  CPUs:   {}", config.vm.cpu_count.to_string().cyan());
    println!(
        "  Memory: {} GiB",
        (config.vm.memory_bytes / (1024 * 1024 * 1024))
            .to_string()
            .cyan()
    );
    println!(
        "  Docker: {}",
        config.docker_socket_path.display().to_string().cyan()
    );

    // Find the makod binary (same directory as the mako binary)
    let makod_path = std::env::current_exe()?.parent().unwrap().join("makod");

    if !makod_path.exists() {
        eprintln!(
            "{} makod binary not found at {}",
            "Error:".red().bold(),
            makod_path.display()
        );
        eprintln!("Build it with: cargo build --release -p makod");
        std::process::exit(1);
    }

    // Ensure makod is signed with virtualization entitlement
    let entitlements_path = makod_path.with_file_name("../../../crates/daemon/entitlements.plist");
    let workspace_entitlements = find_workspace_root()
        .ok()
        .map(|r| r.join("crates/daemon/entitlements.plist"));
    let ent_path = if entitlements_path.exists() {
        Some(entitlements_path)
    } else {
        workspace_entitlements.filter(|p| p.exists())
    };
    if let Some(ent) = ent_path {
        let _ = Command::new("codesign")
            .args(["--entitlements"])
            .arg(&ent)
            .args(["--force", "-s", "-"])
            .arg(&makod_path)
            .output();
    }

    if foreground {
        println!("{}", "Running in foreground (Ctrl+C to stop)...".yellow());
        let status = Command::new(&makod_path).status()?;
        if !status.success() {
            anyhow::bail!("makod exited with status: {status}");
        }
    } else {
        let child = Command::new(&makod_path)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()?;

        println!("  PID:    {}", child.id().to_string().cyan());
        println!();
        println!(
            "VM is starting. Use {} to check status.",
            "mako status".cyan()
        );
        println!(
            "To use Docker: {}",
            format!(
                "export DOCKER_HOST=unix://{}",
                config.docker_socket_path.display()
            )
            .yellow()
        );
    }

    Ok(())
}

pub async fn stop() -> anyhow::Result<()> {
    println!("{}", "Stopping Mako...".green().bold());

    let pid_file = mako_data_dir().join("makod.pid");
    let mut stopped = false;

    if pid_file.exists() {
        if let Ok(pid_str) = std::fs::read_to_string(&pid_file) {
            if let Ok(pid) = pid_str.trim().parse::<i32>() {
                unsafe {
                    if libc::kill(pid, libc::SIGTERM) == 0 {
                        println!("  Sent SIGTERM to makod (PID {})", pid);
                        // Wait up to 5s for graceful shutdown
                        for _ in 0..50 {
                            std::thread::sleep(std::time::Duration::from_millis(100));
                            if libc::kill(pid, 0) != 0 {
                                break;
                            }
                        }
                        stopped = true;
                    }
                }
            }
        }
        std::fs::remove_file(&pid_file).ok();
    }

    if !stopped {
        let output = Command::new("pkill").args(["-f", "makod"]).output()?;
        stopped = output.status.success();
    }

    if stopped {
        println!("{}", "Mako stopped.".green());
    } else {
        println!("{}", "No running Mako instance found.".yellow());
    }

    let config = MakoConfig::load()?;
    if config.docker_socket_path.exists() {
        std::fs::remove_file(&config.docker_socket_path).ok();
    }

    Ok(())
}

pub async fn status() -> anyhow::Result<()> {
    let config = MakoConfig::load()?;

    println!("{}", "Mako Status".bold());

    // Check if makod is running
    let makod_running = Command::new("pgrep")
        .args(["-f", "makod"])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);

    if makod_running {
        println!("  VM:     {}", "running".green());
    } else {
        println!("  VM:     {}", "stopped".red());
    }

    // Check if Docker socket exists and is responsive
    if config.docker_socket_path.exists() {
        println!("  Docker: {}", "available".green());
        println!(
            "  Socket: {}",
            config.docker_socket_path.display().to_string().cyan()
        );
    } else {
        println!("  Docker: {}", "not available".red());
    }

    // Check VM image
    if config.kernel_path.exists() && config.rootfs_path.exists() {
        println!("  Image:  {}", "installed".green());
    } else {
        println!(
            "  Image:  {} (run {})",
            "not installed".red(),
            "mako setup".cyan()
        );
    }

    Ok(())
}

pub async fn setup() -> anyhow::Result<()> {
    println!("{}", "Mako Setup".bold());

    let workspace_root = find_workspace_root()?;
    let setup_script = workspace_root.join("vm-image/scripts/setup-no-docker.sh");

    if !setup_script.exists() {
        anyhow::bail!(
            "Setup script not found at {}. Are you in the Mako project directory?",
            setup_script.display()
        );
    }

    // Check for e2fsprogs dependency
    let has_mke2fs = Command::new("sh")
        .args(["-c", "command -v mke2fs || test -x /opt/homebrew/opt/e2fsprogs/sbin/mke2fs || test -x /usr/local/opt/e2fsprogs/sbin/mke2fs"])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);

    if !has_mke2fs {
        println!(
            "{} mke2fs not found. Installing e2fsprogs...",
            "Note:".yellow().bold()
        );
        let status = Command::new("brew")
            .args(["install", "e2fsprogs"])
            .status()?;
        if !status.success() {
            anyhow::bail!("Failed to install e2fsprogs. Run: brew install e2fsprogs");
        }
    }

    println!("Building VM image (no Docker required)...\n");

    let status = Command::new("bash").arg(&setup_script).status()?;

    if !status.success() {
        anyhow::bail!("VM image build failed");
    }

    Ok(())
}

pub async fn info() -> anyhow::Result<()> {
    let config = MakoConfig::load()?;
    println!("{}", "Mako Info".bold());
    println!("  Version: {}", env!("CARGO_PKG_VERSION"));
    println!("  Arch:    {}", std::env::consts::ARCH);
    println!("  Kernel:  {}", config.kernel_path.display());
    println!("  Rootfs:  {}", config.rootfs_path.display());
    println!("  Socket:  {}", config.docker_socket_path.display());
    println!("  Data:    {}", mako_data_dir().display());
    Ok(())
}

pub async fn config_show() -> anyhow::Result<()> {
    let config = MakoConfig::load()?;
    println!("{}", serde_json::to_string_pretty(&config)?);
    Ok(())
}

pub async fn config_reset() -> anyhow::Result<()> {
    let config = MakoConfig::default();
    config.save()?;
    println!("{}", "Configuration reset to defaults".green());
    Ok(())
}

pub async fn config_set(key: &str, value: &str) -> anyhow::Result<()> {
    let mut config = MakoConfig::load()?;
    match key {
        "vm.cpus" => config.vm.cpu_count = value.parse()?,
        "vm.memory" => {
            let gib: u64 = value.parse()?;
            config.vm.memory_bytes = gib * 1024 * 1024 * 1024;
        }
        "vm.disk" => {
            let gib: u64 = value.parse()?;
            config.vm.disk_size_bytes = gib * 1024 * 1024 * 1024;
        }
        "vm.rosetta" => config.vm.rosetta = value.parse()?,
        key if key.starts_with("vm.share.") => {
            // vm.share.tag=/host/path or vm.share.tag=/host/path:ro
            let tag = key.strip_prefix("vm.share.").unwrap();
            let (path, read_only) = if value.ends_with(":ro") {
                (value.trim_end_matches(":ro").to_string(), true)
            } else {
                (value.to_string(), false)
            };
            config.vm.shared_directories.retain(|s| s.mount_tag != tag);
            config
                .vm
                .shared_directories
                .push(mako_common::types::SharedDirectory {
                    host_path: Some(std::path::PathBuf::from(path)),
                    mount_tag: tag.to_string(),
                    read_only,
                });
        }
        other => anyhow::bail!("unknown config key: {other}"),
    }
    config.save()?;
    println!("{} = {}", key.cyan(), value.green());
    Ok(())
}

// -- Kubernetes (K3s) commands --

pub async fn k8s_enable() -> anyhow::Result<()> {
    let config = MakoConfig::load()?;
    let socket = &config.docker_socket_path;

    if !socket.exists() {
        eprintln!(
            "{} Mako is not running. Start it with {}",
            "Error:".red().bold(),
            "mako start".cyan()
        );
        std::process::exit(1);
    }

    println!("{}", "Enabling Kubernetes (K3s)...".green().bold());

    // Run K3s as a Docker container inside the Mako VM
    let status = Command::new("docker")
        .env("DOCKER_HOST", format!("unix://{}", socket.display()))
        .args([
            "run",
            "-d",
            "--name",
            "mako-k3s",
            "--privileged",
            "--restart",
            "unless-stopped",
            "-p",
            "6443:6443",
            "-e",
            "K3S_KUBECONFIG_OUTPUT=/output/kubeconfig.yaml",
            "-e",
            "K3S_KUBECONFIG_MODE=644",
            "-v",
            "mako-k3s-data:/var/lib/rancher/k3s",
            "-v",
            "mako-k3s-output:/output",
            "rancher/k3s:latest",
            "server",
            "--docker",
            "--disable=traefik",
            "--write-kubeconfig-mode=644",
        ])
        .status()?;

    if !status.success() {
        // Maybe already running
        let check = Command::new("docker")
            .env("DOCKER_HOST", format!("unix://{}", socket.display()))
            .args(["ps", "-q", "-f", "name=mako-k3s"])
            .output()?;
        if !check.stdout.is_empty() {
            println!("{}", "K3s is already running.".yellow());
            return Ok(());
        }
        // Try to start stopped container
        Command::new("docker")
            .env("DOCKER_HOST", format!("unix://{}", socket.display()))
            .args(["start", "mako-k3s"])
            .status()?;
    }

    println!("  Waiting for K3s to be ready...");
    for i in 1..=30 {
        std::thread::sleep(std::time::Duration::from_secs(2));
        let output = Command::new("docker")
            .env("DOCKER_HOST", format!("unix://{}", socket.display()))
            .args([
                "exec",
                "mako-k3s",
                "kubectl",
                "get",
                "nodes",
                "--no-headers",
            ])
            .output()?;
        if output.status.success() {
            let stdout = String::from_utf8_lossy(&output.stdout);
            if stdout.contains("Ready") {
                println!("{}", "Kubernetes is ready!".green().bold());
                println!();
                println!("  Get kubeconfig: {}", "mako kubernetes kubeconfig".cyan());
                println!(
                    "  Or:             {}",
                    "mako kubernetes kubeconfig > ~/.kube/mako-config".cyan()
                );
                println!(
                    "                  {}",
                    "export KUBECONFIG=~/.kube/mako-config".yellow()
                );
                return Ok(());
            }
        }
        if i % 5 == 0 {
            println!("  Still waiting... ({i}s)");
        }
    }

    println!(
        "{}",
        "K3s started but may still be initializing. Check with: mako kubernetes status".yellow()
    );
    Ok(())
}

pub async fn k8s_disable() -> anyhow::Result<()> {
    let config = MakoConfig::load()?;
    let socket = &config.docker_socket_path;

    println!("{}", "Disabling Kubernetes...".green().bold());

    Command::new("docker")
        .env("DOCKER_HOST", format!("unix://{}", socket.display()))
        .args(["stop", "mako-k3s"])
        .status()?;

    Command::new("docker")
        .env("DOCKER_HOST", format!("unix://{}", socket.display()))
        .args(["rm", "mako-k3s"])
        .status()?;

    println!("{}", "Kubernetes disabled.".green());
    println!(
        "  Data volumes preserved. To remove: {}",
        "docker volume rm mako-k3s-data mako-k3s-output".cyan()
    );
    Ok(())
}

pub async fn k8s_status() -> anyhow::Result<()> {
    let config = MakoConfig::load()?;
    let socket = &config.docker_socket_path;

    if !socket.exists() {
        println!("  Mako:       {}", "not running".red());
        println!("  Kubernetes: {}", "unavailable".red());
        return Ok(());
    }

    let output = Command::new("docker")
        .env("DOCKER_HOST", format!("unix://{}", socket.display()))
        .args(["ps", "-q", "-f", "name=mako-k3s"])
        .output()?;

    if output.stdout.is_empty() {
        println!("  Kubernetes: {}", "disabled".yellow());
        return Ok(());
    }

    println!("  Kubernetes: {}", "enabled".green());

    let nodes = Command::new("docker")
        .env("DOCKER_HOST", format!("unix://{}", socket.display()))
        .args(["exec", "mako-k3s", "kubectl", "get", "nodes"])
        .output()?;

    if nodes.status.success() {
        println!();
        print!("{}", String::from_utf8_lossy(&nodes.stdout));
    }

    Ok(())
}

pub async fn k8s_kubeconfig() -> anyhow::Result<()> {
    let config = MakoConfig::load()?;
    let socket = &config.docker_socket_path;

    let output = Command::new("docker")
        .env("DOCKER_HOST", format!("unix://{}", socket.display()))
        .args(["exec", "mako-k3s", "cat", "/output/kubeconfig.yaml"])
        .output()?;

    if !output.status.success() {
        // Fallback: try inline kubeconfig
        let output = Command::new("docker")
            .env("DOCKER_HOST", format!("unix://{}", socket.display()))
            .args(["exec", "mako-k3s", "cat", "/etc/rancher/k3s/k3s.yaml"])
            .output()?;

        if !output.status.success() {
            eprintln!(
                "{} K3s is not running or kubeconfig not available. Enable with {}",
                "Error:".red().bold(),
                "mako kubernetes enable".cyan()
            );
            std::process::exit(1);
        }
        // Replace 127.0.0.1 with localhost for macOS access
        let kubeconfig =
            String::from_utf8_lossy(&output.stdout).replace("127.0.0.1:6443", "localhost:6443");
        print!("{kubeconfig}");
    } else {
        let kubeconfig =
            String::from_utf8_lossy(&output.stdout).replace("127.0.0.1:6443", "localhost:6443");
        print!("{kubeconfig}");
    }

    Ok(())
}

pub async fn docker_passthrough(args: &[&str]) -> anyhow::Result<()> {
    let config = MakoConfig::load()?;
    let socket = config.docker_socket_path;

    if !socket.exists() {
        eprintln!(
            "{} Mako is not running. Start it with {}",
            "Error:".red().bold(),
            "mako start".cyan()
        );
        std::process::exit(1);
    }

    let status = Command::new("docker")
        .args(args)
        .env("DOCKER_HOST", format!("unix://{}", socket.display()))
        .status()?;

    std::process::exit(status.code().unwrap_or(1));
}

fn find_workspace_root() -> anyhow::Result<std::path::PathBuf> {
    let mut dir = std::env::current_dir()?;
    loop {
        if dir.join("Cargo.toml").exists() && dir.join("vm-image").exists() {
            return Ok(dir);
        }
        if !dir.pop() {
            anyhow::bail!("Could not find Mako workspace root");
        }
    }
}
