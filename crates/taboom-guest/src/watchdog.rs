use tracing::debug;

pub fn notify_ready() {
    if let Ok(sock) = std::env::var("NOTIFY_SOCKET") {
        debug!(socket = %sock, "notifying systemd: READY=1");
        let _ = sd_notify(&sock, "READY=1");
    }
}

pub fn notify_watchdog() {
    if let Ok(sock) = std::env::var("NOTIFY_SOCKET") {
        let _ = sd_notify(&sock, "WATCHDOG=1");
    }
}

#[cfg(unix)]
fn sd_notify(sock_path: &str, msg: &str) -> std::io::Result<()> {
    use std::os::unix::net::UnixDatagram;
    let socket = UnixDatagram::unbound()?;
    socket.send_to(msg.as_bytes(), sock_path)?;
    Ok(())
}

#[cfg(not(unix))]
fn sd_notify(_sock_path: &str, _msg: &str) -> std::io::Result<()> {
    Ok(())
}
