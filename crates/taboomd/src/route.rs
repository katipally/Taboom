use crate::network::{judge_exit, GeoInfo, GeoLookup, GEOLITE_REQUIRED};
use crate::persona::{Persona, Route};
use crate::vault::Vault;
use anyhow::{bail, Context, Result};
use base64::Engine;
use chrono::{DateTime, Utc};
use serde::Serialize;
use std::net::{IpAddr, SocketAddr};
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::watch;
use tracing::{debug, info, warn};

/// Where Chrome's `--proxy-server` points for proxy routes.
pub const FORWARDER_ADDR: &str = "127.0.0.1:1080";
const UPSTREAM_TIMEOUT: Duration = Duration::from_secs(20);
/// A local client that connects and stalls mid-handshake would otherwise hold a task forever.
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);

/// The persona's upstream proxy. Credentials live only in this process's memory.
pub struct Upstream {
    http: bool,
    host: String,
    port: u16,
    creds: Option<(String, String)>,
}

impl Upstream {
    /// None for a direct route. `auth = "vault:<name>"` is read from the vault, unlocked with the
    /// passphrase in TABOOM_VAULT_PASSPHRASE_FILE (default: the Docker secret path).
    pub fn resolve(persona: &Persona, home: &Path) -> Result<Option<Arc<Self>>> {
        let Some(proxy) = persona.route.proxy() else { return Ok(None) };
        let (host, port) = persona.route.endpoint()?;
        let creds = match proxy.auth.as_deref().map(|a| a.strip_prefix("vault:").unwrap_or(a)) {
            Some(name) => Some(vault_credentials(home, name)?),
            None => None,
        };
        Ok(Some(Arc::new(Self { http: matches!(persona.route, Route::Http(_)), host, port, creds })))
    }

    async fn connect(&self, target: &Target) -> Result<TcpStream> {
        let mut s = TcpStream::connect((self.host.as_str(), self.port))
            .await
            .with_context(|| format!("connecting to the upstream proxy {}:{}", self.host, self.port))?;
        if self.http {
            let authority = target.authority();
            let mut req = format!("CONNECT {authority} HTTP/1.1\r\nHost: {authority}\r\n");
            if let Some((user, pass)) = &self.creds {
                let token = base64::engine::general_purpose::STANDARD.encode(format!("{user}:{pass}"));
                req.push_str(&format!("Proxy-Authorization: Basic {token}\r\n"));
            }
            req.push_str("\r\n");
            s.write_all(req.as_bytes()).await?;
            let head = read_http_head(&mut s).await?;
            if head.split_whitespace().nth(1) != Some("200") {
                bail!("upstream refused CONNECT {authority}: {}", head.lines().next().unwrap_or_default());
            }
            return Ok(s);
        }
        let method = if self.creds.is_some() { 2 } else { 0 };
        s.write_all(&[5, 1, method]).await?;
        let mut reply = [0u8; 2];
        s.read_exact(&mut reply).await?;
        if reply != [5, method] {
            bail!("upstream SOCKS5 does not accept {}", if method == 2 { "password auth" } else { "no auth" });
        }
        if let Some((user, pass)) = &self.creds {
            let mut msg = vec![1, user.len() as u8];
            msg.extend(user.as_bytes());
            msg.push(pass.len() as u8);
            msg.extend(pass.as_bytes());
            s.write_all(&msg).await?;
            s.read_exact(&mut reply).await?;
            if reply[1] != 0 {
                bail!("upstream SOCKS5 rejected the credentials");
            }
        }
        let mut msg = vec![5, 1, 0];
        msg.extend(target.socks_bytes());
        s.write_all(&msg).await?;
        let mut head = [0u8; 4];
        s.read_exact(&mut head).await?;
        if head[1] != 0 {
            bail!("upstream SOCKS5 could not reach {} (reply {})", target.authority(), head[1]);
        }
        read_target(&mut s, head[3]).await?; // the proxy's bound address, unused
        Ok(s)
    }
}

fn vault_credentials(home: &Path, name: &str) -> Result<(String, String)> {
    let file = std::env::var("TABOOM_VAULT_PASSPHRASE_FILE").unwrap_or_else(|_| "/run/secrets/taboom_vault_passphrase".into());
    let passphrase = std::fs::read_to_string(&file)
        .with_context(|| format!("route.auth needs the vault passphrase in {file} (a Docker secret or mounted file)"))?;
    let vault = Vault::open(home);
    vault.unlock(passphrase.trim_end_matches(['\r', '\n']))?;
    let record = vault.get_secret(name)?;
    vault.lock();
    let value = String::from_utf8(record.with_context(|| format!("vault has no secret {name:?}"))?.encrypted_value)
        .context("proxy credentials are not UTF-8")?;
    let (user, pass) = value.split_once(':').with_context(|| format!("vault secret {name:?} must be user:password"))?;
    if user.is_empty() || user.len() > 255 || pass.len() > 255 {
        bail!("vault secret {name:?}: user must be 1-255 bytes and password at most 255");
    }
    Ok((user.into(), pass.into()))
}

/// A SOCKS5 CONNECT target, kept as the client sent it so names resolve at the proxy.
enum Target {
    Ip(IpAddr, u16),
    Name(String, u16),
}

impl Target {
    fn authority(&self) -> String {
        match self {
            Target::Ip(IpAddr::V6(ip), port) => format!("[{ip}]:{port}"),
            Target::Ip(ip, port) => format!("{ip}:{port}"),
            Target::Name(name, port) => format!("{name}:{port}"),
        }
    }

    fn socks_bytes(&self) -> Vec<u8> {
        let (mut out, port) = match self {
            Target::Ip(IpAddr::V4(ip), port) => ([&[1u8][..], &ip.octets()].concat(), port),
            Target::Ip(IpAddr::V6(ip), port) => ([&[4u8][..], &ip.octets()].concat(), port),
            Target::Name(name, port) => ([&[3u8, name.len() as u8][..], name.as_bytes()].concat(), port),
        };
        out.extend(port.to_be_bytes());
        out
    }
}

async fn read_target(s: &mut (impl AsyncRead + Unpin), atyp: u8) -> Result<Target> {
    let host = match atyp {
        1 => {
            let mut b = [0u8; 4];
            s.read_exact(&mut b).await?;
            Err(IpAddr::from(b))
        }
        4 => {
            let mut b = [0u8; 16];
            s.read_exact(&mut b).await?;
            Err(IpAddr::from(b))
        }
        3 => {
            let len = s.read_u8().await? as usize;
            let mut b = vec![0u8; len];
            s.read_exact(&mut b).await?;
            Ok(String::from_utf8(b).context("target name is not UTF-8")?)
        }
        other => bail!("unknown SOCKS5 address type {other}"),
    };
    let port = s.read_u16().await?;
    Ok(match host {
        Ok(name) => Target::Name(name, port),
        Err(ip) => Target::Ip(ip, port),
    })
}

async fn read_http_head(s: &mut TcpStream) -> Result<String> {
    let mut head = Vec::new();
    while !head.ends_with(b"\r\n\r\n") {
        if head.len() > 8192 {
            bail!("upstream sent an oversized HTTP response head");
        }
        head.push(s.read_u8().await?);
    }
    Ok(String::from_utf8_lossy(&head).into_owned())
}

/// Local SOCKS5 server (no auth, CONNECT only) that dials every target through the upstream
/// with its credentials, because Chrome's `--proxy-server` cannot carry them. There is no
/// direct path: a dead upstream, or `open` turned off by a failed route check, means no traffic,
/// and turning `open` off also cuts every tunnel already running.
pub async fn forward(listener: TcpListener, upstream: Arc<Upstream>, open: watch::Receiver<bool>) {
    loop {
        let client = match listener.accept().await {
            Ok((client, _)) => client,
            Err(e) => {
                warn!("forwarder accept failed: {e}");
                tokio::time::sleep(Duration::from_millis(200)).await;
                continue;
            }
        };
        if !*open.borrow() {
            continue; // dropped unanswered
        }
        let upstream = Arc::clone(&upstream);
        let open = open.clone();
        tokio::spawn(async move {
            if let Err(e) = relay(client, &upstream, open).await {
                debug!("forwarder connection ended: {e:#}");
            }
        });
    }
}

async fn relay(mut client: TcpStream, upstream: &Upstream, mut open: watch::Receiver<bool>) -> Result<()> {
    let target = tokio::time::timeout(HANDSHAKE_TIMEOUT, client_handshake(&mut client))
        .await
        .context("client handshake timed out")??;
    // reply codes: 0 ok, 1 general failure
    let fail = |code: u8| [5, code, 0, 1, 0, 0, 0, 0, 0, 0];
    let mut server = match tokio::time::timeout(UPSTREAM_TIMEOUT, upstream.connect(&target)).await {
        Ok(Ok(server)) => server,
        Ok(Err(e)) => {
            client.write_all(&fail(1)).await?;
            return Err(e);
        }
        Err(_) => {
            client.write_all(&fail(1)).await?;
            bail!("upstream timed out reaching {}", target.authority());
        }
    };
    client.write_all(&fail(0)).await?;
    tokio::select! {
        copied = tokio::io::copy_bidirectional(&mut client, &mut server) => { copied?; }
        // A closed sender means the monitor is gone; treat it as closed too.
        _ = open.wait_for(|ok| !ok) => bail!("route check failed; tunnel to {} cut", target.authority()),
    }
    Ok(())
}

/// Reads the client's greeting and CONNECT request; answers refusals itself.
async fn client_handshake(client: &mut TcpStream) -> Result<Target> {
    let mut hello = [0u8; 2];
    client.read_exact(&mut hello).await?;
    let mut methods = vec![0u8; hello[1] as usize];
    client.read_exact(&mut methods).await?;
    if hello[0] != 5 || !methods.contains(&0) {
        client.write_all(&[5, 0xff]).await?;
        bail!("client did not offer SOCKS5 without auth");
    }
    client.write_all(&[5, 0]).await?;
    let mut req = [0u8; 4];
    client.read_exact(&mut req).await?;
    let target = read_target(client, req[3]).await?;
    if req[0] != 5 || req[1] != 1 {
        // 7: command not supported
        client.write_all(&[5, 7, 0, 1, 0, 0, 0, 0, 0, 0]).await?;
        bail!("only SOCKS5 CONNECT is supported");
    }
    Ok(target)
}

/// One route check's outcome, as `persona_status` reports it.
#[derive(Debug, Clone, Serialize)]
pub struct Health {
    pub ok: bool,
    pub route: &'static str,
    pub checked_at: DateTime<Utc>,
    pub exit: Option<GeoInfo>,
    pub detail: String,
}

/// Fetches the exit IP through the route (for a proxy, through a private forwarder on the same
/// upstream) and judges it against the persona.
pub async fn check(persona: &Persona, upstream: Option<&Arc<Upstream>>, geo: &GeoLookup) -> Health {
    let outcome = async {
        if upstream.is_some() && geo.databases() != (true, true) {
            bail!(GEOLITE_REQUIRED);
        }
        let ip = match upstream {
            Some(up) => {
                let listener = TcpListener::bind("127.0.0.1:0").await?;
                let addr = listener.local_addr()?;
                let (_open, gate) = watch::channel(true);
                let task = tokio::spawn(forward(listener, Arc::clone(up), gate));
                let ip = exit_ip(Some(addr)).await;
                task.abort();
                ip?
            }
            None => match exit_ip(None).await {
                Ok(ip) => ip,
                // nothing can leak around a direct route, so being offline is not a failure
                Err(e) => return Ok((None, format!("exit IP not verified: {e:#}"))),
            },
        };
        let info = geo.lookup(ip);
        judge_exit(&info, geo.databases(), persona).map_err(anyhow::Error::msg)?;
        let detail = match (upstream, geo.databases()) {
            (None, (false, _)) => "ok; GeoLite2 not installed, exit country not checked".to_string(),
            _ => "ok".to_string(),
        };
        anyhow::Ok((Some(info), detail))
    }
    .await;
    let (ok, exit, detail) = match outcome {
        Ok((exit, detail)) => (true, exit, detail),
        Err(e) => (false, None, format!("{e:#}")),
    };
    Health { ok, route: persona.route.kind(), checked_at: Utc::now(), exit, detail }
}

/// The exit IP an HTTPS echo service sees (TABOOM_ROUTE_ECHO_URL, default api.ipify.org).
async fn exit_ip(socks: Option<SocketAddr>) -> Result<IpAddr> {
    let url = std::env::var("TABOOM_ROUTE_ECHO_URL").unwrap_or_else(|_| "https://api.ipify.org".into());
    let mut cmd = tokio::process::Command::new("curl");
    cmd.args(["-sS", "--fail", "--max-time", "20"]);
    match socks {
        Some(addr) => cmd.arg("--proxy").arg(format!("socks5h://{addr}")),
        None => cmd.args(["--noproxy", "*"]),
    };
    let out = cmd.arg(&url).output().await.context("running curl")?;
    if !out.status.success() {
        bail!("{url} unreachable: {}", String::from_utf8_lossy(&out.stderr).trim());
    }
    let body = String::from_utf8_lossy(&out.stdout);
    body.trim().parse().with_context(|| format!("{url} did not answer with an IP"))
}

/// Route health for the running daemon. A failed check closes the forwarder and makes the
/// handler refuse actions until a later check passes.
pub struct Monitor {
    persona: Arc<Persona>,
    upstream: Option<Arc<Upstream>>,
    geo: GeoLookup,
    open: watch::Sender<bool>,
    health: Mutex<Health>,
}

impl Monitor {
    pub fn new(persona: Arc<Persona>, upstream: Option<Arc<Upstream>>, geo: GeoLookup) -> Self {
        let health = Health {
            ok: false,
            route: persona.route.kind(),
            checked_at: Utc::now(),
            exit: None,
            detail: "not checked yet".into(),
        };
        Self { persona, upstream, geo, open: watch::channel(false).0, health: Mutex::new(health) }
    }

    pub fn health(&self) -> Health {
        self.health.lock().unwrap().clone()
    }

    #[cfg(test)]
    pub fn set(&self, health: Health) {
        self.open.send_replace(health.ok);
        *self.health.lock().unwrap() = health;
    }

    pub async fn recheck(&self) -> Health {
        let health = check(&self.persona, self.upstream.as_ref(), &self.geo).await;
        if health.ok {
            info!(route = health.route, exit = ?health.exit.as_ref().map(|e| e.ip), "route check passed");
        } else {
            warn!(route = health.route, "route check failed: {}", health.detail);
        }
        self.open.send_replace(health.ok);
        *self.health.lock().unwrap() = health.clone();
        health
    }

    /// Serves the forwarder on FORWARDER_ADDR (proxy routes only) and rechecks every `every`.
    pub async fn run(self: Arc<Self>, every: Duration) -> Result<()> {
        if let Some(up) = &self.upstream {
            let listener = TcpListener::bind(FORWARDER_ADDR).await.with_context(|| format!("binding {FORWARDER_ADDR}"))?;
            tokio::spawn(forward(listener, Arc::clone(up), self.open.subscribe()));
        }
        let me = Arc::clone(&self);
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(every).await;
                me.recheck().await;
            }
        });
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One length-prefixed field of a SOCKS5 password message.
    async fn field(s: &mut TcpStream) -> Vec<u8> {
        let mut b = vec![0u8; s.read_u8().await.unwrap() as usize];
        s.read_exact(&mut b).await.unwrap();
        b
    }

    /// A SOCKS5 proxy that wants user:pass and answers each tunnel with "via proxy".
    async fn fake_upstream(user: &'static str, pass: &'static str) -> SocketAddr {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            while let Ok((mut s, _)) = listener.accept().await {
                tokio::spawn(async move {
                    let mut b = [0u8; 3];
                    s.read_exact(&mut b).await.unwrap();
                    assert_eq!(b, [5, 1, 2], "forwarder must offer password auth");
                    s.write_all(&[5, 2]).await.unwrap();
                    assert_eq!(s.read_u8().await.unwrap(), 1);
                    let ok = field(&mut s).await == user.as_bytes() && field(&mut s).await == pass.as_bytes();
                    s.write_all(&[1, if ok { 0 } else { 1 }]).await.unwrap();
                    if !ok {
                        return;
                    }
                    let mut head = [0u8; 4];
                    s.read_exact(&mut head).await.unwrap();
                    let target = read_target(&mut s, head[3]).await.unwrap();
                    assert_eq!(target.authority(), "example.com:443", "names must reach the proxy unresolved");
                    s.write_all(&[5, 0, 0, 1, 0, 0, 0, 0, 0, 0]).await.unwrap();
                    s.write_all(b"via proxy").await.unwrap();
                    // hold the tunnel open until the client side goes away
                    let _ = s.read(&mut [0u8; 1]).await;
                });
            }
        });
        addr
    }

    /// Connects through the forwarder to example.com:443; Ok(bytes the tunnel returned).
    async fn through(forwarder: SocketAddr) -> Result<Vec<u8>> {
        Ok(tunnel(forwarder).await?.1)
    }

    /// An open tunnel to example.com:443 and the greeting it returned.
    async fn tunnel(forwarder: SocketAddr) -> Result<(TcpStream, Vec<u8>)> {
        let mut c = TcpStream::connect(forwarder).await?;
        c.write_all(&[5, 1, 0]).await?;
        let mut r = [0u8; 2];
        c.read_exact(&mut r).await?;
        let mut req = vec![5, 1, 0];
        req.extend(Target::Name("example.com".into(), 443).socks_bytes());
        c.write_all(&req).await?;
        let mut head = [0u8; 10];
        c.read_exact(&mut head).await?;
        if head[1] != 0 {
            bail!("forwarder replied {}", head[1]);
        }
        let mut body = vec![0u8; 9];
        c.read_exact(&mut body).await?;
        Ok((c, body))
    }

    async fn forwarder(upstream: Upstream, open: bool) -> (SocketAddr, watch::Sender<bool>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let (gate, rx) = watch::channel(open);
        tokio::spawn(forward(listener, Arc::new(upstream), rx));
        (addr, gate)
    }

    fn upstream(addr: SocketAddr, pass: &str) -> Upstream {
        Upstream { http: false, host: addr.ip().to_string(), port: addr.port(), creds: Some(("shop".into(), pass.into())) }
    }

    #[tokio::test]
    async fn forwarder_adds_credentials_and_keeps_names_remote() {
        let proxy = fake_upstream("shop", "s3cret").await;
        let (fwd, _gate) = forwarder(upstream(proxy, "s3cret"), true).await;
        assert_eq!(through(fwd).await.unwrap(), b"via proxy");
    }

    #[tokio::test]
    async fn forwarder_fails_closed() {
        let proxy = fake_upstream("shop", "s3cret").await;
        let (fwd, _) = forwarder(upstream(proxy, "wrong"), true).await;
        assert!(through(fwd).await.unwrap_err().to_string().contains("replied 1"), "bad credentials");

        let dead = TcpListener::bind("127.0.0.1:0").await.unwrap().local_addr().unwrap();
        let (fwd, _) = forwarder(upstream(dead, "s3cret"), true).await;
        assert!(through(fwd).await.is_err(), "dead upstream has no direct fallback");

        let (fwd, gate) = forwarder(upstream(proxy, "s3cret"), false).await;
        assert!(through(fwd).await.is_err(), "closed after a failed route check");
        gate.send_replace(true);
        assert_eq!(through(fwd).await.unwrap(), b"via proxy", "reopens after a passing check");
    }

    #[tokio::test]
    async fn failed_check_cuts_open_tunnels() {
        let proxy = fake_upstream("shop", "s3cret").await;
        let (fwd, gate) = forwarder(upstream(proxy, "s3cret"), true).await;
        let (mut c, _) = tunnel(fwd).await.unwrap();
        gate.send_replace(false);
        let read = tokio::time::timeout(Duration::from_secs(2), c.read(&mut [0u8; 1])).await;
        assert!(matches!(read, Ok(Ok(0)) | Ok(Err(_))), "tunnel must close, got {read:?}");
    }


    #[tokio::test]
    async fn proxy_route_without_geolite_is_refused() {
        let persona = Persona::parse(
            "name = \"a\"\ncountry = \"DE\"\n[route]\ntype = \"socks5\"\nurl = \"127.0.0.1:9\"",
        )
        .unwrap();
        let up = Upstream::resolve(&persona, Path::new("/nonexistent")).unwrap();
        let geo = GeoLookup::open(Path::new("/nonexistent"));
        let health = check(&persona, up.as_ref(), &geo).await;
        assert!(!health.ok);
        assert_eq!(health.route, "socks5");
        assert!(health.detail.contains("GeoLite2"), "{}", health.detail);
    }

    #[test]
    fn vault_reference_needs_the_passphrase_file() {
        let persona = Persona::parse(
            "name = \"a\"\ncountry = \"DE\"\n[route]\ntype = \"http\"\nurl = \"h:8080\"\nauth = \"vault:p\"",
        )
        .unwrap();
        let home = tempfile::tempdir().unwrap();
        let err = Upstream::resolve(&persona, home.path()).err().unwrap();
        assert!(format!("{err:#}").contains("passphrase"), "{err:#}");
        assert!(Upstream::resolve(&Persona::parse("name = \"a\"\ncountry = \"DE\"").unwrap(), home.path()).unwrap().is_none());
    }
}
