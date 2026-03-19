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

    // Find makod process and send SIGTERM
    let output = Command::new("pkill").args(["-f", "makod"]).output()?;

    if output.status.success() {
        println!("{}", "Mako stopped.".green());
    } else {
        println!("{}", "No running Mako instance found.".yellow());
    }

    // Clean up the socket file
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
        other => anyhow::bail!("unknown config key: {other}"),
    }
    config.save()?;
    println!("{} = {}", key.cyan(), value.green());
    Ok(())
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
