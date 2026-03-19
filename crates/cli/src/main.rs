mod commands;

use clap::{CommandFactory, Parser, Subcommand};
use clap_complete::Shell;

#[derive(Parser)]
#[command(name = "mako", about = "Fast, lightweight Docker for macOS", version)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Start the Mako VM and Docker engine
    Start {
        /// Number of CPUs to allocate
        #[arg(long)]
        cpus: Option<u32>,
        /// Memory in GiB
        #[arg(long)]
        memory: Option<u32>,
        /// Run in foreground (don't daemonize)
        #[arg(long, short)]
        foreground: bool,
        /// Force a cold boot (discard saved VM state)
        #[arg(long)]
        cold: bool,
    },
    /// Stop the Mako VM
    Stop,
    /// Show status of the VM and Docker engine
    Status,
    /// Build and install the VM image
    Setup,
    /// Show Mako version and system info
    Info,
    /// Edit Mako configuration
    Config {
        #[command(subcommand)]
        action: ConfigAction,
    },
    /// Generate shell completions
    Completions {
        /// Shell to generate completions for
        #[arg(value_enum)]
        shell: Shell,
    },
    /// List Docker images
    Images,
    /// Show container logs (or daemon logs with --daemon)
    Logs {
        /// Container name or ID (omit when using --daemon)
        container: Option<String>,
        /// Show makod daemon logs instead of container logs
        #[arg(long)]
        daemon: bool,
        /// Follow log output
        #[arg(long, short)]
        follow: bool,
        /// Number of lines to show from the end
        #[arg(long, short = 'n')]
        tail: Option<String>,
    },
    /// Execute a command in a running container
    Exec {
        /// Container name or ID
        container: String,
        /// Command and arguments
        #[arg(trailing_var_arg = true, required = true)]
        command: Vec<String>,
        /// Interactive mode
        #[arg(long, short)]
        interactive: bool,
        /// Allocate a pseudo-TTY
        #[arg(long, short)]
        tty: bool,
    },
    /// List running containers (alias for docker ps)
    Ps {
        /// Show all containers (not just running)
        #[arg(long, short)]
        all: bool,
    },
    /// Manage Kubernetes (K3s)
    Kubernetes {
        #[command(subcommand)]
        action: KubernetesAction,
    },
    /// Run a container
    Run {
        /// Docker image
        image: String,
        /// Command and arguments
        #[arg(trailing_var_arg = true)]
        command: Vec<String>,
        /// Run in background
        #[arg(long, short)]
        detach: bool,
        /// Remove container on exit
        #[arg(long)]
        rm: bool,
        /// Container name
        #[arg(long)]
        name: Option<String>,
        /// Publish ports (e.g. 8080:80)
        #[arg(long, short)]
        publish: Vec<String>,
    },
}

#[derive(Subcommand)]
enum KubernetesAction {
    /// Enable Kubernetes (downloads and starts K3s in the VM)
    Enable,
    /// Disable Kubernetes (stops K3s)
    Disable,
    /// Show Kubernetes status
    Status,
    /// Print kubeconfig to stdout
    Kubeconfig,
}

#[derive(Subcommand)]
enum ConfigAction {
    /// Show current configuration
    Show,
    /// Reset to default configuration
    Reset,
    /// Set a configuration value
    Set { key: String, value: String },
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Commands::Start {
            cpus,
            memory,
            foreground,
            cold,
        } => commands::start(cpus, memory, foreground, cold).await,
        Commands::Stop => commands::stop().await,
        Commands::Status => commands::status().await,
        Commands::Setup => commands::setup().await,
        Commands::Info => commands::info().await,
        Commands::Config { action } => match action {
            ConfigAction::Show => commands::config_show().await,
            ConfigAction::Reset => commands::config_reset().await,
            ConfigAction::Set { key, value } => commands::config_set(&key, &value).await,
        },
        Commands::Completions { shell } => {
            clap_complete::generate(shell, &mut Cli::command(), "mako", &mut std::io::stdout());
            Ok(())
        }
        Commands::Kubernetes { action } => match action {
            KubernetesAction::Enable => commands::k8s_enable().await,
            KubernetesAction::Disable => commands::k8s_disable().await,
            KubernetesAction::Status => commands::k8s_status().await,
            KubernetesAction::Kubeconfig => commands::k8s_kubeconfig().await,
        },
        Commands::Images => commands::docker_passthrough(&["images"]).await,
        Commands::Logs {
            container,
            daemon,
            follow,
            tail,
        } => {
            if daemon {
                commands::daemon_logs(follow, tail.as_deref()).await
            } else {
                let container = container.unwrap_or_else(|| {
                    eprintln!("Error: container name required (or use --daemon for makod logs)");
                    std::process::exit(1);
                });
                let mut args = vec!["logs"];
                if follow {
                    args.push("-f");
                }
                let tail_val;
                if let Some(ref t) = tail {
                    args.push("--tail");
                    tail_val = t.clone();
                    args.push(&tail_val);
                }
                args.push(&container);
                commands::docker_passthrough(&args).await
            }
        }
        Commands::Exec {
            container,
            command,
            interactive,
            tty,
        } => {
            let mut args = vec!["exec".to_string()];
            if interactive {
                args.push("-i".to_string());
            }
            if tty {
                args.push("-t".to_string());
            }
            args.push(container);
            args.extend(command);
            let str_args: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
            commands::docker_passthrough(&str_args).await
        }
        Commands::Ps { all } => {
            let mut args = vec!["ps"];
            if all {
                args.push("-a");
            }
            commands::docker_passthrough(&args).await
        }
        Commands::Run {
            image,
            command,
            detach,
            rm,
            name,
            publish,
        } => {
            let mut args = vec!["run".to_string()];
            if detach {
                args.push("-d".to_string());
            }
            if rm {
                args.push("--rm".to_string());
            }
            if let Some(ref n) = name {
                args.push("--name".to_string());
                args.push(n.clone());
            }
            for p in &publish {
                args.push("-p".to_string());
                args.push(p.clone());
            }
            args.push(image);
            args.extend(command);
            let str_args: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
            commands::docker_passthrough(&str_args).await
        }
    }
}
