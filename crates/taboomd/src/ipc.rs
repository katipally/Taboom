use crate::audit::AuditLog;
use crate::vault::Vault;
use anyhow::{Context, Result};
use serde_json::{json, Value};
use std::path::Path;
use std::sync::Arc;
use taboom_core::admin::{AdminCommand, AdminRequest, AdminResponse, SecretType};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixListener;
use tokio::task::JoinHandle;
use tracing::{error, info};

struct IpcState {
    vault: Arc<Vault>,
    audit: Arc<AuditLog>,
}

pub async fn start_listener(
    home: &Path,
    vault: Arc<Vault>,
    audit: Arc<AuditLog>,
) -> Result<JoinHandle<()>> {
    let sock_path = home.join("run").join("taboomd.sock");
    let run_dir = sock_path.parent().context("admin socket path has no parent")?;
    std::fs::create_dir_all(run_dir)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        // The parent stays private even during bind, before chmod is applied to the socket.
        std::fs::set_permissions(run_dir, std::fs::Permissions::from_mode(0o700))?;
    }

    if sock_path.exists() {
        std::fs::remove_file(&sock_path)?;
    }

    let listener = UnixListener::bind(&sock_path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Err(e) = std::fs::set_permissions(&sock_path, std::fs::Permissions::from_mode(0o600)) {
            let _ = std::fs::remove_file(&sock_path);
            return Err(e).with_context(|| format!("restricting admin socket {}", sock_path.display()));
        }
    }
    info!(path = %sock_path.display(), "private JSON-lines admin socket bound");

    let state = Arc::new(IpcState { vault, audit });
    let handle = tokio::spawn(async move {
        loop {
            match listener.accept().await {
                Ok((stream, _)) => {
                    let st = Arc::clone(&state);
                    tokio::spawn(async move {
                        if let Err(e) = handle_connection(stream, st).await {
                            // Never include request bytes: they can contain passphrases or secrets.
                            error!("admin socket connection failed: {e}");
                        }
                    });
                }
                Err(e) => error!("admin socket accept failed: {e}"),
            }
        }
    });
    Ok(handle)
}

async fn handle_connection(stream: tokio::net::UnixStream, state: Arc<IpcState>) -> Result<()> {
    let (reader, mut writer) = stream.into_split();
    let mut reader = BufReader::new(reader);

    loop {
        let mut line = Vec::new();
        let bytes_read = (&mut reader)
            .take((taboom_core::admin::ADMIN_MAX_LINE_BYTES + 1) as u64)
            .read_until(b'\n', &mut line)
            .await?;
        if bytes_read == 0 {
            break;
        }

        let too_large = bytes_read > taboom_core::admin::ADMIN_MAX_LINE_BYTES;
        let terminated = line.last() == Some(&b'\n');
        if terminated {
            line.pop();
        }
        let response = if too_large {
            AdminResponse::failure(0, "request_too_large", "admin request exceeds 65536 bytes")
        } else if !terminated {
            AdminResponse::failure(0, "incomplete_request", "admin requests must end with a newline")
        } else {
            // Vault init, unlock and writes each run scrypt (~1 s); keep it off the async workers.
            let st = Arc::clone(&state);
            tokio::task::spawn_blocking(move || process_line(&line, &st)).await?
        };
        let mut encoded = serde_json::to_vec(&response)?;
        if encoded.len() > taboom_core::admin::ADMIN_MAX_LINE_BYTES {
            encoded = serde_json::to_vec(&AdminResponse::failure(
                response.id,
                "response_too_large",
                "admin response exceeds 65536 bytes",
            ))?;
        }
        writer.write_all(&encoded).await?;
        writer.write_all(b"\n").await?;

        if too_large || !terminated {
            break;
        }
    }
    Ok(())
}

fn process_line(line: &[u8], state: &IpcState) -> AdminResponse {
    if line.len() > taboom_core::admin::ADMIN_MAX_LINE_BYTES {
        return AdminResponse::failure(0, "request_too_large", "admin request exceeds 65536 bytes");
    }
    let request: AdminRequest = match serde_json::from_slice(line) {
        Ok(request) => request,
        Err(_) => return AdminResponse::failure(0, "invalid_request", "request must be a valid typed admin JSON object"),
    };
    let id = request.id;
    let result = match request.command {
        AdminCommand::Ping => Ok(json!({"message": "pong"})),
        AdminCommand::VaultStatus => Ok(json!({"state": state.vault.status()})),
        AdminCommand::VaultInit { passphrase } => match state.vault.init(&passphrase) {
            Ok(()) => {
                let _ = state.audit.log("vault_init", "vault initialized");
                Ok(json!({"state": "unlocked"}))
            }
            Err(e) => Err(("vault_error", e.to_string())),
        },
        AdminCommand::VaultUnlock { passphrase } => match state.vault.unlock(&passphrase) {
            Ok(()) => {
                let _ = state.audit.log("vault_unlock", "vault unlocked");
                Ok(json!({"state": "unlocked"}))
            }
            Err(e) => Err(("vault_error", e.to_string())),
        },
        AdminCommand::VaultLock => {
            state.vault.lock();
            let _ = state.audit.log("vault_lock", "vault locked");
            Ok(json!({"state": state.vault.status()}))
        }
        AdminCommand::VaultAdd { name, value, secret_type, domains } => {
            let saved_type: SecretType = secret_type.into();
            let seed_ok = !matches!(saved_type, SecretType::TotpSeed)
                || crate::totp::generate_totp(&value, std::time::SystemTime::now()).is_ok();
            if !seed_ok {
                return AdminResponse::failure(id, "vault_error", "TOTP seed is not valid base32");
            }
            match state.vault.add_secret(&name, value.as_bytes(), domains.clone(), saved_type) {
                Ok(()) => {
                    let _ = state.audit.log(
                        "vault_secret_added",
                        &format!("name={name} domains={}", domains.join(",")),
                    );
                    Ok(json!({"name": name, "added": true}))
                }
                Err(e) => Err(("vault_error", e.to_string())),
            }
        }
        AdminCommand::VaultRemove { name } => match state.vault.remove_secret(&name) {
            Ok(()) => {
                let _ = state.audit.log("vault_secret_removed", &format!("name={name}"));
                Ok(json!({"name": name, "removed": true}))
            }
            Err(e) => Err(("vault_error", e.to_string())),
        },
        AdminCommand::VaultList => match state.vault.list_secrets() {
            Ok(secrets) => {
                let list: Vec<Value> = secrets
                    .into_iter()
                    .map(|(name, secret_type, domains)| {
                        let kind = match secret_type {
                            SecretType::Password => "password",
                            SecretType::TotpSeed => "totp",
                            SecretType::Note => "note",
                        };
                        json!({"name": name, "type": kind, "domains": domains})
                    })
                    .collect();
                Ok(json!({"secrets": list}))
            }
            Err(e) => Err(("vault_error", e.to_string())),
        },
    };

    match result {
        Ok(result) => AdminResponse::success(id, result),
        Err((code, message)) => AdminResponse::failure(id, code, &message),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use taboom_core::admin::{AdminSecretType, AdminRequest};
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt};
    use tokio::net::UnixStream;

    fn state() -> (tempfile::TempDir, IpcState) {
        let temp = tempfile::tempdir().unwrap();
        let vault = Arc::new(Vault::open(temp.path()));
        let audit = Arc::new(AuditLog::open(&temp.path().join("audit.jsonl")).unwrap());
        (temp, IpcState { vault, audit })
    }

    #[test]
    fn typed_vault_add_keeps_space_secret_and_domains_without_logging_value() {
        let (temp, state) = state();
        state.vault.init("pass phrase").unwrap();
        let request = AdminRequest {
            id: 1,
            command: AdminCommand::VaultAdd {
                name: "shop".into(),
                value: "secret with spaces".into(),
                secret_type: AdminSecretType::Password,
                domains: vec!["shop.example".into(), "account.example".into()],
            },
        };
        let line = serde_json::to_vec(&request).unwrap();
        let response = process_line(&line, &state);
        assert!(response.ok);
        assert_eq!(state.vault.get_secret("shop").unwrap().unwrap().encrypted_value, b"secret with spaces");
        assert_eq!(state.vault.get_secret("shop").unwrap().unwrap().allowed_domains, ["shop.example", "account.example"]);
        let audit = std::fs::read_to_string(temp.path().join("audit.jsonl")).unwrap();
        assert!(!audit.contains("secret with spaces"));
    }

    #[test]
    fn malformed_and_oversized_requests_get_structured_errors() {
        let (_temp, state) = state();
        let malformed = process_line(b"{not json", &state);
        assert_eq!(malformed.error.unwrap().code, "invalid_request");
        let oversized = process_line(&vec![b'a'; taboom_core::admin::ADMIN_MAX_LINE_BYTES + 1], &state);
        assert_eq!(oversized.error.unwrap().code, "request_too_large");
    }

    #[tokio::test]
    async fn socket_is_private_and_oversized_lines_are_bounded() {
        use std::os::unix::fs::PermissionsExt;

        let (temp, state) = state();
        let handle = start_listener(temp.path(), Arc::clone(&state.vault), Arc::clone(&state.audit)).await.unwrap();
        let socket = temp.path().join("run/taboomd.sock");
        let mode = std::fs::metadata(&socket).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);

        let mut stream = UnixStream::connect(socket).await.unwrap();
        let mut request = vec![b'a'; taboom_core::admin::ADMIN_MAX_LINE_BYTES + 1];
        request.push(b'\n');
        stream.write_all(&request).await.unwrap();
        let mut response = String::new();
        BufReader::new(stream).read_line(&mut response).await.unwrap();
        let response: AdminResponse = serde_json::from_str(&response).unwrap();
        assert_eq!(response.error.unwrap().code, "request_too_large");
        handle.abort();
    }
}
