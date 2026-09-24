use anyhow::{Context, Result, bail};
use serde_json::{Value, json};
use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::time::Duration;

const MAX_BRIDGE_LINE: usize = 64 * 1024;
const MAX_TARGET_MESSAGE: usize = 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PageScheme {
    Http,
    Https,
}

impl PageScheme {
    fn as_str(self) -> &'static str {
        match self {
            Self::Http => "http",
            Self::Https => "https",
        }
    }

    pub(crate) fn from_str(scheme: &str) -> Option<Self> {
        if scheme.eq_ignore_ascii_case("http") {
            Some(Self::Http)
        } else if scheme.eq_ignore_ascii_case("https") {
            Some(Self::Https)
        } else {
            None
        }
    }
}

/// A tab or popup whose content a page script can write without its own web host: about:blank,
/// data:, blob:, file:, or an HTTP(S) URL too malformed to name a host. An opener can draw a
/// fake address bar into one while it navigates itself to the login site, so any such page
/// refuses secret typing.
#[derive(Debug)]
pub struct OpaquePageOpen;

impl std::fmt::Display for OpaquePageOpen {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("a blank, data or file page is open")
    }
}

impl std::error::Error for OpaquePageOpen {}

/// Browser-owned pages that no website script controls.
const BROWSER_UI_SCHEMES: [&str; 3] = ["chrome", "chrome-search", "devtools"];

/// A page's normalized origin host and transport security, without path or query data.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PageHost {
    pub host: String,
    pub scheme: PageScheme,
}

impl PageHost {
    fn from_url(url: &str) -> Option<Self> {
        let (raw_scheme, _) = url.split_once("://")?;
        let scheme = PageScheme::from_str(raw_scheme)?;
        Some(Self { host: http_url_host(url)?, scheme })
    }

    fn from_bridge(value: &Value) -> Result<Self> {
        let scheme = value["scheme"].as_str()
            .and_then(PageScheme::from_str)
            .context("Chrome target bridge returned an invalid page scheme")?;
        let host = value["host"].as_str()
            .and_then(crate::vault::normalize_domain)
            .context("Chrome target bridge returned an invalid page host")?;
        Ok(Self { host, scheme })
    }
}

pub trait PageTargets: Send + Sync {
    fn page_hosts(&self) -> Result<Vec<PageHost>>;
}

pub struct CdpPageTargets {
    socket: PathBuf,
}

impl CdpPageTargets {
    pub fn new(home: &std::path::Path) -> Self {
        Self { socket: home.join("run").join("browser-cdp.sock") }
    }
}

impl PageTargets for CdpPageTargets {
    fn page_hosts(&self) -> Result<Vec<PageHost>> {
        let mut stream = UnixStream::connect(&self.socket)
            .context("Chrome's private remote-debugging pipe is unavailable")?;
        stream.set_read_timeout(Some(Duration::from_secs(5)))?;
        stream.set_write_timeout(Some(Duration::from_secs(2)))?;
        stream.write_all(b"get-targets\n")?;

        let mut line = read_bounded_line(&mut stream, MAX_BRIDGE_LINE)?;
        if line.is_empty() || line.len() > MAX_BRIDGE_LINE || line.last() != Some(&b'\n') {
            bail!("Chrome target bridge returned an invalid response");
        }
        line.pop();
        let response: Value = serde_json::from_slice(&line)
            .context("Chrome target bridge returned invalid JSON")?;
        page_hosts_from_bridge_response(&response)
    }
}

fn page_hosts_from_bridge_response(response: &Value) -> Result<Vec<PageHost>> {
    let opaque = response["opaque_pages"].as_u64()
        .context("Chrome target bridge did not report opaque pages")?;
    if opaque > 0 {
        return Err(anyhow::Error::new(OpaquePageOpen));
    }
    let pages = response["pages"].as_array()
        .context("Chrome target bridge did not return page targets")?;
    pages.iter().map(PageHost::from_bridge).collect()
}

/// Returns a normalized HTTP(S) hostname without exposing path, query or fragment data.
pub fn http_url_host(url: &str) -> Option<String> {
    let (scheme, rest) = url.split_once("://")?;
    if !scheme.eq_ignore_ascii_case("http") && !scheme.eq_ignore_ascii_case("https") {
        return None;
    }
    let authority_end = rest.find(|c| matches!(c, '/' | '?' | '#')).unwrap_or(rest.len());
    let authority = &rest[..authority_end];
    if authority.is_empty() || authority.contains('@') || authority.bytes().any(|b| b.is_ascii_whitespace()) {
        return None;
    }
    let host = if let Some(bracketed) = authority.strip_prefix('[') {
        let (host, suffix) = bracketed.split_once(']')?;
        if !suffix.is_empty() && !valid_port(suffix.strip_prefix(':')?) {
            return None;
        }
        host
    } else {
        match authority.rsplit_once(':') {
            Some((host, port)) if !host.contains(':') && valid_port(port) => host,
            Some(_) if authority.contains(':') => return None,
            _ => authority,
        }
    };
    crate::vault::normalize_domain(host)
}

/// Private helper run as Chrome's parent. The only accepted query is a constant
/// `Target.getTargets`; page IDs, titles, and all other CDP methods stay inside this process.
pub fn serve_pipe_queries(
    stdin: &mut impl Write,
    receiver: &std::sync::mpsc::Receiver<Result<String, String>>,
    mut client: UnixStream,
    next_id: &mut u64,
) -> Result<()> {
    client.set_read_timeout(Some(Duration::from_secs(2)))?;
    client.set_write_timeout(Some(Duration::from_secs(2)))?;
    let mut line = read_bounded_line(&mut client, MAX_BRIDGE_LINE)?;
    if line.is_empty() || line.len() > MAX_BRIDGE_LINE || line.last() != Some(&b'\n') {
        bail!("invalid bridge request");
    }
    line.pop();
    let health = line == b"health";
    if !health && line != b"get-targets" {
        bail!("unsupported bridge request");
    }

    let id = *next_id;
    *next_id = next_id.wrapping_add(1).max(1);
    let mut request = serde_json::to_vec(&json!({ "id": id, "method": "Target.getTargets" }))?;
    request.push(0);
    stdin.write_all(&request)?;
    stdin.flush()?;

    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    let mut pages = None;
    while std::time::Instant::now() < deadline {
        let remaining = deadline.saturating_duration_since(std::time::Instant::now());
        match receiver.recv_timeout(remaining) {
            Ok(Ok(message)) => {
                let value: Value = serde_json::from_str(&message)
                    .context("invalid message on Chrome's remote-debugging pipe")?;
                if value["id"].as_u64() != Some(id) {
                    continue; // drop asynchronous Target events; never log them
                }
                if !value["error"].is_null() {
                    bail!("Chrome refused Target.getTargets");
                }
                let targets = value["result"]["targetInfos"].as_array()
                    .context("Chrome Target.getTargets response has no target list")?;
                if health {
                    serde_json::to_writer(&mut client, &json!({ "ready": true }))?;
                    client.write_all(b"\n")?;
                    return Ok(());
                }
                let (hosts, opaque) = page_hosts_from_target_infos(targets)?;
                let hosts: Vec<Value> = hosts
                    .into_iter()
                    .map(|page| json!({ "scheme": page.scheme.as_str(), "host": page.host }))
                    .collect();
                pages = Some((hosts, opaque));
                break;
            }
            Ok(Err(_)) => bail!("Chrome's remote-debugging pipe closed"),
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                bail!("Chrome Target.getTargets timed out");
            }
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                bail!("Chrome's remote-debugging pipe closed");
            }
        }
    }
    let (pages, opaque) = pages.context("Chrome Target.getTargets timed out")?;
    serde_json::to_writer(&mut client, &json!({ "pages": pages, "opaque_pages": opaque }))?;
    client.write_all(b"\n")?;
    Ok(())
}

/// Web page hosts, plus how many pages have no web host and are not browser UI.
fn page_hosts_from_target_infos(targets: &[Value]) -> Result<(Vec<PageHost>, usize)> {
    let mut page_hosts = Vec::new();
    let mut opaque = 0;
    for target in targets {
        if target["type"].as_str() == Some("page") {
            let url = target["url"].as_str()
                .context("Chrome page target did not include a URL")?;
            match PageHost::from_url(url) {
                Some(page) => page_hosts.push(page),
                None => {
                    let scheme = url.split_once(':').map_or("", |(scheme, _)| scheme);
                    if !BROWSER_UI_SCHEMES.iter().any(|ui| scheme.eq_ignore_ascii_case(ui)) {
                        opaque += 1;
                    }
                }
            }
        }
    }
    page_hosts.sort_by(|a, b| {
        a.host.cmp(&b.host).then_with(|| a.scheme.as_str().cmp(b.scheme.as_str()))
    });
    page_hosts.dedup();
    Ok((page_hosts, opaque))
}

fn read_bounded_line(reader: &mut impl Read, maximum: usize) -> Result<Vec<u8>> {
    let mut line = Vec::with_capacity(maximum.min(256));
    let mut byte = [0u8; 1];
    loop {
        if line.len() > maximum {
            break;
        }
        match reader.read(&mut byte)? {
            0 => break,
            _ => {
                line.push(byte[0]);
                if byte[0] == b'\n' {
                    break;
                }
            }
        }
    }
    Ok(line)
}

pub fn read_pipe_messages(
    mut reader: impl Read,
    sender: std::sync::mpsc::Sender<Result<String, String>>,
) {
    loop {
        let mut message = Vec::new();
        let mut byte = [0u8; 1];
        loop {
            match reader.read_exact(&mut byte) {
                Ok(()) if byte[0] == 0 => break,
                Ok(()) if message.len() < MAX_TARGET_MESSAGE => message.push(byte[0]),
                Ok(()) => {
                    let _ = sender.send(Err("Chrome target response exceeded 1 MiB".into()));
                    return;
                }
                Err(error) => {
                    let _ = sender.send(Err(error.to_string()));
                    return;
                }
            }
        }
        if let Ok(message) = String::from_utf8(message) {
            if sender.send(Ok(message)).is_err() {
                return;
            }
        } else if sender.send(Err("Chrome target response was not UTF-8".into())).is_err() {
            return;
        }
    }
}

fn valid_port(port: &str) -> bool {
    port.parse::<u16>().is_ok_and(|port| port > 0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader};

    #[test]
    fn host_parser_strips_path_query_and_normalizes_case() {
        assert_eq!(http_url_host("HTTPS://GitHub.COM.:443/login?next=secret#form").as_deref(), Some("github.com"));
        assert_eq!(http_url_host("https://[::1]:8443/"), Some("::1".into()));
        assert_eq!(http_url_host("file:///etc/passwd"), None);
        assert_eq!(http_url_host("https://user:password@example.com/"), None);
        assert_eq!(http_url_host("https://example.com:bogus/"), None);
    }

    #[test]
    fn bridge_response_exposes_normalized_hosts_for_multiple_pages() {
        let response = json!({ "pages": [
            {"scheme":"https", "host":"Accounts.EXAMPLE."},
            {"scheme":"http", "host":"help.example"},
        ], "opaque_pages": 0 });
        assert_eq!(page_hosts_from_bridge_response(&response).unwrap(), vec![
            PageHost { host: "accounts.example".into(), scheme: PageScheme::Https },
            PageHost { host: "help.example".into(), scheme: PageScheme::Http },
        ]);
    }

    #[test]
    fn zero_page_targets_are_empty_and_invalid_bridge_candidates_refuse() {
        assert!(page_hosts_from_bridge_response(&json!({ "pages": [], "opaque_pages": 0 })).unwrap().is_empty());
        let opaque = page_hosts_from_bridge_response(&json!({ "pages": [], "opaque_pages": 1 })).unwrap_err();
        assert!(opaque.is::<OpaquePageOpen>());

        for response in [
            json!({ "pages": [] }),
            json!({ "pages": ["https://one.example"], "opaque_pages": 0 }),
            json!({ "pages": [{"scheme":"file", "host":"example.com"}], "opaque_pages": 0 }),
            json!({ "pages": [{"scheme":"https", "host":"bad host"}], "opaque_pages": 0 }),
        ] {
            assert!(page_hosts_from_bridge_response(&response).is_err());
        }
    }

    #[test]
    fn page_target_host_extraction_discards_paths_and_rejects_non_web_urls() {
        assert_eq!(
            PageHost::from_url("HTTPS://GitHub.COM.:443/login?token=secret#form"),
            Some(PageHost { host: "github.com".into(), scheme: PageScheme::Https }),
        );
        assert_eq!(
            PageHost::from_url("http://github.com/login"),
            Some(PageHost { host: "github.com".into(), scheme: PageScheme::Http }),
        );
        assert_eq!(PageHost::from_url("chrome://settings"), None);
    }

    #[test]
    fn script_writable_pages_are_counted_but_browser_ui_is_not() {
        let targets: Vec<Value> = [
            "about:blank", "about:srcdoc", "data:text/html,<p>x", "blob:https://evil.example/1",
            "file:///tmp/x.html", "https://user@evil.example/", "chrome://newtab/",
            "chrome-search://local-ntp/", "devtools://devtools/bundled/inspector.html",
            "https://github.com/login",
        ].iter().map(|url| json!({"type": "page", "url": url})).collect();
        let (hosts, opaque) = page_hosts_from_target_infos(&targets).unwrap();
        assert_eq!(opaque, 6);
        assert_eq!(hosts, vec![PageHost { host: "github.com".into(), scheme: PageScheme::Https }]);
    }

    #[test]
    fn page_target_filter_omits_paths_and_non_page_targets() {
        let targets: Vec<Value> = serde_json::from_str(concat!(
            r#"["#,
            r#"{"type":"page","url":"https://accounts.example/login?token=private"},"#,
            r#"{"type":"page","url":"https://accounts.example/profile"},"#,
            r#"{"type":"page","url":"http://docs.example/help"},"#,
            r#"{"type":"service_worker","url":"https://worker.example/private"}"#,
            "]"
        )).unwrap();
        let (candidates, opaque) = page_hosts_from_target_infos(&targets).unwrap();
        assert_eq!(opaque, 0);
        assert_eq!(candidates, vec![
            PageHost { host: "accounts.example".into(), scheme: PageScheme::Https },
            PageHost { host: "docs.example".into(), scheme: PageScheme::Http },
        ]);
        assert!(page_hosts_from_target_infos(&[json!({"type":"page"})]).is_err());

        let many_targets: Vec<Value> = (0..100).map(|i| json!({
            "type": "page", "url": format!("https://site-{i:03}.example/login")
        })).collect();
        let (many_hosts, _) = page_hosts_from_target_infos(&many_targets).unwrap();
        assert_eq!(many_hosts.len(), 100);
        let wire_candidates: Vec<Value> = many_hosts.iter().map(|page| json!({
            "scheme": page.scheme.as_str(), "host": page.host
        })).collect();
        assert!(serde_json::to_vec(&json!({ "pages": wire_candidates })).unwrap().len() < MAX_BRIDGE_LINE);
    }

    #[test]
    fn target_bridge_sends_only_hosts_and_schemes_to_the_local_socket() {
        let (tx, rx) = std::sync::mpsc::channel();
        tx.send(Ok(concat!(
            r#"{"id":1,"result":{"targetInfos":["#,
            r#"{"type":"page","url":"https://accounts.example/login?token=private"},"#,
            r#"{"type":"page","url":"http://docs.example/help"},"#,
            r#"{"type":"service_worker","url":"https://worker.example/private"}"#,
            "]}}"
        ).to_string())).unwrap();
        let mut cdp_input = std::io::Cursor::new(Vec::new());
        let (server, mut client) = UnixStream::pair().unwrap();
        client.write_all(b"get-targets\n").unwrap();
        let mut next_id = 1;
        serve_pipe_queries(&mut cdp_input, &rx, server, &mut next_id).unwrap();

        let mut response = String::new();
        BufReader::new(client).read_line(&mut response).unwrap();
        let value: Value = serde_json::from_str(&response).unwrap();
        assert_eq!(value["pages"], json!([
            {"scheme":"https", "host":"accounts.example"},
            {"scheme":"http", "host":"docs.example"},
        ]));
        assert_eq!(value["opaque_pages"], 0);
        assert!(!response.contains("private"));
        assert!(!response.contains("/login"));
        assert!(!response.contains("/help"));
        let request = String::from_utf8(cdp_input.into_inner()).unwrap();
        assert!(request.contains("\"method\":\"Target.getTargets\""));
        assert!(request.ends_with('\0'));
    }

    #[test]
    fn health_check_round_trips_target_get_targets() {
        let (tx, rx) = std::sync::mpsc::channel();
        tx.send(Ok(r#"{"id":1,"result":{"targetInfos":[]}}"#.to_string())).unwrap();
        let mut cdp_input = std::io::Cursor::new(Vec::new());
        let (server, mut client) = UnixStream::pair().unwrap();
        client.write_all(b"health\n").unwrap();
        let mut next_id = 1;
        serve_pipe_queries(&mut cdp_input, &rx, server, &mut next_id).unwrap();

        let mut response = String::new();
        BufReader::new(client).read_line(&mut response).unwrap();
        assert_eq!(response, "{\"ready\":true}\n");
        let request = String::from_utf8(cdp_input.into_inner()).unwrap();
        assert!(request.contains("\"method\":\"Target.getTargets\""));
        assert!(request.ends_with('\0'));
    }

    #[test]
    fn pipe_message_reader_splits_nul_terminated_json() {
        let (tx, rx) = std::sync::mpsc::channel();
        read_pipe_messages(&b"{\"id\":1}\0{\"id\":2}\0"[..], tx);
        assert_eq!(rx.recv().unwrap().unwrap(), "{\"id\":1}");
        assert_eq!(rx.recv().unwrap().unwrap(), "{\"id\":2}");
    }
}
