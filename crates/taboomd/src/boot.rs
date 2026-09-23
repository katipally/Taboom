use crate::browser::WEBRTC_PROXY_ONLY_FLAG;
use crate::network::GeoLookup;
use crate::persona::{self, Engine, Persona, Route};
use crate::route::{self, Upstream, FORWARDER_ADDR};
use anyhow::{bail, Context, Result};
use serde_json::{json, Value};
use std::path::Path;

/// `taboomd boot-check`: validates the persona, writes what applies it to the desktop and
/// Chrome, and verifies the route. Any error exits non-zero and Chrome never starts.
pub async fn boot_check(home: &Path, name: &str) -> Result<()> {
    let (persona, created) = persona::load_or_create(home, name)?;
    if created {
        println!("[taboom] first boot: wrote personas/{name}.toml (US, direct route); edit it and restart to change it");
    }
    persona.validate(&persona::probe_system()?)?;
    let upstream = Upstream::resolve(&persona, home)?;
    write_runtime(&persona, home)?;

    let health = route::check(&persona, upstream.as_ref(), &GeoLookup::open(home)).await;
    if !health.ok {
        bail!("route check failed ({} route): {}", health.route, health.detail);
    }
    let exit = health.exit.map_or("unverified".to_string(), |e| {
        format!("{} {} AS{}", e.ip, e.country.unwrap_or_default(), e.asn.unwrap_or_default())
    });
    println!(
        "[taboom] persona {name}: {} {} {} {}x{}@{} route {} exit {exit} ({})",
        persona.country, persona.timezone, persona.identity.locale,
        persona.screen.width, persona.screen.height, persona.screen.scale,
        persona.route.kind(), health.detail
    );
    Ok(())
}

/// run/env (sourced by the entrypoint and taboom-browser), run/sway.conf, run/fonts.conf and
/// the profile's Preferences.
fn write_runtime(p: &Persona, home: &Path) -> Result<()> {
    let run = home.join("run");
    std::fs::create_dir_all(&run)?;
    let profile = persona::profile_dir(home, &p.name)?;
    let fonts = run.join("fonts.conf");

    std::fs::write(run.join("env"), env_file(p, &profile, &fonts))?;
    std::fs::write(
        run.join("sway.conf"),
        format!(
            "output HEADLESS-1 mode {}x{} scale {}\ninput type:keyboard xkb_layout {}\n",
            p.screen.width, p.screen.height, p.screen.scale, p.identity.keyboard_layout
        ),
    )?;
    std::fs::write(&fonts, fontconfig(&p.identity.languages))?;
    write_preferences(&profile.join("Default").join("Preferences"), &p.accept_languages())
}

fn env_file(p: &Persona, profile: &Path, fonts: &Path) -> String {
    env_file_with_hardware(
        p,
        profile,
        fonts,
        crate::hardware::visible_cpus(),
        crate::hardware::mem_total_mb(),
    )
}

fn env_file_with_hardware(
    p: &Persona,
    profile: &Path,
    fonts: &Path,
    visible_cpus: u32,
    ram_total_mb: Option<u64>,
) -> String {
    let mut chrome_flags = vec![format!("--lang={}", p.identity.languages[0])];
    let mut fortress_flags = fortress_flags(p, visible_cpus, ram_total_mb);
    if !matches!(p.route, Route::Direct) {
        let proxy_flag = format!("--proxy-server=socks5://{FORWARDER_ADDR}");
        // no local DNS at all: every name goes to the proxy
        let resolver_flag = "--host-resolver-rules=MAP * ~NOTFOUND , EXCLUDE 127.0.0.1".to_string();
        chrome_flags.push(proxy_flag.clone());
        chrome_flags.push(resolver_flag.clone());
        fortress_flags.push(proxy_flag);
        fortress_flags.push(resolver_flag);
        // Chrome gets this from the managed policy; a Chromium fork may not read Chrome's path.
        fortress_flags.push(WEBRTC_PROXY_ONLY_FLAG.to_string());
    }
    let mut vars = vec![
        ("TABOOM_PERSONA", p.name.clone()),
        ("TABOOM_BROWSER_ENGINE", engine_name(p.browser.engine).into()),
        ("TZ", p.timezone.clone()),
        ("LANG", p.identity.locale.clone()),
        ("LANGUAGE", p.language_environment()),
        ("XKB_DEFAULT_LAYOUT", p.identity.keyboard_layout.clone()),
        ("FONTCONFIG_FILE", fonts.display().to_string()),
        ("TABOOM_PROFILE", profile.display().to_string()),
    ];
    if p.browser.engine == Engine::Fortress {
        // The upstream launcher normally replaces both values with its generic defaults. Keep
        // the persona's locale and Taboom's language-filtered font configuration in force.
        vars.push(("TILION_TZ", p.timezone.clone()));
        vars.push(("TILION_LANG", p.accept_languages()));
        vars.push(("TABOOM_FONTCONFIG_FILE", fonts.display().to_string()));
    }
    let mut out = String::from("# written by taboomd boot-check from the persona file; do not edit\n");
    for (k, v) in vars {
        out.push_str(&format!("export {k}={}\n", sh_quote(&v)));
    }
    out.push_str(&format!("TABOOM_CHROME_FLAGS={}\n", shell_array(&chrome_flags)));
    out.push_str(&format!("TABOOM_FORTRESS_FLAGS={}\n", shell_array(&fortress_flags)));
    out
}

fn engine_name(engine: Engine) -> &'static str {
    match engine {
        Engine::Chrome => "chrome",
        Engine::Fortress => "fortress",
    }
}

/// Only use the Fortress switches documented by its pinned release. In particular, the two
/// seed switches are distinct surfaces; there is no generic seed flag.
fn fortress_flags(p: &Persona, visible_cpus: u32, ram_total_mb: Option<u64>) -> Vec<String> {
    let mut flags = vec![
        format!("--lang={}", p.identity.languages[0]),
        format!("--accept-lang={}", p.accept_languages()),
        format!("--uxr-country={}", p.country),
        format!("--uxr-timezone={}", p.timezone),
        format!("--uxr-languages={}", p.accept_languages()),
        format!("--uxr-screen-width={}", p.screen.width),
        format!("--uxr-screen-height={}", p.screen.height),
        format!("--uxr-canvas-seed={}", p.seed()),
        format!("--uxr-audio-seed={}", p.seed()),
        format!(
            "--uxr-hw-concurrency={}",
            p.cpus.unwrap_or(visible_cpus)
        ),
    ];
    if let Some(ram_mb) = p.ram_mb.or(ram_total_mb) {
        let device_memory = crate::hardware::device_memory_gb(ram_mb);
        flags.push(format!("--uxr-device-memory={device_memory:.0}"));
    }
    flags
}

fn shell_array(flags: &[String]) -> String {
    let flags: Vec<String> = flags.iter().map(|f| sh_quote(f)).collect();
    format!("({})", flags.join(" "))
}

fn sh_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

/// Families a machine set up for these languages would have. The image carries more; the rest
/// are hidden so the font list stays coherent with the locale.
fn font_families(languages: &[String]) -> Vec<&'static str> {
    let mut families = vec![
        "Liberation Sans", "Liberation Serif", "Liberation Mono",
        "DejaVu Sans", "DejaVu Serif", "DejaVu Sans Mono",
        "Noto Sans", "Noto Serif", "Noto Sans Mono", "Noto Color Emoji",
    ];
    for lang in languages {
        let extra: &[&str] = match lang.split('-').next().unwrap_or_default() {
            "ja" => &["Noto Sans CJK JP", "Noto Serif CJK JP"],
            "zh" => &["Noto Sans CJK SC", "Noto Serif CJK SC", "Noto Sans CJK TC", "Noto Serif CJK TC"],
            "ko" => &["Noto Sans CJK KR", "Noto Serif CJK KR"],
            "ar" | "fa" | "ur" => &["Noto Sans Arabic", "Noto Naskh Arabic"],
            "he" => &["Noto Sans Hebrew", "Noto Serif Hebrew"],
            "hi" | "mr" | "ne" => &["Noto Sans Devanagari", "Noto Serif Devanagari"],
            "th" => &["Noto Sans Thai", "Noto Serif Thai"],
            _ => &[],
        };
        for f in extra {
            if !families.contains(f) {
                families.push(f);
            }
        }
    }
    families
}

fn fontconfig(languages: &[String]) -> String {
    let accept: String = font_families(languages)
        .iter()
        .map(|f| format!("      <pattern><patelt name=\"family\"><string>{f}</string></patelt></pattern>\n"))
        .collect();
    format!(
        r#"<?xml version="1.0"?>
<!DOCTYPE fontconfig SYSTEM "urn:fontconfig:fonts.dtd">
<!-- written by taboomd boot-check from the persona's languages; do not edit -->
<fontconfig>
  <include ignore_missing="no">/etc/fonts/fonts.conf</include>
  <selectfont>
    <acceptfont>
{accept}    </acceptfont>
    <rejectfont><glob>/*</glob></rejectfont>
  </selectfont>
</fontconfig>
"#
    )
}

/// Sets `intl.accept_languages` before Chrome starts; a new profile also restores its last
/// session on launch.
fn write_preferences(path: &Path, accept_languages: &str) -> Result<()> {
    let mut prefs: Value = match std::fs::read(path) {
        Ok(bytes) => serde_json::from_slice(&bytes).with_context(|| format!("{} is not JSON", path.display()))?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => json!({ "session": { "restore_on_startup": 1 } }),
        Err(e) => return Err(e).with_context(|| format!("reading {}", path.display())),
    };
    let root = prefs.as_object_mut().with_context(|| format!("{} is not a JSON object", path.display()))?;
    let intl = root.entry("intl").or_insert_with(|| json!({}));
    intl.as_object_mut().context("Preferences intl is not an object")?
        .insert("accept_languages".into(), accept_languages.into());
    std::fs::create_dir_all(path.parent().context("Preferences path has no folder")?)?;
    let tmp = path.with_extension("taboom-tmp");
    std::fs::write(&tmp, serde_json::to_vec(&prefs)?)?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shop() -> Persona {
        Persona::parse(
            "name = \"shop\"\ncountry = \"DE\"\n[screen]\nwidth = 2560\nheight = 1440\nscale = 1.25\n\
             [route]\ntype = \"socks5\"\nurl = \"h:1080\"",
        )
        .unwrap()
    }

    #[test]
    fn runtime_files_apply_every_field() {
        let home = tempfile::tempdir().unwrap();
        write_runtime(&shop(), home.path()).unwrap();
        let run = home.path().join("run");

        let env = std::fs::read_to_string(run.join("env")).unwrap();
        for line in [
            "export TZ='Europe/Berlin'",
            "export LANG='de_DE.UTF-8'",
            "export LANGUAGE='de_DE:de:en'",
            "export XKB_DEFAULT_LAYOUT='de'",
            "'--lang=de-DE' '--proxy-server=socks5://127.0.0.1:1080' '--host-resolver-rules=MAP * ~NOTFOUND , EXCLUDE 127.0.0.1'",
        ] {
            assert!(env.contains(line), "{line} missing from\n{env}");
        }
        let sway = std::fs::read_to_string(run.join("sway.conf")).unwrap();
        assert!(sway.contains("mode 2560x1440 scale 1.25") && sway.contains("xkb_layout de"), "{sway}");
        let fonts = std::fs::read_to_string(run.join("fonts.conf")).unwrap();
        assert!(fonts.contains("Liberation Sans") && !fonts.contains("CJK"), "{fonts}");

        let prefs: Value = serde_json::from_slice(&std::fs::read(home.path().join("profiles/shop/Default/Preferences")).unwrap()).unwrap();
        assert_eq!(prefs["intl"]["accept_languages"], "de-DE,de,en");
        assert_eq!(prefs["session"]["restore_on_startup"], 1);
    }

    #[test]
    fn preferences_keep_what_chrome_wrote() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("Preferences");
        std::fs::write(&path, r#"{"intl":{"accept_languages":"en-US,en","x":1},"session":{"restore_on_startup":4}}"#).unwrap();
        write_preferences(&path, "ja,en").unwrap();
        let prefs: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(prefs, json!({"intl": {"accept_languages": "ja,en", "x": 1}, "session": {"restore_on_startup": 4}}));
        std::fs::write(&path, "not json").unwrap();
        assert!(write_preferences(&path, "ja").is_err(), "a corrupt profile is not silently replaced");
    }

    #[test]
    fn direct_route_has_no_proxy_flags_and_quotes_safely() {
        let mut p = shop();
        p.route = Route::Direct;
        let env = env_file(&p, Path::new("/h/it's"), Path::new("/f"));
        assert!(!env.contains("proxy-server"));
        assert!(env.contains(r"export TABOOM_PROFILE='/h/it'\''s'"), "{env}");
    }

    #[test]
    fn fortress_runtime_maps_supported_fields_and_pins_canvas_audio_seeds() {
        let mut p = shop();
        p.browser.engine = Engine::Fortress;
        p.cpus = Some(6);
        p.humanizer.seed = Some(81723);
        let env = env_file_with_hardware(
            &p,
            Path::new("/profiles/shop"),
            Path::new("/run/fonts.conf"),
            8,
            Some(3500),
        );
        for flag in [
            "TABOOM_BROWSER_ENGINE='fortress'",
            "'--uxr-country=DE'",
            "'--uxr-timezone=Europe/Berlin'",
            "'--uxr-languages=de-DE,de,en'",
            "'--uxr-screen-width=2560'",
            "'--uxr-screen-height=1440'",
            "'--uxr-canvas-seed=81723'",
            "'--uxr-audio-seed=81723'",
            "'--uxr-hw-concurrency=6'",
            "'--uxr-device-memory=4'",
            "'--proxy-server=socks5://127.0.0.1:1080'",
            "'--force-webrtc-ip-handling-policy=disable_non_proxied_udp'",
        ] {
            assert!(env.contains(flag), "{flag} missing from\n{env}");
        }
        assert!(!env.contains("--user-agent"), "Fortress must keep UA and Client-Hints coherent");
        assert!(!env.contains("--uxr-seed"), "Fortress has separate canvas/audio seed switches");
        assert!(env.contains("export FONTCONFIG_FILE='/run/fonts.conf'"), "{env}");
        assert!(env.contains("export TZ='Europe/Berlin'"), "{env}");
        assert!(env.contains("export TILION_TZ='Europe/Berlin'"), "{env}");
        assert!(env.contains("export TILION_LANG='de-DE,de,en'"), "{env}");
        assert!(env.contains("export TABOOM_FONTCONFIG_FILE='/run/fonts.conf'"), "{env}");

        p.cpus = None;
        let env = env_file_with_hardware(
            &p,
            Path::new("/profiles/shop"),
            Path::new("/run/fonts.conf"),
            8,
            Some(3500),
        );
        assert!(env.contains("'--uxr-hw-concurrency=8'"), "default CPU count missing from\n{env}");
    }

    #[test]
    fn fonts_follow_every_language() {
        let langs = |l: &[&str]| l.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert!(font_families(&langs(&["ja", "en"])).contains(&"Noto Sans CJK JP"));
        assert!(font_families(&langs(&["en-IN", "en", "hi"])).contains(&"Noto Sans Devanagari"));
        assert!(!font_families(&langs(&["de-DE"])).iter().any(|f| f.contains("CJK")));
    }
}
