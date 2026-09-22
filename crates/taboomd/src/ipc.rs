use crate::audit::AuditLog;
use crate::config::DaemonConfig;
use crate::liveview;
use crate::persona::PersonaRegistry;
use crate::totp;
use crate::vault::{SecretType, Vault};
use anyhow::Result;
use serde_json::json;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::SystemTime;
use taboom_vmm::{QemuConfig, Platform, ScreenConfig, VmSupervisor};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixListener;
use tokio::sync::Mutex as AsyncMutex;
use tokio::task::JoinHandle;
use tracing::{error, info};

struct IpcState {
    vault: Arc<Vault>,
    personas: Arc<PersonaRegistry>,
    audit: Arc<AuditLog>,
    liveview_secret: Vec<u8>,
    home: std::path::PathBuf,
    supervisors: AsyncMutex<HashMap<String, VmSupervisor>>,
}

pub async fn start_listener(
    config: &DaemonConfig,
    vault: Arc<Vault>,
    personas: Arc<PersonaRegistry>,
    audit: Arc<AuditLog>,
) -> Result<JoinHandle<()>> {
    let sock_path = config.home.join("run").join("taboomd.sock");

    if sock_path.exists() {
        std::fs::remove_file(&sock_path)?;
    }

    let listener = UnixListener::bind(&sock_path)?;
    info!(path = %sock_path.display(), "IPC socket bound");

    let state = Arc::new(IpcState {
        vault,
        personas,
        audit,
        liveview_secret: b"taboom-liveview-default-key-0000".to_vec(),
        home: config.home.clone(),
        supervisors: AsyncMutex::new(HashMap::new()),
    });

    let handle = tokio::spawn(async move {
        loop {
            match listener.accept().await {
                Ok((stream, _)) => {
                    let st = Arc::clone(&state);
                    tokio::spawn(async move {
                        if let Err(e) = handle_connection(stream, &st).await {
                            error!("IPC connection error: {e}");
                        }
                    });
                }
                Err(e) => {
                    error!("IPC accept error: {e}");
                }
            }
        }
    });

    Ok(handle)
}

async fn handle_connection(
    stream: tokio::net::UnixStream,
    state: &IpcState,
) -> Result<()> {
    let (reader, mut writer) = stream.into_split();
    let mut lines = BufReader::new(reader).lines();

    while let Some(line) = lines.next_line().await? {
        let response = process_command(&line, state).await;
        writer.write_all(response.as_bytes()).await?;
        writer.write_all(b"\n").await?;
    }

    Ok(())
}

async fn process_command(line: &str, state: &IpcState) -> String {
    let parts: Vec<&str> = line.trim().splitn(2, ' ').collect();
    match parts.first().copied() {
        Some("ping") => json!({"status": "ok", "message": "pong"}).to_string(),
        Some("status") => {
            let sups = state.supervisors.lock().await;
            let running: Vec<&str> = sups.iter()
                .filter(|(_, s)| s.is_running())
                .map(|(name, _)| name.as_str())
                .collect();
            json!({"status": "ok", "vms": running}).to_string()
        }
        Some("start") => {
            let persona = parts.get(1).unwrap_or(&"");
            if persona.is_empty() {
                return json!({"status": "error", "message": "missing persona name"}).to_string();
            }
            match state.personas.get(persona) {
                Some(p) => {
                    let base_image = state.home.join("images").join("v1").join("base.qcow2");
                    if !base_image.exists() {
                        return json!({
                            "status": "error",
                            "message": "base image not found, run `taboom image build` first"
                        }).to_string();
                    }

                    let platform = match Platform::detect() {
                        Some(pl) => pl,
                        None => return json!({
                            "status": "error",
                            "message": "unsupported platform for QEMU"
                        }).to_string(),
                    };

                    let persona_dir = state.home.join("personas").join(persona);
                    let run_dir = state.home.join("run");
                    let screen = p.hardware.as_ref().map(|hw| ScreenConfig {
                        width: hw.screen_width,
                        height: hw.screen_height,
                        dpr: hw.dpr,
                    });

                    let overlay = persona_dir.join("root.qcow2");
                    let home = persona_dir.join("home.qcow2");
                    let cidata = state.home.join("cidata.iso");

                    let qemu_config = QemuConfig {
                        name: p.name.clone(),
                        cpus: p.cpus,
                        ram_mb: p.ram_mb,
                        base_image: base_image.clone(),
                        overlay_image: overlay,
                        home_image: home,
                        cloud_init_iso: if cidata.exists() { Some(cidata) } else { None },
                        qmp_socket: run_dir.join(format!("{}.qmp", p.name)),
                        control_socket: run_dir.join(format!("{}.ctl", p.name)),
                        pidfile: run_dir.join(format!("{}.pid", p.name)),
                        platform,
                        display: false,
                        route: p.route.to_proto_route(),
                        screen,
                    };

                    let browser_config = p.browser.to_proto_config();
                    let _ = state.audit.log_persona("vm_start", persona, &format!(
                        "cpus={} ram={}MB browser_langs={}",
                        p.cpus, p.ram_mb, browser_config.accept_languages,
                    ));

                    let mut supervisor = VmSupervisor::new(qemu_config);
                    match supervisor.start().await {
                        Ok(()) => {
                            info!(persona = %p.name, "VM started");
                            state.supervisors.lock().await.insert(p.name.clone(), supervisor);
                            json!({"status": "ok", "message": format!("VM started for '{persona}'")}).to_string()
                        }
                        Err(e) => {
                            error!(persona = %p.name, error = %e, "VM start failed");
                            json!({"status": "error", "message": format!("VM start failed: {e}")}).to_string()
                        }
                    }
                }
                None => json!({"status": "error", "message": format!("persona '{persona}' not found")}).to_string(),
            }
        }
        Some("stop") => {
            let persona = parts.get(1).unwrap_or(&"");
            if persona.is_empty() {
                return json!({"status": "error", "message": "missing persona name"}).to_string();
            }
            let _ = state.audit.log_persona("vm_stop", persona, "requested via IPC");
            let mut sups = state.supervisors.lock().await;
            if let Some(sup) = sups.get_mut(*persona) {
                match sup.stop().await {
                    Ok(()) => {
                        sups.remove(*persona);
                        json!({"status": "ok", "message": format!("VM stopped for '{persona}'")}).to_string()
                    }
                    Err(e) => json!({"status": "error", "message": format!("stop failed: {e}")}).to_string(),
                }
            } else {
                json!({"status": "ok", "message": format!("no running VM for '{persona}'")}).to_string()
            }
        }
        Some("persona-list") => {
            let names: Vec<String> = state.personas.list().into_iter().map(|p| p.name).collect();
            json!({"status": "ok", "personas": names}).to_string()
        }
        Some("vault-status") => {
            json!({"status": "ok", "vault": state.vault.status()}).to_string()
        }
        Some("vault-unlock") => {
            let passphrase = parts.get(1).unwrap_or(&"");
            match state.vault.unlock(passphrase) {
                Ok(()) => json!({"status": "ok", "message": "vault unlocked"}).to_string(),
                Err(e) => json!({"status": "error", "message": e.to_string()}).to_string(),
            }
        }
        Some("vault-lock") => {
            state.vault.lock();
            json!({"status": "ok", "message": "vault locked"}).to_string()
        }
        Some("vault-init") => {
            let passphrase = parts.get(1).unwrap_or(&"");
            match state.vault.init(passphrase) {
                Ok(()) => json!({"status": "ok", "message": "vault initialized"}).to_string(),
                Err(e) => json!({"status": "error", "message": e.to_string()}).to_string(),
            }
        }
        Some("vault-add") => {
            let args = parts.get(1).unwrap_or(&"");
            let arg_parts: Vec<&str> = args.splitn(3, ' ').collect();
            let name = arg_parts.first().unwrap_or(&"");
            let value = arg_parts.get(1).unwrap_or(&"");
            let stype = arg_parts.get(2).unwrap_or(&"password");
            let secret_type = match *stype {
                "totp" => SecretType::TotpSeed,
                "note" => SecretType::Note,
                _ => SecretType::Password,
            };
            match state.vault.add_secret(name, value.as_bytes(), vec![], secret_type) {
                Ok(()) => json!({"status": "ok", "message": format!("secret '{name}' added")}).to_string(),
                Err(e) => json!({"status": "error", "message": e.to_string()}).to_string(),
            }
        }
        Some("vault-remove") => {
            let name = parts.get(1).unwrap_or(&"");
            match state.vault.remove_secret(name) {
                Ok(()) => json!({"status": "ok", "message": format!("secret '{name}' removed")}).to_string(),
                Err(e) => json!({"status": "error", "message": e.to_string()}).to_string(),
            }
        }
        Some("vault-list") => match state.vault.list_secrets() {
            Ok(secrets) => {
                let list: Vec<_> = secrets
                    .iter()
                    .map(|(name, stype, domains)| {
                        json!({"name": name, "type": format!("{stype:?}"), "domains": domains})
                    })
                    .collect();
                json!({"status": "ok", "secrets": list}).to_string()
            }
            Err(e) => json!({"status": "error", "message": e.to_string()}).to_string(),
        },
        Some("secret-resolve") => {
            let args = parts.get(1).unwrap_or(&"");
            let arg_parts: Vec<&str> = args.splitn(2, ' ').collect();
            let name = arg_parts.first().unwrap_or(&"");
            let domain = arg_parts.get(1).unwrap_or(&"");
            resolve_secret(state, name, domain)
        }
        Some("liveview-token") => {
            let persona = parts.get(1).unwrap_or(&"");
            let expires = chrono::Utc::now().timestamp() as u64 + 3600;
            let token = liveview::sign_url(persona, &state.liveview_secret, expires);
            let valid = liveview::verify_token(persona, &state.liveview_secret, expires, &token);
            let session = liveview::LiveViewSession {
                persona_id: persona.to_string(),
                url: liveview::watch_url(),
                token: token.clone(),
                expires_at: expires,
                mode: liveview::LiveViewMode::Watch,
            };
            json!({
                "status": "ok",
                "token": session.token,
                "url": session.url,
                "mode": format!("{:?}", session.mode),
                "expires": session.expires_at,
                "verified": valid,
            })
            .to_string()
        }
        Some("derive-persona") => {
            let country = parts.get(1).unwrap_or(&"US");
            let seed = 42u64;
            let (tz, identity, langs, hw) = crate::persona::derive_persona(Some(country), seed);
            json!({
                "status": "ok",
                "timezone": tz,
                "locale": identity.locale,
                "keyboard": identity.keyboard_layout,
                "languages": langs,
                "hardware": hw.label,
                "screen": format!("{}x{}", hw.screen_width, hw.screen_height),
            })
            .to_string()
        }
        Some("handoff-resolve") => {
            let args = parts.get(1).unwrap_or(&"");
            let arg_parts: Vec<&str> = args.splitn(2, ' ').collect();
            let id_str = arg_parts.first().unwrap_or(&"");
            let action = arg_parts.get(1).unwrap_or(&"done");
            match uuid::Uuid::parse_str(id_str) {
                Ok(_id) => {
                    let status = match *action {
                        "abort" => crate::handoff::HandoffStatus::Aborted,
                        _ => crate::handoff::HandoffStatus::Done,
                    };
                    json!({"status": "ok", "message": format!("resolve not available via IPC (use MCP), would set to {status:?}"), "is_terminal": status == crate::handoff::HandoffStatus::Done || status == crate::handoff::HandoffStatus::Aborted}).to_string()
                }
                Err(e) => json!({"status": "error", "message": e.to_string()}).to_string(),
            }
        }
        Some("disk-create") => {
            let args = parts.get(1).unwrap_or(&"");
            let arg_parts: Vec<&str> = args.splitn(2, ' ').collect();
            let path_str = arg_parts.first().unwrap_or(&"");
            let size_gb: u32 = arg_parts.get(1).and_then(|s| s.parse().ok()).unwrap_or(20);
            let path = std::path::PathBuf::from(path_str);
            match taboom_vmm::create_empty_disk(&path, size_gb).await {
                Ok(()) => json!({"status": "ok", "path": path_str, "size_gb": size_gb}).to_string(),
                Err(e) => json!({"status": "error", "message": e.to_string()}).to_string(),
            }
        }
        Some("mcp-config") => {
            let format = parts.get(1).unwrap_or(&"generic");
            let config = match *format {
                "claude-code" => crate::mcp::generate_claude_code_config("127.0.0.1", 3456, None),
                _ => crate::mcp::generate_mcp_config_json("127.0.0.1", 3456, None),
            };
            config.to_string()
        }
        Some(cmd) => json!({"status": "error", "message": format!("unknown command: {cmd}")}).to_string(),
        None => json!({"status": "error", "message": "empty command"}).to_string(),
    }
}

fn resolve_secret(state: &IpcState, name: &str, domain: &str) -> String {
    let record = match state.vault.get_secret(name) {
        Ok(Some(r)) => r,
        Ok(None) => {
            return json!({"status": "error", "message": format!("secret '{name}' not found")}).to_string();
        }
        Err(e) => {
            return json!({"status": "error", "message": e.to_string()}).to_string();
        }
    };

    if !domain.is_empty() {
        match state.vault.check_domain(name, domain) {
            Ok(true) => {}
            Ok(false) => {
                let _ = state.audit.log_secret_refused(name, domain, &record.allowed_domains);
                return json!({"status": "refused", "reason": "domain not allowed"}).to_string();
            }
            Err(e) => {
                return json!({"status": "error", "message": e.to_string()}).to_string();
            }
        }
    }

    let value = match record.secret_type {
        SecretType::TotpSeed => {
            let seed_str = String::from_utf8_lossy(&record.encrypted_value);
            match totp::generate_totp(&seed_str, SystemTime::now()) {
                Ok(code) => code,
                Err(e) => {
                    return json!({"status": "error", "message": e.to_string()}).to_string();
                }
            }
        }
        _ => String::from_utf8_lossy(&record.encrypted_value).to_string(),
    };

    let _ = state.audit.log_secret_used(name, if domain.is_empty() { "ipc" } else { domain });
    json!({"status": "ok", "value": value}).to_string()
}
