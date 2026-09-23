use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Maximum encoded size of a single admin JSON-lines request or response.
pub const ADMIN_MAX_LINE_BYTES: usize = 64 * 1024;

/// Vault record type. Its serde variant names preserve the existing encrypted file format.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum SecretType {
    Password,
    TotpSeed,
    Note,
}

/// CLI-facing type names used by the admin protocol.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AdminSecretType {
    Password,
    Totp,
    Note,
}

impl From<AdminSecretType> for SecretType {
    fn from(value: AdminSecretType) -> Self {
        match value {
            AdminSecretType::Password => Self::Password,
            AdminSecretType::Totp => Self::TotpSeed,
            AdminSecretType::Note => Self::Note,
        }
    }
}

/// One request on the private Unix socket. The secret-bearing variants intentionally do not
/// implement `Debug`, so accidental debug logging cannot print passphrases or secret values.
#[derive(Serialize, Deserialize)]
#[serde(tag = "cmd")]
pub enum AdminCommand {
    #[serde(rename = "ping")]
    Ping,
    #[serde(rename = "vault.status")]
    VaultStatus,
    #[serde(rename = "vault.init")]
    VaultInit { passphrase: String },
    #[serde(rename = "vault.unlock")]
    VaultUnlock { passphrase: String },
    #[serde(rename = "vault.lock")]
    VaultLock,
    #[serde(rename = "vault.add")]
    VaultAdd {
        name: String,
        value: String,
        secret_type: AdminSecretType,
        domains: Vec<String>,
    },
    #[serde(rename = "vault.rm")]
    VaultRemove { name: String },
    #[serde(rename = "vault.ls")]
    VaultList,
}

#[derive(Serialize, Deserialize)]
pub struct AdminRequest {
    pub id: u64,
    #[serde(flatten)]
    pub command: AdminCommand,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AdminError {
    pub code: String,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AdminResponse {
    pub id: u64,
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<AdminError>,
}

impl AdminResponse {
    pub fn success(id: u64, result: Value) -> Self {
        Self { id, ok: true, result: Some(result), error: None }
    }

    pub fn failure(id: u64, code: &str, message: &str) -> Self {
        Self {
            id,
            ok: false,
            result: None,
            error: Some(AdminError { code: code.into(), message: message.into() }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn vault_add_round_trip_preserves_spaces_and_domain_lists() {
        let request = AdminRequest {
            id: 7,
            command: AdminCommand::VaultAdd {
                name: "work-login".into(),
                value: "a secret with several spaces".into(),
                secret_type: AdminSecretType::Password,
                domains: vec!["example.com".into(), "accounts.example.net".into()],
            },
        };
        let encoded = serde_json::to_vec(&request).unwrap();
        let value: Value = serde_json::from_slice(&encoded).unwrap();
        assert_eq!(value["cmd"], "vault.add");
        assert_eq!(value["value"], "a secret with several spaces");
        assert_eq!(value["domains"], json!(["example.com", "accounts.example.net"]));
        let decoded: AdminRequest = serde_json::from_value(value).unwrap();
        if let AdminCommand::VaultAdd { value, domains, .. } = decoded.command {
            assert_eq!(value, "a secret with several spaces");
            assert_eq!(domains, ["example.com", "accounts.example.net"]);
        } else {
            panic!("request changed command");
        }
    }

    #[test]
    fn failures_are_structured() {
        let response = AdminResponse::failure(4, "vault_locked", "vault is locked");
        assert_eq!(serde_json::to_value(response).unwrap(), json!({
            "id": 4, "ok": false,
            "error": { "code": "vault_locked", "message": "vault is locked" }
        }));
    }
}
