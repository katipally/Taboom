pub use taboom_core::consistency::*;

use crate::hardware::{device_memory_gb, mem_total_mb, visible_cpus};
use crate::persona::{Engine, Persona};
use serde_json::Value;
use std::collections::HashMap;
use std::path::Path;

/// Observe what the running processes actually got: Chrome's own environment and flags, the
/// layout vinput built its keymap from, and sway's output. A value is None when its process is
/// not running, which the check reports as a mismatch rather than trusting the boot env file.
pub fn observe(screen: Option<(u32, u32, f64)>, persona: &Persona) -> Applied {
    let profile = std::env::var_os("TABOOM_PROFILE");
    let browser = profile.as_deref().map(Path::new).and_then(browser_process);
    let browser_env = |key: &str| browser.as_ref().and_then(|b| b.env.get(key).cloned());
    let fortress_flag = |name: &str| -> Option<u64> {
        let prefix = format!("--{name}=");
        browser.as_ref()?.args.iter().find_map(|a| a.strip_prefix(&prefix)?.parse().ok())
    };
    let fortress = persona.browser.engine == Engine::Fortress;
    Applied {
        timezone: browser_env("TZ"),
        locale: browser_env("LANG"),
        language_environment: browser_env("LANGUAGE"),
        chrome_accept_languages: observe_chrome_accept_languages(profile.as_deref().map(Path::new)),
        keyboard_layout: find_process(|args| args.first().is_some_and(|a| a.ends_with("vinput")))
            .and_then(|p| p.env.get("XKB_DEFAULT_LAYOUT").cloned()),
        screen,
        hardware_concurrency: match fortress {
            true => fortress_flag("uxr-hw-concurrency").map_or(0, |n| n as u32),
            false => visible_cpus(),
        },
        device_memory: match fortress {
            true => fortress_flag("uxr-device-memory").map(|gb| gb as f64),
            false => mem_total_mb().map(device_memory_gb),
        },
    }
}

struct Process {
    args: Vec<String>,
    env: HashMap<String, String>,
}

/// The browser's main process (renderers and helpers carry `--type=`), found by its profile.
fn browser_process(profile: &Path) -> Option<Process> {
    let flag = format!("--user-data-dir={}", profile.display());
    find_process(|args| args.contains(&flag) && !args.iter().any(|a| a.starts_with("--type=")))
}

/// First process whose argv matches. O(processes); only persona_status and startup call it.
fn find_process(matches: impl Fn(&[String]) -> bool) -> Option<Process> {
    let nul_split = |bytes: Vec<u8>| -> Vec<String> {
        bytes.split(|b| *b == 0).filter(|s| !s.is_empty()).map(|s| String::from_utf8_lossy(s).into_owned()).collect()
    };
    std::fs::read_dir("/proc").ok()?.flatten().find_map(|entry| {
        let args = nul_split(std::fs::read(entry.path().join("cmdline")).ok()?);
        if !matches(&args) {
            return None;
        }
        let env = nul_split(std::fs::read(entry.path().join("environ")).ok()?)
            .into_iter()
            .filter_map(|kv| kv.split_once('=').map(|(k, v)| (k.to_string(), v.to_string())))
            .collect();
        Some(Process { args, env })
    })
}

/// True once the browser for this container's profile is running.
pub fn browser_running() -> bool {
    std::env::var_os("TABOOM_PROFILE").is_some_and(|p| browser_process(Path::new(&p)).is_some())
}

/// Read the value Chrome will load from its active profile. Missing paths, malformed JSON, and
/// absent or mistyped keys all return None so consistency checking reports them as mismatches.
fn observe_chrome_accept_languages(profile: Option<&Path>) -> Option<String> {
    let profile = profile?;
    let path = profile.join("Default").join("Preferences");
    let bytes = std::fs::read(path).ok()?;
    let preferences: Value = serde_json::from_slice(&bytes).ok()?;
    preferences.get("intl")?.get("accept_languages")?.as_str().map(str::to_owned)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn observes_chrome_accept_languages_from_active_profile_preferences() {
        let profile = tempfile::tempdir().unwrap();
        let preferences = profile.path().join("Default/Preferences");
        std::fs::create_dir_all(preferences.parent().unwrap()).unwrap();
        std::fs::write(&preferences, json!({"intl":{"accept_languages":"de-DE,de,en"}}).to_string()).unwrap();

        assert_eq!(
            observe_chrome_accept_languages(Some(profile.path())).as_deref(),
            Some("de-DE,de,en")
        );
    }

    #[test]
    fn missing_or_invalid_chrome_preferences_have_no_applied_value() {
        let profile = tempfile::tempdir().unwrap();
        assert_eq!(observe_chrome_accept_languages(Some(profile.path())), None);
        let preferences = profile.path().join("Default/Preferences");
        std::fs::create_dir_all(preferences.parent().unwrap()).unwrap();
        std::fs::write(&preferences, "not json").unwrap();
        assert_eq!(observe_chrome_accept_languages(Some(profile.path())), None);
        assert_eq!(observe_chrome_accept_languages(None), None);
    }
}
