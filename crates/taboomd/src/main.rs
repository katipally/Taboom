mod config;
mod audit;
mod consistency;
mod handler;
mod handoff;
mod hardware;
mod liveview;
mod local;
mod mcp;
mod network;
mod persona;
mod lease;
mod ipc;
mod recording;
mod tools;
mod totp;
mod vault;

use anyhow::Result;
use std::sync::Arc;
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

    let config = config::DaemonConfig::load()?;
    info!(home = %config.home.display(), "taboomd starting");

    config.ensure_dirs()?;

    let audit_log = Arc::new(audit::AuditLog::open(
        &config.home.join("audit").join("taboomd.jsonl"),
    )?);
    audit_log.log("daemon_start", "taboomd starting up")?;

    let personas = Arc::new(persona::PersonaRegistry::load(&config.home)?);
    info!(count = personas.count(), "loaded personas");

    validate_personas(&personas);

    let geo = network::GeoLookup::open(&config.home);
    let route_checker = Arc::new(network::RouteChecker::new(geo));
    validate_routes(&personas, &route_checker);
    info!("route checker initialized");

    let lease_mgr = lease::LeaseManager::new();

    let vault = Arc::new(vault::Vault::open(&config.home));
    info!(status = vault.status(), "vault initialized");

    let notify_spec = std::env::var("TABOOM_NOTIFY").unwrap_or_default();
    let notify_config = if notify_spec.is_empty() {
        handoff::HandoffNotifyConfig::default()
    } else {
        handoff::parse_notify_config(&notify_spec)
    };
    let handoff_mgr = handoff::HandoffManager::new(
        liveview::takeover_url(),
        notify_config,
    );

    let mcp_config = mcp::McpConfig::default();
    let public_url = std::env::var("TABOOM_PUBLIC_URL")
        .unwrap_or_else(|_| format!("http://localhost:{}", mcp_config.port));
    let recorder = Arc::new(recording::Recorder::open(&config.home, public_url)?);

    let handler = Arc::new(handler::ToolHandler::new(
        lease_mgr,
        handoff_mgr,
        Arc::clone(&personas),
        recorder,
    ));

    let mcp_handle = mcp::start_server(mcp_config, handler).await?;
    info!("MCP server started");

    let ipc_handle = ipc::start_listener(
        &config,
        Arc::clone(&vault),
        Arc::clone(&personas),
        Arc::clone(&audit_log),
    )
    .await?;
    info!("IPC listener started");

    info!("taboomd ready, waiting for shutdown signal");
    signal::ctrl_c().await?;
    info!("shutdown signal received");

    mcp_handle.abort();
    ipc_handle.abort();

    audit_log.log("daemon_stop", "taboomd shutting down")?;
    cleanup_socket(&config);

    drop(vault);
    drop(route_checker);
    drop(personas);
    info!("taboomd stopped");
    Ok(())
}

fn validate_personas(personas: &persona::PersonaRegistry) {
    for p in &personas.list() {
        let report = consistency::check_persona(p);
        if !report.all_passed() {
            for c in &report.checks {
                if !c.passed {
                    warn!(
                        persona = %p.name,
                        check = %c.name,
                        detail = %c.detail,
                        "consistency check failed"
                    );
                }
            }
        }
    }

    let all = personas.list();
    for i in 0..all.len() {
        for j in (i + 1)..all.len() {
            let check = consistency::check_pair_distinct(&all[i], &all[j]);
            if !check.passed {
                warn!(check = %check.name, detail = %check.detail, "pair distinctness failed");
            }
        }
    }
}

fn validate_routes(
    personas: &persona::PersonaRegistry,
    route_checker: &network::RouteChecker,
) {
    if !route_checker.geo_available() {
        info!("GeoIP databases not available, skipping exit geo checks");
    }

    for p in personas.list() {
        let warnings = route_checker.validate_at_creation(&p.route, &p.timezone);
        for w in warnings {
            warn!(persona = %p.name, "{w}");
        }

        if let persona::RouteConfig::Proxy { address, .. } = &p.route {
            if let Ok(ip) = address.parse::<std::net::IpAddr>() {
                let health = route_checker.check_exit_geo(ip, &p.timezone);
                if health.tz_mismatch {
                    warn!(
                        persona = %p.name,
                        country = ?health.geo.country,
                        "exit geo does not match persona timezone"
                    );
                }
                if health.is_datacenter {
                    warn!(persona = %p.name, "exit IP belongs to a datacenter ASN");
                }
            }
        }
    }
}

fn cleanup_socket(config: &config::DaemonConfig) {
    let sock = config.home.join("run").join("taboomd.sock");
    let _ = std::fs::remove_file(sock);
}
