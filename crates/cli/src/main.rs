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
        } => commands::start(cpus, memory, foreground).await,
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
    }
}
