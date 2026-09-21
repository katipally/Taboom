use anyhow::{Context, Result};
use std::path::Path;
use tokio::process::Command;

pub async fn create_overlay(base: &Path, overlay: &Path) -> Result<()> {
    let status = Command::new("qemu-img")
        .args([
            "create",
            "-f",
            "qcow2",
            "-b",
            &base.display().to_string(),
            "-F",
            "qcow2",
            &overlay.display().to_string(),
        ])
        .status()
        .await
        .context("failed to run qemu-img")?;

    anyhow::ensure!(status.success(), "qemu-img create failed with {status}");
    Ok(())
}

pub async fn create_empty_disk(path: &Path, size_gb: u32) -> Result<()> {
    let status = Command::new("qemu-img")
        .args([
            "create",
            "-f",
            "qcow2",
            &path.display().to_string(),
            &format!("{}G", size_gb),
        ])
        .status()
        .await
        .context("failed to run qemu-img")?;

    anyhow::ensure!(status.success(), "qemu-img create failed with {status}");
    Ok(())
}
