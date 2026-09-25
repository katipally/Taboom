use anyhow::{bail, Context, Result};
use base64::Engine as _;
use chrono::Utc;
use clap::{Parser, Subcommand, ValueEnum};
use nix::sys::signal::{sigaction, SaFlags, SigAction, SigHandler, SigSet, Signal};
use nix::{errno::Errno, libc};
use rand::RngCore;
use serde_json::{json, Value};
use std::cell::UnsafeCell;
use std::collections::VecDeque;
use std::io::{self, BufRead, BufReader, Read, Write};
use std::mem::MaybeUninit;
use std::os::fd::AsRawFd;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::Command as ProcessCommand;
use std::sync::atomic::{AtomicI32, Ordering};
use std::time::Duration;
use taboom_core::admin::{AdminCommand, AdminRequest, AdminResponse, AdminSecretType, ADMIN_MAX_LINE_BYTES};
use taboom_core::persona::{check_name, derive_persona, fortress_is_installed, Persona, System};

#[derive(Parser)]
#[command(name = "taboom", version, about = "Private local administration for the Taboom container")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Manage the encrypted local vault.
    Vault {
        #[command(subcommand)]
        command: VaultCommand,
    },
    /// Create, show, or validate the active persona.
    Persona {
        #[command(subcommand)]
        command: PersonaCommand,
    },
    /// Show active persona, route, and session status.
    Status,
    /// Print the last N lines of Chrome's log.
    Logs {
        #[arg(short = 'n', default_value_t = 50)]
        lines: usize,
    },
    /// List recordings or create a signed replay link.
    Recordings {
        #[command(subcommand)]
        command: RecordingCommand,
    },
    /// Call an MCP tool directly, without an LLM. JSON must be an object.
    Call {
        tool: String,
        arguments: String,
    },
    /// Capture a manual detector-site snapshot for the active persona.
    Gauntlet,
}

#[derive(Subcommand)]
enum VaultCommand {
    /// Create an encrypted vault and unlock it.
    Init,
    /// Unlock the existing vault for this daemon process.
    Unlock,
    /// Lock the vault in this daemon process.
    Lock,
    /// Add a secret. Its value is prompted with terminal echo disabled.
    Add {
        name: String,
        #[arg(long = "type", value_enum)]
        secret_type: CliSecretType,
        #[arg(long, value_delimiter = ',', required = true)]
        domains: Vec<String>,
    },
    /// Remove a secret by name.
    #[command(alias = "remove")]
    Rm { name: String },
    /// List secret names, types, and allowed domains (never secret values).
    Ls,
    /// Show vault lock state.
    Status,
}

#[derive(Clone, Copy, ValueEnum)]
enum CliSecretType {
    Password,
    Totp,
    Note,
}

impl From<CliSecretType> for AdminSecretType {
    fn from(value: CliSecretType) -> Self {
        match value {
            CliSecretType::Password => Self::Password,
            CliSecretType::Totp => Self::Totp,
            CliSecretType::Note => Self::Note,
        }
    }
}

#[derive(Subcommand)]
enum PersonaCommand {
    /// Safely create a persona TOML file; restart the container to apply it.
    New {
        name: String,
        #[arg(long)]
        country: String,
    },
    /// Print the active persona TOML.
    Show,
    /// Validate the persona and report applied runtime and route health.
    Check,
}

#[derive(Subcommand)]
enum RecordingCommand {
    /// List recent recordings.
    Ls,
    /// Create a signed share link for a recording ID.
    Share { id: String },
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Command::Vault { command } => vault_command(command),
        Command::Persona { command } => persona_command(command),
        Command::Status => print_json(&mcp_client()?.call_tool("persona_status", &json!({}))?),
        Command::Logs { lines } => print_logs(lines),
        Command::Recordings { command } => recordings_command(command),
        Command::Call { tool, arguments } => {
            let args = parse_arguments(&arguments)?;
            print_json(&mcp_client()?.call_tool(&tool, &args)?)
        }
        Command::Gauntlet => gauntlet_command(),
    }
}

fn gauntlet_command() -> Result<()> {
    let client = mcp_client()?;
    let lab = lab_dir()?;
    let timestamp = Utc::now();
    let recorded_at = timestamp.to_rfc3339();
    let run_id = timestamp.format("%Y%m%dT%H%M%S%.3fZ").to_string();
    let run_dir = lab.join("gauntlet").join(&run_id);
    std::fs::create_dir_all(&run_dir)
        .with_context(|| format!("creating gauntlet output directory {}", run_dir.display()))?;

    let status = call_tool_json(&client, "persona_status", &json!({}))?;
    if status.get("session").is_some_and(|session| !session.is_null()) {
        bail!("a desktop session is already active; end it before running the gauntlet");
    }
    let persona = status.get("persona").and_then(Value::as_str)
        .context("persona_status did not report the active persona")?.to_string();
    let engine = status.get("declared").and_then(|p| p.get("browser"))
        .and_then(|b| b.get("engine")).and_then(Value::as_str)
        .context("persona_status did not report the browser engine")?.to_string();

    let started = call_tool_json(&client, "session_start", &json!({}))?;
    if started.get("status").and_then(Value::as_str) != Some("started") {
        bail!("gauntlet could not start a fresh session: {}", started);
    }

    // Always release the desktop lease, including when capture/OCR fails. The first screenshot
    // is a capability check: after secret typing the normal MCP path refuses visual output, so
    // this command stops before navigating to any detector site.
    let pages_result = capture_gauntlet_pages(&client, &run_dir);
    let end_result = call_tool_json(&client, "session_end", &json!({}));
    let pages = match (pages_result, end_result) {
        (Ok(pages), Ok(_)) => pages,
        (Err(error), Ok(_)) => return Err(error),
        (Err(error), Err(end_error)) => {
            bail!("{error:#}; session_end also failed: {end_error:#}");
        }
        (Ok(_), Err(error)) => return Err(error.context("ending gauntlet desktop session")),
    };

    let (commit, commit_source) = git_commit();
    if commit == "unknown" {
        eprintln!("Could not read git metadata; recording commit as unknown. Set TABOOM_GIT_COMMIT to include it.");
    }
    let run = json!({
        "run_id": run_id,
        "recorded_at": recorded_at,
        "persona": persona,
        "engine": engine,
        "commit": commit,
        "commit_source": commit_source,
        "pages": pages,
    });
    let scoreboard = lab.join("scoreboard.json");
    append_gauntlet_run(&scoreboard, run)?;
    println!("Gauntlet artifacts saved in {}", run_dir.display());
    println!("Scoreboard updated at {}", scoreboard.display());
    Ok(())
}

const GAUNTLET_PAGES: [(&str, &str); 4] = [
    ("creepjs", "https://abrahamjuliot.github.io/creepjs/"),
    ("sannysoft", "https://bot.sannysoft.com/"),
    ("browserscan", "https://www.browserscan.net/"),
    ("pixelscan", "https://pixelscan.net/"),
];

fn capture_gauntlet_pages(client: &McpClient, run_dir: &Path) -> Result<Vec<Value>> {
    screenshot_png(client)?;
    let mut pages = Vec::with_capacity(GAUNTLET_PAGES.len());
    for (name, url) in GAUNTLET_PAGES {
        call_tool_json(client, "open_url", &json!({ "url": url }))?;
        call_tool_json(client, "wait", &json!({ "ms": 15_000 }))?;

        let screenshot_name = format!("{name}.png");
        let screenshot_path = run_dir.join(&screenshot_name);
        let screenshot = screenshot_png(client)?;
        std::fs::write(&screenshot_path, screenshot)
            .with_context(|| format!("writing {}", screenshot_path.display()))?;

        let lines = call_tool_json(client, "read_text", &json!({}))?;
        let ocr_text = ocr_lines_to_text(&lines);
        let text_name = format!("{name}.txt");
        let text_path = run_dir.join(&text_name);
        std::fs::write(&text_path, ocr_text)
            .with_context(|| format!("writing {}", text_path.display()))?;

        pages.push(json!({
            "name": name,
            "url": url,
            "screenshot": format!("gauntlet/{}/{}", run_dir.file_name().and_then(|n| n.to_str()).unwrap_or(""), screenshot_name),
            "ocr_text": format!("gauntlet/{}/{}", run_dir.file_name().and_then(|n| n.to_str()).unwrap_or(""), text_name),
        }));
    }
    Ok(pages)
}

fn screenshot_png(client: &McpClient) -> Result<Vec<u8>> {
    let result = client.call_tool_raw("screenshot", &json!({}))?;
    screenshot_png_bytes(&result)
}

fn screenshot_png_bytes(result: &Value) -> Result<Vec<u8>> {
    ensure_tool_success("screenshot", result)?;
    let image = result.get("content").and_then(Value::as_array)
        .and_then(|blocks| blocks.iter().find(|block| block.get("type").and_then(Value::as_str) == Some("image")))
        .context("screenshot tool returned no image block")?;
    let mime = image.get("mimeType").and_then(Value::as_str).unwrap_or_default();
    if mime != "image/png" {
        bail!("screenshot tool returned unsupported image type {mime:?}");
    }
    let encoded = image.get("data").and_then(Value::as_str).context("screenshot image had no base64 data")?;
    let bytes = base64::engine::general_purpose::STANDARD.decode(encoded)
        .context("screenshot image was not valid base64")?;
    if !bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        bail!("screenshot image data was not a PNG file");
    }
    Ok(bytes)
}

fn call_tool_json(client: &McpClient, tool: &str, arguments: &Value) -> Result<Value> {
    let result = client.call_tool_raw(tool, arguments)?;
    ensure_tool_success(tool, &result)?;
    let text = result.get("content").and_then(Value::as_array)
        .and_then(|blocks| blocks.iter().find(|block| block.get("type").and_then(Value::as_str) == Some("text")))
        .and_then(|block| block.get("text")).and_then(Value::as_str)
        .context("MCP tool returned no text content")?;
    serde_json::from_str(text).with_context(|| format!("{tool} returned malformed JSON content"))
}

fn ensure_tool_success(tool: &str, result: &Value) -> Result<()> {
    if result.get("isError").and_then(Value::as_bool).unwrap_or(false) {
        let message = result.get("content").and_then(Value::as_array)
            .and_then(|blocks| blocks.iter().find_map(|block| block.get("text").and_then(Value::as_str)))
            .unwrap_or("request refused");
        bail!("{tool} failed: {message}");
    }
    Ok(())
}

fn ocr_lines_to_text(lines: &Value) -> String {
    if let Some(lines) = lines.as_array() {
        return lines.iter().filter_map(|line| line.get("text").and_then(Value::as_str))
            .collect::<Vec<_>>().join("\n");
    }
    lines.as_str().map(str::to_owned).unwrap_or_else(|| lines.to_string())
}

fn lab_dir() -> Result<PathBuf> {
    if let Some(path) = std::env::var_os("TABOOM_LAB_DIR") {
        return Ok(PathBuf::from(path));
    }
    let cwd = std::env::current_dir()?;
    for ancestor in cwd.ancestors() {
        let candidate = ancestor.join("lab");
        if candidate.is_dir() {
            return Ok(candidate);
        }
    }
    Ok(taboom_home()?.join("lab"))
}

fn git_commit() -> (String, &'static str) {
    if let Some(commit) = std::env::var("TABOOM_GIT_COMMIT").ok().filter(|value| !value.trim().is_empty()) {
        return (commit.trim().to_string(), "TABOOM_GIT_COMMIT");
    }
    if let Some(path) = std::env::var_os("TABOOM_GIT_DIR").map(PathBuf::from) {
        if let Some(commit) = read_git_commit(&path) {
            return (commit, "TABOOM_GIT_DIR");
        }
    }
    if let Ok(output) = ProcessCommand::new("git").args(["rev-parse", "--short=12", "HEAD"]).output() {
        if output.status.success() {
            let commit = String::from_utf8_lossy(&output.stdout).trim().to_string();
            if !commit.is_empty() {
                return (commit, "working-tree");
            }
        }
    }
    ("unknown".into(), "unknown")
}

fn read_git_commit(path: &Path) -> Option<String> {
    let git_dir = if path.is_dir() {
        path.to_path_buf()
    } else {
        let contents = std::fs::read_to_string(path).ok()?;
        let target = contents.trim().strip_prefix("gitdir:")?.trim();
        let target = PathBuf::from(target);
        if target.is_absolute() { target } else { path.parent()?.join(target) }
    };
    let output = ProcessCommand::new("git")
        .arg(format!("--git-dir={}", git_dir.display()))
        .args(["rev-parse", "--short=12", "HEAD"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let commit = String::from_utf8_lossy(&output.stdout).trim().to_string();
    (!commit.is_empty()).then_some(commit)
}

fn append_gauntlet_run(path: &Path, run: Value) -> Result<()> {
    let parent = path.parent().context("scoreboard path has no parent directory")?;
    std::fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
    let mut board = if path.exists() {
        let contents = std::fs::read(path).with_context(|| format!("reading {}", path.display()))?;
        let current: Value = serde_json::from_slice(&contents).context("parsing scoreboard JSON")?;
        if current.is_array() {
            json!({ "results": current, "gauntlet_runs": [] })
        } else if current.is_object() {
            current
        } else {
            bail!("scoreboard must be an array or JSON object");
        }
    } else {
        json!({ "results": [], "gauntlet_runs": [] })
    };
    let object = board.as_object_mut().context("scoreboard is not an object")?;
    let runs = object.entry("gauntlet_runs").or_insert_with(|| json!([]));
    let runs = runs.as_array_mut().context("scoreboard gauntlet_runs must be an array")?;
    runs.push(run);

    let mut temp = tempfile::Builder::new().prefix(".scoreboard-").tempfile_in(parent)?;
    serde_json::to_writer_pretty(temp.as_file_mut(), &board)?;
    temp.as_file_mut().write_all(b"\n")?;
    temp.as_file().sync_all()?;
    temp.persist(path).map_err(|error| error.error)
        .with_context(|| format!("saving {}", path.display()))?;
    Ok(())
}

fn vault_command(command: VaultCommand) -> Result<()> {
    let value = match command {
        VaultCommand::Init => {
            let passphrase = read_secret("New vault passphrase: ")?;
            if passphrase.is_empty() {
                bail!("passphrase must not be empty");
            }
            let confirmation = read_secret("Confirm passphrase: ")?;
            if passphrase != confirmation {
                bail!("passphrases did not match");
            }
            admin(AdminCommand::VaultInit { passphrase })?
        }
        VaultCommand::Unlock => {
            let passphrase = read_secret("Vault passphrase: ")?;
            if passphrase.is_empty() {
                bail!("passphrase must not be empty");
            }
            admin(AdminCommand::VaultUnlock { passphrase })?
        }
        VaultCommand::Lock => admin(AdminCommand::VaultLock)?,
        VaultCommand::Add { name, secret_type, domains } => {
            if name.trim().is_empty() {
                bail!("secret name must not be empty");
            }
            let value = read_secret("Secret value: ")?;
            if value.is_empty() {
                bail!("secret value must not be empty");
            }
            admin(AdminCommand::VaultAdd {
                name,
                value,
                secret_type: secret_type.into(),
                domains,
            })?
        }
        VaultCommand::Rm { name } => admin(AdminCommand::VaultRemove { name })?,
        VaultCommand::Ls => admin(AdminCommand::VaultList)?,
        VaultCommand::Status => admin(AdminCommand::VaultStatus)?,
    };
    print_json(&value)
}

fn persona_command(command: PersonaCommand) -> Result<()> {
    match command {
        PersonaCommand::New { name, country } => create_persona(&name, &country),
        PersonaCommand::Show => {
            let path = active_persona_path()?;
            let contents = std::fs::read_to_string(&path)
                .with_context(|| format!("reading {}", path.display()))?;
            print!("{contents}");
            Ok(())
        }
        PersonaCommand::Check => {
            let path = active_persona_path()?;
            let text = std::fs::read_to_string(&path)
                .with_context(|| format!("reading {}", path.display()))?;
            let persona = Persona::parse(&text).context("parsing active persona")?;
            persona.validate(&probe_system()?)?;
            let status = mcp_client()?.call_tool("persona_status", &json!({}))?;
            print_json(&status)?;
            let runtime = status.get("result").unwrap_or(&status);
            let mismatch = runtime.get("mismatches").and_then(Value::as_array).is_some_and(|m| !m.is_empty());
            let route_ok = runtime.get("route").and_then(|r| r.get("ok")).and_then(Value::as_bool).unwrap_or(false);
            if status.get("isError").and_then(Value::as_bool) == Some(true) || mismatch || !route_ok {
                bail!("runtime consistency or route check reported a failure");
            }
            Ok(())
        }
    }
}

fn create_persona(name: &str, country: &str) -> Result<()> {
    check_name(name)?;
    if country.len() != 2 || !country.bytes().all(|c| c.is_ascii_uppercase()) {
        bail!("country must be a two-letter uppercase ISO code such as DE");
    }
    let seed = rand::thread_rng().next_u64() & i64::MAX as u64;
    let persona = derive_persona(name, country, seed)?;
    persona.validate(&probe_system()?)?;

    let home = taboom_home()?;
    let dir = home.join("personas");
    std::fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
    let path = dir.join(format!("{name}.toml"));
    let serialized = toml::to_string_pretty(&persona)?;
    write_persona_safely(&dir, &path, serialized.as_bytes())?;
    println!("Created {}. Restart the container to apply it.", path.display());
    Ok(())
}

fn write_persona_safely(dir: &Path, path: &Path, contents: &[u8]) -> Result<()> {
    let mut temp = tempfile::Builder::new().prefix(".taboom-persona-").tempfile_in(dir)
        .with_context(|| format!("preparing persona file in {}", dir.display()))?;
    temp.write_all(contents)?;
    temp.as_file().sync_all()?;
    temp.persist_noclobber(path).map_err(|error| {
        if path.exists() {
            anyhow::anyhow!("persona {} already exists", path.display())
        } else {
            anyhow::Error::new(error.error).context(format!("creating {}", path.display()))
        }
    })?;
    Ok(())
}

fn probe_system() -> Result<System> {
    let zone_tab = std::fs::read_to_string("/usr/share/zoneinfo/zone.tab")
        .context("reading /usr/share/zoneinfo/zone.tab (is tzdata installed?)")?;
    let xkb = PathBuf::from("/usr/share/X11/xkb/symbols");
    let keyboard_layouts = std::fs::read_dir(&xkb).ok().map(|entries| {
        entries.flatten()
            .filter_map(|entry| entry.file_type().ok().filter(|kind| kind.is_file()).map(|_| entry.file_name().to_string_lossy().into_owned()))
            .collect()
    });
    let cpus = std::fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|s| s.lines().find_map(|line| line.strip_prefix("Cpus_allowed_list:").map(|v| taboom_core::hardware::count_cpu_list(v.trim()))))
        .or_else(|| std::thread::available_parallelism().ok().map(|n| n.get() as u32))
        .unwrap_or(1);
    Ok(System {
        zone_tab,
        keyboard_layouts,
        cpus,
        ram_mb: taboom_core::hardware::physical_memory_mb(),
        architecture: std::env::consts::ARCH.into(),
        host_architecture: taboom_core::hardware::host_architecture(),
        fortress_available: fortress_is_installed(),
    })
}

fn recordings_command(command: RecordingCommand) -> Result<()> {
    let (tool, args) = match command {
        RecordingCommand::Ls => ("recording_list", json!({"limit": 100})),
        RecordingCommand::Share { id } => ("recording_share", json!({"id": id})),
    };
    print_json(&mcp_client()?.call_tool(tool, &args)?)
}

fn print_logs(lines: usize) -> Result<()> {
    let path = taboom_home()?.join("logs").join("chrome.log");
    let file = match std::fs::File::open(&path) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            bail!("Chrome log not found at {}", path.display());
        }
        Err(error) => return Err(error).with_context(|| format!("reading {}", path.display())),
    };
    let tail = last_log_lines(BufReader::new(file), lines)
        .with_context(|| format!("reading {}", path.display()))?;
    for line in tail {
        println!("{line}");
    }
    Ok(())
}

fn last_log_lines<R: BufRead>(reader: R, count: usize) -> io::Result<Vec<String>> {
    let mut tail = VecDeque::new();
    for line in reader.lines() {
        let line = line?;
        if count == 0 {
            continue;
        }
        if tail.len() == count {
            tail.pop_front();
        }
        tail.push_back(line);
    }
    Ok(tail.into_iter().collect())
}

fn parse_arguments(source: &str) -> Result<Value> {
    let value: Value = serde_json::from_str(source).context("arguments must be valid JSON")?;
    if !value.is_object() {
        bail!("tool arguments must be a JSON object");
    }
    Ok(value)
}

fn taboom_home() -> Result<PathBuf> {
    if let Some(home) = std::env::var_os("TABOOM_HOME") {
        return Ok(PathBuf::from(home));
    }
    std::env::var_os("HOME")
        .map(|home| PathBuf::from(home).join(".taboom"))
        .context("TABOOM_HOME and HOME are both unset")
}

fn active_persona_path() -> Result<PathBuf> {
    let name = std::env::var("TABOOM_PERSONA").unwrap_or_else(|_| "default".into());
    check_name(&name)?;
    Ok(taboom_home()?.join("personas").join(format!("{name}.toml")))
}

fn admin(command: AdminCommand) -> Result<Value> {
    let request = AdminRequest { id: 1, command };
    let mut encoded = serde_json::to_vec(&request)?;
    if encoded.len() > ADMIN_MAX_LINE_BYTES {
        bail!("admin request exceeds {} bytes", ADMIN_MAX_LINE_BYTES);
    }
    encoded.push(b'\n');
    let socket_path = taboom_home()?.join("run").join("taboomd.sock");
    let mut stream = UnixStream::connect(&socket_path)
        .with_context(|| format!("connecting to {}; is taboomd running?", socket_path.display()))?;
    stream.set_read_timeout(Some(Duration::from_secs(30)))?;
    stream.set_write_timeout(Some(Duration::from_secs(30)))?;
    stream.write_all(&encoded)?;
    let mut response_line = String::new();
    BufReader::new(stream).read_line(&mut response_line)?;
    if response_line.len() > ADMIN_MAX_LINE_BYTES {
        bail!("admin response exceeds {} bytes", ADMIN_MAX_LINE_BYTES);
    }
    let response: AdminResponse = serde_json::from_str(&response_line).context("admin returned malformed JSON")?;
    if response.id != request.id {
        bail!("admin response id did not match the request");
    }
    if !response.ok {
        let error = response.error.context("admin returned an unspecified error")?;
        bail!("{}: {}", error.code, error.message);
    }
    Ok(response.result.unwrap_or(Value::Null))
}

fn read_secret(prompt: &str) -> Result<String> {
    use std::io::IsTerminal;
    let stdin = io::stdin();
    if !stdin.is_terminal() {
        bail!("secret entry requires an interactive terminal");
    }
    let fd = stdin.as_raw_fd();
    let original = read_terminal(fd).context("reading terminal settings")?;
    let mut hidden = original;
    hidden.c_lflag &= !(libc::ECHO as libc::tcflag_t);
    print!("{prompt}");
    io::stdout().flush()?;
    let mut echo = EchoGuard::new(fd, original);
    // Install before disabling echo, so every point at which input could be hidden has a
    // handler that restores it. The handler exits directly rather than unwinding through libc.
    echo.install_signal_handlers()?;
    echo.active = true;
    write_terminal(fd, &hidden).context("disabling terminal echo")?;
    let mut value = String::new();
    let read_result = stdin.lock().read_line(&mut value);
    let restore_result = echo.restore();
    println!();
    restore_result.context("restoring terminal echo")?;
    read_result.context("reading secret")?;
    while value.ends_with(['\n', '\r']) {
        value.pop();
    }
    Ok(value)
}

fn read_terminal(fd: libc::c_int) -> nix::Result<libc::termios> {
    let mut original = MaybeUninit::<libc::termios>::uninit();
    if unsafe { libc::tcgetattr(fd, original.as_mut_ptr()) } == -1 {
        return Err(Errno::last());
    }
    Ok(unsafe { original.assume_init() })
}

fn write_terminal(fd: libc::c_int, settings: &libc::termios) -> nix::Result<()> {
    if unsafe { libc::tcsetattr(fd, libc::TCSANOW, settings) } == -1 {
        return Err(Errno::last());
    }
    Ok(())
}

/// A stable saved copy lets the SIGINT handler restore terminal settings without borrowing a
/// stack value that could disappear during concurrent signal delivery and guard cleanup.
struct SignalTermios(UnsafeCell<MaybeUninit<libc::termios>>);

// Only the CLI's single active secret prompt writes this slot. The signal handler reads it only
// after publishing an active fd, and the process exits immediately after handling SIGINT.
unsafe impl Sync for SignalTermios {}

static SIGNAL_ORIGINAL_TERMIOS: SignalTermios =
    SignalTermios(UnsafeCell::new(MaybeUninit::uninit()));
static SIGNAL_TERMINAL_FD: AtomicI32 = AtomicI32::new(-1);

/// Signals that end the prompt; each restores echo and exits with 128 + signo.
const RESTORE_SIGNALS: [Signal; 4] = [Signal::SIGINT, Signal::SIGTERM, Signal::SIGHUP, Signal::SIGQUIT];

extern "C" fn restore_echo_on_signal(signo: libc::c_int) {
    let fd = SIGNAL_TERMINAL_FD.load(Ordering::SeqCst);
    if fd >= 0 {
        let original = unsafe { &*(*SIGNAL_ORIGINAL_TERMIOS.0.get()).as_ptr() };
        unsafe {
            libc::tcsetattr(fd, libc::TCSANOW, original);
        }
    }
    unsafe { libc::_exit(128 + signo) }
}

/// Restores terminal echo during ordinary errors, Rust unwinding, or a terminating signal while
/// a hidden prompt is active. Ctrl+Z is ignored for the prompt's lifetime, since a stopped job
/// would leave the shell with echo off.
struct EchoGuard {
    fd: libc::c_int,
    original: libc::termios,
    active: bool,
    previous_actions: Vec<(Signal, SigAction)>,
}

impl EchoGuard {
    fn new(fd: libc::c_int, original: libc::termios) -> Self {
        Self {
            fd,
            original,
            active: false,
            previous_actions: Vec::new(),
        }
    }

    fn install_signal_handlers(&mut self) -> Result<()> {
        unsafe {
            (*SIGNAL_ORIGINAL_TERMIOS.0.get()).write(self.original);
        }
        SIGNAL_TERMINAL_FD.store(self.fd, Ordering::SeqCst);
        let restore = SigAction::new(SigHandler::Handler(restore_echo_on_signal), SaFlags::empty(), SigSet::empty());
        let ignore = SigAction::new(SigHandler::SigIgn, SaFlags::empty(), SigSet::empty());
        let actions = RESTORE_SIGNALS.iter().map(|s| (*s, &restore)).chain([(Signal::SIGTSTP, &ignore)]);
        for (signal, action) in actions {
            match unsafe { sigaction(signal, action) } {
                Ok(previous) => self.previous_actions.push((signal, previous)),
                Err(error) => return Err(error).context("installing terminal restore handler"),
            }
        }
        Ok(())
    }

    fn restore(&mut self) -> nix::Result<()> {
        let result = write_terminal(self.fd, &self.original);
        if result.is_ok() {
            self.active = false;
        }
        result
    }
}

impl Drop for EchoGuard {
    fn drop(&mut self) {
        if self.active {
            let _ = write_terminal(self.fd, &self.original);
        }
        // Restore the caller's signal policy only after restoring the terminal. Keep the saved
        // state published until every sigaction succeeds, so a concurrent signal remains safe
        // while this guard is being dropped.
        let restored = self
            .previous_actions
            .drain(..)
            .rev()
            .fold(true, |ok, (signal, previous)| unsafe { sigaction(signal, &previous) }.is_ok() && ok);
        if restored {
            SIGNAL_TERMINAL_FD.store(-1, Ordering::SeqCst);
        }
    }
}

struct McpClient {
    addr: String,
    bearer: Option<String>,
}

fn mcp_client() -> Result<McpClient> {
    let port = std::env::var("TABOOM_MCP_PORT").ok().and_then(|p| p.parse::<u16>().ok()).unwrap_or(3456);
    let bearer = cli_bearer()?;
    Ok(McpClient { addr: format!("127.0.0.1:{port}"), bearer })
}

fn cli_bearer() -> Result<Option<String>> {
    let tokens = std::env::var("TABOOM_MCP_TOKENS").unwrap_or_default();
    let tokens: Vec<(String, String)> = tokens
        .split(',')
        .filter_map(|pair| pair.split_once('='))
        .map(|(name, token)| (name.trim().to_owned(), token.trim().to_owned()))
        .filter(|(name, token)| !name.is_empty() && !token.is_empty())
        .collect();
    if tokens.is_empty() {
        if std::env::var_os("TABOOM_CLI_CLIENT").is_some() {
            bail!("TABOOM_CLI_CLIENT was set but TABOOM_MCP_TOKENS contains no valid clients");
        }
        return Ok(None);
    }
    let selected = std::env::var("TABOOM_CLI_CLIENT").ok();
    let token = match selected {
        Some(client) => tokens.iter().find(|(name, _)| name == &client).map(|(_, token)| token),
        None if tokens.len() == 1 => Some(&tokens[0].1),
        None => bail!("set TABOOM_CLI_CLIENT to a configured MCP token name so CLI calls keep one stable session identity"),
    }
    .context("TABOOM_CLI_CLIENT does not match any configured MCP token name")?;
    Ok(Some(token.clone()))
}

impl McpClient {
    fn call_tool(&self, tool: &str, arguments: &Value) -> Result<Value> {
        Ok(format_mcp_tool_result(self.call_tool_raw(tool, arguments)?))
    }

    /// Return MCP's original result blocks. The gauntlet uses this only to persist screenshot
    /// image bytes; ordinary calls continue going through `call_tool` and omit images in output.
    fn call_tool_raw(&self, tool: &str, arguments: &Value) -> Result<Value> {
        if !arguments.is_object() {
            bail!("tool arguments must be a JSON object");
        }
        let body = serde_json::to_vec(&json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "tools/call",
            "params": { "name": tool, "arguments": arguments },
        }))?;
        if body.len() > 4 * 1024 * 1024 {
            bail!("MCP request exceeds 4 MiB");
        }
        let mut stream = std::net::TcpStream::connect(&self.addr)
            .with_context(|| format!("connecting to MCP at {}; is taboomd running?", self.addr))?;
        stream.set_read_timeout(Some(Duration::from_secs(120)))?;
        stream.set_write_timeout(Some(Duration::from_secs(30)))?;
        let mut headers = format!(
            "POST /mcp HTTP/1.1\r\nHost: {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n",
            self.addr,
            body.len()
        );
        if let Some(bearer) = &self.bearer {
            headers.push_str(&format!("Authorization: Bearer {bearer}\r\n"));
        }
        headers.push_str("\r\n");
        stream.write_all(headers.as_bytes())?;
        stream.write_all(&body)?;
        let mut response = Vec::new();
        stream.take(16 * 1024 * 1024).read_to_end(&mut response)?;
        parse_mcp_rpc_result(&response)
    }
}

fn parse_mcp_rpc_result(response: &[u8]) -> Result<Value> {
    let split = response.windows(4).position(|window| window == b"\r\n\r\n")
        .context("MCP returned an invalid HTTP response")?;
    let headers = std::str::from_utf8(&response[..split]).context("MCP response headers are not UTF-8")?;
    let status = headers.lines().next().and_then(|line| line.split_whitespace().nth(1)).unwrap_or("0");
    if !status.starts_with('2') {
        bail!("MCP HTTP request failed with status {status}");
    }
    let body = &response[split + 4..];
    let chunked = headers.lines().any(|line| {
        line.split_once(':').is_some_and(|(name, value)| {
            name.eq_ignore_ascii_case("transfer-encoding")
                && value.split(',').any(|encoding| encoding.trim().eq_ignore_ascii_case("chunked"))
        })
    });
    let decoded;
    let body = if chunked {
        decoded = decode_chunked(body)?;
        decoded.as_slice()
    } else {
        body
    };
    let rpc: Value = serde_json::from_slice(body).context("MCP returned malformed JSON")?;
    if let Some(error) = rpc.get("error") {
        bail!("MCP error: {}", error.get("message").and_then(Value::as_str).unwrap_or("request failed"));
    }
    let result = rpc.get("result").cloned().context("MCP response has no result")?;
    Ok(result)
}

fn format_mcp_tool_result(result: Value) -> Value {
    let is_error = result.get("isError").and_then(Value::as_bool).unwrap_or(false);
    let blocks = result.get("content").and_then(Value::as_array).cloned().unwrap_or_default();
    let mut output = Vec::new();
    for block in blocks {
        match block.get("type").and_then(Value::as_str) {
            Some("text") => {
                let text = block.get("text").and_then(Value::as_str).unwrap_or_default();
                output.push(serde_json::from_str(text).unwrap_or_else(|_| Value::String(text.into())));
            }
            Some("image") => output.push(json!({
                "image": block.get("mimeType").and_then(Value::as_str).unwrap_or("image/*"),
                "omitted_from_terminal": true,
            })),
            _ => output.push(block),
        }
    }
    if output.len() == 1 {
        json!({ "isError": is_error, "result": output.remove(0) })
    } else {
        json!({ "isError": is_error, "result": output })
    }
}

fn decode_chunked(mut body: &[u8]) -> Result<Vec<u8>> {
    let mut decoded = Vec::new();
    loop {
        let end = body.windows(2).position(|window| window == b"\r\n")
            .context("invalid chunked MCP response")?;
        let size_text = std::str::from_utf8(&body[..end]).context("invalid chunk size")?;
        let size_text = size_text.split(';').next().unwrap_or_default();
        let size = usize::from_str_radix(size_text.trim(), 16).context("invalid chunk size")?;
        body = &body[end + 2..];
        if size == 0 {
            return Ok(decoded);
        }
        if body.len() < size + 2 || &body[size..size + 2] != b"\r\n" {
            bail!("truncated chunked MCP response");
        }
        decoded.extend_from_slice(&body[..size]);
        body = &body[size + 2..];
        if decoded.len() > 16 * 1024 * 1024 {
            bail!("MCP response exceeds 16 MiB");
        }
    }
}

fn print_json(value: &Value) -> Result<()> {
    println!("{}", serde_json::to_string_pretty(value)?);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;
    use std::os::fd::FromRawFd;
    use std::process::{Command as ProcessCommand, Stdio};
    use std::thread;

    #[test]
    fn call_arguments_must_be_an_object() {
        assert_eq!(parse_arguments(r#"{"text":"hello world"}"#).unwrap()["text"], "hello world");
        assert!(parse_arguments("[]").is_err());
        assert!(parse_arguments("not json").is_err());
    }

    #[test]
    fn mcp_call_keeps_json_and_uses_stable_bearer() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            stream.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut headers = String::new();
            loop {
                let mut line = String::new();
                reader.read_line(&mut line).unwrap();
                if line == "\r\n" || line.is_empty() {
                    break;
                }
                headers.push_str(&line);
            }
            assert!(headers.contains("Authorization: Bearer fixed-token\r\n"));
            let length = headers.lines().find_map(|line| {
                line.to_ascii_lowercase().strip_prefix("content-length: ").and_then(|s| s.trim().parse::<usize>().ok())
            }).unwrap();
            let mut body = vec![0; length];
            reader.read_exact(&mut body).unwrap();
            let rpc: Value = serde_json::from_slice(&body).unwrap();
            assert_eq!(rpc["method"], "tools/call");
            assert_eq!(rpc["params"]["arguments"]["text"], "hello from cli");
            let response = json!({
                "jsonrpc":"2.0", "id":1,
                "result":{"isError":false,"content":[{"type":"text","text":"{\"done\":true}"}]}
            });
            let payload = serde_json::to_vec(&response).unwrap();
            write!(stream, "HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n").unwrap();
            write!(stream, "{:X}\r\n", payload.len()).unwrap();
            stream.write_all(&payload).unwrap();
            write!(stream, "\r\n0\r\n\r\n").unwrap();
        });
        let client = McpClient { addr: address.to_string(), bearer: Some("fixed-token".into()) };
        let result = client.call_tool("type", &json!({"text":"hello from cli"})).unwrap();
        assert_eq!(result["isError"], false);
        assert_eq!(result["result"]["done"], true);
        server.join().unwrap();
    }

    #[test]
    fn gauntlet_reads_raw_png_blocks_and_refuses_suppressed_visuals() {
        use base64::Engine as _;

        let png = b"\x89PNG\r\n\x1a\nfixture";
        let raw = json!({
            "isError": false,
            "content": [{
                "type": "image",
                "mimeType": "image/png",
                "data": base64::engine::general_purpose::STANDARD.encode(png),
            }],
        });
        assert_eq!(screenshot_png_bytes(&raw).unwrap(), png);

        let suppressed = json!({
            "isError": true,
            "content": [{"type": "text", "text": "screen images and OCR are disabled"}],
        });
        let error = screenshot_png_bytes(&suppressed).unwrap_err();
        assert!(error.to_string().contains("screen images and OCR are disabled"));
    }

    #[test]
    fn gauntlet_append_preserves_legacy_score_rows() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("scoreboard.json");
        std::fs::write(&path, r#"[{"test_name":"mouse","human_score":0.8,"taboom_score":0.7,"date":"2026-09-26"}]"#).unwrap();

        append_gauntlet_run(&path, json!({"persona":"test","engine":"chrome","commit":"abc1234"})).unwrap();
        append_gauntlet_run(&path, json!({"persona":"test2","engine":"chrome","commit":"unknown"})).unwrap();
        let board: Value = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
        assert_eq!(board["results"][0]["test_name"], "mouse");
        assert_eq!(board["gauntlet_runs"].as_array().unwrap().len(), 2);
    }

    #[test]
    fn cli_vault_add_accepts_comma_separated_domains() {
        let args = Cli::try_parse_from([
            "taboom", "vault", "add", "shop", "--type", "password", "--domains", "shop.example,login.example",
        ]).unwrap();
        assert!(matches!(args.command, Command::Vault { command: VaultCommand::Add { domains, .. } } if domains == ["shop.example", "login.example"]));
    }

    #[test]
    fn logs_keep_only_the_requested_last_lines() {
        let tail = last_log_lines(io::Cursor::new("first\nsecond\nthird\n"), 2).unwrap();
        assert_eq!(tail, ["second", "third"]);
        assert!(last_log_lines(io::Cursor::new("first\nsecond\n"), 0).unwrap().is_empty());
    }

    #[test]
    fn persona_writer_is_private_and_never_replaces_an_existing_file() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("shop.toml");
        write_persona_safely(dir.path(), &path, b"name = 'shop'\n").unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "name = 'shop'\n");
        assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
        assert!(write_persona_safely(dir.path(), &path, b"overwritten").is_err());
        assert_eq!(std::fs::read_to_string(path).unwrap(), "name = 'shop'\n");
    }

    #[test]
    fn sigint_restores_terminal_settings_before_exiting() {
        const CHILD_ENV: &str = "TABOOM_TEST_SIGINT_PROMPT_CHILD";
        if std::env::var_os(CHILD_ENV).is_some() {
            let _ = read_secret("secret: ");
            panic!("SIGINT should exit the child after restoring terminal settings");
        }

        let pty = nix::pty::openpty(None, None).expect("open PTY");
        let slave_fd = pty.slave.as_raw_fd();
        let child_stdin = unsafe { libc::dup(slave_fd) };
        assert!(child_stdin >= 0, "duplicate PTY slave for child stdin");
        let child_stdin = unsafe { std::fs::File::from_raw_fd(child_stdin) };
        let test_binary = std::env::current_exe().expect("current test binary");
        let mut child = ProcessCommand::new(test_binary)
            .args(["--exact", "tests::sigint_restores_terminal_settings_before_exiting", "--nocapture"])
            .env(CHILD_ENV, "1")
            .stdin(Stdio::from(child_stdin))
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn isolated prompt test");

        let mut echo_disabled = false;
        for _ in 0..200 {
            if read_terminal(slave_fd).expect("read PTY settings").c_lflag
                & (libc::ECHO as libc::tcflag_t)
                == 0
            {
                echo_disabled = true;
                break;
            }
            if child.try_wait().expect("check child status").is_some() {
                break;
            }
            thread::sleep(Duration::from_millis(10));
        }
        if !echo_disabled {
            let _ = child.kill();
            let _ = child.wait();
            panic!("child did not disable terminal echo before the timeout");
        }

        nix::sys::signal::kill(nix::unistd::Pid::from_raw(child.id() as i32), Signal::SIGINT)
            .expect("send SIGINT to hidden prompt");
        let status = child.wait().expect("wait for hidden prompt child");
        assert_eq!(status.code(), Some(130));
        let restored = read_terminal(slave_fd).expect("read restored PTY settings");
        assert_ne!(restored.c_lflag & (libc::ECHO as libc::tcflag_t), 0);
    }
}
