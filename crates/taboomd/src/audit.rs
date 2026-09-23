use anyhow::{Context, Result};
use chrono::Utc;
use serde_json::json;
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::Path;
use std::sync::Mutex;

pub struct AuditLog {
    file: Mutex<File>,
}

impl AuditLog {
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .with_context(|| format!("opening audit log at {}", path.display()))?;

        Ok(Self {
            file: Mutex::new(file),
        })
    }

    pub fn log(&self, event: &str, detail: &str) -> Result<()> {
        let entry = json!({
            "ts": Utc::now().to_rfc3339(),
            "event": event,
            "detail": detail,
        });

        let mut line = serde_json::to_string(&entry)?;
        line.push('\n');

        let mut f = self.file.lock().unwrap();
        f.write_all(line.as_bytes())?;
        f.flush()?;
        Ok(())
    }

    pub fn log_secret_used(&self, secret_name: &str, domain: &str) -> Result<()> {
        let entry = json!({
            "ts": Utc::now().to_rfc3339(),
            "event": "secret_used",
            "secret_name": secret_name,
            "domain": domain,
        });

        let mut line = serde_json::to_string(&entry)?;
        line.push('\n');

        let mut f = self.file.lock().unwrap();
        f.write_all(line.as_bytes())?;
        f.flush()?;
        Ok(())
    }

    pub fn log_secret_refused(
        &self,
        secret_name: &str,
        observed_domain: &str,
        allowed_domains: &[String],
    ) -> Result<()> {
        let entry = json!({
            "ts": Utc::now().to_rfc3339(),
            "event": "secret_refused",
            "secret_name": secret_name,
            "observed_domain": observed_domain,
            "allowed_domains": allowed_domains,
        });

        let mut line = serde_json::to_string(&entry)?;
        line.push('\n');

        let mut f = self.file.lock().unwrap();
        f.write_all(line.as_bytes())?;
        f.flush()?;
        Ok(())
    }

}
