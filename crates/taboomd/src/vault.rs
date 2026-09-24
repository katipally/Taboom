use age::secrecy::SecretString;
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::net::IpAddr;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

pub use taboom_core::admin::SecretType;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SecretRecord {
    pub name: String,
    pub encrypted_value: Vec<u8>,
    pub allowed_domains: Vec<String>,
    pub secret_type: SecretType,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct VaultData {
    secrets: BTreeMap<String, SecretRecord>,
}

pub enum VaultState {
    Locked,
    Unlocked { passphrase: String },
    NoVault,
}

pub struct Vault {
    path: PathBuf,
    state: Mutex<VaultState>,
    data: Mutex<Option<VaultData>>,
}

impl Vault {
    pub fn open(home: &Path) -> Self {
        let path = home.join("vault").join("vault.age");
        let state = if path.exists() {
            VaultState::Locked
        } else {
            VaultState::NoVault
        };
        Self {
            path,
            state: Mutex::new(state),
            data: Mutex::new(None),
        }
    }

    pub fn init(&self, passphrase: &str) -> Result<()> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("creating vault directory at {}", parent.display()))?;
        }

        if self.path.exists() {
            bail!("vault already exists at {}", self.path.display());
        }

        let empty = VaultData::default();
        self.write_encrypted(&empty, passphrase)?;

        let mut state = self.state.lock().unwrap();
        *state = VaultState::Unlocked {
            passphrase: passphrase.to_string(),
        };
        let mut data = self.data.lock().unwrap();
        *data = Some(empty);

        Ok(())
    }

    pub fn unlock(&self, passphrase: &str) -> Result<()> {
        let vault_data = self.read_encrypted(passphrase)?;

        let mut state = self.state.lock().unwrap();
        *state = VaultState::Unlocked {
            passphrase: passphrase.to_string(),
        };
        let mut data = self.data.lock().unwrap();
        *data = Some(vault_data);

        Ok(())
    }

    pub fn lock(&self) {
        let mut state = self.state.lock().unwrap();
        *state = if self.path.exists() {
            VaultState::Locked
        } else {
            VaultState::NoVault
        };
        let mut data = self.data.lock().unwrap();
        *data = None;
    }

    pub fn status(&self) -> &'static str {
        let state = self.state.lock().unwrap();
        match &*state {
            VaultState::Locked => "locked",
            VaultState::Unlocked { .. } => "unlocked",
            VaultState::NoVault => "no vault",
        }
    }

    pub fn add_secret(
        &self,
        name: &str,
        value: &[u8],
        allowed_domains: Vec<String>,
        secret_type: SecretType,
    ) -> Result<()> {
        let allowed_domains = allowed_domains
            .iter()
            .map(|domain| {
                normalize_domain(domain)
                    .ok_or_else(|| anyhow::anyhow!("invalid allowed domain {domain:?}"))
            })
            .collect::<Result<Vec<_>>>()?;
        let passphrase = self.require_unlocked()?;
        let mut data_guard = self.data.lock().unwrap();
        let data = data_guard.as_mut().ok_or_else(locked)?;

        if data.secrets.contains_key(name) {
            bail!("secret '{}' already exists", name);
        }

        data.secrets.insert(
            name.to_string(),
            SecretRecord {
                name: name.to_string(),
                encrypted_value: value.to_vec(),
                allowed_domains,
                secret_type,
            },
        );

        self.write_encrypted(data, &passphrase)?;
        Ok(())
    }

    pub fn get_secret(&self, name: &str) -> Result<Option<SecretRecord>> {
        self.require_unlocked()?;
        let data_guard = self.data.lock().unwrap();
        let data = data_guard.as_ref().ok_or_else(locked)?;
        Ok(data.secrets.get(name).cloned())
    }

    /// Reads only the allow-list and type so policy can be checked before the secret bytes are
    /// copied out of the unlocked vault.
    pub fn secret_metadata(&self, name: &str) -> Result<Option<(SecretType, Vec<String>)>> {
        self.require_unlocked()?;
        let data_guard = self.data.lock().unwrap();
        let data = data_guard.as_ref().ok_or_else(locked)?;
        Ok(data.secrets.get(name).map(|s| (s.secret_type.clone(), s.allowed_domains.clone())))
    }

    pub fn list_secrets(&self) -> Result<Vec<(String, SecretType, Vec<String>)>> {
        self.require_unlocked()?;
        let data_guard = self.data.lock().unwrap();
        let data = data_guard.as_ref().ok_or_else(locked)?;
        Ok(data
            .secrets
            .values()
            .map(|s| (s.name.clone(), s.secret_type.clone(), s.allowed_domains.clone()))
            .collect())
    }

    pub fn remove_secret(&self, name: &str) -> Result<()> {
        let passphrase = self.require_unlocked()?;
        let mut data_guard = self.data.lock().unwrap();
        let data = data_guard.as_mut().ok_or_else(locked)?;

        if data.secrets.remove(name).is_none() {
            bail!("secret '{}' not found", name);
        }

        self.write_encrypted(data, &passphrase)?;
        Ok(())
    }

    #[cfg(test)]
    pub fn check_domain(&self, secret_name: &str, observed_domain: &str) -> Result<bool> {
        self.require_unlocked()?;
        let data_guard = self.data.lock().unwrap();
        let data = data_guard.as_ref().ok_or_else(locked)?;

        let record = data
            .secrets
            .get(secret_name)
            .ok_or_else(|| anyhow::anyhow!("secret '{}' not found", secret_name))?;

        Ok(!record.allowed_domains.is_empty()
            && domain_matches(&record.allowed_domains, observed_domain))
    }

    fn require_unlocked(&self) -> Result<String> {
        let state = self.state.lock().unwrap();
        match &*state {
            VaultState::Unlocked { passphrase } => Ok(passphrase.clone()),
            VaultState::Locked => bail!("vault is locked; unlock it first"),
            VaultState::NoVault => bail!("no vault exists; run `taboom vault init`"),
        }
    }

    fn write_encrypted(&self, data: &VaultData, passphrase: &str) -> Result<()> {
        let mut cbor_buf = Vec::new();
        ciborium::into_writer(data, &mut cbor_buf)
            .map_err(|e| anyhow::anyhow!("CBOR encode failed: {e}"))?;

        let encryptor =
            age::Encryptor::with_user_passphrase(SecretString::from(passphrase.to_string()));

        let mut encrypted = Vec::new();
        let mut writer = encryptor
            .wrap_output(&mut encrypted)
            .map_err(|e| anyhow::anyhow!("age encrypt failed: {e}"))?;
        writer.write_all(&cbor_buf)?;
        writer
            .finish()
            .map_err(|e| anyhow::anyhow!("age finalize failed: {e}"))?;

        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let tmp = self.path.with_extension("age.tmp");
        std::fs::write(&tmp, &encrypted)?;
        std::fs::rename(&tmp, &self.path)?;

        Ok(())
    }

    fn read_encrypted(&self, passphrase: &str) -> Result<VaultData> {
        let encrypted = std::fs::read(&self.path)
            .with_context(|| format!("reading vault at {}", self.path.display()))?;

        let decryptor = age::Decryptor::new(&encrypted[..])
            .map_err(|e| anyhow::anyhow!("age decryptor init failed: {e}"))?;

        if !decryptor.is_scrypt() {
            bail!("vault file is not passphrase-encrypted");
        }

        let identity = age::scrypt::Identity::new(SecretString::from(passphrase.to_string()));
        let mut decrypted = Vec::new();
        let mut reader = decryptor
            .decrypt(std::iter::once(&identity as &dyn age::Identity))
            .map_err(|e| anyhow::anyhow!("decryption failed (wrong passphrase?): {e}"))?;
        reader.read_to_end(&mut decrypted)?;

        let data: VaultData = ciborium::from_reader(&decrypted[..])
            .map_err(|e| anyhow::anyhow!("CBOR decode failed: {e}"))?;

        Ok(data)
    }
}

pub fn domain_matches(allowed: &[String], observed: &str) -> bool {
    let Some(observed) = normalize_domain(observed) else {
        return false;
    };
    allowed.iter().filter_map(|d| normalize_domain(d)).any(|domain| {
        observed == domain || observed.ends_with(&format!(".{domain}"))
    })
}

/// `lock()` can clear the data between `require_unlocked()` and the data lock.
fn locked() -> anyhow::Error {
    anyhow::anyhow!("vault is locked")
}

/// Canonical DNS host or IP form used for both persisted allow-lists and observed URLs.
/// URLs, ports, credentials, paths and Unicode lookalikes are deliberately not accepted.
pub fn normalize_domain(input: &str) -> Option<String> {
    let domain = input.trim().strip_suffix('.').unwrap_or(input.trim());
    if domain.is_empty() || domain.len() > 253 {
        return None;
    }
    let domain = domain.to_ascii_lowercase();
    if domain.parse::<IpAddr>().is_ok() {
        return Some(domain);
    }
    if !domain.is_ascii() || domain.chars().any(|c| matches!(c, '/' | ':' | '@' | '?' | '#')) {
        return None;
    }
    let valid = domain.split('.').all(|label| {
        !label.is_empty()
            && label.len() <= 63
            && label.as_bytes()[0].is_ascii_alphanumeric()
            && label.as_bytes()[label.len() - 1].is_ascii_alphanumeric()
            && label.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
    });
    valid.then_some(domain)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn temp_home() -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().to_path_buf();
        (dir, home)
    }

    #[test]
    fn vault_round_trip() {
        let (_dir, home) = temp_home();
        let vault = Vault::open(&home);
        assert_eq!(vault.status(), "no vault");

        vault.init("test-pass").unwrap();
        assert_eq!(vault.status(), "unlocked");

        vault
            .add_secret(
                "github",
                b"hunter2",
                vec!["github.com".into()],
                SecretType::Password,
            )
            .unwrap();

        let list = vault.list_secrets().unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].0, "github");

        let record = vault.get_secret("github").unwrap().unwrap();
        assert_eq!(record.encrypted_value, b"hunter2");

        vault.lock();
        assert_eq!(vault.status(), "locked");

        vault.unlock("test-pass").unwrap();
        let record2 = vault.get_secret("github").unwrap().unwrap();
        assert_eq!(record2.encrypted_value, b"hunter2");

        vault.remove_secret("github").unwrap();
        assert!(vault.list_secrets().unwrap().is_empty());
    }

    #[test]
    fn wrong_passphrase_rejected() {
        let (_dir, home) = temp_home();
        let vault = Vault::open(&home);
        vault.init("correct").unwrap();
        vault.lock();
        assert!(vault.unlock("wrong").is_err());
    }

    #[test]
    fn domain_matching_exact() {
        assert!(domain_matches(&["github.com".into()], "github.com"));
        assert!(!domain_matches(&["github.com".into()], "evil.com"));
    }

    #[test]
    fn domain_matching_subdomain() {
        assert!(domain_matches(&["github.com".into()], "sub.github.com"));
        assert!(!domain_matches(&["github.com".into()], "notgithub.com"));
    }

    #[test]
    fn domain_matching_case_insensitive() {
        assert!(domain_matches(&["GitHub.com".into()], "GITHUB.COM"));
    }

    #[test]
    fn empty_allowed_domains_are_denied() {
        let (_dir, home) = temp_home();
        let vault = Vault::open(&home);
        vault.init("pass").unwrap();
        vault
            .add_secret("open", b"val", vec![], SecretType::Note)
            .unwrap();
        assert!(!vault.check_domain("open", "anything.com").unwrap());
    }

    #[test]
    fn canonical_domain_matching_normalizes_case_and_trailing_dot() {
        assert!(domain_matches(&["GitHub.COM.".into()], "API.GITHUB.com."));
        assert!(domain_matches(&["github.com".into()], "github.com"));
        assert!(!domain_matches(&["github.com".into()], "notgithub.com"));
        assert!(!domain_matches(&["https://github.com".into()], "github.com"));
    }

    #[test]
    fn invalid_domain_is_rejected_when_saving() {
        let (_dir, home) = temp_home();
        let vault = Vault::open(&home);
        vault.init("pass").unwrap();
        assert!(vault.add_secret("bad", b"value", vec!["https://example.com".into()], SecretType::Password).is_err());
        assert!(vault.add_secret("port", b"value", vec!["example.com:443".into()], SecretType::Password).is_err());
    }

    #[test]
    fn ipv6_hosts_are_accepted_in_canonical_form() {
        assert_eq!(normalize_domain("2001:db8::1").as_deref(), Some("2001:db8::1"));
        assert!(domain_matches(&["2001:db8::1".into()], "2001:db8::1"));
    }
}
