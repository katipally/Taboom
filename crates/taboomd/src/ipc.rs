use crate::audit::AuditLog;
use crate::config::DaemonConfig;
use crate::liveview;
use crate::persona::PersonaRegistry;
use crate::totp;
use crate::vault::{SecretType, Vault};
use anyhow::Result;
use serde_json::json;
use std::sync::Arc;
use std::time::SystemTime;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixListener;
use tokio::task::JoinHandle;
use tracing::{error, info};

struct IpcState {
    vault: Arc<Vault>,
    personas: Arc<PersonaRegistry>,
    audit: Arc<AuditLog>,
    liveview_secret: Vec<u8>,
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
        let response = process_command(&line, state);
        writer.write_all(response.as_bytes()).await?;
        writer.write_all(b"\n").await?;
    }

    Ok(())
}

fn process_command(line: &str, state: &IpcState) -> String {
    let parts: Vec<&str> = line.trim().splitn(2, ' ').collect();
    match parts.first().copied() {
        Some("ping") => json!({"status": "ok", "message": "pong"}).to_string(),
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
