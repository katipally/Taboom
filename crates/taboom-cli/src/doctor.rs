use anyhow::Result;
use std::path::Path;
use tokio::process::Command;

struct Check {
    name: &'static str,
    passed: bool,
    detail: String,
}

pub async fn run(home: &Path) -> Result<()> {
    let mut checks = Vec::new();

    checks.push(check_accelerator().await);
    checks.push(check_qemu().await);
    checks.push(check_ram().await);
    checks.push(check_disk(home).await);
    checks.push(check_home_dir(home).await);

    let all_passed = checks.iter().all(|c| c.passed);

    for c in &checks {
        let icon = if c.passed { "[OK]" } else { "[!!]" };
        println!("{} {}: {}", icon, c.name, c.detail);
    }

    if all_passed {
        println!("\nAll checks passed. System is ready.");
    } else {
        println!("\nSome checks failed. Fix the issues above before proceeding.");
    }

    Ok(())
}

async fn check_accelerator() -> Check {
    if cfg!(target_os = "linux") {
        let kvm_exists = Path::new("/dev/kvm").exists();
        Check {
            name: "Hardware acceleration (KVM)",
            passed: kvm_exists,
            detail: if kvm_exists {
                "/dev/kvm available".into()
            } else {
                "/dev/kvm not found; enable KVM in BIOS".into()
            },
        }
    } else if cfg!(target_os = "macos") {
        let output = Command::new("sysctl")
            .args(["kern.hv_support"])
            .output()
            .await;
        match output {
            Ok(o) => {
                let stdout = String::from_utf8_lossy(&o.stdout);
                let supported = stdout.contains(": 1");
                Check {
                    name: "Hardware acceleration (HVF)",
                    passed: supported,
                    detail: if supported {
                        "Hypervisor.framework available".into()
                    } else {
                        "Hypervisor.framework not available".into()
                    },
                }
            }
            Err(e) => Check {
                name: "Hardware acceleration (HVF)",
                passed: false,
                detail: format!("failed to check: {e}"),
            },
        }
    } else {
        Check {
            name: "Hardware acceleration",
            passed: false,
            detail: "unsupported platform".into(),
        }
    }
}

async fn check_qemu() -> Check {
    let output = Command::new("qemu-system-x86_64")
        .arg("--version")
        .output()
        .await;
    match output {
        Ok(o) if o.status.success() => {
            let version = String::from_utf8_lossy(&o.stdout);
            let first_line = version.lines().next().unwrap_or("unknown");
            Check {
                name: "QEMU",
                passed: true,
                detail: first_line.to_string(),
            }
        }
        Ok(_) => Check {
            name: "QEMU",
            passed: false,
            detail: "qemu-system-x86_64 found but returned error".into(),
        },
        Err(_) => Check {
            name: "QEMU",
            passed: false,
            detail: "qemu-system-x86_64 not found in PATH".into(),
        },
    }
}

async fn check_ram() -> Check {
    let total_mb = get_total_ram_mb().await;
    match total_mb {
        Some(mb) => {
            let enough = mb >= 8192;
            Check {
                name: "RAM",
                passed: enough,
                detail: format!("{} MB total (need >= 8192 MB)", mb),
            }
        }
        None => Check {
            name: "RAM",
            passed: false,
            detail: "could not determine system RAM".into(),
        },
    }
}

async fn get_total_ram_mb() -> Option<u64> {
    if cfg!(target_os = "macos") {
        let output = Command::new("sysctl")
            .args(["-n", "hw.memsize"])
            .output()
            .await
            .ok()?;
        let bytes: u64 = String::from_utf8_lossy(&output.stdout)
            .trim()
            .parse()
            .ok()?;
        Some(bytes / (1024 * 1024))
    } else if cfg!(target_os = "linux") {
        let meminfo = tokio::fs::read_to_string("/proc/meminfo").await.ok()?;
        for line in meminfo.lines() {
            if line.starts_with("MemTotal:") {
                let kb: u64 = line
                    .split_whitespace()
                    .nth(1)?
                    .parse()
                    .ok()?;
                return Some(kb / 1024);
            }
        }
        None
    } else {
        None
    }
}

async fn check_disk(home: &Path) -> Check {
    let dir = if home.exists() {
        home.to_path_buf()
    } else {
        home.parent()
            .map(|p| p.to_path_buf())
            .unwrap_or_else(|| std::path::PathBuf::from("/"))
    };

    let output = Command::new("df")
        .args(["-m", &dir.display().to_string()])
        .output()
        .await;

    match output {
        Ok(o) if o.status.success() => {
            let stdout = String::from_utf8_lossy(&o.stdout);
            if let Some(line) = stdout.lines().nth(1) {
                let avail: Option<u64> = line.split_whitespace().nth(3).and_then(|s| s.parse().ok());
                if let Some(mb) = avail {
                    let enough = mb >= 20_000;
                    return Check {
                        name: "Disk space",
                        passed: enough,
                        detail: format!("{} MB available (need >= 20000 MB)", mb),
                    };
                }
            }
            Check {
                name: "Disk space",
                passed: false,
                detail: "could not parse df output".into(),
            }
        }
        _ => Check {
            name: "Disk space",
            passed: false,
            detail: "could not check disk space".into(),
        },
    }
}

async fn check_home_dir(home: &Path) -> Check {
    if home.exists() {
        Check {
            name: "Taboom home",
            passed: true,
            detail: format!("{} exists", home.display()),
        }
    } else {
        Check {
            name: "Taboom home",
            passed: true,
            detail: format!("{} will be created on first use", home.display()),
        }
    }
}
