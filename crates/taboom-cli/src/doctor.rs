use anyhow::Result;
use tokio::process::Command;

struct Check {
    name: &'static str,
    passed: bool,
    detail: String,
}

pub async fn run() -> Result<()> {
    let checks = [
        check_command("Docker CLI", "docker", &["--version"]).await,
        check_command("Docker engine", "docker", &["info"]).await,
        check_command("Docker Compose v2", "docker", &["compose", "version"]).await,
    ];

    for check in &checks {
        let icon = if check.passed { "[OK]" } else { "[!!]" };
        println!("{icon} {}: {}", check.name, check.detail);
    }

    if checks.iter().all(|check| check.passed) {
        println!("\nDocker is ready to run Taboom.");
    } else {
        println!("\nInstall or start Docker Desktop, then run `docker compose up -d --build`.");
    }

    Ok(())
}

async fn check_command(name: &'static str, program: &str, args: &[&str]) -> Check {
    match Command::new(program).args(args).output().await {
        Ok(output) if output.status.success() => {
            let stdout = String::from_utf8_lossy(&output.stdout);
            let detail = stdout.lines().next().unwrap_or("available").to_string();
            Check {
                name,
                passed: true,
                detail,
            }
        }
        Ok(output) => {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let detail = stderr.lines().next().unwrap_or("command failed").to_string();
            Check {
                name,
                passed: false,
                detail,
            }
        }
        Err(error) => Check {
            name,
            passed: false,
            detail: error.to_string(),
        },
    }
}
