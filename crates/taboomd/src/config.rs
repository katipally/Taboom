use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Serialize, Deserialize)]
pub struct DaemonConfig {
    #[serde(skip)]
    pub home: PathBuf,
}

impl DaemonConfig {
    pub fn load() -> Result<Self> {
        let home = std::env::var("TABOOM_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|_| {
                dirs_home().join(".taboom")
            });

        let config_path = home.join("config.toml");

        let mut config = if config_path.exists() {
            let content = std::fs::read_to_string(&config_path)
                .with_context(|| format!("reading {}", config_path.display()))?;
            toml::from_str::<DaemonConfig>(&content)
                .with_context(|| format!("parsing {}", config_path.display()))?
        } else {
            DaemonConfig {
                home: PathBuf::new(),
            }
        };

        config.home = home;
        Ok(config)
    }

    pub fn ensure_dirs(&self) -> Result<()> {
        for sub in ["personas", "run", "audit", "logs", "vault"] {
            std::fs::create_dir_all(self.home.join(sub))
                .with_context(|| format!("creating {}", self.home.join(sub).display()))?;
        }
        Ok(())
    }
}

fn dirs_home() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
}
