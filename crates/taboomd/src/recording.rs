use crate::liveview;
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
        Ok(Self { dir, key, public_url, active: Mutex::new(HashMap::new()) })
    }

    pub fn start(&self, client: &str, persona: &str) -> Result<String> {
        let mut active = self.active.lock().unwrap();
        if let Some(a) = active.get(client) {
            return Ok(a.id.clone());
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
        let Ok(entries) = std::fs::read_dir(&self.dir) else {
            return vec![];
        };
        let mut metas: Vec<Meta> = entries
            .flatten()
            .filter_map(|e| self.meta(&e.file_name().to_string_lossy()).ok())
            .filter(|m| persona.map_or(true, |p| m.persona == p))
            .collect();
        metas.sort_by(|a, b| b.started_at.cmp(&a.started_at));
        metas
            .into_iter()
            .take(limit)
            .map(|m| {
                let steps = self.count_events(&m.id);
                (m, steps)
            })
            .collect()
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
        let sig = liveview::sign_url(id, &self.key, exp);
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
        exp >= unix_now() && liveview::verify_token(id, &self.key, exp, sig)
    }

    fn session_dir(&self, id: &str) -> Result<PathBuf> {
        if !safe_name(id) {
            bail!("bad recording id");
        }
        Ok(self.dir.join(id))
    }
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

fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Self-contained replay page. Frames load lazily so long sessions stay light.
pub fn replay_html(meta: &Meta, events: &[Event], video: Option<&Video>, query: &str) -> String {
    let mut rows = String::new();
    for e in events {
        let args = if e.args.as_object().map_or(true, |o| o.is_empty()) {
            String::new()
        } else {
            e.args.to_string()
        };
        let frame = match &e.frame {
            Some(f) => format!(
                r#"<a href="{id}/frames/{f}?{query}" target="_blank"><img loading="lazy" src="{id}/frames/{f}?{query}" alt="step {step}"></a>"#,
                id = esc(&meta.id), f = esc(f), query = esc(query), step = e.step
            ),
            None => String::new(),
        };
        let error = e.error.as_deref().map(|m| format!(r#"<p class="err">{}</p>"#, esc(m))).unwrap_or_default();
        rows.push_str(&format!(
            r#"<li class="{cls}"><div class="head"><span class="n">#{step}</span><b>{tool}</b><time>{ts}</time><span class="ms">{ms} ms</span></div><code>{args}</code>{error}{frame}</li>"#,
            cls = if e.ok { "ok" } else { "bad" },
            step = e.step,
            tool = esc(&e.tool),
            ts = e.ts.format("%H:%M:%S"),
            ms = e.duration_ms,
            args = esc(&args),
        ));
    }
    let video = match video {
        Some(v) => format!(
            r#"<video controls preload="metadata" src="{id}/video/{f}?{q}"></video><p class="sub">{note}</p>"#,
            id = esc(&meta.id),
            f = v.file,
            q = esc(query),
            note = if v.finished { "Screen video of the whole session." } else { "Video still recording or not finalized; reload later." },
        ),
        None => String::new(),
    };
    let ended = meta.ended_at.map(|t| t.format("%Y-%m-%d %H:%M:%S UTC").to_string()).unwrap_or_else(|| "still running".into());
    format!(
        r#"<!doctype html><html lang="en"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1">
<title>Taboom recording</title><style>
:root{{--bg:#f7f7f8;--card:#fff;--fg:#1b1b1f;--mute:#6b6b76;--line:#e3e3e8;--bad:#c62828}}
@media (prefers-color-scheme:dark){{:root{{--bg:#131316;--card:#1d1d22;--fg:#ececf1;--mute:#9a9aa6;--line:#2c2c33;--bad:#ff6b6b}}}}
*{{box-sizing:border-box}}body{{margin:0;background:var(--bg);color:var(--fg);font:15px/1.5 system-ui,sans-serif}}
main{{max-width:1100px;margin:0 auto;padding:24px 16px}}h1{{font-size:1.3rem;margin:0 0 4px}}.sub{{color:var(--mute);margin:0 0 20px;overflow-wrap:anywhere}}
ol{{list-style:none;padding:0;margin:0;display:grid;gap:12px}}li{{background:var(--card);border:1px solid var(--line);border-radius:10px;padding:12px;min-width:0}}
li.bad{{border-color:var(--bad)}}.head{{display:flex;flex-wrap:wrap;gap:8px 12px;align-items:baseline}}.n,time,.ms{{color:var(--mute);font-size:.85rem}}
code{{display:block;margin:6px 0;font-size:.85rem;color:var(--mute);overflow-wrap:anywhere;white-space:pre-wrap}}.err{{color:var(--bad);margin:4px 0;overflow-wrap:anywhere}}
img,video{{display:block;width:100%;height:auto;border-radius:6px;border:1px solid var(--line);margin-top:8px}}video{{margin:0 0 6px;background:#000}}
</style></head><body><main><h1>{persona}</h1><p class="sub">{id} &middot; started {start} &middot; ended {ended} &middot; {n} steps</p>{video}<ol>{rows}</ol></main></body></html>"#,
        persona = esc(&meta.persona),
        id = esc(&meta.id),
        start = meta.started_at.format("%Y-%m-%d %H:%M:%S UTC"),
        ended = esc(&ended),
        n = events.len(),
    )
}

/// The page at `/`. `recent` is ready-made HTML.
pub fn home_html(view: &str, takeover: &str, mcp: &str, recent: &str) -> String {
    format!(
        r#"<!doctype html><html lang="en"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1">
<title>Taboom</title><style>
:root{{--bg:#f7f7f8;--card:#fff;--fg:#1b1b1f;--mute:#6b6b76;--line:#e3e3e8;--acc:#3b5bdb}}
@media (prefers-color-scheme:dark){{:root{{--bg:#131316;--card:#1d1d22;--fg:#ececf1;--mute:#9a9aa6;--line:#2c2c33;--acc:#7b93ff}}}}
*{{box-sizing:border-box}}body{{margin:0;background:var(--bg);color:var(--fg);font:15px/1.5 system-ui,sans-serif}}
main{{max-width:760px;margin:0 auto;padding:28px 16px}}h1{{margin:0 0 4px;font-size:1.5rem}}h2{{font-size:1.05rem;margin:28px 0 8px}}
.sub{{color:var(--mute)}}a{{color:var(--acc)}}.btns{{display:flex;flex-wrap:wrap;gap:10px;margin-top:16px}}
.btn{{display:inline-block;padding:12px 18px;border-radius:10px;background:var(--acc);color:#fff;text-decoration:none;font-weight:600}}
.btn.alt{{background:var(--card);color:var(--fg);border:1px solid var(--line)}}
code,pre{{font:13px/1.5 ui-monospace,monospace;background:var(--card);border:1px solid var(--line);border-radius:8px}}
pre{{padding:12px;white-space:pre-wrap;overflow-wrap:anywhere}}code{{padding:1px 5px}}ul{{padding-left:18px}}li{{margin:6px 0;overflow-wrap:anywhere}}
</style></head><body><main>
<h1>Taboom</h1><p class="sub">A real desktop and browser for your AI agents.</p>
<div class="btns"><a class="btn" href="{view}" target="_blank">Watch live</a><a class="btn alt" href="{takeover}" target="_blank">Take over</a></div>
<h2>Connect an agent</h2><pre>claude mcp add --transport http --scope user taboom {mcp}</pre>
<p class="sub">Any MCP client: streamable HTTP at <code>{mcp}</code></p>
<h2>Recent sessions</h2>{recent}
</main></body></html>"#,
        view = esc(view),
        takeover = esc(takeover),
        mcp = esc(mcp),
    )
}

pub fn esc(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

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
}
