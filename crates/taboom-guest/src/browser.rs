use anyhow::{Context, Result, bail};
use serde_json::json;
use std::path::{Path, PathBuf};
use taboom_proto::{BrowserConfig, BrowserState, BrowserStateReport};
use tokio::process::Command;
use tracing::{info, warn};

const CHROME_BIN: &str = "/usr/bin/google-chrome-stable";
const PROFILE_DIR: &str = "/home/taboom/.config/google-chrome";
const POLICY_DIR: &str = "/etc/opt/chrome/policies/managed";
const LOCK_FILE: &str = "/home/taboom/.config/google-chrome/SingletonLock";

pub struct BrowserSupervisor {
    state: BrowserState,
    child: Option<tokio::process::Child>,
    config: BrowserConfig,
    user: String,
}

impl BrowserSupervisor {
    pub fn new(config: BrowserConfig) -> Self {
        Self {
            state: BrowserState::NotInstalled,
            child: None,
            config,
            user: "taboom".into(),
        }
    }

    pub fn state(&self) -> &BrowserState {
        &self.state
    }

    pub fn state_report(&self) -> BrowserStateReport {
        let pid = self.child.as_ref().and_then(|c| c.id());

        BrowserStateReport {
            state: self.state.clone(),
            pid,
            version: None,
        }
    }

    pub async fn ensure_installed(&mut self) -> Result<()> {
        if Path::new(CHROME_BIN).exists() {
            self.state = BrowserState::Installed;
            info!("Chrome already installed");
            return Ok(());
        }

        self.state = BrowserState::Installing;
        info!("installing Google Chrome from apt");

        install_chrome_apt().await?;

        if !Path::new(CHROME_BIN).exists() {
            self.state = BrowserState::NotInstalled;
            bail!("Chrome binary not found after installation");
        }

        self.state = BrowserState::Installed;
        info!("Chrome installed successfully");
        Ok(())
    }

    pub async fn start(&mut self) -> Result<()> {
        match self.state {
            BrowserState::NotInstalled | BrowserState::Installing => {
                bail!("Chrome is not installed");
            }
            BrowserState::Running => {
                info!("Chrome already running");
                return Ok(());
            }
            _ => {}
        }

        cleanup_stale_lock().await;
        write_profile_prefs(&self.config).await?;

        self.state = BrowserState::Starting;
        info!("starting Chrome");

        let child = Command::new("sudo")
            .args(["-u", &self.user, CHROME_BIN])
            .args(chrome_launch_args())
            .env("XDG_RUNTIME_DIR", "/run/user/1000")
            .env("WAYLAND_DISPLAY", "wayland-1")
            .kill_on_drop(false)
            .spawn()
            .context("spawning Chrome")?;

        self.child = Some(child);
        self.state = BrowserState::Running;
        info!("Chrome started");
        Ok(())
    }

    pub async fn stop(&mut self) -> Result<()> {
        if let Some(mut child) = self.child.take() {
            info!("stopping Chrome gracefully");
            let _ = child.kill().await;
            let _ = child.wait().await;
        }
        self.state = BrowserState::Stopped;
        Ok(())
    }

    pub async fn check_and_restart(&mut self) -> Result<()> {
        let exited = match &mut self.child {
            Some(child) => child.try_wait().ok().flatten().is_some(),
            None => self.state == BrowserState::Running,
        };

        if exited {
            warn!("Chrome exited unexpectedly, restarting");
            self.child = None;
            self.state = BrowserState::Crashed;
            self.start().await?;
        }

        Ok(())
    }
}

fn chrome_launch_args() -> Vec<&'static str> {
    vec![
        "--ozone-platform=wayland",
        &concat_user_data_dir(),
        "--no-first-run",
        "--no-default-browser-check",
        "--restore-last-session",
    ]
}

fn concat_user_data_dir() -> &'static str {
    // Leak a static reference; called once at startup
    let s = format!("--user-data-dir={PROFILE_DIR}");
    Box::leak(s.into_boxed_str())
}

async fn install_chrome_apt() -> Result<()> {
    run_cmd("bash", &[
        "-c",
        concat!(
            "wget -qO- https://dl.google.com/linux/linux_signing_key.pub ",
            "| gpg --dearmor -o /usr/share/keyrings/google-chrome.gpg"
        ),
    ]).await.context("importing Chrome signing key")?;

    run_cmd("bash", &[
        "-c",
        concat!(
            "echo 'deb [arch=amd64 signed-by=/usr/share/keyrings/google-chrome.gpg] ",
            "https://dl.google.com/linux/chrome/deb/ stable main' ",
            "> /etc/apt/sources.list.d/google-chrome.list"
        ),
    ]).await.context("adding Chrome apt source")?;

    run_cmd("apt-get", &["update", "-qq"]).await.context("apt update")?;

    run_cmd("apt-get", &[
        "install", "-y", "-qq", "google-chrome-stable",
    ]).await.context("installing google-chrome-stable")?;

    setup_unattended_upgrades().await?;

    Ok(())
}

async fn setup_unattended_upgrades() -> Result<()> {
    run_cmd("apt-get", &[
        "install", "-y", "-qq", "unattended-upgrades",
    ]).await.context("installing unattended-upgrades")?;

    let origins = concat!(
        "Unattended-Upgrade::Allowed-Origins {\n",
        "    \"${distro_id}:${distro_codename}-security\";\n",
        "    \"Google LLC:stable\";\n",
        "};\n",
        "Unattended-Upgrade::Automatic-Reboot \"false\";\n",
    );

    tokio::fs::write(
        "/etc/apt/apt.conf.d/51taboom-unattended",
        origins,
    ).await.context("writing unattended-upgrades config")?;

    Ok(())
}

async fn write_profile_prefs(config: &BrowserConfig) -> Result<()> {
    let prefs_dir = PathBuf::from(PROFILE_DIR).join("Default");
    tokio::fs::create_dir_all(&prefs_dir).await?;

    let prefs_path = prefs_dir.join("Preferences");

    let prefs = json!({
        "intl": {
            "accept_languages": config.accept_languages,
        },
        "download": {
            "default_directory": config.download_dir,
            "prompt_for_download": false,
        },
        "session": {
            "restore_on_startup": 1,
        },
        "browser": {
            "check_default_browser": false,
        },
    });

    let content = serde_json::to_string_pretty(&prefs)?;
    tokio::fs::write(&prefs_path, content).await?;

    // Fix ownership so the taboom user can read/write the profile
    let _ = Command::new("chown")
        .args(["-R", "taboom:taboom", PROFILE_DIR])
        .status()
        .await;

    Ok(())
}

async fn cleanup_stale_lock() {
    if Path::new(LOCK_FILE).exists() {
        info!("removing stale Chrome lock file");
        let _ = tokio::fs::remove_file(LOCK_FILE).await;
    }
}

pub async fn install_managed_policy() -> Result<()> {
    tokio::fs::create_dir_all(POLICY_DIR).await?;

    let policy = json!({
        "WebRtcIPHandlingPolicy": "disable_non_proxied_udp",
        "HideFirstRunUI": true,
        "DefaultBrowserSettingEnabled": false,
        "ShowFullUrlsInAddressBar": true,
        "AutofillCreditCardEnabled": false,
        "PasswordManagerEnabled": false,
        "MetricsReportingEnabled": false,
        "SpellcheckEnabled": true,
        "TranslateEnabled": false,
        "BackgroundModeEnabled": false,
        "BrowserSignin": 0,
        "SyncDisabled": true,
    });

    let content = serde_json::to_string_pretty(&policy)?;
    tokio::fs::write(
        PathBuf::from(POLICY_DIR).join("taboom-policy.json"),
        content,
    ).await.context("writing Chrome managed policy")?;

    info!("Chrome managed policy installed");
    Ok(())
}

async fn run_cmd(program: &str, args: &[&str]) -> Result<()> {
    let status = Command::new(program)
        .args(args)
        .status()
        .await
        .with_context(|| format!("running {program}"))?;

    if !status.success() {
        bail!("{program} failed with {status}");
    }

    Ok(())
}

pub async fn get_chrome_version() -> Option<String> {
    let output = Command::new(CHROME_BIN)
        .arg("--version")
        .output()
        .await
        .ok()?;

    if output.status.success() {
        Some(
            String::from_utf8_lossy(&output.stdout)
                .trim()
                .to_string(),
        )
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn launch_args_contain_wayland() {
        let args = chrome_launch_args();
        assert!(args.contains(&"--ozone-platform=wayland"));
    }

    #[test]
    fn launch_args_no_debug_or_automation() {
        let args = chrome_launch_args();
        for arg in &args {
            assert!(!arg.contains("remote-debugging"));
            assert!(!arg.contains("enable-automation"));
            assert!(!arg.contains("headless"));
        }
    }

    #[tokio::test]
    async fn state_report_initial() {
        let sup = BrowserSupervisor::new(BrowserConfig::default());
        let report = sup.state_report();
        assert_eq!(report.state, BrowserState::NotInstalled);
        assert!(report.pid.is_none());
    }
}
