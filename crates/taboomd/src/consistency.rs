use crate::hardware::device_memory_bucket;
use crate::persona::PersonaConfig;
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

pub fn check_persona(persona: &PersonaConfig) -> ConsistencyReport {
    let mut checks = Vec::new();

    if let Some(ref hw) = persona.hardware {
        checks.push(check_screen_dimensions(hw.screen_width, hw.screen_height));
        checks.push(check_dpr(hw.dpr));
        checks.push(check_device_memory(persona.ram_mb, hw.dpr));
        checks.push(check_worker_cores(persona.cpus));
    }

    if let Some(ref id) = persona.identity {
        checks.push(check_languages_accept_language(
            &id.languages,
            &persona.browser.accept_languages,
        ));
    }

    checks.push(check_timezone_nonempty(&persona.timezone));

    ConsistencyReport { checks }
}

pub fn check_pair_distinct(a: &PersonaConfig, b: &PersonaConfig) -> ConsistencyCheck {
    let mut diffs = 0u32;

    if a.cpus != b.cpus {
        diffs += 1;
    }
    if a.ram_mb != b.ram_mb {
        diffs += 1;
    }

    let (a_w, a_h, a_dpr) = a
        .hardware
        .as_ref()
        .map(|h| (h.screen_width, h.screen_height, h.dpr))
        .unwrap_or((0, 0, 0.0));
    let (b_w, b_h, b_dpr) = b
        .hardware
        .as_ref()
        .map(|h| (h.screen_width, h.screen_height, h.dpr))
        .unwrap_or((0, 0, 0.0));

    if a_w != b_w || a_h != b_h {
        diffs += 1;
    }
    if (a_dpr - b_dpr).abs() > 0.01 {
        diffs += 1;
    }

    let passed = diffs >= 1;
    ConsistencyCheck {
        name: format!("pair-distinct({}, {})", a.name, b.name),
        passed,
        detail: if passed {
            format!("{diffs} hardware difference(s)")
        } else {
            "identical hardware fingerprints on same host".into()
        },
    }
}

fn check_screen_dimensions(w: u32, h: u32) -> ConsistencyCheck {
    let passed = w >= 800 && h >= 600 && w <= 7680 && h <= 4320;
    ConsistencyCheck {
        name: "screen-dimensions".into(),
        passed,
        detail: format!("{w}x{h}"),
    }
}

fn check_dpr(dpr: f64) -> ConsistencyCheck {
    let passed = (1.0..=3.0).contains(&dpr);
    ConsistencyCheck {
        name: "dpr-range".into(),
        passed,
        detail: format!("{dpr}"),
    }
}

fn check_device_memory(ram_mb: u32, _dpr: f64) -> ConsistencyCheck {
    let bucket = device_memory_bucket(ram_mb);
    let passed = bucket >= 2.0;
    ConsistencyCheck {
        name: "device-memory".into(),
        passed,
        detail: format!("ram={ram_mb}MB -> deviceMemory={bucket}"),
    }
}

fn check_worker_cores(cpus: u32) -> ConsistencyCheck {
    let passed = (1..=128).contains(&cpus);
    ConsistencyCheck {
        name: "worker-cores".into(),
        passed,
        detail: format!("hardwareConcurrency={cpus}"),
    }
}

fn check_languages_accept_language(languages: &[String], accept_lang: &str) -> ConsistencyCheck {
    if languages.is_empty() {
        return ConsistencyCheck {
            name: "languages-match".into(),
            passed: true,
            detail: "no languages configured, skipping".into(),
        };
    }
    let first = &languages[0];
    let passed = accept_lang.starts_with(first.as_str());
    ConsistencyCheck {
        name: "languages-match".into(),
        passed,
        detail: format!("primary={first}, Accept-Language starts with '{}'", accept_lang.split(',').next().unwrap_or("")),
    }
}

fn check_timezone_nonempty(tz: &str) -> ConsistencyCheck {
    let passed = !tz.is_empty() && tz.contains('/');
    ConsistencyCheck {
        name: "timezone-format".into(),
        passed,
        detail: tz.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::persona::{BrowserSettings, HardwareConfig, IdentityConfig};

    fn test_persona(name: &str, cpus: u32, ram: u32, w: u32, h: u32, dpr: f64) -> PersonaConfig {
        PersonaConfig {
            name: name.into(),
            cpus,
            ram_mb: ram,
            timezone: "America/New_York".into(),
            route: crate::persona::RouteConfig::Direct,
            browser: BrowserSettings::default(),
            hardware: Some(HardwareConfig {
                screen_width: w,
                screen_height: h,
                dpr,
            }),
            identity: Some(IdentityConfig {
                keyboard_layout: "us".into(),
                locale: "en_US.UTF-8".into(),
                languages: vec!["en-US".into(), "en".into()],
                fonts_packages: vec![],
            }),
            humanizer_seed: None,
            humanizer_style: None,
        }
    }

    #[test]
    fn consistent_persona_passes() {
        let p = test_persona("alice", 4, 8192, 1920, 1080, 1.0);
        let report = check_persona(&p);
        assert!(report.all_passed(), "failed: {:?}", report.checks);
    }

    #[test]
    fn bad_dpr_fails() {
        let p = test_persona("bob", 4, 8192, 1920, 1080, 5.0);
        let report = check_persona(&p);
        assert!(!report.all_passed());
    }

    #[test]
    fn pair_must_differ() {
        let a = test_persona("a", 4, 8192, 1920, 1080, 1.0);
        let b = test_persona("b", 4, 8192, 1920, 1080, 1.0);
        let check = check_pair_distinct(&a, &b);
        assert!(!check.passed);

        let c = test_persona("c", 8, 16384, 2560, 1440, 1.25);
        let check2 = check_pair_distinct(&a, &c);
        assert!(check2.passed);
    }
}
