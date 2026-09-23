use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::io::{self, BufRead, Write};
use std::path::Path;

#[derive(Debug, Serialize, Deserialize)]
pub struct PersonaConfig {
    pub name: String,
    pub cpus: u32,
    pub ram_mb: u32,
    #[serde(default = "default_timezone")]
    pub timezone: String,
    #[serde(default)]
    pub route: RouteConfig,
    #[serde(default)]
    pub browser: BrowserSettings,
    #[serde(default)]
    pub hardware: Option<HardwareConfig>,
    #[serde(default)]
    pub identity: Option<IdentityConfig>,
    #[serde(default)]
    pub humanizer_seed: Option<u64>,
    #[serde(default)]
    pub humanizer_style: Option<HumanizerStyle>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct HardwareConfig {
    pub screen_width: u32,
    pub screen_height: u32,
    #[serde(default = "default_dpr")]
    pub dpr: f64,
}

fn default_dpr() -> f64 {
    1.0
}

#[derive(Debug, Serialize, Deserialize)]
pub struct IdentityConfig {
    #[serde(default = "default_keyboard")]
    pub keyboard_layout: String,
    #[serde(default = "default_locale")]
    pub locale: String,
    #[serde(default)]
    pub languages: Vec<String>,
    #[serde(default)]
    pub fonts_packages: Vec<String>,
}

fn default_keyboard() -> String {
    "us".into()
}
fn default_locale() -> String {
    "en_US.UTF-8".into()
}

#[derive(Debug, Serialize, Deserialize)]
pub struct HumanizerStyle {
    #[serde(default = "default_speed")]
    pub speed: SpeedClass,
    #[serde(default)]
    pub typo_rate: f64,
    #[serde(default)]
    pub overshoot_tendency: f64,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SpeedClass {
    Slow,
    Medium,
    Fast,
}

fn default_speed() -> SpeedClass {
    SpeedClass::Medium
}

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum RouteConfig {
    #[default]
    Direct,
    Proxy {
        address: String,
        port: u16,
        #[serde(default)]
        username: Option<String>,
        #[serde(default)]
        password: Option<String>,
        #[serde(default = "default_proxy_protocol")]
        protocol: ProxyProtocolConfig,
    },
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ProxyProtocolConfig {
    Socks5,
    Http,
}

fn default_proxy_protocol() -> ProxyProtocolConfig {
    ProxyProtocolConfig::Socks5
}

#[derive(Debug, Serialize, Deserialize)]
pub struct BrowserSettings {
    #[serde(default = "default_accept_languages")]
    pub accept_languages: String,
    #[serde(default = "default_download_dir")]
    pub download_dir: String,
}

impl Default for BrowserSettings {
    fn default() -> Self {
        Self {
            accept_languages: default_accept_languages(),
            download_dir: default_download_dir(),
        }
    }
}

fn default_accept_languages() -> String {
    "en-US,en".into()
}
fn default_download_dir() -> String {
    "/home/taboom/Downloads".into()
}

fn default_timezone() -> String {
    "America/New_York".into()
}

struct DerivedPersona {
    timezone: String,
    identity: IdentityConfig,
    accept_languages: String,
    screen_width: u32,
    screen_height: u32,
    dpr: f64,
}

fn derive_persona(seed: u64) -> DerivedPersona {
    use rand::prelude::*;
    use rand::SeedableRng;

    let mut rng = rand::rngs::StdRng::seed_from_u64(seed);

    struct ScreenEntry {
        w: u32,
        h: u32,
        dpr: f64,
        weight: u32,
    }

    let screens = [
        ScreenEntry { w: 1366, h: 768,  dpr: 1.0,  weight: 15 },
        ScreenEntry { w: 1920, h: 1080, dpr: 1.0,  weight: 30 },
        ScreenEntry { w: 1920, h: 1080, dpr: 1.25, weight: 10 },
        ScreenEntry { w: 2560, h: 1440, dpr: 1.0,  weight: 8 },
        ScreenEntry { w: 2560, h: 1440, dpr: 1.25, weight: 5 },
        ScreenEntry { w: 2560, h: 1600, dpr: 2.0,  weight: 7 },
        ScreenEntry { w: 3840, h: 2160, dpr: 1.5,  weight: 3 },
        ScreenEntry { w: 1920, h: 1200, dpr: 1.0,  weight: 10 },
    ];

    let weights: Vec<u32> = screens.iter().map(|s| s.weight).collect();
    let dist = rand::distributions::WeightedIndex::new(&weights).unwrap();
    let pick = &screens[rng.sample(dist)];

    DerivedPersona {
        timezone: "America/New_York".into(),
        identity: IdentityConfig {
            keyboard_layout: "us".into(),
            locale: "en_US.UTF-8".into(),
            languages: vec!["en-US".into(), "en".into()],
            fonts_packages: vec![],
        },
        accept_languages: "en-US,en".into(),
        screen_width: pick.w,
        screen_height: pick.h,
        dpr: pick.dpr,
    }
}

fn personas_dir(home: &Path) -> std::path::PathBuf {
    home.join("personas")
}

fn ensure_dirs(home: &Path) -> Result<()> {
    for sub in ["personas", "run", "audit", "logs"] {
        std::fs::create_dir_all(home.join(sub))
            .with_context(|| format!("creating {}", home.join(sub).display()))?;
    }
    Ok(())
}

pub async fn persona_create(home: &Path, name: &str, cpus: u32, ram: u32) -> Result<()> {
    ensure_dirs(home)?;
    let path = personas_dir(home).join(format!("{name}.toml"));
    if path.exists() {
        bail!("persona '{name}' already exists");
    }

    let seed: u64 = rand::random();
    let derived = derive_persona(seed);

    let config = PersonaConfig {
        name: name.to_string(),
        cpus,
        ram_mb: ram,
        timezone: derived.timezone,
        route: RouteConfig::Direct,
        browser: BrowserSettings {
            accept_languages: derived.accept_languages,
            download_dir: default_download_dir(),
        },
        hardware: Some(HardwareConfig {
            screen_width: derived.screen_width,
            screen_height: derived.screen_height,
            dpr: derived.dpr,
        }),
        identity: Some(derived.identity),
        humanizer_seed: Some(seed),
        humanizer_style: Some(HumanizerStyle {
            speed: SpeedClass::Medium,
            typo_rate: 0.02,
            overshoot_tendency: 0.15,
        }),
    };

    let content = toml::to_string_pretty(&config)?;
    std::fs::write(&path, content)?;
    println!("Created persona '{name}' (seed={seed})");
    Ok(())
}

pub async fn persona_list(home: &Path) -> Result<()> {
    let dir = personas_dir(home);
    if !dir.exists() {
        println!("No personas found.");
        return Ok(());
    }

    let mut entries: Vec<_> = std::fs::read_dir(&dir)?
        .filter_map(|e| e.ok())
        .filter(|e| {
            e.path()
                .extension()
                .map(|ext| ext == "toml")
                .unwrap_or(false)
        })
        .collect();
    entries.sort_by_key(|e| e.file_name());

    if entries.is_empty() {
        println!("No personas found.");
        return Ok(());
    }

    println!(
        "{:<20} {:>4} {:>8} {:>12} {:>10} {:>8} {:>8}",
        "NAME", "CPUS", "RAM (MB)", "SCREEN", "LOCALE", "ROUTE", "STYLE"
    );
    for entry in entries {
        let content = std::fs::read_to_string(entry.path())?;
        let config: PersonaConfig = toml::from_str(&content)?;
        let screen = config
            .hardware
            .as_ref()
            .map(|h| format!("{}x{}", h.screen_width, h.screen_height))
            .unwrap_or_else(|| "-".into());
        let locale = config
            .identity
            .as_ref()
            .map(|i| i.locale.as_str())
            .unwrap_or("-");
        let route = match &config.route {
            RouteConfig::Direct => "direct",
            RouteConfig::Proxy { .. } => "proxy",
        };
        let style = config
            .humanizer_style
            .as_ref()
            .map(|s| format!("{:?}", s.speed))
            .unwrap_or_else(|| "-".into());
        println!(
            "{:<20} {:>4} {:>8} {:>12} {:>10} {:>8} {:>8}",
            config.name, config.cpus, config.ram_mb, screen, locale, route, style
        );
    }
    Ok(())
}

pub async fn persona_show(home: &Path, name: &str) -> Result<()> {
    let path = personas_dir(home).join(format!("{name}.toml"));
    if !path.exists() {
        bail!("persona '{name}' not found");
    }
    let content = std::fs::read_to_string(&path)?;
    print!("{content}");
    Ok(())
}

pub async fn persona_delete(home: &Path, name: &str) -> Result<()> {
    let path = personas_dir(home).join(format!("{name}.toml"));
    if !path.exists() {
        bail!("persona '{name}' not found");
    }
    std::fs::remove_file(&path)?;
    println!("Deleted persona '{name}'");
    Ok(())
}

pub async fn logs(home: &Path, persona: Option<&str>, lines: usize) -> Result<()> {
    let audit_dir = home.join("audit");
    if !audit_dir.exists() {
        println!("No logs found.");
        return Ok(());
    }

    let log_file = match persona {
        Some(p) => audit_dir.join(format!("{p}.jsonl")),
        None => audit_dir.join("taboomd.jsonl"),
    };

    if !log_file.exists() {
        println!("No logs found at {}", log_file.display());
        return Ok(());
    }

    let content = std::fs::read_to_string(&log_file)?;
    let total: Vec<&str> = content.lines().collect();
    let start = total.len().saturating_sub(lines);
    for line in &total[start..] {
        println!("{line}");
    }
    Ok(())
}

pub async fn vault_init(home: &Path) -> Result<()> {
    let vault_path = home.join("vault").join("vault.age");
    if vault_path.exists() {
        bail!("vault already exists");
    }

    let passphrase = prompt_passphrase("Enter passphrase for new vault: ")?;
    let confirm = prompt_passphrase("Confirm passphrase: ")?;
    if passphrase != confirm {
        bail!("passphrases do not match");
    }

    std::fs::create_dir_all(home.join("vault"))?;
    std::fs::write(&vault_path, b"")?;
    println!("Vault initialized at {}", vault_path.display());
    Ok(())
}

pub async fn vault_unlock(home: &Path) -> Result<()> {
    let passphrase = prompt_passphrase("Enter vault passphrase: ")?;
    let sock = daemon_sock(home)?;
    let resp = ipc_command(&sock, &format!("vault-unlock {passphrase}")).await?;
    println!("{resp}");
    Ok(())
}

pub async fn vault_lock(home: &Path) -> Result<()> {
    let sock = daemon_sock(home)?;
    let resp = ipc_command(&sock, "vault-lock").await?;
    println!("{resp}");
    Ok(())
}

pub async fn vault_add(
    home: &Path,
    name: &str,
    secret_type: &str,
    _domains: Vec<String>,
) -> Result<()> {
    let value = prompt_passphrase(&format!("Enter value for secret '{name}': "))?;
    let sock = daemon_sock(home)?;
    let resp = ipc_command(&sock, &format!("vault-add {name} {value} {secret_type}")).await?;
    println!("{resp}");
    Ok(())
}

pub async fn vault_list(home: &Path) -> Result<()> {
    let sock = daemon_sock(home)?;
    let resp = ipc_command(&sock, "vault-list").await?;
    println!("{resp}");
    Ok(())
}

pub async fn vault_remove(home: &Path, name: &str) -> Result<()> {
    let sock = daemon_sock(home)?;
    let resp = ipc_command(&sock, &format!("vault-remove {name}")).await?;
    println!("{resp}");
    Ok(())
}

pub async fn vault_status(home: &Path) -> Result<()> {
    let vault_path = home.join("vault").join("vault.age");
    if vault_path.exists() {
        println!("Vault exists at {}", vault_path.display());
        println!("Status: locked (daemon not queried yet)");
    } else {
        println!("No vault found. Run `taboom vault init` to create one.");
    }
    Ok(())
}

pub async fn connect_claude_code(host: &str, port: u16, token: Option<&str>) -> Result<()> {
    let url = format!("http://{host}:{port}/mcp");
    let mut cmd = std::process::Command::new("claude");
    cmd.args(["mcp", "add", "--transport", "http", "--scope", "user", "taboom", &url]);
    if let Some(tok) = token {
        cmd.args(["--header", &format!("Authorization: Bearer {tok}")]);
    }

    let status = cmd
        .status()
        .context("running `claude mcp add` (is Claude Code installed and on PATH?)")?;
    if !status.success() {
        bail!("`claude mcp add` failed; remove an old entry with `claude mcp remove taboom -s user`");
    }
    println!("Registered taboom at {url}. Run `claude mcp list` to check it.");
    Ok(())
}

pub async fn connect_generic(host: &str, port: u16, token: Option<&str>) -> Result<()> {
    let mut config = serde_json::json!({
        "mcpServers": {
            "taboom": {
                "url": format!("http://{host}:{port}/mcp"),
            }
        }
    });

    if let Some(tok) = token {
        config["mcpServers"]["taboom"]["headers"] = serde_json::json!({
            "Authorization": format!("Bearer {tok}")
        });
    }

    println!("{}", serde_json::to_string_pretty(&config)?);
    Ok(())
}

pub async fn stdio_bridge(host: &str, port: u16, token: Option<&str>) -> Result<()> {
    let url = format!("http://{host}:{port}/mcp");
    let auth_header = token.map(|t| format!("Bearer {t}"));

    let stdin = io::stdin();
    let stdout = io::stdout();

    for line in stdin.lock().lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }

        let mut cmd = std::process::Command::new("curl");
        cmd.args(["-s", "-X", "POST", &url])
            .args(["-H", "Content-Type: application/json"])
            .args(["-d", &line]);

        if let Some(ref auth) = auth_header {
            cmd.args(["-H", &format!("Authorization: {auth}")]);
        }

        let output = cmd.output()?;
        // Notifications come back as 202 with no body; a blank line would corrupt stdio framing.
        if output.stdout.is_empty() {
            continue;
        }
        let mut out = stdout.lock();
        out.write_all(&output.stdout)?;
        out.write_all(b"\n")?;
        out.flush()?;
    }
    Ok(())
}

fn daemon_sock(home: &Path) -> Result<std::path::PathBuf> {
    let sock = home.join("run").join("taboomd.sock");
    if !sock.exists() {
        bail!("taboomd is not running (socket not found)");
    }
    Ok(sock)
}

async fn ipc_command(sock_path: &Path, command: &str) -> Result<String> {
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    use tokio::net::UnixStream;

    let stream = UnixStream::connect(sock_path)
        .await
        .context("connecting to taboomd")?;

    let (reader, mut writer) = stream.into_split();
    writer.write_all(command.as_bytes()).await?;
    writer.write_all(b"\n").await?;
    writer.flush().await?;
    writer.shutdown().await?;

    let mut lines = BufReader::new(reader).lines();
    let response = lines
        .next_line()
        .await?
        .unwrap_or_else(|| "no response from daemon".into());

    Ok(response)
}

fn prompt_passphrase(prompt: &str) -> Result<String> {
    eprint!("{prompt}");
    io::stderr().flush()?;
    let mut line = String::new();
    io::stdin().lock().read_line(&mut line)?;
    let trimmed = line.trim_end_matches('\n').trim_end_matches('\r');
    if trimmed.is_empty() {
        bail!("empty passphrase");
    }
    Ok(trimmed.to_string())
}
