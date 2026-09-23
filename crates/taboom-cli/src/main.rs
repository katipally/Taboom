mod cmd;
mod doctor;

use anyhow::Result;
use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "taboom", about = "Self-hosted computer-use appliance")]
struct Cli {
    #[command(subcommand)]
    command: Commands,

    #[arg(long, env = "TABOOM_HOME")]
    home: Option<std::path::PathBuf>,
}

#[derive(Subcommand)]
enum Commands {
    Doctor,
    Persona {
        #[command(subcommand)]
        action: PersonaAction,
    },
    Vault {
        #[command(subcommand)]
        action: VaultAction,
    },
    Logs {
        #[arg(help = "Persona name")]
        persona: Option<String>,
        #[arg(short = 'n', default_value = "50")]
        lines: usize,
    },
    Connect {
        #[command(subcommand)]
        target: ConnectTarget,
    },
    Stdio {
        #[arg(long, default_value = "127.0.0.1")]
        host: String,
        #[arg(long, default_value = "3456")]
        port: u16,
        #[arg(long)]
        token: Option<String>,
    },
}

#[derive(Subcommand)]
enum ConnectTarget {
    ClaudeCode {
        #[arg(long, default_value = "127.0.0.1")]
        host: String,
        #[arg(long, default_value = "3456")]
        port: u16,
        #[arg(long)]
        token: Option<String>,
    },
    Generic {
        #[arg(long, default_value = "127.0.0.1")]
        host: String,
        #[arg(long, default_value = "3456")]
        port: u16,
        #[arg(long)]
        token: Option<String>,
    },
}

#[derive(Subcommand)]
enum PersonaAction {
    Create {
        #[arg(help = "Persona name")]
        name: String,
        #[arg(long, default_value = "2")]
        cpus: u32,
        #[arg(long, default_value = "4096")]
        ram: u32,
    },
    List,
    Show {
        name: String,
    },
    Delete {
        name: String,
    },
}

#[derive(Subcommand)]
enum VaultAction {
    Init,
    Unlock,
    Lock,
    Add {
        #[arg(help = "Secret name")]
        name: String,
        #[arg(long, value_parser = parse_secret_type, default_value = "password")]
        r#type: String,
        #[arg(long, value_delimiter = ',')]
        domains: Vec<String>,
    },
    List,
    Remove {
        #[arg(help = "Secret name")]
        name: String,
    },
    Status,
}

fn parse_secret_type(s: &str) -> Result<String, String> {
    match s {
        "password" | "totp" | "note" => Ok(s.to_string()),
        _ => Err(format!("invalid type: {s} (expected password, totp, or note)")),
    }
}

fn taboom_home(cli_home: Option<&std::path::Path>) -> std::path::PathBuf {
    if let Some(h) = cli_home {
        return h.to_path_buf();
    }
    dirs_next().join(".taboom")
}

fn dirs_next() -> std::path::PathBuf {
    home_dir().unwrap_or_else(|| std::path::PathBuf::from("."))
}

fn home_dir() -> Option<std::path::PathBuf> {
    std::env::var_os("HOME").map(std::path::PathBuf::from)
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let cli = Cli::parse();
    let home = taboom_home(cli.home.as_deref());

    match cli.command {
        Commands::Doctor => doctor::run().await,
        Commands::Persona { action } => match action {
            PersonaAction::Create { name, cpus, ram } => {
                cmd::persona_create(&home, &name, cpus, ram).await
            }
            PersonaAction::List => cmd::persona_list(&home).await,
            PersonaAction::Show { name } => cmd::persona_show(&home, &name).await,
            PersonaAction::Delete { name } => cmd::persona_delete(&home, &name).await,
        },
        Commands::Vault { action } => match action {
            VaultAction::Init => cmd::vault_init(&home).await,
            VaultAction::Unlock => cmd::vault_unlock(&home).await,
            VaultAction::Lock => cmd::vault_lock(&home).await,
            VaultAction::Add {
                name,
                r#type,
                domains,
            } => cmd::vault_add(&home, &name, &r#type, domains).await,
            VaultAction::List => cmd::vault_list(&home).await,
            VaultAction::Remove { name } => cmd::vault_remove(&home, &name).await,
            VaultAction::Status => cmd::vault_status(&home).await,
        }
        Commands::Logs { persona, lines } => cmd::logs(&home, persona.as_deref(), lines).await,
        Commands::Connect { target } => match target {
            ConnectTarget::ClaudeCode { host, port, token } => {
                cmd::connect_claude_code(&host, port, token.as_deref()).await
            }
            ConnectTarget::Generic { host, port, token } => {
                cmd::connect_generic(&host, port, token.as_deref()).await
            }
        },
        Commands::Stdio { host, port, token } => {
            cmd::stdio_bridge(&host, port, token.as_deref()).await
        }
    }
}
