mod audit;
mod browser;
mod boot;
mod cdp;
mod consistency;
mod handler;
mod hardware;
mod ipc;
mod lease;
mod liveview;
mod local;
mod mcp;
mod network;
mod persona;
mod recording;
mod route;
mod tools;
mod totp;
mod vault;

use anyhow::{bail, Context, Result};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use tokio::signal;
use tracing::{info, warn};

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let env = DaemonEnv::from_process();
    env.ensure_dirs()?;
    match std::env::args().nth(1).as_deref() {
        Some("boot-check") => boot::boot_check(&env.home, &env.persona).await,
        Some("browser") => {
            let profile = std::env::var_os("TABOOM_PROFILE")
                .map(PathBuf::from)
                .context("TABOOM_PROFILE was not set by boot-check")?;
            let engine = std::env::var("TABOOM_BROWSER_ENGINE")
                .context("TABOOM_BROWSER_ENGINE was not set by boot-check")?;
            let engine = browser::parse_engine(&engine)?;
            let args: Vec<String> = std::env::args().skip(2).collect();
            browser::launch(&env.home, &profile, engine, &args)
        }
        Some("browser-check") => browser::check(&env.home),
        Some("serve") | None => serve(env).await,
        Some(other) => bail!("unknown command {other:?}; use boot-check or serve"),
    }
}

async fn serve(env: DaemonEnv) -> Result<()> {
    info!(home = %env.home.display(), persona = %env.persona, "taboomd starting");

    let audit_log = Arc::new(audit::AuditLog::open(
        &env.home.join("audit").join("taboomd.jsonl"),
    )?);
    audit_log.log("daemon_start", "taboomd starting up")?;

    // boot-check created and validated it moments ago; serving never invents a persona
    let path = env.home.join("personas").join(format!("{}.toml", env.persona));
    let text = std::fs::read_to_string(&path).with_context(|| format!("reading {} (run boot-check first)", path.display()))?;
    let persona = Arc::new(persona::Persona::parse(&text).with_context(|| format!("parsing {}", path.display()))?);

    let upstream = route::Upstream::resolve(&persona, &env.home)?;
    let monitor = Arc::new(route::Monitor::new(
        Arc::clone(&persona),
        upstream,
        network::GeoLookup::open(&env.home),
    ));
    // The entrypoint launches Chrome only after serve is up, so failing here keeps it closed.
    let first = monitor.recheck().await;
    if !first.ok {
        bail!("route check failed at startup: {}", first.detail);
    }
    let every = std::env::var("TABOOM_ROUTE_RECHECK_S")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(300u64)
        .max(30);
    Arc::clone(&monitor).run(Duration::from_secs(every)).await?;
    info!(every_s = every, "route monitor started");

    let local = local::LocalExecutor::new();
    let (probe, declared) = (local.clone(), Arc::clone(&persona));
    std::thread::spawn(move || {
        // The entrypoint starts Chrome after serve is up, and the check reads Chrome's process.
        for _ in 0..120 {
            if consistency::browser_running() {
                break;
            }
            std::thread::sleep(Duration::from_millis(500));
        }
        let report = consistency::check_applied(&declared, &consistency::observe(probe.output().ok(), &declared));
        if report.all_passed() {
            info!("persona applied as declared");
        }
        for c in report.checks.iter().filter(|c| !c.passed) {
            warn!(check = %c.name, detail = %c.detail, "persona not applied as declared");
        }
    });

    let vault = Arc::new(vault::Vault::open(&env.home));
    info!(status = vault.status(), "vault initialized");

    let mcp_config = mcp::McpConfig::default();
    let public_url = std::env::var("TABOOM_PUBLIC_URL")
        .unwrap_or_else(|_| format!("http://localhost:{}", mcp_config.port));
    let recorder = Arc::new(recording::Recorder::open(&env.home, public_url)?);

    let handler = Arc::new(
        handler::ToolHandler::new(persona, monitor, recorder, local).with_secret_context(
            Arc::clone(&vault),
            Arc::clone(&audit_log),
            Arc::new(cdp::CdpPageTargets::new(&env.home)),
            &env.home,
        )?,
    );

    let mcp_handle = mcp::start_server(mcp_config, Arc::clone(&handler), Arc::clone(&audit_log), &env.home).await?;
    info!("MCP server started");

    let ipc_handle = ipc::start_listener(
        &env.home,
        Arc::clone(&vault),
        Arc::clone(&audit_log),
    )
    .await?;
    info!("IPC listener started");

    info!("taboomd ready, waiting for shutdown signal");
    // `docker stop` sends SIGTERM; Ctrl+C in a foreground run sends SIGINT
    let mut term = signal::unix::signal(signal::unix::SignalKind::terminate())?;
    tokio::select! {
        r = signal::ctrl_c() => r?,
        _ = term.recv() => {}
    }
    info!("shutdown signal received");

    mcp_handle.abort();
    ipc_handle.abort();
    let session = Arc::clone(&handler);
    if let Err(e) = tokio::task::spawn_blocking(move || session.shutdown()).await {
        warn!("session cleanup at shutdown failed: {e}");
    }

    audit_log.log("daemon_stop", "taboomd shutting down")?;
    cleanup_socket(&env.home);

    drop(vault);
    info!("taboomd stopped");
    Ok(())
}

fn cleanup_socket(home: &Path) {
    let sock = home.join("run").join("taboomd.sock");
    let _ = std::fs::remove_file(sock);
}

struct DaemonEnv {
    home: PathBuf,
    /// TABOOM_PERSONA, default "default": the one persona this container runs.
    persona: String,
}

impl DaemonEnv {
    fn from_process() -> Self {
        let home = std::env::var("TABOOM_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|_| {
                let home = std::env::var_os("HOME")
                    .map(PathBuf::from)
                    .unwrap_or_else(|| PathBuf::from("."));
                home.join(".taboom")
            });
        let persona = std::env::var("TABOOM_PERSONA").ok().filter(|p| !p.is_empty()).unwrap_or_else(|| "default".into());
        Self { home, persona }
    }

    fn ensure_dirs(&self) -> Result<()> {
        for sub in ["personas", "run", "audit", "logs", "vault"] {
            let dir = self.home.join(sub);
            std::fs::create_dir_all(&dir)
                .with_context(|| format!("creating {}", dir.display()))?;
        }
        Ok(())
    }
}
