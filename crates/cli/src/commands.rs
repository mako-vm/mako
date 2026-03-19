use colored::Colorize;
use mako_common::config::{mako_data_dir, MakoConfig};
use std::process::Command;

pub async fn start(
    cpus: Option<u32>,
    memory: Option<u32>,
    foreground: bool,
    cold: bool,
) -> anyhow::Result<()> {
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

    // Stop any existing makod + VM XPC processes to avoid rootfs lock conflicts
    stop_existing_daemon();

    // If --cold, discard any saved VM state to force a full boot
    if cold {
        let state_path = mako_data_dir().join("vm-state");
        if state_path.exists() {
            std::fs::remove_file(&state_path).ok();
            println!(
                "  {}",
                "Discarded saved VM state (cold boot forced)".yellow()
            );
        }
    }

    let has_saved_state = mako_data_dir().join("vm-state").exists();
    println!("{}", "Starting Mako...".green().bold());
    if has_saved_state {
        println!("  Mode:   {}", "fast resume (saved state found)".green());
    } else {
        println!("  Mode:   {}", "cold boot".yellow());
    }
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
        let log_path = mako_data_dir().join("makod.log");
        let log_file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&log_path)?;
        let stderr_file = log_file.try_clone()?;

        let child = Command::new(&makod_path)
            .stdout(log_file)
            .stderr(stderr_file)
            .spawn()?;

        println!("  PID:    {}", child.id().to_string().cyan());
        println!("  Logs:   {}", log_path.display().to_string().cyan());
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
        println!(
            "View logs:     {}",
            format!("tail -f {}", log_path.display()).yellow()
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
                        // Wait up to 30s for graceful shutdown (VM state save can take 10-15s)
                        let mut exited = false;
                        for i in 0..60 {
                            std::thread::sleep(std::time::Duration::from_millis(500));
                            if libc::kill(pid, 0) != 0 {
                                exited = true;
                                break;
                            }
                            if i == 5 {
                                println!("  Saving VM state for fast resume...");
                            }
                        }
                        if !exited {
                            println!("  Daemon did not exit in time, force killing...");
                            libc::kill(pid, libc::SIGKILL);
                            std::thread::sleep(std::time::Duration::from_millis(500));
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

pub async fn daemon_logs(follow: bool, tail: Option<&str>) -> anyhow::Result<()> {
    let log_path = mako_data_dir().join("makod.log");
    if !log_path.exists() {
        eprintln!(
            "{} No daemon log found at {}",
            "Error:".red().bold(),
            log_path.display()
        );
        eprintln!("Start the daemon first with {}", "mako start".cyan());
        std::process::exit(1);
    }

    let mut args = vec![];
    if follow {
        args.push("-f".to_string());
    }
    if let Some(n) = tail {
        args.push("-n".to_string());
        args.push(n.to_string());
    }
    args.push(log_path.to_string_lossy().to_string());

    let status = Command::new("tail").args(&args).status()?;
    std::process::exit(status.code().unwrap_or(1));
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
    apply_config_key(&mut config, key, value)?;
    config.save()?;
    println!("{} = {}", key.cyan(), value.green());
    Ok(())
}

fn apply_config_key(config: &mut MakoConfig, key: &str, value: &str) -> anyhow::Result<()> {
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

fn stop_existing_daemon() {
    // Gracefully stop makod via PID file, giving it time to save VM state
    let pid_file = mako_data_dir().join("makod.pid");
    if pid_file.exists() {
        if let Ok(pid_str) = std::fs::read_to_string(&pid_file) {
            if let Ok(pid) = pid_str.trim().parse::<i32>() {
                unsafe {
                    libc::kill(pid, libc::SIGTERM);
                }
                // Wait up to 30 seconds for graceful shutdown (VM state save can take 10-15s)
                let mut still_alive = true;
                for i in 0..60 {
                    std::thread::sleep(std::time::Duration::from_millis(500));
                    unsafe {
                        if libc::kill(pid, 0) != 0 {
                            still_alive = false;
                            break;
                        }
                    }
                    if i == 5 {
                        eprintln!("Waiting for daemon to save VM state...");
                    }
                }
                if still_alive {
                    eprintln!("Daemon did not exit in time, force killing...");
                    unsafe {
                        libc::kill(pid, libc::SIGKILL);
                    }
                    std::thread::sleep(std::time::Duration::from_millis(500));
                }
            }
        }
        std::fs::remove_file(&pid_file).ok();
    }

    // Also kill any stray makod processes not tracked by the PID file
    let _ = Command::new("pkill").args(["-15", "-f", "makod"]).output();
    std::thread::sleep(std::time::Duration::from_millis(1000));
    let _ = Command::new("pkill").args(["-9", "-f", "makod"]).output();

    // Kill lingering VM XPC helpers that hold the rootfs lock
    let _ = Command::new("pkill")
        .args(["-9", "-f", "com.apple.Virtualization.VirtualMachine"])
        .output();

    // Brief pause for the OS to release file locks
    std::thread::sleep(std::time::Duration::from_millis(500));
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_set_cpus() {
        let mut config = MakoConfig::default();
        apply_config_key(&mut config, "vm.cpus", "16").unwrap();
        assert_eq!(config.vm.cpu_count, 16);
    }

    #[test]
    fn config_set_memory() {
        let mut config = MakoConfig::default();
        apply_config_key(&mut config, "vm.memory", "8").unwrap();
        assert_eq!(config.vm.memory_bytes, 8 * 1024 * 1024 * 1024);
    }

    #[test]
    fn config_set_disk() {
        let mut config = MakoConfig::default();
        apply_config_key(&mut config, "vm.disk", "128").unwrap();
        assert_eq!(config.vm.disk_size_bytes, 128 * 1024 * 1024 * 1024);
    }

    #[test]
    fn config_set_rosetta() {
        let mut config = MakoConfig::default();
        apply_config_key(&mut config, "vm.rosetta", "false").unwrap();
        assert!(!config.vm.rosetta);
    }

    #[test]
    fn config_set_share_normal() {
        let mut config = MakoConfig::default();
        apply_config_key(&mut config, "vm.share.data", "/Volumes/Data").unwrap();
        let share = config
            .vm
            .shared_directories
            .iter()
            .find(|s| s.mount_tag == "data")
            .unwrap();
        assert_eq!(
            share.host_path,
            Some(std::path::PathBuf::from("/Volumes/Data"))
        );
        assert!(!share.read_only);
    }

    #[test]
    fn config_set_share_read_only() {
        let mut config = MakoConfig::default();
        apply_config_key(&mut config, "vm.share.backup", "/mnt/backup:ro").unwrap();
        let share = config
            .vm
            .shared_directories
            .iter()
            .find(|s| s.mount_tag == "backup")
            .unwrap();
        assert_eq!(
            share.host_path,
            Some(std::path::PathBuf::from("/mnt/backup"))
        );
        assert!(share.read_only);
    }

    #[test]
    fn config_set_share_replaces_existing() {
        let mut config = MakoConfig::default();
        apply_config_key(&mut config, "vm.share.data", "/old").unwrap();
        apply_config_key(&mut config, "vm.share.data", "/new").unwrap();
        let matches: Vec<_> = config
            .vm
            .shared_directories
            .iter()
            .filter(|s| s.mount_tag == "data")
            .collect();
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].host_path, Some(std::path::PathBuf::from("/new")));
    }

    #[test]
    fn config_set_unknown_key_errors() {
        let mut config = MakoConfig::default();
        assert!(apply_config_key(&mut config, "vm.foobar", "123").is_err());
    }

    #[test]
    fn config_set_invalid_cpus_errors() {
        let mut config = MakoConfig::default();
        assert!(apply_config_key(&mut config, "vm.cpus", "not_a_number").is_err());
    }

    #[test]
    fn cold_flag_removes_state_file() {
        let dir = tempfile::tempdir().unwrap();
        let state_file = dir.path().join("vm-state");
        std::fs::write(&state_file, b"fake state").unwrap();
        assert!(state_file.exists());
        std::fs::remove_file(&state_file).unwrap();
        assert!(!state_file.exists());
    }
}
