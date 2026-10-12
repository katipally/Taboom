use anyhow::{bail, Context, Result};
use chrono::{DateTime, Utc};
use rand::RngCore;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// One recording per lease: `recordings/<id>/{meta.json, events.jsonl, frames/}`.
pub struct Recorder {
    dir: PathBuf,
    key: Vec<u8>,
    public_url: String,
    /// TABOOM_RECORDINGS_MAX_GB; unset keeps everything.
    max_bytes: Option<u64>,
    active: Mutex<HashMap<String, Active>>,
}

struct Active {
    id: String,
    step: u32,
    events: File,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Meta {
    pub id: String,
    pub persona: String,
    pub client: String,
    pub started_at: DateTime<Utc>,
    #[serde(default)]
    pub ended_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Event {
    pub step: u32,
    pub ts: DateTime<Utc>,
    pub tool: String,
    pub args: Value,
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    pub duration_ms: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub frame: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Video {
    pub file: &'static str,
    pub bytes: u64,
    pub finished: bool,
}

/// Where the frame for a step comes from.
pub enum Frame {
    None,
    /// The exact image the agent was shown.
    Bytes(Vec<u8>, &'static str),
    /// Filled in later by a background capture after the action settles.
    Pending(&'static str),
}

impl Recorder {
    pub fn open(home: &Path, public_url: String) -> Result<Self> {
        let dir = home.join("recordings");
        std::fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
        let key_path = dir.join(".share-key");
        let key = match std::fs::read(&key_path) {
            Ok(k) if k.len() == 32 => k,
            _ => {
                let mut k = vec![0u8; 32];
                rand::thread_rng().fill_bytes(&mut k);
                std::fs::write(&key_path, &k)?;
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    std::fs::set_permissions(&key_path, std::fs::Permissions::from_mode(0o600))?;
                }
                k
            }
        };
        let max_bytes = std::env::var("TABOOM_RECORDINGS_MAX_GB")
            .ok()
            .and_then(|s| s.parse::<f64>().ok())
            .filter(|gb| gb.is_finite() && *gb > 0.0)
            .map(|gb| (gb * 1e9) as u64);
        Ok(Self { dir, key, public_url, max_bytes, active: Mutex::new(HashMap::new()) })
    }

    pub fn start(&self, client: &str, persona: &str) -> Result<String> {
        let mut active = self.active.lock().unwrap();
        if let Some(a) = active.get(client) {
            return Ok(a.id.clone());
        }
        if let Some(max) = self.max_bytes {
            let live: Vec<&str> = active.values().map(|a| a.id.as_str()).collect();
            self.prune(max, &live);
        }
        let now = Utc::now();
        let short = uuid::Uuid::new_v4().simple().to_string();
        let id = format!("{}-{}-{}", now.format("%Y%m%d-%H%M%S"), slug(persona), &short[..6]);
        let dir = self.dir.join(&id);
        std::fs::create_dir_all(dir.join("frames"))?;
        let meta = Meta {
            id: id.clone(),
            persona: persona.into(),
            client: client.into(),
            started_at: now,
            ended_at: None,
        };
        std::fs::write(dir.join("meta.json"), serde_json::to_vec_pretty(&meta)?)?;
        let events = OpenOptions::new().create(true).append(true).open(dir.join("events.jsonl"))?;
        active.insert(client.into(), Active { id: id.clone(), step: 0, events });
        Ok(id)
    }

    pub fn stop(&self, client: &str) {
        let Some(a) = self.active.lock().unwrap().remove(client) else {
            return;
        };
        let path = self.dir.join(&a.id).join("meta.json");
        if let Ok(mut meta) = self.meta(&a.id) {
            meta.ended_at = Some(Utc::now());
            let _ = serde_json::to_vec_pretty(&meta).map(|b| std::fs::write(path, b));
        }
    }

    pub fn current(&self, client: &str) -> Option<String> {
        self.active.lock().unwrap().get(client).map(|a| a.id.clone())
    }

    /// Appends an event. Returns the frame path to write when the frame is `Pending`.
    pub fn record(
        &self,
        client: &str,
        tool: &str,
        args: Value,
        error: Option<&str>,
        duration_ms: u64,
        frame: Frame,
    ) -> Option<PathBuf> {
        let mut active = self.active.lock().unwrap();
        let a = active.get_mut(client)?;
        a.step += 1;
        let frames = self.dir.join(&a.id).join("frames");
        let (name, pending) = match frame {
            Frame::None => (None, None),
            Frame::Bytes(bytes, ext) => {
                let name = format!("{:05}.{ext}", a.step);
                let _ = std::fs::write(frames.join(&name), bytes);
                (Some(name), None)
            }
            Frame::Pending(ext) => {
                let name = format!("{:05}.{ext}", a.step);
                (Some(name.clone()), Some(frames.join(name)))
            }
        };
        let event = Event {
            step: a.step,
            ts: Utc::now(),
            tool: tool.into(),
            args,
            ok: error.is_none(),
            error: error.map(str::to_string),
            duration_ms,
            frame: name,
        };
        let mut line = serde_json::to_vec(&event).ok()?;
        line.push(b'\n');
        let _ = a.events.write_all(&line);
        pending
    }

    pub fn meta(&self, id: &str) -> Result<Meta> {
        let path = self.session_dir(id)?.join("meta.json");
        Ok(serde_json::from_slice(&std::fs::read(&path).with_context(|| format!("recording {id} not found"))?)?)
    }

    /// Newest first. O(n log n) over session folders; each folder is only stat'd and its meta read.
    pub fn list(&self, persona: Option<&str>, limit: usize) -> Vec<(Meta, usize)> {
        self.list_page(persona, None, None, 0, limit).0
    }

    /// Filter and page session metadata before counting events for the visible page.
    pub fn list_page(
        &self,
        persona: Option<&str>,
        search: Option<&str>,
        active: Option<bool>,
        offset: usize,
        limit: usize,
    ) -> (Vec<(Meta, usize)>, usize) {
        let Ok(entries) = std::fs::read_dir(&self.dir) else {
            return (vec![], 0);
        };
        let search = search
            .map(str::trim)
            .filter(|query| !query.is_empty())
            .map(str::to_lowercase);
        let mut metas: Vec<Meta> = entries
            .flatten()
            .filter_map(|e| self.meta(&e.file_name().to_string_lossy()).ok())
            .filter(|m| persona.map_or(true, |p| m.persona == p))
            .filter(|m| active.map_or(true, |active| m.ended_at.is_none() == active))
            .filter(|m| {
                search.as_ref().map_or(true, |query| {
                    [&m.id, &m.persona, &m.client]
                        .iter()
                        .any(|value| value.to_lowercase().contains(query))
                })
            })
            .collect();
        metas.sort_by(|a, b| b.started_at.cmp(&a.started_at));
        let total = metas.len();
        let page = metas
            .into_iter()
            .skip(offset)
            .take(limit)
            .map(|m| {
                let steps = self.count_events(&m.id);
                (m, steps)
            })
            .collect();
        (page, total)
    }

    /// Deletes the oldest sessions, never a live one, until the folder fits in `max` bytes.
    /// O(files) to size every session, O(n log n) to order them.
    fn prune(&self, max: u64, live: &[&str]) {
        let Ok(entries) = std::fs::read_dir(&self.dir) else {
            return;
        };
        let mut sessions: Vec<(Meta, u64)> = entries
            .flatten()
            .filter_map(|e| self.meta(&e.file_name().to_string_lossy()).ok())
            .map(|m| {
                let bytes = dir_bytes(&self.dir.join(&m.id));
                (m, bytes)
            })
            .collect();
        let mut total: u64 = sessions.iter().map(|(_, b)| b).sum();
        sessions.sort_by_key(|(meta, _)| meta.started_at);
        for (meta, bytes) in sessions {
            if total <= max {
                break;
            }
            if live.contains(&meta.id.as_str()) {
                continue;
            }
            match std::fs::remove_dir_all(self.dir.join(&meta.id)) {
                Ok(()) => {
                    total -= bytes;
                    tracing::info!(id = %meta.id, bytes, "recording deleted by retention");
                }
                Err(e) => tracing::warn!(id = %meta.id, "recording retention could not delete: {e}"),
            }
        }
    }

    fn count_events(&self, id: &str) -> usize {
        self.session_dir(id)
            .and_then(|d| Ok(File::open(d.join("events.jsonl"))?))
            .map(|f| BufReader::new(f).lines().count())
            .unwrap_or(0)
    }

    /// Events with `from_step <= step`, at most `limit`, plus the total count. Streams the file.
    pub fn events(&self, id: &str, from_step: u32, limit: usize) -> Result<(Vec<Event>, usize)> {
        let file = File::open(self.session_dir(id)?.join("events.jsonl"))
            .with_context(|| format!("recording {id} not found"))?;
        let mut page = Vec::new();
        let mut total = 0;
        for line in BufReader::new(file).lines() {
            let Ok(event) = serde_json::from_str::<Event>(&line?) else {
                continue;
            };
            total += 1;
            if event.step >= from_step && page.len() < limit {
                page.push(event);
            }
        }
        Ok((page, total))
    }

    pub fn frame_path(&self, id: &str, file: &str) -> Result<PathBuf> {
        if !safe_name(file) {
            bail!("bad frame name");
        }
        Ok(self.session_dir(id)?.join("frames").join(file))
    }

    /// Signed replay page link, plus the video link when a video exists.
    pub fn share_url(&self, id: &str, ttl_s: u64) -> Result<(String, Option<String>)> {
        self.session_dir(id)?;
        let exp = unix_now() + ttl_s;
        let sig = sign_url(id, &self.key, exp);
        let base = format!("{}/recordings/{id}", self.public_url.trim_end_matches('/'));
        let video = self.video(id).map(|v| format!("{base}/video/{}?exp={exp}&sig={sig}", v.file));
        Ok((format!("{base}?exp={exp}&sig={sig}"), video))
    }

    /// `video.mp4` once finalized; `video.mkv` while recording or if finalizing failed.
    pub fn video(&self, id: &str) -> Option<Video> {
        let dir = self.session_dir(id).ok()?;
        ["video.mp4", "video.mkv"].into_iter().find_map(|file| {
            let bytes = std::fs::metadata(dir.join(file)).ok()?.len();
            Some(Video { file, bytes, finished: file.ends_with(".mp4") })
        })
    }

    pub fn public_url(&self) -> &str {
        self.public_url.trim_end_matches('/')
    }

    pub fn session_path(&self, id: &str) -> Result<PathBuf> {
        self.session_dir(id)
    }

    pub fn verify(&self, id: &str, exp: u64, sig: &str) -> bool {
        exp >= unix_now() && verify_token(id, &self.key, exp, sig)
    }

    fn session_dir(&self, id: &str) -> Result<PathBuf> {
        if !safe_name(id) {
            bail!("bad recording id");
        }
        Ok(self.dir.join(id))
    }
}

fn dir_bytes(dir: &Path) -> u64 {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return 0;
    };
    entries
        .flatten()
        .filter_map(|e| Some((e.path(), e.metadata().ok()?)))
        .map(|(path, meta)| if meta.is_dir() { dir_bytes(&path) } else { meta.len() })
        .sum()
}

fn safe_name(s: &str) -> bool {
    !s.is_empty()
        && !s.starts_with('.')
        && s.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
}

fn slug(s: &str) -> String {
    let slug: String = s
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c.to_ascii_lowercase() } else { '-' })
        .take(32)
        .collect();
    if slug.is_empty() { "persona".into() } else { slug }
}

fn sign_url(recording_id: &str, secret: &[u8], expires_at: u64) -> String {
    use hmac::{Hmac, Mac};
    use sha2::Sha256;

    let message = format!("{recording_id}:{expires_at}");
    let mut mac = Hmac::<Sha256>::new_from_slice(secret).expect("HMAC accepts any key length");
    mac.update(message.as_bytes());
    hex::encode(mac.finalize().into_bytes())
}

fn verify_token(recording_id: &str, secret: &[u8], expires_at: u64, token: &str) -> bool {
    let expected = sign_url(recording_id, secret, expires_at);
    constant_time_eq(expected.as_bytes(), token.as_bytes())
}

fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

const APP_CSS: &str = r#"
:root{color-scheme:light;--bezel:#f2f2ee;--panel:#fff;--well:#f8f8f5;--sub:#ededea;--line:rgba(26,29,28,.12);--line-strong:rgba(26,29,28,.22);--ink:#1b1d1c;--ink-2:#4e5350;--muted:#767c78;--accent:#4568ff;--accent-soft:#edf1ff;--good:#34794e;--good-soft:#eaf5ed;--bad:#b54135;--bad-soft:#faece9;--lift:0 1px 2px rgba(24,26,24,.04),0 0 0 1px var(--line);--well-shadow:inset 0 1px 3px rgba(24,26,24,.06),inset 0 0 0 1px var(--line)}
@media(prefers-color-scheme:dark){:root{color-scheme:dark;--bezel:#141412;--panel:#1d1d1a;--well:#252522;--sub:#1a1a17;--line:rgba(255,255,255,.11);--line-strong:rgba(255,255,255,.2);--ink:#f5f5ef;--ink-2:#c2c4bc;--muted:#93968e;--accent:#93b0ff;--accent-soft:rgba(147,176,255,.14);--good:#69cf91;--good-soft:rgba(105,207,145,.11);--bad:#ff887d;--bad-soft:rgba(255,136,125,.1);--lift:0 1px 2px rgba(0,0,0,.35),inset 0 1px 0 rgba(255,255,255,.035),0 0 0 1px var(--line);--well-shadow:inset 0 1px 3px rgba(0,0,0,.35),inset 0 0 0 1px var(--line)}}
*{box-sizing:border-box}html{scroll-behavior:smooth}body{margin:0;background:var(--bezel);color:var(--ink);font:14px/1.5 -apple-system,BlinkMacSystemFont,"Segoe UI",sans-serif;-webkit-font-smoothing:antialiased;text-rendering:optimizeLegibility}button,input,select{color:inherit;font:inherit}a{color:inherit}:focus-visible{outline:2px solid var(--accent);outline-offset:2px}[hidden]{display:none!important}
.skip-link{position:absolute;left:12px;top:-60px;z-index:20;padding:9px 13px;background:var(--panel);border-radius:8px;box-shadow:var(--lift)}.skip-link:focus{top:12px}.app-shell{width:min(1440px,100%);min-height:100vh;margin:0 auto;padding:18px 26px 48px}.detail-shell{width:min(1120px,100%)}
.topbar{display:flex;min-height:58px;align-items:center;gap:28px;padding:0 16px;border-radius:15px;background:var(--panel);box-shadow:var(--lift)}.brand{display:inline-flex;align-items:center;gap:10px;color:var(--ink);font-size:15px;font-weight:650;letter-spacing:-.025em;text-decoration:none}.brand-mark{display:grid;width:28px;height:28px;place-items:center;border-radius:8px;background:var(--ink);color:var(--panel);font-size:13px;font-weight:700}.primary-nav{display:flex;align-items:center;gap:5px}.primary-nav a{display:inline-flex;min-height:38px;align-items:center;padding:0 12px;border-radius:8px;color:var(--muted);font-size:12.5px;text-decoration:none}.primary-nav a:hover,.primary-nav a[aria-current=page]{background:var(--well);color:var(--ink)}.topbar-spacer{flex:1}.service-status{display:inline-flex;align-items:center;gap:8px;color:var(--muted);font-size:12px;white-space:nowrap}.status-dot{width:8px;height:8px;flex:0 0 auto;border-radius:50%;background:var(--muted);box-shadow:0 0 0 3px var(--sub)}.service-status[data-state=ready] .status-dot{background:var(--good);box-shadow:0 0 0 3px var(--good-soft)}.service-status[data-state=error] .status-dot{background:var(--bad);box-shadow:0 0 0 3px var(--bad-soft)}
.main-content{padding:38px 0 0}.page-heading{display:flex;align-items:end;justify-content:space-between;gap:20px;margin:0 2px 22px}.eyebrow,.meta-label{margin:0;color:var(--muted);font:10px/1.3 ui-monospace,SFMono-Regular,Menlo,monospace;letter-spacing:.1em;text-transform:uppercase}h1,h2,h3,p{margin-top:0}h1{margin-bottom:7px;font-size:clamp(26px,3vw,34px);font-weight:640;letter-spacing:-.045em;line-height:1.12;text-wrap:balance}h2{margin-bottom:4px;font-size:16px;font-weight:620;letter-spacing:-.025em}h3{margin-bottom:4px;font-size:14px;font-weight:610}.page-description{max-width:58ch;margin-bottom:0;color:var(--ink-2);font-size:13px}.surface{min-width:0;border-radius:16px;background:var(--panel);box-shadow:var(--lift)}.workspace-grid{display:grid;grid-template-columns:minmax(0,1.2fr) minmax(320px,.8fr);gap:16px;align-items:stretch}.live-panel,.connect-panel,.sessions-panel{padding:22px}.panel-heading{display:flex;align-items:flex-start;gap:12px}.panel-icon{display:grid;width:38px;height:38px;flex:0 0 auto;place-items:center;border-radius:11px;background:var(--accent-soft);color:var(--accent)}.panel-icon svg{width:18px;height:18px}.panel-copy{min-width:0;flex:1}.panel-copy p:last-child{margin-bottom:0;color:var(--muted);font-size:12.5px}.live-badge,.state-badge{display:inline-flex;align-items:center;gap:7px;white-space:nowrap;border-radius:7px;background:var(--good-soft);color:var(--good);padding:5px 8px;font-size:10px;font-weight:650;letter-spacing:.04em;text-transform:uppercase}.live-badge .status-dot,.state-badge .status-dot{width:6px;height:6px;background:currentColor;box-shadow:none}.action-row{display:flex;flex-wrap:wrap;gap:9px;margin-top:24px}.button{display:inline-flex;min-height:38px;align-items:center;justify-content:center;gap:8px;border:1px solid transparent;border-radius:9px;padding:0 14px;font-size:12.5px;font-weight:600;text-decoration:none;cursor:pointer;transition:transform .12s ease,background-color .15s ease,border-color .15s ease}.button:active{transform:translateY(1px)}.button-primary{background:var(--ink);color:var(--panel)}.button-primary:hover{opacity:.88}.button-secondary{border-color:var(--line);background:var(--panel);color:var(--ink-2);box-shadow:0 1px 2px rgba(0,0,0,.04)}.button-secondary:hover{background:var(--well);color:var(--ink)}.button-small{min-height:32px;padding-inline:10px;font-size:11.5px}.panel-footnote{margin:16px 0 0;color:var(--muted);font-size:11.5px}
.command-block{margin-top:15px}.command-label{display:flex;justify-content:space-between;align-items:center;gap:10px;margin-bottom:7px;color:var(--ink-2);font-size:11.5px;font-weight:600}.copy-line{display:flex;align-items:stretch;gap:7px}.code-well{min-width:0;flex:1;overflow:hidden;border-radius:9px;background:var(--well);box-shadow:var(--well-shadow)}.code-well code{display:block;overflow-wrap:anywhere;padding:11px 12px;color:var(--ink-2);font:11px/1.5 ui-monospace,SFMono-Regular,Menlo,monospace;white-space:pre-wrap}.copy-button{min-width:62px;min-height:38px;border:1px solid var(--line);border-radius:9px;background:var(--panel);color:var(--ink-2);font-size:11px;cursor:pointer}.copy-button:hover{background:var(--well);color:var(--ink)}.endpoint-line{display:flex;align-items:center;gap:8px;margin-top:12px;color:var(--muted);font-size:11.5px}.endpoint-line code{color:var(--ink-2);font:11px ui-monospace,SFMono-Regular,Menlo,monospace;overflow-wrap:anywhere}.privacy-note{margin:12px 0 0;color:var(--muted);font-size:11px}
.sessions-panel{margin-top:18px}.section-heading{display:flex;align-items:center;justify-content:space-between;gap:18px;margin-bottom:17px}.section-heading p{margin-bottom:0;color:var(--muted);font-size:12px}.session-total{font:11px ui-monospace,SFMono-Regular,Menlo,monospace;color:var(--muted);font-variant-numeric:tabular-nums;white-space:nowrap}.filters{display:flex;flex-wrap:wrap;gap:8px;align-items:center}.search-field,.filter-select{min-height:37px;border:1px solid var(--line);border-radius:9px;background:var(--panel);color:var(--ink);font-size:12px}.search-field{width:min(300px,100%);padding:0 11px}.filter-select{padding:0 30px 0 10px}.search-field::placeholder{color:var(--muted)}.search-field:focus,.filter-select:focus{outline:2px solid var(--accent);outline-offset:1px}.table-scroll{overflow-x:auto;overscroll-behavior-inline:contain;border-radius:11px;box-shadow:var(--well-shadow)}.session-table{width:100%;min-width:790px;border-collapse:separate;border-spacing:0;text-align:left}.session-table th{height:39px;padding:0 12px;border-bottom:1px solid var(--line);background:var(--well);color:var(--muted);font:10px ui-monospace,SFMono-Regular,Menlo,monospace;letter-spacing:.08em;text-transform:uppercase;white-space:nowrap}.session-table td{height:66px;padding:9px 12px;border-bottom:1px solid var(--line);color:var(--ink-2);font-size:12px;vertical-align:middle}.session-table tbody tr:last-child td{border-bottom:0}.session-table tbody tr:hover{background:color-mix(in srgb,var(--well) 60%,transparent)}.session-name{display:flex;min-width:180px;flex-direction:column;align-items:flex-start;gap:4px}.session-name strong{color:var(--ink);font-size:12.5px;font-weight:600}.session-name code{color:var(--muted);font:10px ui-monospace,SFMono-Regular,Menlo,monospace}.session-date,.session-number{font-variant-numeric:tabular-nums;white-space:nowrap}.session-number{text-align:right}.state-badge{padding:4px 7px;background:var(--sub);color:var(--muted);font-size:9px}.state-badge.active{background:var(--good-soft);color:var(--good)}.artifacts{display:flex;align-items:center;gap:7px;white-space:nowrap}.artifact-link{color:var(--accent);font-size:11.5px;font-weight:600;text-decoration:none}.artifact-link:hover{text-decoration:underline;text-underline-offset:3px}.artifact-note{color:var(--muted);font-size:10.5px}.table-message{min-height:22px;margin:10px 1px 0;color:var(--muted);font-size:11px}.empty-state{padding:42px 18px;text-align:center}.empty-state strong{display:block;margin-bottom:6px;color:var(--ink);font-size:14px}.empty-state p{max-width:44ch;margin:0 auto;color:var(--muted);font-size:12px}.pager{display:flex;align-items:center;justify-content:space-between;gap:12px;margin-top:14px}.pager-label{color:var(--muted);font:10.5px ui-monospace,SFMono-Regular,Menlo,monospace;font-variant-numeric:tabular-nums}.pager-actions{display:flex;gap:7px}.pager button{min-height:32px;border:1px solid var(--line);border-radius:8px;background:var(--panel);padding:0 10px;color:var(--ink-2);font-size:11px;cursor:pointer}.pager button:disabled{opacity:.42;cursor:default}
.token-gate{max-width:470px;margin:8px auto;padding:22px;border-radius:13px;background:var(--well);box-shadow:var(--well-shadow)}.token-gate p{margin-bottom:13px;color:var(--muted);font-size:12px}.token-form{display:flex;gap:8px}.token-form input{min-width:0;flex:1;min-height:38px;border:1px solid var(--line-strong);border-radius:8px;background:var(--panel);padding:0 10px}.token-message{min-height:19px;margin:8px 0 0!important;color:var(--bad)!important}.library-topline{display:flex;justify-content:flex-end;margin-bottom:10px}.sr-only{position:absolute;width:1px;height:1px;padding:0;margin:-1px;overflow:hidden;clip:rect(0,0,0,0);white-space:nowrap;border:0}.page-footer{display:flex;justify-content:space-between;gap:14px;margin:16px 2px 0;color:var(--muted);font-size:10.5px}
.detail-shell{width:min(1120px,100%)}.back-link{display:inline-flex;min-height:36px;align-items:center;gap:8px;color:var(--ink-2);font-size:12px;text-decoration:none}.back-link:hover{color:var(--accent)}.detail-heading{display:flex;align-items:flex-end;justify-content:space-between;gap:20px;margin:22px 2px 17px}.detail-id{margin:0;color:var(--muted);font:11px ui-monospace,SFMono-Regular,Menlo,monospace;overflow-wrap:anywhere}.detail-facts{display:flex;flex-wrap:wrap;gap:8px 18px;margin-top:12px;color:var(--muted);font-size:11.5px}.detail-facts span{font-variant-numeric:tabular-nums}.recording-player{margin-bottom:16px;padding:17px}.recording-player video{display:block;width:100%;max-height:68vh;border-radius:10px;background:#101110}.player-empty{display:grid;min-height:130px;place-items:center;border-radius:10px;background:var(--well);color:var(--muted);font-size:12px}.player-note{margin:10px 1px 0;color:var(--muted);font-size:11px}.events-panel{padding:20px}.event-list{display:grid;gap:8px;padding:0;margin:14px 0 0;list-style:none}.event-row{min-width:0;border-radius:10px;background:var(--well);box-shadow:var(--well-shadow);padding:12px}.event-heading{display:flex;flex-wrap:wrap;align-items:center;gap:8px 12px}.event-step{color:var(--muted);font:10px ui-monospace,SFMono-Regular,Menlo,monospace;font-variant-numeric:tabular-nums}.event-tool{color:var(--ink);font-size:12.5px;font-weight:620}.event-time,.event-duration{color:var(--muted);font-size:10.5px;font-variant-numeric:tabular-nums}.event-result{margin-left:auto;color:var(--good);font-size:10px}.event-row.failed{box-shadow:inset 2px 0 0 var(--bad),var(--well-shadow)}.event-row.failed .event-result,.event-error{color:var(--bad)}.event-error{margin:9px 0 0;font-size:11px;overflow-wrap:anywhere}.event-args{margin-top:10px;color:var(--muted);font-size:10.5px}.event-args summary{width:fit-content;cursor:pointer}.event-args pre{overflow:auto;max-height:260px;margin:8px 0 0;border-radius:8px;background:var(--panel);padding:10px;color:var(--ink-2);font:10.5px/1.55 ui-monospace,SFMono-Regular,Menlo,monospace;white-space:pre-wrap;overflow-wrap:anywhere}.event-frame{display:block;width:fit-content;max-width:100%;margin-top:10px;color:var(--accent);font-size:11px;text-decoration:none}.event-frame:hover{text-decoration:underline;text-underline-offset:3px}.event-frame img{display:block;width:min(100%,780px);max-height:460px;margin-top:7px;border:1px solid var(--line);border-radius:8px;object-fit:contain;object-position:left top}
@media(max-width:900px){.app-shell{padding:12px 14px 32px}.workspace-grid{grid-template-columns:1fr}.connect-panel{display:grid;grid-template-columns:minmax(0,1fr) minmax(0,1fr);gap:0 18px}.connect-panel .panel-heading{grid-column:1/-1;margin-bottom:5px}.command-block{min-width:0}}
@media(max-width:620px){.app-shell{padding:8px 9px 24px}.topbar{min-height:52px;gap:8px;padding:0 10px}.brand{gap:7px;font-size:13px}.brand-mark{width:25px;height:25px}.primary-nav{gap:0}.primary-nav a{min-height:34px;padding:0 7px;font-size:10.5px}.service-status{gap:6px;font-size:0}.service-status .status-dot{width:9px;height:9px}.main-content{padding-top:25px}.page-heading{align-items:flex-start;flex-direction:column;margin-bottom:16px}.live-panel,.connect-panel,.sessions-panel{padding:15px}.connect-panel{display:block}.section-heading{align-items:flex-start;flex-direction:column;gap:12px}.filters{width:100%}.search-field{width:100%;flex-basis:100%}.filter-select{flex:1}.sessions-panel{margin-top:12px}.detail-heading{align-items:flex-start;flex-direction:column}.recording-player,.events-panel{padding:13px}.event-result{margin-left:0}.page-footer{flex-direction:column}}
@media(max-width:620px){.primary-nav a:first-child{display:none}.live-badge{display:none}.endpoint-line{flex-wrap:wrap}.endpoint-line code{min-width:0;flex:1}.copy-line{min-width:0}}
@media(prefers-reduced-motion:reduce){*,*::before,*::after{scroll-behavior:auto!important;transition-duration:.01ms!important}}
"#;

/// Self-contained replay page. Frames load lazily so long sessions stay light.
pub fn replay_html(meta: &Meta, events: &[Event], video: Option<&Video>, query: &str) -> String {
    let rows = events
        .iter()
        .map(|event| {
            let args = if event.args.as_object().map_or(true, |object| object.is_empty()) {
                String::new()
            } else {
                format!(
                    r#"<details class="event-args"><summary>Show tool arguments</summary><pre>{}</pre></details>"#,
                    esc(&event.args.to_string())
                )
            };
            let error = event.error.as_deref().map_or_else(String::new, |message| {
                format!(r#"<p class="event-error">{}</p>"#, esc(message))
            });
            let frame = event.frame.as_deref().map_or_else(String::new, |file| {
                let href = format!("{}/frames/{}?{}", esc(&meta.id), esc(file), esc(query));
                format!(
                    r#"<a class="event-frame" href="{href}" target="_blank" rel="noopener">Open step screenshot<img loading="lazy" src="{href}" alt="Screen after step {step}"></a>"#,
                    step = event.step,
                )
            });
            format!(
                r#"<li class="event-row {class}"><div class="event-heading"><span class="event-step">STEP {step:04}</span><b class="event-tool">{tool}</b><time class="event-time">{time}</time><span class="event-duration">{duration} ms</span><span class="event-result">{result}</span></div>{error}{args}{frame}</li>"#,
                class = if event.ok { "" } else { "failed" },
                step = event.step,
                tool = esc(&event.tool),
                time = event.ts.format("%H:%M:%S"),
                duration = event.duration_ms,
                result = if event.ok { "Completed" } else { "Failed" },
            )
        })
        .collect::<String>();
    let video = video.map_or_else(
        || r#"<div class="player-empty">No video was captured for this session.</div>"#.into(),
        |video| {
            let note = if video.finished {
                "Screen video of the session."
            } else {
                "The video is still recording or could not be finalized. Refresh later."
            };
            format!(
                r#"<video controls preload="metadata" src="{id}/video/{file}?{query}"></video><p class="player-note">{note}</p>"#,
                id = esc(&meta.id),
                file = esc(video.file),
                query = esc(query),
            )
        },
    );
    let status = if meta.ended_at.is_some() { "Completed" } else { "Live" };
    let ended = meta
        .ended_at
        .map(|time| time.format("%b %-d, %Y · %H:%M UTC").to_string())
        .unwrap_or_else(|| "Still running".into());
    let rows = if rows.is_empty() {
        r#"<li class="empty-state"><strong>No steps recorded yet</strong><p>Refresh this page after the session has time to record its first action.</p></li>"#.to_string()
    } else {
        rows
    };

    format!(
        r#"<!doctype html><html lang="en"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"><meta name="color-scheme" content="light dark"><title>Session replay · {persona} · Taboom</title><style>{APP_CSS}</style></head><body>
<a class="skip-link" href="/#main">Skip to session details</a><div class="app-shell detail-shell">
<header class="topbar"><a class="brand" href="/"><span class="brand-mark" aria-hidden="true">T</span><span>Taboom</span></a><span class="topbar-spacer"></span><a class="back-link" href="/">← All sessions</a></header>
<main id="main"><section class="detail-heading"><div><p class="eyebrow">Session replay</p><h1>{persona}</h1><p class="detail-id">{id}</p><div class="detail-facts"><span>Agent {client}</span><span>Started {started}</span><span>Ended {ended}</span><span>{steps} steps</span></div></div><span class="state-badge {state}"><span class="status-dot" aria-hidden="true"></span>{status}</span></section>
<section class="surface recording-player" aria-labelledby="video-heading"><div class="section-heading"><div><p class="eyebrow">Recording</p><h2 id="video-heading">Screen video</h2></div></div>{video}</section>
<section class="surface events-panel" aria-labelledby="events-heading"><div class="section-heading"><div><p class="eyebrow">Timeline</p><h2 id="events-heading">Session steps</h2><p>Tool calls in chronological order. Expand a row to inspect its arguments.</p></div><span class="session-total">{steps} STEPS</span></div><ol class="event-list">{rows}</ol></section>
</main></div></body></html>"#,
        APP_CSS = APP_CSS,
        persona = esc(&meta.persona),
        id = esc(&meta.id),
        client = esc(&meta.client),
        started = meta.started_at.format("%b %-d, %Y · %H:%M UTC"),
        ended = esc(&ended),
        state = if meta.ended_at.is_some() { "" } else { "active" },
        status = status,
        steps = events.len(),
        video = video,
        rows = rows,
    )
}

/// The dashboard shell; session data is loaded from the token-protected JSON endpoint.
pub fn home_html(view: &str, takeover: &str, mcp: &str, auth_required: bool) -> String {
    let command = format!(
        "claude mcp add --transport http --scope user taboom {mcp}",
        mcp = esc(mcp)
    );
    HOME_HTML
        .replace("__APP_CSS__", APP_CSS)
        .replace("__VIEW_URL__", &esc(view))
        .replace("__TAKEOVER_URL__", &esc(takeover))
        .replace("__MCP_URL__", &esc(mcp))
        .replace("__CLAUDE_COMMAND__", &command)
        .replace("__AUTH_REQUIRED__", if auth_required { "true" } else { "false" })
        .replace("__GATE_STATE__", if auth_required { "" } else { " hidden" })
        .replace("__LIBRARY_STATE__", if auth_required { " hidden" } else { "" })
        .replace("__LOCK_STATE__", if auth_required { "" } else { " hidden" })
}

const HOME_HTML: &str = r#"<!doctype html><html lang="en"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"><meta name="color-scheme" content="light dark"><meta name="description" content="Your self-hosted Taboom browser desktop and session recordings."><title>Taboom workspace</title><style>__APP_CSS__</style></head><body data-auth-required="__AUTH_REQUIRED__">
<a class="skip-link" href="/#main">Skip to workspace</a><div class="app-shell">
<header class="topbar"><a class="brand" href="/"><span class="brand-mark" aria-hidden="true">T</span><span>Taboom</span></a><nav class="primary-nav" aria-label="Workspace"><a href="/#workspace" aria-current="page">Workspace</a><a href="/#sessions">Sessions</a><a href="/#connect">Connect</a></nav><span class="topbar-spacer"></span><div class="service-status" id="service-status" data-state="checking" role="status"><span class="status-dot" aria-hidden="true"></span><span id="service-status-text">Checking desktop</span></div></header>
<main id="main" class="main-content"><section id="workspace" class="page-heading"><div><p class="eyebrow">Self-hosted browser workspace</p><h1>Your browser desktop</h1><p class="page-description">Watch the desktop, connect an agent, and review what happened in each session.</p></div></section>
<div class="workspace-grid"><section class="surface live-panel" aria-labelledby="live-heading"><div class="panel-heading"><span class="panel-icon" aria-hidden="true"><svg viewBox="0 0 24 24" fill="none"><rect x="3.5" y="4.5" width="17" height="12" rx="2" stroke="currentColor" stroke-width="1.5"/><path d="M8 20h8M12 16.5V20" stroke="currentColor" stroke-width="1.5" stroke-linecap="round"/></svg></span><div class="panel-copy"><p class="meta-label">Live desktop</p><h2 id="live-heading">Your Taboom browser</h2><p>Watch the current screen or take over the mouse and keyboard.</p></div><span class="live-badge"><span class="status-dot" aria-hidden="true"></span>One persona</span></div><div class="action-row"><a class="button button-primary" href="__VIEW_URL__" target="_blank" rel="noopener">Watch live</a><a class="button button-secondary" href="__TAKEOVER_URL__" target="_blank" rel="noopener">Take over</a></div><p class="panel-footnote">Watch is view-only. Take over temporarily gives your browser control of the desktop.</p></section>
<section id="connect" class="surface connect-panel" aria-labelledby="connect-heading"><div class="panel-heading"><span class="panel-icon" aria-hidden="true"><svg viewBox="0 0 24 24" fill="none"><path d="M8 7.5 3.5 12 8 16.5M16 7.5l4.5 4.5-4.5 4.5M14 4l-4 16" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round"/></svg></span><div class="panel-copy"><p class="meta-label">MCP connection</p><h2 id="connect-heading">Connect an agent</h2><p>Use the HTTP endpoint with Claude Code or another MCP client.</p></div></div><div class="command-block"><div class="command-label"><span>Claude Code</span></div><div class="copy-line"><div class="code-well"><code id="claude-command">__CLAUDE_COMMAND__</code></div><button class="copy-button" type="button" data-copy-target="claude-command">Copy</button></div></div><div class="endpoint-line"><span>Endpoint</span><code id="mcp-endpoint">__MCP_URL__</code><button class="copy-button button-small" type="button" data-copy-target="mcp-endpoint">Copy URL</button></div><p class="privacy-note">When bearer tokens are enabled, include the token as an Authorization header in the MCP client.</p></section></div>
<section id="sessions" class="surface sessions-panel" aria-labelledby="sessions-heading"><div class="section-heading"><div><p class="eyebrow">Run history</p><h2 id="sessions-heading">Sessions</h2><p>Search by session ID, persona, or agent. Replay links expire after 24 hours.</p></div><span id="session-total" class="session-total" aria-live="polite">Loading sessions</span></div>
<div id="token-gate" class="token-gate"__GATE_STATE__><p class="meta-label">Private session library</p><h3>Unlock with an MCP token</h3><p>The token stays in this browser tab and is sent only to this Taboom server.</p><form id="token-form" class="token-form"><label class="sr-only" for="session-token">Bearer token</label><input id="session-token" type="password" autocomplete="off" spellcheck="false" placeholder="Paste bearer token" required><button id="unlock-button" class="button button-primary" type="submit">Unlock</button></form><p id="token-message" class="token-message" role="status"></p></div>
<div id="session-library"__LIBRARY_STATE__><div class="library-topline"><button id="lock-button" class="button button-secondary button-small" type="button"__LOCK_STATE__>Lock library</button></div><form id="session-filters" class="filters" role="search"><label class="sr-only" for="session-search">Search sessions</label><input class="search-field" id="session-search" type="search" placeholder="Search sessions…"><label class="sr-only" for="session-status">Filter by session status</label><select class="filter-select" id="session-status"><option value="all">All sessions</option><option value="active">Active</option><option value="completed">Completed</option></select><button class="button button-secondary button-small" type="reset">Clear filters</button></form><p id="table-message" class="table-message" role="status" aria-live="polite"></p><div id="table-scroll" class="table-scroll" tabindex="0" aria-label="Scrollable session table"><table class="session-table"><thead><tr><th scope="col">Session</th><th scope="col">Agent</th><th scope="col">Started</th><th scope="col">Duration</th><th scope="col" class="session-number">Steps</th><th scope="col">Artifacts</th></tr></thead><tbody id="session-rows"></tbody></table></div><div id="empty-state" class="empty-state" hidden><strong id="empty-title">No sessions yet</strong><p id="empty-description">Connect an agent and its runs will appear here.</p></div><nav class="pager" aria-label="Session pages"><span id="pager-label" class="pager-label">Page 1</span><div class="pager-actions"><button id="previous-page" type="button" disabled>Previous</button><button id="next-page" type="button" disabled>Next</button></div></nav></div>
<noscript><p class="table-message">Enable JavaScript to search and page through session recordings.</p></noscript></section>
<footer class="page-footer"><span>Taboom runs on your machine.</span><span>Session replays are signed links and can be opened by anyone who has the link.</span></footer></main></div>
<script>
(() => {
  const authRequired = document.body.dataset.authRequired === "true";
  const tokenKey = "taboom-session-token";
  const tokenGate = document.getElementById("token-gate");
  const tokenForm = document.getElementById("token-form");
  const tokenField = document.getElementById("session-token");
  const tokenMessage = document.getElementById("token-message");
  const library = document.getElementById("session-library");
  const rows = document.getElementById("session-rows");
  const tableScroll = document.getElementById("table-scroll");
  const tableMessage = document.getElementById("table-message");
  const totalLabel = document.getElementById("session-total");
  const searchField = document.getElementById("session-search");
  const statusField = document.getElementById("session-status");
  const pageLabel = document.getElementById("pager-label");
  const previousButton = document.getElementById("previous-page");
  const nextButton = document.getElementById("next-page");
  const emptyState = document.getElementById("empty-state");
  const emptyTitle = document.getElementById("empty-title");
  const emptyDescription = document.getElementById("empty-description");
  const lockButton = document.getElementById("lock-button");
  let token = authRequired ? sessionStorage.getItem(tokenKey) : "";
  let page = 1;
  let pageCount = 0;
  let request;
  let authEpoch = 0;
  let searchTimer;

  const setGate = (message = "") => {
    authEpoch += 1;
    if (request) request.abort();
    tokenGate.hidden = false;
    library.hidden = true;
    rows.replaceChildren();
    totalLabel.textContent = "Locked";
    tableMessage.textContent = "";
    tokenMessage.textContent = message;
  };

  const formatDate = (value) => {
    const date = new Date(value);
    return Number.isNaN(date.valueOf()) ? value : new Intl.DateTimeFormat(undefined, { dateStyle: "medium", timeStyle: "short" }).format(date);
  };

  const formatDuration = (started, ended) => {
    if (!ended) return "In progress";
    const seconds = Math.max(0, Math.floor((Date.parse(ended) - Date.parse(started)) / 1000));
    if (seconds < 60) return `${seconds}s`;
    if (seconds < 3600) return `${Math.floor(seconds / 60)}m ${seconds % 60}s`;
    const hours = Math.floor(seconds / 3600);
    const minutes = Math.floor((seconds % 3600) / 60);
    return hours < 24 ? `${hours}h ${minutes}m` : `${Math.floor(hours / 24)}d ${hours % 24}h`;
  };

  const makeLink = (label, href) => {
    if (!href) return null;
    let url;
    try { url = new URL(href, window.location.href); } catch { return null; }
    if (url.protocol !== "http:" && url.protocol !== "https:") return null;
    const link = document.createElement("a");
    link.className = "artifact-link";
    link.href = url.href;
    link.target = "_blank";
    link.rel = "noopener";
    link.textContent = label;
    return link;
  };

  const renderRows = (sessions) => {
    rows.replaceChildren();
    for (const session of sessions) {
      const active = !session.ended_at;
      const row = document.createElement("tr");
      const name = document.createElement("th");
      name.scope = "row";
      const nameStack = document.createElement("div");
      nameStack.className = "session-name";
      const persona = document.createElement("strong");
      persona.textContent = session.persona || "Taboom session";
      const state = document.createElement("span");
      state.className = active ? "state-badge active" : "state-badge";
      state.textContent = active ? "Live" : "Completed";
      const id = document.createElement("code");
      id.textContent = session.id;
      nameStack.append(persona, state, id);
      name.append(nameStack);

      const client = document.createElement("td");
      client.textContent = session.client || "Unknown agent";
      const started = document.createElement("td");
      started.className = "session-date";
      const time = document.createElement("time");
      time.dateTime = session.started_at;
      time.textContent = formatDate(session.started_at);
      started.append(time);
      const duration = document.createElement("td");
      duration.textContent = formatDuration(session.started_at, session.ended_at);
      const steps = document.createElement("td");
      steps.className = "session-number";
      steps.textContent = new Intl.NumberFormat().format(session.steps);

      const artifacts = document.createElement("td");
      const actions = document.createElement("div");
      actions.className = "artifacts";
      const replay = makeLink("Replay", session.replay_url);
      if (replay) actions.append(replay);
      if (session.video && session.video.finished) {
        const video = makeLink("Video", session.video_url);
        if (video) actions.append(video);
      } else if (session.video) {
        const note = document.createElement("span");
        note.className = "artifact-note";
        note.textContent = active ? "Recording" : "Finalizing";
        actions.append(note);
      } else {
        const note = document.createElement("span");
        note.className = "artifact-note";
        note.textContent = "Replay only";
        actions.append(note);
      }
      artifacts.append(actions);
      row.append(name, client, started, duration, steps, artifacts);
      rows.append(row);
    }
  };

  const loadSessions = async () => {
    if (request) request.abort();
    request = new AbortController();
    const epoch = authEpoch;
    const requestToken = token;
    const params = new URLSearchParams({ page: String(page), status: statusField.value });
    if (searchField.value.trim()) params.set("q", searchField.value.trim());
    const headers = new Headers();
    if (requestToken) headers.set("Authorization", `Bearer ${requestToken}`);
    tableMessage.textContent = "Loading sessions…";
    try {
      const response = await fetch(`/api/sessions?${params}`, { headers, cache: "no-store", signal: request.signal });
      if (response.status === 401 && authRequired) {
        token = "";
        sessionStorage.removeItem(tokenKey);
        setGate("That token was not accepted. Check it and try again.");
        return false;
      }
      if (!response.ok) throw new Error(`Session request failed (${response.status}).`);
      const data = await response.json();
      if (epoch !== authEpoch) return false;
      tokenGate.hidden = true;
      library.hidden = false;
      pageCount = data.pages;
      page = data.page;
      renderRows(data.sessions);
      totalLabel.textContent = `${new Intl.NumberFormat().format(data.total)} ${data.total === 1 ? "session" : "sessions"}`;
      const first = data.total ? (page - 1) * data.page_size + 1 : 0;
      const last = Math.min(page * data.page_size, data.total);
      pageLabel.textContent = data.total ? `Showing ${first}–${last} of ${data.total}` : "No results";
      previousButton.disabled = page <= 1;
      nextButton.disabled = pageCount === 0 || page >= pageCount;
      tableScroll.hidden = data.total === 0;
      emptyState.hidden = data.total !== 0;
      if (data.total === 0) {
        const filtered = searchField.value.trim() || statusField.value !== "all";
        emptyTitle.textContent = filtered ? "No matching sessions" : "No sessions yet";
        emptyDescription.textContent = filtered ? "Try another search or clear the filters." : "Connect an agent and its runs will appear here.";
      }
      tableMessage.textContent = "Newest sessions appear first.";
      return true;
    } catch (error) {
      if (error.name === "AbortError") return false;
      if (authRequired && !tokenGate.hidden) {
        tokenMessage.textContent = error.message || "Could not load sessions.";
      } else {
        tableMessage.textContent = error.message || "Could not load sessions.";
      }
      return false;
    }
  };

  const healthStatus = document.getElementById("service-status");
  const healthText = document.getElementById("service-status-text");
  fetch("/healthz", { cache: "no-store" }).then(async (response) => {
    const detail = await response.json();
    healthStatus.dataset.state = response.ok ? "ready" : "error";
    healthText.textContent = response.ok ? "Desktop ready" : "Desktop needs attention";
    if (!response.ok) healthStatus.title = Object.entries(detail).filter(([, value]) => value !== "ok").map(([name]) => name).join(", ");
  }).catch(() => {
    healthStatus.dataset.state = "error";
    healthText.textContent = "Health check unavailable";
  });

  tokenForm.addEventListener("submit", async (event) => {
    event.preventDefault();
    token = tokenField.value.trim();
    if (!token) return;
    authEpoch += 1;
    const accepted = await loadSessions();
    if (accepted) sessionStorage.setItem(tokenKey, token);
    tokenField.value = "";
  });
  lockButton.addEventListener("click", () => {
    sessionStorage.removeItem(tokenKey);
    token = "";
    tokenField.value = "";
    page = 1;
    setGate();
  });
  searchField.addEventListener("input", () => {
    page = 1;
    clearTimeout(searchTimer);
    searchTimer = setTimeout(loadSessions, 220);
  });
  statusField.addEventListener("change", () => { page = 1; loadSessions(); });
  document.getElementById("session-filters").addEventListener("reset", () => {
    setTimeout(() => { page = 1; loadSessions(); }, 0);
  });
  previousButton.addEventListener("click", () => { if (page > 1) { page -= 1; loadSessions(); } });
  nextButton.addEventListener("click", () => { if (page < pageCount) { page += 1; loadSessions(); } });
  document.querySelectorAll("[data-copy-target]").forEach((button) => {
    button.addEventListener("click", async () => {
      const content = document.getElementById(button.dataset.copyTarget)?.textContent?.trim();
      if (!content) return;
      try {
        await navigator.clipboard.writeText(content);
        button.textContent = "Copied";
        setTimeout(() => { button.textContent = button.dataset.copyTarget === "mcp-endpoint" ? "Copy URL" : "Copy"; }, 1400);
      } catch {
        button.textContent = "Select to copy";
      }
    });
  });

  if (authRequired && !token) setGate();
  else loadSessions();
})();
</script></body></html>"#;

pub fn esc(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn signed_urls_bind_recording_and_expiry() {
        let secret = b"test-secret-key-32bytes-long!!!!";
        let token = sign_url("recording-1", secret, 1_700_000_000);
        assert!(verify_token("recording-1", secret, 1_700_000_000, &token));
        assert!(!verify_token("recording-2", secret, 1_700_000_000, &token));
        assert!(!verify_token("recording-1", secret, 1_700_000_001, &token));
    }

    #[test]
    fn records_pages_and_shares() {
        let tmp = tempfile::tempdir().unwrap();
        let rec = Recorder::open(tmp.path(), "http://localhost:3456".into()).unwrap();
        let id = rec.start("agent", "default").unwrap();
        assert_eq!(rec.start("agent", "default").unwrap(), id, "start is idempotent per client");

        rec.record("agent", "screenshot", json!({}), None, 5, Frame::Bytes(vec![1, 2], "png"));
        let pending = rec.record("agent", "click", json!({"x": 1}), Some("boom"), 7, Frame::Pending("jpg"));
        assert!(pending.unwrap().ends_with("frames/00002.jpg"));
        assert!(rec.record("stranger", "click", json!({}), None, 1, Frame::None).is_none());

        let (page, total) = rec.events(&id, 2, 10).unwrap();
        assert_eq!(total, 2);
        assert_eq!(page.len(), 1);
        assert_eq!(page[0].error.as_deref(), Some("boom"));
        assert!(rec.frame_path(&id, "00001.png").unwrap().exists());

        rec.stop("agent");
        let list = rec.list(None, 10);
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].1, 2);
        assert!(list[0].0.ended_at.is_some());

        assert!(rec.video(&id).is_none());
        std::fs::write(rec.session_path(&id).unwrap().join("video.mkv"), b"x").unwrap();
        assert!(!rec.video(&id).unwrap().finished);
        let (url, video) = rec.share_url(&id, 60).unwrap();
        assert!(video.unwrap().contains("/video/video.mkv?exp="));
        let q: HashMap<_, _> = url.split_once('?').unwrap().1.split('&').filter_map(|p| p.split_once('=')).collect();
        assert!(rec.verify(&id, q["exp"].parse().unwrap(), q["sig"]));
        assert!(!rec.verify(&id, q["exp"].parse::<u64>().unwrap() + 1, q["sig"]));
        assert!(!rec.verify(&id, 1, q["sig"]), "expired links fail");
    }

    #[test]
    fn session_pages_filter_search_and_live_status() {
        let tmp = tempfile::tempdir().unwrap();
        let rec = Recorder::open(tmp.path(), "http://localhost:3456".into()).unwrap();
        let first = rec.start("agent-one", "work").unwrap();
        rec.stop("agent-one");
        let second = rec.start("agent-two", "shop").unwrap();
        rec.stop("agent-two");
        let active = rec.start("agent-three", "shop").unwrap();

        let (all, total) = rec.list_page(None, None, None, 0, 10);
        assert_eq!(total, 3);
        assert_eq!(all[0].0.id, active);
        assert_eq!(all[2].0.id, first);
        let (page, _) = rec.list_page(None, None, None, 1, 1);
        assert_eq!(page[0].0.id, second);

        let (shop, shop_total) = rec.list_page(None, Some("SHOP"), None, 0, 10);
        assert_eq!(shop_total, 2);
        assert!(shop.iter().all(|(meta, _)| meta.persona == "shop"));

        let (live, live_total) = rec.list_page(None, None, Some(true), 0, 10);
        assert_eq!(live_total, 1);
        assert_eq!(live[0].0.id, active);

        let (agent, agent_total) = rec.list_page(None, Some("AGENT-TWO"), None, 0, 10);
        assert_eq!(agent_total, 1);
        assert_eq!(agent[0].0.id, second);
    }

    #[test]
    fn retention_deletes_oldest_but_never_live() {
        let tmp = tempfile::tempdir().unwrap();
        let rec = Recorder::open(tmp.path(), "http://x".into()).unwrap();
        let mut ids = vec![];
        for client in ["a", "b", "c"] {
            let id = rec.start(client, "p").unwrap();
            std::fs::write(rec.session_path(&id).unwrap().join("video.mp4"), vec![0u8; 1000]).unwrap();
            ids.push(id);
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        rec.stop("b");
        rec.stop("c");
        rec.prune(2600, &[ids[0].as_str()]);
        assert!(rec.meta(&ids[0]).is_ok(), "live session kept even though it is oldest");
        assert!(rec.meta(&ids[1]).is_err(), "oldest finished session deleted");
        assert!(rec.meta(&ids[2]).is_ok(), "stops once under the limit");
    }

    #[test]
    fn rejects_path_tricks() {
        let tmp = tempfile::tempdir().unwrap();
        let rec = Recorder::open(tmp.path(), "http://x".into()).unwrap();
        assert!(rec.meta("../etc").is_err());
        assert!(rec.frame_path("abc", "../meta.json").is_err());
        assert!(rec.frame_path("abc", ".share-key").is_err());
    }

    #[test]
    fn replay_escapes_content() {
        let meta = Meta { id: "r1".into(), persona: "<p>".into(), client: "c".into(), started_at: Utc::now(), ended_at: None };
        let ev = Event { step: 1, ts: Utc::now(), tool: "type".into(), args: json!({"text": "<script>"}), ok: true, error: None, duration_ms: 1, frame: None };
        let html = replay_html(&meta, &[ev], None, "exp=1&sig=a");
        assert!(!html.contains("<script>"));
        assert!(html.contains("&lt;p&gt;"));
    }

    #[test]
    fn dashboard_shows_workspace_and_gates_private_sessions() {
        let public = home_html(
            "http://localhost:6080/vnc.html?view=1&resize=scale",
            "http://localhost:6080/vnc.html?takeover=1",
            "http://localhost:3456/mcp",
            false,
        );
        assert!(public.contains("Your browser desktop"));
        assert!(public.contains("Sessions"));
        assert!(public.contains("Copy URL"));
        assert!(public.contains("data-auth-required=\"false\""));
        assert!(public.contains("href=\"http://localhost:6080/vnc.html?view=1&amp;resize=scale\""));
        assert!(!public.contains("__APP_CSS__"));
        assert!(!public.contains("__AUTH_REQUIRED__"));

        let protected = home_html(
            "http://localhost:6080/watch",
            "http://localhost:6080/takeover",
            "http://localhost:3456/mcp",
            true,
        );
        assert!(protected.contains("data-auth-required=\"true\""));
        assert!(protected.contains("Unlock with an MCP token"));
        assert!(protected.contains("id=\"session-library\" hidden"));
        assert!(!protected.contains("Authorization: Bearer"));
    }
}
