use crate::persona::Persona;
use crate::sites::{sites_for_country, SiteEntry};
use anyhow::{Context, Result};
use md5::{Digest, Md5};
use rand::seq::SliceRandom;
use rand::{Rng, SeedableRng};
use rusqlite::Connection;
use serde_json::{json, Value};
use std::path::Path;

const CHROME_EPOCH_OFFSET_US: i64 = 11_644_473_600_000_000;

fn chrome_time(unix_secs: i64) -> i64 {
    unix_secs * 1_000_000 + CHROME_EPOCH_OFFSET_US
}

pub fn seed_profile(default_dir: &Path, persona: &Persona) -> Result<()> {
    let mut rng = rand::rngs::StdRng::seed_from_u64(persona.seed());
    let sites = sites_for_country(&persona.country);
    let now_unix = 1_727_500_000_i64 + (persona.seed() % 86400) as i64;

    let history = seed_history(default_dir, &sites, &mut rng, now_unix)?;
    seed_bookmarks(default_dir, &sites, &mut rng, now_unix)?;
    seed_top_sites(default_dir, &history)?;
    Ok(())
}

pub fn enhance_preferences(prefs_path: &Path, persona: &Persona) -> Result<()> {
    let bytes = std::fs::read(prefs_path).with_context(|| format!("reading {}", prefs_path.display()))?;
    let mut prefs: Value = serde_json::from_slice(&bytes).with_context(|| format!("{} is not JSON", prefs_path.display()))?;
    let root = prefs.as_object_mut().context("Preferences is not a JSON object")?;

    root.entry("bookmark_bar")
        .or_insert_with(|| json!({}))
        .as_object_mut()
        .context("bookmark_bar is not an object")?
        .insert("show_on_all_tabs".into(), json!(true));

    root.entry("browser")
        .or_insert_with(|| json!({}))
        .as_object_mut()
        .context("browser is not an object")?
        .insert("has_seen_welcome_page".into(), json!(true));

    root.entry("profile")
        .or_insert_with(|| json!({}))
        .as_object_mut()
        .context("profile is not an object")?
        .insert("name".into(), json!(persona.name));

    root.entry("profile")
        .or_insert_with(|| json!({}))
        .as_object_mut()
        .context("profile is not an object")?
        .insert("avatar_index".into(), json!((persona.seed() % 28) as i64));

    let tmp = prefs_path.with_extension("taboom-tmp");
    std::fs::write(&tmp, serde_json::to_vec(&prefs)?)?;
    std::fs::rename(&tmp, prefs_path)?;
    Ok(())
}

struct HistoryEntry {
    url: String,
    title: String,
    visit_count: i64,
}

fn seed_history(
    default_dir: &Path,
    sites: &[&'static SiteEntry],
    rng: &mut rand::rngs::StdRng,
    now_unix: i64,
) -> Result<Vec<HistoryEntry>> {
    let path = default_dir.join("History");
    let conn = Connection::open(&path).with_context(|| format!("creating {}", path.display()))?;

    conn.execute_batch(
        "CREATE TABLE urls (
            id INTEGER PRIMARY KEY,
            url TEXT NOT NULL,
            title TEXT NOT NULL,
            visit_count INTEGER DEFAULT 0 NOT NULL,
            typed_count INTEGER DEFAULT 0 NOT NULL,
            last_visit_time INTEGER NOT NULL,
            hidden INTEGER DEFAULT 0 NOT NULL
        );
        CREATE TABLE visits (
            id INTEGER PRIMARY KEY,
            url INTEGER NOT NULL,
            visit_time INTEGER NOT NULL,
            from_visit INTEGER DEFAULT 0 NOT NULL,
            transition INTEGER DEFAULT 805306368 NOT NULL,
            visit_duration INTEGER DEFAULT 0 NOT NULL
        );"
    )?;

    let url_count = rng.gen_range(50..=200).min(sites.len());
    let mut chosen: Vec<&SiteEntry> = sites.choose_multiple(rng, url_count).copied().collect();
    chosen.shuffle(rng);

    let spread_secs = rng.gen_range(2 * 7 * 86400..=6 * 7 * 86400) as i64;
    let earliest_unix = now_unix - spread_secs;

    let mut entries = Vec::with_capacity(chosen.len());
    let mut visit_id = 1i64;

    let tx = conn.unchecked_transaction()?;
    for (idx, site) in chosen.iter().enumerate() {
        let url_id = (idx + 1) as i64;
        let max_visits = ((site.weight as f64 / 5.0).ceil() as i64).clamp(1, 15);
        let visit_count = rng.gen_range(1..=max_visits);
        let is_typed = rng.gen_ratio(1, 10);
        let typed_count = if is_typed { rng.gen_range(1..=visit_count) } else { 0 };

        let mut last_visit = 0i64;
        for _ in 0..visit_count {
            let visit_unix = rng.gen_range(earliest_unix..=now_unix);
            let visit_chrome = chrome_time(visit_unix);
            last_visit = last_visit.max(visit_chrome);
            let transition = if is_typed && rng.gen_bool(0.5) { 805306368 } else { 0 };
            let duration_us = rng.gen_range(5_000_000..=300_000_000_i64);

            tx.execute(
                "INSERT INTO visits (id, url, visit_time, from_visit, transition, visit_duration) VALUES (?1, ?2, ?3, 0, ?4, ?5)",
                rusqlite::params![visit_id, url_id, visit_chrome, transition, duration_us],
            )?;
            visit_id += 1;
        }

        tx.execute(
            "INSERT INTO urls (id, url, title, visit_count, typed_count, last_visit_time, hidden) VALUES (?1, ?2, ?3, ?4, ?5, ?6, 0)",
            rusqlite::params![url_id, site.url, site.title, visit_count, typed_count, last_visit],
        )?;

        entries.push(HistoryEntry {
            url: site.url.to_string(),
            title: site.title.to_string(),
            visit_count,
        });
    }
    tx.commit()?;
    Ok(entries)
}

fn seed_bookmarks(
    default_dir: &Path,
    sites: &[&'static SiteEntry],
    rng: &mut rand::rngs::StdRng,
    now_unix: i64,
) -> Result<()> {
    let bookmarkable: Vec<&SiteEntry> = sites.iter().copied().filter(|s| s.bookmarkable).collect();
    let count = rng.gen_range(10..=30).min(bookmarkable.len());
    let chosen: Vec<&&SiteEntry> = bookmarkable.choose_multiple(rng, count).collect();

    let folder_names = ["News", "Shopping", "Work", "Entertainment"];
    let folder_count = rng.gen_range(2..=4).min(folder_names.len());

    let mut id_counter = 4i64; // 1=bookmark_bar, 2=other, 3=synced
    let now_chrome = chrome_time(now_unix).to_string();
    let base_seed = rng.gen::<u64>();

    let mut folders: Vec<Value> = Vec::new();
    let chunks: Vec<Vec<&&SiteEntry>> = {
        let mut cs: Vec<Vec<&&SiteEntry>> = (0..folder_count).map(|_| Vec::new()).collect();
        for (i, site) in chosen.iter().enumerate() {
            cs[i % folder_count].push(site);
        }
        cs
    };

    for (fi, chunk) in chunks.iter().enumerate() {
        let folder_id = id_counter;
        id_counter += 1;

        let mut children = Vec::new();
        for (si, site) in chunk.iter().enumerate() {
            let child_id = id_counter;
            id_counter += 1;
            let guid = deterministic_guid(base_seed, folder_id as u64 * 1000 + si as u64);
            let added_unix = now_unix - rng.gen_range(1 * 86400..=30 * 86400) as i64;
            let added_chrome = chrome_time(added_unix).to_string();
            let used_unix = rng.gen_range(added_unix..=now_unix);
            let used_chrome = chrome_time(used_unix).to_string();

            children.push(json!({
                "date_added": added_chrome,
                "date_last_used": used_chrome,
                "guid": guid,
                "id": child_id.to_string(),
                "name": site.title,
                "type": "url",
                "url": site.url,
            }));
        }

        let folder_guid = deterministic_guid(base_seed, fi as u64);
        folders.push(json!({
            "children": children,
            "date_added": now_chrome,
            "date_last_used": "0",
            "date_modified": now_chrome,
            "guid": folder_guid,
            "id": folder_id.to_string(),
            "name": folder_names[fi],
            "type": "folder",
        }));
    }

    let bar_guid = deterministic_guid(base_seed, 10000);
    let other_guid = deterministic_guid(base_seed, 10001);
    let synced_guid = deterministic_guid(base_seed, 10002);

    let roots = json!({
        "bookmark_bar": {
            "children": folders,
            "date_added": now_chrome,
            "date_last_used": "0",
            "date_modified": now_chrome,
            "guid": bar_guid,
            "id": "1",
            "name": "Bookmarks bar",
            "type": "folder",
        },
        "other": {
            "children": [],
            "date_added": now_chrome,
            "date_last_used": "0",
            "date_modified": now_chrome,
            "guid": other_guid,
            "id": "2",
            "name": "Other bookmarks",
            "type": "folder",
        },
        "synced": {
            "children": [],
            "date_added": now_chrome,
            "date_last_used": "0",
            "date_modified": now_chrome,
            "guid": synced_guid,
            "id": "3",
            "name": "Mobile bookmarks",
            "type": "folder",
        },
    });

    let checksum = bookmarks_checksum(&roots);
    let bookmarks = json!({
        "checksum": checksum,
        "roots": roots,
        "version": 1,
    });

    let path = default_dir.join("Bookmarks");
    let tmp = path.with_extension("taboom-tmp");
    std::fs::write(&tmp, serde_json::to_string_pretty(&bookmarks)?)?;
    std::fs::rename(&tmp, &path)?;
    Ok(())
}

fn bookmarks_checksum(roots: &Value) -> String {
    let mut hasher = Md5::new();
    for key in &["bookmark_bar", "other", "synced"] {
        if let Some(folder) = roots.get(key) {
            checksum_node(&mut hasher, folder);
        }
    }
    format!("{:x}", hasher.finalize())
}

fn checksum_node(hasher: &mut Md5, node: &Value) {
    if let Some(id) = node.get("id").and_then(Value::as_str) {
        hasher.update(id.as_bytes());
    }
    if let Some(title) = node.get("name").and_then(Value::as_str) {
        hasher.update(title.as_bytes());
    }
    if let Some(url) = node.get("url").and_then(Value::as_str) {
        hasher.update(url.as_bytes());
    }
    if let Some(node_type) = node.get("type").and_then(Value::as_str) {
        hasher.update(node_type.as_bytes());
    }
    if let Some(children) = node.get("children").and_then(Value::as_array) {
        for child in children {
            checksum_node(hasher, child);
        }
    }
}

fn deterministic_guid(base_seed: u64, index: u64) -> String {
    let mut hasher = Md5::new();
    hasher.update(base_seed.to_le_bytes());
    hasher.update(index.to_le_bytes());
    let bytes = hasher.finalize();
    format!(
        "{:02x}{:02x}{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
        bytes[0], bytes[1], bytes[2], bytes[3],
        bytes[4], bytes[5],
        bytes[6], bytes[7],
        bytes[8], bytes[9],
        bytes[10], bytes[11], bytes[12], bytes[13], bytes[14], bytes[15],
    )
}

fn seed_top_sites(default_dir: &Path, history: &[HistoryEntry]) -> Result<()> {
    let path = default_dir.join("Top Sites");
    let conn = Connection::open(&path).with_context(|| format!("creating {}", path.display()))?;

    conn.execute_batch(
        "CREATE TABLE top_sites (
            url TEXT NOT NULL,
            url_rank INTEGER NOT NULL,
            title TEXT NOT NULL,
            redirects TEXT NOT NULL DEFAULT ''
        );"
    )?;

    let mut sorted: Vec<&HistoryEntry> = history.iter().collect();
    sorted.sort_by(|a, b| b.visit_count.cmp(&a.visit_count));
    let top_count = 10.min(sorted.len());

    let tx = conn.unchecked_transaction()?;
    for (rank, entry) in sorted[..top_count].iter().enumerate() {
        tx.execute(
            "INSERT INTO top_sites (url, url_rank, title, redirects) VALUES (?1, ?2, ?3, '')",
            rusqlite::params![entry.url, rank as i64, entry.title],
        )?;
    }
    tx.commit()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_persona(seed: u64) -> Persona {
        crate::persona::derive_persona("warmth-test", "US", seed).unwrap()
    }

    #[test]
    fn seed_profile_creates_all_files() {
        let dir = tempfile::tempdir().unwrap();
        let default = dir.path().join("Default");
        std::fs::create_dir_all(&default).unwrap();

        let persona = test_persona(42);
        seed_profile(&default, &persona).unwrap();

        assert!(default.join("History").exists());
        assert!(default.join("Bookmarks").exists());
        assert!(default.join("Top Sites").exists());
    }

    #[test]
    fn history_has_valid_rows() {
        let dir = tempfile::tempdir().unwrap();
        let default = dir.path().join("Default");
        std::fs::create_dir_all(&default).unwrap();

        let persona = test_persona(42);
        seed_profile(&default, &persona).unwrap();

        let conn = Connection::open(default.join("History")).unwrap();
        let url_count: i64 = conn.query_row("SELECT COUNT(*) FROM urls", [], |r| r.get(0)).unwrap();
        let site_count = crate::sites::sites_for_country("US").len() as i64;
        assert!(url_count >= 1 && url_count <= 200.min(site_count), "url_count={url_count}");

        let visit_count: i64 = conn.query_row("SELECT COUNT(*) FROM visits", [], |r| r.get(0)).unwrap();
        assert!(visit_count >= url_count, "each url should have at least one visit");

        let min_time: i64 = conn.query_row("SELECT MIN(visit_time) FROM visits", [], |r| r.get(0)).unwrap();
        assert!(min_time > CHROME_EPOCH_OFFSET_US, "timestamps should be post-epoch");
    }

    #[test]
    fn bookmarks_has_valid_structure() {
        let dir = tempfile::tempdir().unwrap();
        let default = dir.path().join("Default");
        std::fs::create_dir_all(&default).unwrap();

        let persona = test_persona(99);
        seed_profile(&default, &persona).unwrap();

        let raw = std::fs::read_to_string(default.join("Bookmarks")).unwrap();
        let bm: Value = serde_json::from_str(&raw).unwrap();

        assert_eq!(bm["version"], 1);
        assert!(bm["checksum"].as_str().is_some_and(|s| s.len() == 32));
        assert!(bm["roots"]["bookmark_bar"]["children"].as_array().is_some_and(|a| !a.is_empty()));
        assert_eq!(bm["roots"]["other"]["children"].as_array().unwrap().len(), 0);

        let recomputed = bookmarks_checksum(&bm["roots"]);
        assert_eq!(bm["checksum"].as_str().unwrap(), recomputed);
    }

    #[test]
    fn deterministic_output() {
        let run = |seed: u64| -> (Vec<u8>, String, Vec<u8>) {
            let dir = tempfile::tempdir().unwrap();
            let default = dir.path().join("Default");
            std::fs::create_dir_all(&default).unwrap();
            let persona = test_persona(seed);
            seed_profile(&default, &persona).unwrap();

            let history_dump = {
                let conn = Connection::open(default.join("History")).unwrap();
                let mut stmt = conn.prepare("SELECT id, url, title, visit_count, typed_count, last_visit_time FROM urls ORDER BY id").unwrap();
                let rows: Vec<String> = stmt.query_map([], |row| {
                    Ok(format!(
                        "{}|{}|{}|{}|{}|{}",
                        row.get::<_, i64>(0).unwrap(),
                        row.get::<_, String>(1).unwrap(),
                        row.get::<_, String>(2).unwrap(),
                        row.get::<_, i64>(3).unwrap(),
                        row.get::<_, i64>(4).unwrap(),
                        row.get::<_, i64>(5).unwrap(),
                    ))
                }).unwrap().map(|r| r.unwrap()).collect();
                rows.join("\n").into_bytes()
            };

            let bookmarks = std::fs::read_to_string(default.join("Bookmarks")).unwrap();

            let top_sites_dump = {
                let conn = Connection::open(default.join("Top Sites")).unwrap();
                let mut stmt = conn.prepare("SELECT url, url_rank, title FROM top_sites ORDER BY url_rank").unwrap();
                let rows: Vec<String> = stmt.query_map([], |row| {
                    Ok(format!(
                        "{}|{}|{}",
                        row.get::<_, String>(0).unwrap(),
                        row.get::<_, i64>(1).unwrap(),
                        row.get::<_, String>(2).unwrap(),
                    ))
                }).unwrap().map(|r| r.unwrap()).collect();
                rows.join("\n").into_bytes()
            };

            (history_dump, bookmarks, top_sites_dump)
        };

        let (h1, b1, t1) = run(12345);
        let (h2, b2, t2) = run(12345);
        assert_eq!(h1, h2, "history not deterministic");
        assert_eq!(b1, b2, "bookmarks not deterministic");
        assert_eq!(t1, t2, "top sites not deterministic");

        let (h3, _, _) = run(99999);
        assert_ne!(h1, h3, "different seeds should produce different output");
    }

    #[test]
    fn enhance_preferences_adds_keys() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("Preferences");
        std::fs::write(&path, r#"{"session":{"restore_on_startup":1},"intl":{"accept_languages":"en-US,en"}}"#).unwrap();

        let persona = test_persona(77);
        enhance_preferences(&path, &persona).unwrap();

        let prefs: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(prefs["bookmark_bar"]["show_on_all_tabs"], true);
        assert_eq!(prefs["browser"]["has_seen_welcome_page"], true);
        assert_eq!(prefs["profile"]["name"], "warmth-test");
        assert_eq!(prefs["profile"]["avatar_index"], (77u64 % 28) as i64);
        assert_eq!(prefs["session"]["restore_on_startup"], 1, "existing keys preserved");
        assert_eq!(prefs["intl"]["accept_languages"], "en-US,en", "existing keys preserved");
    }
}
