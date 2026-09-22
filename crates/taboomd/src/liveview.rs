use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum LiveViewMode {
    Watch,
    TakeOver,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LiveViewSession {
    pub persona_id: String,
    pub url: String,
    pub token: String,
    pub expires_at: u64,
    pub mode: LiveViewMode,
}

/// Watch-only live view link. Compose sets TABOOM_VIEW_URL from the published port.
pub fn watch_url() -> String {
    std::env::var("TABOOM_VIEW_URL")
        .unwrap_or_else(|_| "http://localhost:6080/vnc.html?autoconnect=1&resize=scale&view_only=1".into())
}

/// The same live view with mouse and keyboard enabled, for a human taking over.
pub fn takeover_url() -> String {
    watch_url().replace("&view_only=1", "").replace("view_only=1&", "")
}

pub fn sign_url(persona_id: &str, secret: &[u8], expires_at: u64) -> String {
    use hmac::{Hmac, Mac};
    use sha2::Sha256;

    let message = format!("{persona_id}:{expires_at}");
    let mut mac = Hmac::<Sha256>::new_from_slice(secret).expect("HMAC accepts any key length");
    mac.update(message.as_bytes());
    let result = mac.finalize();
    hex::encode(result.into_bytes())
}

pub fn verify_token(persona_id: &str, secret: &[u8], expires_at: u64, token: &str) -> bool {
    let expected = sign_url(persona_id, secret, expires_at);
    constant_time_eq(expected.as_bytes(), token.as_bytes())
}

fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sign_and_verify() {
        let secret = b"test-secret-key-32bytes-long!!!!";
        let token = sign_url("persona-1", secret, 1700000000);
        assert!(verify_token("persona-1", secret, 1700000000, &token));
        assert!(!verify_token("persona-2", secret, 1700000000, &token));
        assert!(!verify_token("persona-1", secret, 1700000001, &token));
    }
}
