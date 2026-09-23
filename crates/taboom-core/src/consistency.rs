use crate::persona::Persona;
use crate::hardware::device_memory_gb;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConsistencyReport {
    pub checks: Vec<ConsistencyCheck>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConsistencyCheck {
    pub name: String,
    pub passed: bool,
    pub detail: String,
}

impl ConsistencyReport {
    pub fn all_passed(&self) -> bool {
        self.checks.iter().all(|c| c.passed)
    }
}

/// What the runtime actually runs with: the browser process's environment, Chrome's language
/// preference from its profile, vinput's keyboard layout, sway's output, and the CPU/memory the
/// browser reports. For Fortress those two are the values on its running command line, because
/// Taboom does not attach CDP to pages.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Applied {
    pub timezone: Option<String>,
    pub locale: Option<String>,
    /// `LANGUAGE` inherited from the boot environment.
    pub language_environment: Option<String>,
    /// `intl.accept_languages` read from Chrome's active profile Preferences.
    pub chrome_accept_languages: Option<String>,
    pub keyboard_layout: Option<String>,
    /// Physical output mode and scale; None when sway is unreachable.
    pub screen: Option<(u32, u32, f64)>,
    pub hardware_concurrency: u32,
    pub device_memory: Option<f64>,
}

/// Declared vs. applied: a value the persona states but the runtime does not show is exactly
/// the mismatch a detector looks for.
pub fn check_applied(p: &Persona, a: &Applied) -> ConsistencyReport {
    let mut checks = vec![
        same("timezone", &p.timezone, a.timezone.as_deref()),
        same("locale", &p.identity.locale, a.locale.as_deref()),
        same(
            "language-environment",
            &p.language_environment(),
            a.language_environment.as_deref(),
        ),
        same("keyboard-layout", &p.identity.keyboard_layout, a.keyboard_layout.as_deref()),
    ];
    // The Preferences key belongs to Chrome. Keep this check scoped to its engine so a future
    // engine can supply its own truthful applied language source.
    if p.browser.engine == crate::persona::Engine::Chrome {
        checks.push(same(
            "chrome-accept-languages",
            &p.accept_languages(),
            a.chrome_accept_languages.as_deref(),
        ));
    }
    let s = &p.screen;
    checks.push(ConsistencyCheck {
        name: "screen".into(),
        passed: a.screen.is_some_and(|(w, h, scale)| w == s.width && h == s.height && (scale - s.scale).abs() < 0.01),
        detail: format!("declared {}x{}@{}, applied {:?}", s.width, s.height, s.scale, a.screen),
    });
    if let Some(cpus) = p.cpus {
        checks.push(ConsistencyCheck {
            name: "cpus".into(),
            passed: cpus == a.hardware_concurrency,
            detail: format!("declared {cpus}, the browser reports {}", a.hardware_concurrency),
        });
    }
    if let Some(ram_mb) = p.ram_mb {
        let expected = device_memory_gb(ram_mb);
        let passed = a.device_memory.is_some_and(|actual| (actual - expected).abs() < 0.01);
        checks.push(ConsistencyCheck {
            name: "ram-mb".into(),
            passed,
            detail: format!("declared {ram_mb} MB maps to {expected} GB, applied deviceMemory {:?}", a.device_memory),
        });
    }
    ConsistencyReport { checks }
}

fn same(name: &str, declared: &str, applied: Option<&str>) -> ConsistencyCheck {
    ConsistencyCheck {
        name: name.into(),
        passed: applied == Some(declared),
        detail: format!("declared {declared:?}, applied {applied:?}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn applied() -> Applied {
        Applied {
            timezone: Some("Europe/Berlin".into()),
            locale: Some("de_DE.UTF-8".into()),
            language_environment: Some("de_DE:de:en".into()),
            chrome_accept_languages: Some("de-DE,de,en".into()),
            keyboard_layout: Some("de".into()),
            screen: Some((1920, 1080, 1.0)),
        hardware_concurrency: 4,
        device_memory: Some(8.0),
        }
    }

    #[test]
    fn applied_runtime_matches() {
        let p = Persona::parse("name = \"a\"\ncountry = \"DE\"\ncpus = 4").unwrap();
        let report = check_applied(&p, &applied());
        assert!(report.all_passed(), "{:?}", report.checks);
    }

    #[test]
    fn each_drift_is_caught() {
        let p = Persona::parse("name = \"a\"\ncountry = \"DE\"\ncpus = 2").unwrap();
        let mut a = applied();
        a.timezone = Some("America/New_York".into());
        a.keyboard_layout = None;
        a.screen = Some((1920, 1080, 1.25));
        let failed: Vec<String> = check_applied(&p, &a)
            .checks
            .into_iter()
            .filter(|c| !c.passed)
            .map(|c| c.name)
            .collect();
        assert_eq!(failed, ["timezone", "keyboard-layout", "screen", "cpus"]);
    }

    #[test]
    fn language_environment_and_chrome_preferences_drift_are_caught() {
        let p = Persona::parse("name = \"a\"\ncountry = \"DE\"").unwrap();
        let mut a = applied();
        a.language_environment = Some("en_US:en".into());
        a.chrome_accept_languages = Some("en-US,en".into());
        let failed: Vec<String> = check_applied(&p, &a)
            .checks
            .into_iter()
            .filter(|c| !c.passed)
            .map(|c| c.name)
            .collect();
        assert_eq!(failed, ["language-environment", "chrome-accept-languages"]);
    }

    #[test]
    fn missing_language_environment_and_chrome_preferences_are_mismatches() {
        let p = Persona::parse("name = \"a\"\ncountry = \"DE\"").unwrap();
        let mut a = applied();
        a.language_environment = None;
        a.chrome_accept_languages = None;
        let failed: Vec<String> = check_applied(&p, &a).checks.into_iter().filter(|c| !c.passed).map(|c| c.name).collect();
        assert_eq!(failed, ["language-environment", "chrome-accept-languages"]);
    }

    #[test]
    fn chrome_preferences_check_is_scoped_to_the_chrome_engine() {
        let mut p = Persona::parse("name = \"a\"\ncountry = \"DE\"").unwrap();
        p.browser.engine = crate::persona::Engine::Fortress;
        let mut a = applied();
        a.chrome_accept_languages = None;

        let report = check_applied(&p, &a);
        assert!(report
            .checks
            .iter()
            .all(|check| check.name != "chrome-accept-languages"));
        assert!(report
            .checks
            .iter()
            .any(|check| check.name == "language-environment" && check.passed));
    }

    #[test]
    fn fortress_cpus_are_compared_with_its_running_override() {
        let mut p = Persona::parse("name = \"a\"\ncountry = \"DE\"\ncpus = 16").unwrap();
        p.browser.engine = crate::persona::Engine::Fortress;
        let mut a = applied();
        assert!(check_applied(&p, &a).checks.iter().any(|check| check.name == "cpus" && !check.passed));
        a.hardware_concurrency = 16;
        assert!(check_applied(&p, &a).all_passed());
    }

    #[test]
    fn declared_ram_is_checked_against_the_browser_memory_surface() {
        let p = Persona::parse("name = \"a\"\ncountry = \"DE\"\nram_mb = 4096").unwrap();
        let mut a = applied();
        let report = check_applied(&p, &a);
        let ram = report.checks.iter().find(|check| check.name == "ram-mb").unwrap();
        assert!(!ram.passed);
        a.device_memory = Some(4.0);
        assert!(check_applied(&p, &a).checks.iter().find(|check| check.name == "ram-mb").unwrap().passed);
    }
}
