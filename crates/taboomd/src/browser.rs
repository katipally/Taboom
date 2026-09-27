use anyhow::{Context, Result, bail};
use taboom_core::persona::Engine;
use std::fs::{File, OpenOptions};
use std::io::{self, BufRead, Write};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::time::Duration;

/// Proxy routes only: WebRTC UDP may leave only through the proxy, never around it.
pub const WEBRTC_PROXY_ONLY_FLAG: &str = "--force-webrtc-ip-handling-policy=disable_non_proxied_udp";

static SHUTDOWN_REQUESTED: AtomicBool = AtomicBool::new(false);

/// Launch the selected browser under its pipe-owning parent so its private DevTools transport is
/// inherited at startup. This mode exposes no debug port and forwards only a fixed
/// Target.getTargets query.
pub fn launch(home: &Path, profile: &Path, engine: Engine, flags: &[String]) -> Result<()> {
    validate_flags(engine, flags)?;
    SHUTDOWN_REQUESTED.store(false, Ordering::SeqCst);
    install_shutdown_handlers();
    let run_dir = home.join("run");
    let log_dir = home.join("logs");
    std::fs::create_dir_all(&run_dir)?;
    std::fs::create_dir_all(&log_dir)?;
    std::fs::set_permissions(&run_dir, std::fs::Permissions::from_mode(0o700))?;
    let socket_path = run_dir.join("browser-cdp.sock");
    if UnixStream::connect(&socket_path).is_ok() {
        bail!("another Chrome pipe bridge is already running");
    }
    let _ = std::fs::remove_file(&socket_path);
    let listener = UnixListener::bind(&socket_path)
        .with_context(|| format!("binding private Chrome pipe socket {}", socket_path.display()))?;
    let socket = SocketCleanup::new(socket_path.clone())?.holding(listener);
    let listener = socket.listener.as_ref().expect("held until drop");
    std::fs::set_permissions(&socket_path, std::fs::Permissions::from_mode(0o600))?;
    listener.set_nonblocking(true)?;

    // Dedicated anonymous pipes carry CDP only. Chrome's regular stdio goes to /dev/null/logs.
    let (chrome_input_original, pipe_input) = pipe_pair()?;
    let (pipe_output, chrome_output_original) = pipe_pair()?;
    let chrome_input = duplicate_high(&chrome_input_original)?;
    let chrome_output = duplicate_high(&chrome_output_original)?;
    drop(chrome_input_original);
    drop(chrome_output_original);

    let log = OpenOptions::new()
        .create(true)
        .append(true)
        .open(log_dir.join("chrome.log"))?;
    let mut command = Command::new(executable_for_engine(engine));
    command
        .arg("--ozone-platform=wayland")
        .arg(format!("--user-data-dir={}", profile.display()))
        .arg("--remote-debugging-pipe")
        .args(flags)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::from(log));

    // Chromium reads remote-debugging messages from fd 3 and writes responses to fd 4. Dedicated
    // anonymous pipes keep ordinary process stdin/stdout separate from the CDP byte stream.
    unsafe {
        let input_fd = chrome_input.as_raw_fd();
        let output_fd = chrome_output.as_raw_fd();
        let parent_pid = libc::getpid();
        command.pre_exec(move || install_browser_child(input_fd, output_fd, parent_pid));
    }
    let mut child = command.spawn().with_context(|| {
        format!("starting {} with a private remote-debugging pipe", engine_name(engine))
    })?;
    drop(chrome_input);
    drop(chrome_output);
    let mut pipe_input = File::from(pipe_input);
    let (tx, receiver) = mpsc::channel();
    std::thread::spawn(move || crate::cdp::read_pipe_messages(File::from(pipe_output), tx));

    let result = (|| {
        let mut next_id = 1u64;
        loop {
            if SHUTDOWN_REQUESTED.load(Ordering::SeqCst) {
                break;
            }
            if let Some(status) = child.try_wait()? {
                if !status.success() {
                    bail!("Chrome exited with {status}");
                }
                break;
            }
            match listener.accept() {
                Ok((stream, _)) => {
                    if let Err(error) = crate::cdp::serve_pipe_queries(
                        &mut pipe_input,
                        &receiver,
                        stream,
                        &mut next_id,
                    ) {
                        tracing::debug!("Chrome target query refused: {error}");
                    }
                }
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                    std::thread::sleep(Duration::from_millis(40));
                }
                Err(error) => return Err(error.into()),
            }
        }
        Ok(())
    })();
    terminate_process_group(child.id(), &mut child);
    result
}

fn validate_flags(engine: Engine, flags: &[String]) -> Result<()> {
    for flag in flags {
        if flag == "--user-agent" || flag.starts_with("--user-agent=") {
            bail!("--user-agent is forbidden; it desynchronizes Fortress UA and Client-Hints");
        }
        let allowed = match engine {
            Engine::Chrome => {
                value_flag(flag, "--lang=")
                    || flag == "--proxy-server=socks5://127.0.0.1:1080"
                    || flag == "--host-resolver-rules=MAP * ~NOTFOUND , EXCLUDE 127.0.0.1"
            }
            Engine::Fortress => {
                value_flag(flag, "--lang=")
                    || value_flag(flag, "--accept-lang=")
                    || value_flag(flag, "--uxr-country=")
                    || value_flag(flag, "--uxr-timezone=")
                    || value_flag(flag, "--uxr-languages=")
                    || numeric_flag(flag, "--uxr-screen-width=")
                    || numeric_flag(flag, "--uxr-screen-height=")
                    || numeric_flag(flag, "--uxr-canvas-seed=")
                    || numeric_flag(flag, "--uxr-audio-seed=")
                    || numeric_flag(flag, "--uxr-hw-concurrency=")
                    || numeric_flag(flag, "--uxr-device-memory=")
                    || flag == WEBRTC_PROXY_ONLY_FLAG
                    || flag == "--proxy-server=socks5://127.0.0.1:1080"
                    || flag == "--host-resolver-rules=MAP * ~NOTFOUND , EXCLUDE 127.0.0.1"
            }
        };
        if !allowed {
            bail!("unsupported {} flag; persona startup permits only mapped settings and route flags", engine_name(engine));
        }
    }
    Ok(())
}

fn value_flag(flag: &str, prefix: &str) -> bool {
    flag.strip_prefix(prefix)
        .is_some_and(|value| !value.is_empty() && !value.bytes().any(|b| b == 0 || b.is_ascii_whitespace()))
}

fn numeric_flag(flag: &str, prefix: &str) -> bool {
    flag.strip_prefix(prefix)
        .is_some_and(|value| !value.is_empty() && value.bytes().all(|b| b.is_ascii_digit()))
}

fn executable_for_engine(engine: Engine) -> &'static str {
    match engine {
        Engine::Chrome => "google-chrome-stable",
        Engine::Fortress => "/opt/fortress/tilion",
    }
}

pub fn parse_engine(value: &str) -> Result<Engine> {
    match value {
        "chrome" => Ok(Engine::Chrome),
        "fortress" => Ok(Engine::Fortress),
        other => bail!("invalid TABOOM_BROWSER_ENGINE {other:?}; expected chrome or fortress"),
    }
}

fn engine_name(engine: Engine) -> &'static str {
    match engine {
        Engine::Chrome => "Chrome",
        Engine::Fortress => "Fortress",
    }
}

pub fn check(home: &Path) -> Result<()> {
    let mut stream = UnixStream::connect(home.join("run").join("browser-cdp.sock"))
        .context("the browser's private remote-debugging pipe is not running")?;
    stream.set_read_timeout(Some(Duration::from_secs(7)))?;
    stream.set_write_timeout(Some(Duration::from_secs(2)))?;
    stream.write_all(b"health\n")?;
    let mut reply = String::new();
    std::io::BufReader::new(stream).read_line(&mut reply)?;
    if reply.trim() != r#"{"ready":true}"# {
        bail!("the browser's private remote-debugging pipe did not pass its health check");
    }
    Ok(())
}

fn install_shutdown_handlers() {
    extern "C" fn request_shutdown(_: libc::c_int) {
        SHUTDOWN_REQUESTED.store(true, Ordering::SeqCst);
    }
    // SAFETY: the handler only performs an atomic store and uses the platform C signal ABI.
    unsafe {
        libc::signal(libc::SIGTERM, request_shutdown as *const () as libc::sighandler_t);
        libc::signal(libc::SIGINT, request_shutdown as *const () as libc::sighandler_t);
    }
}

fn terminate_process_group(pid: u32, child: &mut std::process::Child) {
    let group = -(pid as i32);
    // SAFETY: kill targets the dedicated process group created for this Chrome instance.
    unsafe { libc::kill(group, libc::SIGTERM); }
    let deadline = std::time::Instant::now() + Duration::from_secs(3);
    while std::time::Instant::now() < deadline && process_group_exists(group) {
        let _ = child.try_wait();
        std::thread::sleep(Duration::from_millis(50));
    }
    if process_group_exists(group) {
        // SAFETY: kill the same dedicated process group if any Chrome child ignored SIGTERM.
        unsafe { libc::kill(group, libc::SIGKILL); }
    }
    let _ = child.wait();
}

fn process_group_exists(group: i32) -> bool {
    // SAFETY: signal 0 checks a process group without changing its state.
    if unsafe { libc::kill(group, 0) } == 0 {
        return true;
    }
    io::Error::last_os_error().raw_os_error() != Some(libc::ESRCH)
}

struct SocketCleanup {
    path: std::path::PathBuf,
    device: u64,
    inode: u64,
    /// Our own listener, closed first on drop so a connect probe only reaches a successor.
    listener: Option<UnixListener>,
}

impl SocketCleanup {
    fn new(path: std::path::PathBuf) -> Result<Self> {
        use std::os::unix::fs::MetadataExt;
        let metadata = std::fs::symlink_metadata(&path)?;
        Ok(Self { path, device: metadata.dev(), inode: metadata.ino(), listener: None })
    }

    fn holding(mut self, listener: UnixListener) -> Self {
        self.listener = Some(listener);
        self
    }
}

impl Drop for SocketCleanup {
    fn drop(&mut self) {
        use std::os::unix::fs::MetadataExt;
        // A stale wrapper can replace the socket while this parent is exiting; never remove a
        // successor's live endpoint. Linux reuses a freed inode number at once, so device and
        // inode alone can match the successor: anything still answering is not ours.
        drop(self.listener.take());
        // A child forked elsewhere can hold our listener for the microseconds before its exec;
        // only a socket still answering after a few retries is a successor's.
        let successor_live = (0..5).all(|_| {
            let answered = UnixStream::connect(&self.path).is_ok();
            if answered {
                std::thread::sleep(Duration::from_millis(10));
            }
            answered
        });
        if successor_live {
            return;
        }
        if std::fs::symlink_metadata(&self.path)
            .is_ok_and(|metadata| metadata.dev() == self.device && metadata.ino() == self.inode)
        {
            let _ = std::fs::remove_file(&self.path);
        }
    }
}

fn pipe_pair() -> Result<(OwnedFd, OwnedFd)> {
    let mut fds = [-1; 2];
    // SAFETY: fds points to two writable raw-fd slots.
    if unsafe { libc::pipe(fds.as_mut_ptr()) } < 0 {
        return Err(io::Error::last_os_error().into());
    }
    for fd in fds {
        // SAFETY: pipe returned valid open descriptors.
        if unsafe { libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC) } < 0 {
            let error = io::Error::last_os_error();
            // SAFETY: both descriptors are owned by this function on failure.
            unsafe { libc::close(fds[0]); libc::close(fds[1]); }
            return Err(error.into());
        }
    }
    // SAFETY: pipe2 returned two newly owned descriptors.
    Ok(unsafe { (OwnedFd::from_raw_fd(fds[0]), OwnedFd::from_raw_fd(fds[1])) })
}

fn duplicate_high(fd: &OwnedFd) -> Result<OwnedFd> {
    // Keep source descriptors away from Chrome's fixed fds 3 and 4, even when the parent had
    // closed an ordinary stdio descriptor before launching.
    // SAFETY: F_DUPFD_CLOEXEC duplicates the open descriptor into a new owned slot.
    let duplicate = unsafe { libc::fcntl(fd.as_raw_fd(), libc::F_DUPFD_CLOEXEC, 10) };
    if duplicate < 0 {
        return Err(io::Error::last_os_error().into());
    }
    // SAFETY: fcntl returned a fresh descriptor owned by this process.
    Ok(unsafe { OwnedFd::from_raw_fd(duplicate) })
}

fn install_pipe_fds(input_fd: i32, output_fd: i32) -> io::Result<()> {
    // SAFETY: descriptors originate from live pipe endpoints owned by the parent process.
    if unsafe { libc::dup2(input_fd, 3) } < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: descriptors originate from live pipe endpoints owned by the parent process.
    if unsafe { libc::dup2(output_fd, 4) } < 0 {
        return Err(io::Error::last_os_error());
    }
    // dup2 clears CLOEXEC when source and destination differ; explicitly cover equal-fd cases.
    // SAFETY: 3 and 4 now refer to the intended pipe endpoints.
    if unsafe { libc::fcntl(3, libc::F_SETFD, 0) } < 0
        || unsafe { libc::fcntl(4, libc::F_SETFD, 0) } < 0
    {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

fn install_browser_child(input_fd: i32, output_fd: i32, parent_pid: libc::pid_t) -> io::Result<()> {
    #[cfg(not(target_os = "linux"))]
    let _ = parent_pid;
    // SAFETY: the process group belongs only to this Chrome instance.
    if unsafe { libc::setpgid(0, 0) } < 0 {
        return Err(io::Error::last_os_error());
    }
    #[cfg(target_os = "linux")]
    {
        // The pipe-owning parent can be killed without running Rust destructors. Linux will
        // still signal Chrome on parent death; the normal SIGTERM path also cleans its group.
        // SAFETY: PR_SET_PDEATHSIG configures this child process only.
        if unsafe { libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGTERM, 0, 0, 0) } < 0 {
            return Err(io::Error::last_os_error());
        }
        // Catch the race where the parent died before PR_SET_PDEATHSIG was installed.
        // SAFETY: getppid reads this child's current parent PID.
        if unsafe { libc::getppid() } != parent_pid {
            return Err(io::Error::new(io::ErrorKind::BrokenPipe, "browser bridge exited during Chrome launch"));
        }
    }
    install_pipe_fds(input_fd, output_fd)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader};

    #[test]
    fn dedicated_fds_carry_protocol_without_using_child_stdout() {
        let (chrome_input_original, parent_input) = pipe_pair().unwrap();
        let (parent_output, chrome_output_original) = pipe_pair().unwrap();
        let chrome_input = duplicate_high(&chrome_input_original).unwrap();
        let chrome_output = duplicate_high(&chrome_output_original).unwrap();
        drop(chrome_input_original);
        drop(chrome_output_original);

        let input_fd = chrome_input.as_raw_fd();
        let output_fd = chrome_output.as_raw_fd();
        let mut child = Command::new("/bin/sh");
        child
            .arg("-c")
            .arg("printf 'protocol-response\\n' >&4; IFS= read -r request <&3; [ \"$request\" = protocol-request ]")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        // SAFETY: the child descriptors remain owned until spawn completes.
        unsafe { child.pre_exec(move || install_pipe_fds(input_fd, output_fd)); }
        let mut child = child.spawn().unwrap();
        drop(chrome_input);
        drop(chrome_output);

        let mut parent_output = File::from(parent_output);
        let mut parent_input = File::from(parent_input);
        let mut response = String::new();
        BufReader::new(&mut parent_output).read_line(&mut response).unwrap();
        assert_eq!(response, "protocol-response\n");
        parent_input.write_all(b"protocol-request\n").unwrap();
        parent_input.flush().unwrap();
        assert!(child.wait().unwrap().success());
    }

    #[test]
    fn socket_cleanup_removes_path_on_launch_failure() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("cdp.sock");
        std::fs::write(&path, b"stale").unwrap();
        drop(SocketCleanup::new(path.clone()).unwrap());
        assert!(!path.exists());
    }

    #[test]
    fn chrome_flags_cannot_override_profile_or_remote_debugging_transport() {
        assert!(validate_flags(Engine::Chrome, &["--lang=en-US".into()]).is_ok());
        assert!(validate_flags(Engine::Chrome, &["--remote-debugging-port=9222".into()]).is_err());
        assert!(validate_flags(Engine::Chrome, &["--user-data-dir=/tmp/other".into()]).is_err());
        assert!(validate_flags(Engine::Chrome, &["--remote-debugging-pipe".into()]).is_err());
    }

    #[test]
    fn fortress_flags_are_limited_to_mapped_uxr_surfaces_and_never_user_agent() {
        let mapped = [
            "--uxr-country=DE",
            "--uxr-timezone=Europe/Berlin",
            "--uxr-languages=de-DE,de,en",
            "--uxr-screen-width=2560",
            "--uxr-screen-height=1440",
            "--uxr-hw-concurrency=8",
            "--uxr-canvas-seed=81723",
            "--uxr-audio-seed=81723",
            "--uxr-device-memory=4",
            "--accept-lang=de-DE,de,en",
            "--proxy-server=socks5://127.0.0.1:1080",
        ].map(str::to_string);
        assert!(validate_flags(Engine::Fortress, &mapped).is_ok());
        assert!(validate_flags(Engine::Fortress, &["--uxr-seed=81723".into()]).is_err());
        for flag in ["--user-agent", "--user-agent=Mozilla/5.0"] {
            let err = validate_flags(Engine::Fortress, &[flag.into()]).unwrap_err().to_string();
            assert!(err.contains("--user-agent is forbidden"), "{err}");
        }
        assert!(validate_flags(Engine::Fortress, &["--remote-debugging-port=9222".into()]).is_err());
        assert!(validate_flags(Engine::Fortress, &["--remote-debugging-pipe".into()]).is_err());
        assert_eq!(executable_for_engine(Engine::Chrome), "google-chrome-stable");
        assert_eq!(executable_for_engine(Engine::Fortress), "/opt/fortress/tilion");
        assert_eq!(parse_engine("fortress").unwrap(), Engine::Fortress);
        assert!(parse_engine("unknown").is_err());
    }

    #[test]
    fn old_cleanup_does_not_remove_a_replacement_socket() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("cdp.sock");
        let old_listener = UnixListener::bind(&path).unwrap();
        let old_cleanup = SocketCleanup::new(path.clone()).unwrap();
        drop(old_listener);
        std::fs::remove_file(&path).unwrap();
        let new_listener = UnixListener::bind(&path).unwrap();

        drop(old_cleanup);
        assert!(UnixStream::connect(&path).is_ok());
        drop(new_listener);
    }

    #[test]
    fn reused_inode_does_not_remove_a_live_successor() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("cdp.sock");
        let successor = UnixListener::bind(&path).unwrap();
        // Same device and inode as the live socket: what Linux inode reuse produces.
        drop(SocketCleanup::new(path.clone()).unwrap());
        assert!(UnixStream::connect(&path).is_ok());
        drop(successor);
    }

    #[test]
    fn held_listener_is_closed_and_its_socket_removed() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("cdp.sock");
        let listener = UnixListener::bind(&path).unwrap();
        drop(SocketCleanup::new(path.clone()).unwrap().holding(listener));
        assert!(!path.exists());
    }
}
