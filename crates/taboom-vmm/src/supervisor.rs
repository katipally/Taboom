use crate::qemu::QemuConfig;
use anyhow::{Context, Result};
use tokio::process::Command;
use tracing::{info, warn, error};
use uuid::Uuid;

pub struct VmSupervisor {
    pub id: Uuid,
    pub config: QemuConfig,
    pid: Option<u32>,
}

impl VmSupervisor {
    pub fn new(config: QemuConfig) -> Self {
        Self {
            id: Uuid::new_v4(),
            config,
            pid: None,
        }
    }

    pub async fn start(&mut self) -> Result<()> {
        let args = self.config.build_args();
        info!(vm = %self.config.name, "starting QEMU");

        let qemu_bin = match self.config.platform {
            crate::qemu::Platform::LinuxKvm => {
                if cfg!(target_arch = "aarch64") {
                    "qemu-system-aarch64"
                } else {
                    "qemu-system-x86_64"
                }
            }
            crate::qemu::Platform::MacosHvf => "qemu-system-aarch64",
        };

        let status = Command::new(qemu_bin)
            .args(&args)
            .status()
            .await
            .context(format!("failed to start {qemu_bin}"))?;

        anyhow::ensure!(status.success(), "QEMU exited with {status}");

        self.pid = self.read_pidfile().await;
        info!(vm = %self.config.name, pid = ?self.pid, "QEMU started");
        Ok(())
    }

    pub async fn stop(&mut self) -> Result<()> {
        if let Some(pid) = self.pid {
            info!(vm = %self.config.name, pid, "sending SIGTERM to QEMU");
            #[cfg(unix)]
            {
                use nix::sys::signal::{kill, Signal};
                use nix::unistd::Pid;
                let _ = kill(Pid::from_raw(pid as i32), Signal::SIGTERM);
            }
            self.pid = None;
        }
        Ok(())
    }

    pub fn is_running(&self) -> bool {
        let Some(pid) = self.pid else { return false };
        #[cfg(unix)]
        {
            use nix::sys::signal::kill;
            use nix::unistd::Pid;
            kill(Pid::from_raw(pid as i32), None).is_ok()
        }
        #[cfg(not(unix))]
        {
            let _ = pid;
            false
        }
    }

    pub async fn try_adopt(config: QemuConfig) -> Result<Option<Self>> {
        let pidfile = config.pidfile.clone();
        if !pidfile.exists() {
            return Ok(None);
        }

        let content = tokio::fs::read_to_string(&pidfile)
            .await
            .context("reading pidfile")?;
        let pid: u32 = content.trim().parse().context("parsing pid")?;

        let sup = Self {
            id: Uuid::new_v4(),
            config,
            pid: Some(pid),
        };

        if sup.is_running() {
            info!(pid, "re-adopted running QEMU process");
            Ok(Some(sup))
        } else {
            warn!(pid, "stale pidfile found, cleaning up");
            let _ = tokio::fs::remove_file(&pidfile).await;
            Ok(None)
        }
    }

    async fn read_pidfile(&self) -> Option<u32> {
        match tokio::fs::read_to_string(&self.config.pidfile).await {
            Ok(s) => s.trim().parse().ok(),
            Err(e) => {
                error!("failed to read pidfile: {e}");
                None
            }
        }
    }
}
