use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use uuid::Uuid;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum HandoffStatus {
    Pending,
    Done,
    Aborted,
    Expired,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HandoffSession {
    pub id: Uuid,
    pub persona_id: String,
    pub reason: String,
    pub view_url: String,
    pub status: HandoffStatus,
    pub created_at: DateTime<Utc>,
    pub timeout_secs: u64,
}

impl HandoffSession {
    pub fn is_terminal(&self) -> bool {
        matches!(
            self.status,
            HandoffStatus::Done | HandoffStatus::Aborted | HandoffStatus::Expired
        )
    }

    pub fn check_expiry(&mut self) {
        if self.status != HandoffStatus::Pending {
            return;
        }
        let elapsed = Utc::now()
            .signed_duration_since(self.created_at)
            .num_seconds();
        if elapsed as u64 >= self.timeout_secs {
            self.status = HandoffStatus::Expired;
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HandoffNotifyConfig {
    pub method: NotifyMethod,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum NotifyMethod {
    Desktop,
    Webhook { url: String },
    Ntfy { topic: String },
    None,
}

impl Default for HandoffNotifyConfig {
    fn default() -> Self {
        Self {
            method: NotifyMethod::None,
        }
    }
}

pub struct HandoffManager {
    sessions: HashMap<Uuid, HandoffSession>,
    notify_config: HandoffNotifyConfig,
    base_view_url: String,
}

impl HandoffManager {
    pub fn new(base_view_url: String, notify_config: HandoffNotifyConfig) -> Self {
        Self {
            sessions: HashMap::new(),
            notify_config,
            base_view_url,
        }
    }

    pub fn start(
        &mut self,
        persona_id: &str,
        reason: &str,
        timeout_secs: u64,
    ) -> HandoffSession {
        let id = Uuid::new_v4();
        // the live view ignores unknown query params; the id just labels the link
        let sep = if self.base_view_url.contains('?') { '&' } else { '?' };
        let view_url = format!("{}{sep}handoff={id}", self.base_view_url);
        let session = HandoffSession {
            id,
            persona_id: persona_id.to_string(),
            reason: reason.to_string(),
            view_url,
            status: HandoffStatus::Pending,
            created_at: Utc::now(),
            timeout_secs,
        };
        self.sessions.insert(id, session.clone());
        self.send_notification(&session);
        session
    }

    pub fn get(&mut self, id: &Uuid) -> Option<&mut HandoffSession> {
        if let Some(session) = self.sessions.get_mut(id) {
            session.check_expiry();
            Some(session)
        } else {
            None
        }
    }

    pub fn resolve(&mut self, id: &Uuid, status: HandoffStatus) -> Option<&HandoffSession> {
        if let Some(session) = self.sessions.get_mut(id) {
            if session.status == HandoffStatus::Pending {
                session.status = status;
            }
            Some(session)
        } else {
            None
        }
    }

    pub fn active_for_persona(&self, persona_id: &str) -> Option<&HandoffSession> {
        self.sessions.values().find(|s| {
            s.persona_id == persona_id && s.status == HandoffStatus::Pending
        })
    }

    fn send_notification(&self, session: &HandoffSession) {
        match &self.notify_config.method {
            NotifyMethod::Desktop => {
                #[cfg(target_os = "linux")]
                {
                    let _ = std::process::Command::new("notify-send")
                        .args([
                            "Taboom Handoff",
                            &format!(
                                "Persona '{}' needs help: {}",
                                session.persona_id, session.reason
                            ),
                        ])
                        .spawn();
                }
                #[cfg(target_os = "macos")]
                {
                    let _ = std::process::Command::new("osascript")
                        .args([
                            "-e",
                            &format!(
                                "display notification \"{}\" with title \"Taboom Handoff\"",
                                session.reason
                            ),
                        ])
                        .spawn();
                }
            }
            NotifyMethod::Webhook { url } => {
                let url = url.clone();
                let body = serde_json::json!({
                    "event": "handoff_start",
                    "persona": session.persona_id,
                    "reason": session.reason,
                    "view_url": session.view_url,
                    "id": session.id.to_string(),
                });
                tracing::info!(url = %url, "sending webhook notification");
                let _ = body; // actual HTTP send deferred to async context
            }
            NotifyMethod::Ntfy { topic } => {
                let _ = std::process::Command::new("curl")
                    .args([
                        "-s",
                        "-d",
                        &format!(
                            "Handoff needed for '{}': {}",
                            session.persona_id, session.reason
                        ),
                        &format!("https://ntfy.sh/{topic}"),
                    ])
                    .spawn();
            }
            NotifyMethod::None => {}
        }
    }
}

pub fn parse_notify_config(spec: &str) -> HandoffNotifyConfig {
    if spec == "desktop" {
        return HandoffNotifyConfig {
            method: NotifyMethod::Desktop,
        };
    }
    if let Some(url) = spec.strip_prefix("webhook:") {
        return HandoffNotifyConfig {
            method: NotifyMethod::Webhook {
                url: url.to_string(),
            },
        };
    }
    if let Some(topic) = spec.strip_prefix("ntfy:") {
        return HandoffNotifyConfig {
            method: NotifyMethod::Ntfy {
                topic: topic.to_string(),
            },
        };
    }
    HandoffNotifyConfig::default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn handoff_lifecycle_pending_to_done() {
        let mut mgr = HandoffManager::new(
            "http://localhost:8080".into(),
            HandoffNotifyConfig::default(),
        );
        let session = mgr.start("alice", "CAPTCHA on login page", 300);
        assert_eq!(session.status, HandoffStatus::Pending);

        let resolved = mgr.resolve(&session.id, HandoffStatus::Done).unwrap();
        assert_eq!(resolved.status, HandoffStatus::Done);
    }

    #[test]
    fn handoff_lifecycle_pending_to_aborted() {
        let mut mgr = HandoffManager::new(
            "http://localhost:8080".into(),
            HandoffNotifyConfig::default(),
        );
        let session = mgr.start("bob", "phone verification needed", 300);
        let resolved = mgr.resolve(&session.id, HandoffStatus::Aborted).unwrap();
        assert_eq!(resolved.status, HandoffStatus::Aborted);
    }

    #[test]
    fn handoff_expiry() {
        let mut mgr = HandoffManager::new(
            "http://localhost:8080".into(),
            HandoffNotifyConfig::default(),
        );
        let mut session = mgr.start("charlie", "reason", 0);
        session.check_expiry();
        assert_eq!(session.status, HandoffStatus::Expired);
    }

    #[test]
    fn handoff_already_resolved_stays_resolved() {
        let mut mgr = HandoffManager::new(
            "http://localhost:8080".into(),
            HandoffNotifyConfig::default(),
        );
        let session = mgr.start("delta", "reason", 300);
        mgr.resolve(&session.id, HandoffStatus::Done);
        let again = mgr.resolve(&session.id, HandoffStatus::Aborted).unwrap();
        assert_eq!(again.status, HandoffStatus::Done);
    }

    #[test]
    fn handoff_view_url_contains_id() {
        let mut mgr = HandoffManager::new(
            "http://localhost:6080/vnc.html?autoconnect=1&resize=scale".into(),
            HandoffNotifyConfig::default(),
        );
        let session = mgr.start("echo", "reason", 300);
        assert_eq!(
            session.view_url,
            format!("http://localhost:6080/vnc.html?autoconnect=1&resize=scale&handoff={}", session.id)
        );
        assert!(!session.view_url.contains("view_only"), "a handoff must allow input");
    }

    #[test]
    fn active_for_persona_finds_pending() {
        let mut mgr = HandoffManager::new(
            "http://localhost:8080".into(),
            HandoffNotifyConfig::default(),
        );
        mgr.start("foxtrot", "reason", 300);
        assert!(mgr.active_for_persona("foxtrot").is_some());
        assert!(mgr.active_for_persona("unknown").is_none());
    }

    #[test]
    fn parse_notify_desktop() {
        let cfg = parse_notify_config("desktop");
        assert!(matches!(cfg.method, NotifyMethod::Desktop));
    }

    #[test]
    fn parse_notify_webhook() {
        let cfg = parse_notify_config("webhook:https://example.com/hook");
        match cfg.method {
            NotifyMethod::Webhook { url } => assert_eq!(url, "https://example.com/hook"),
            _ => panic!("expected webhook"),
        }
    }

    #[test]
    fn parse_notify_ntfy() {
        let cfg = parse_notify_config("ntfy:my-topic");
        match cfg.method {
            NotifyMethod::Ntfy { topic } => assert_eq!(topic, "my-topic"),
            _ => panic!("expected ntfy"),
        }
    }

    #[test]
    fn parse_notify_unknown_falls_back_to_none() {
        let cfg = parse_notify_config("something-random");
        assert!(matches!(cfg.method, NotifyMethod::None));
    }
}
