use crate::hardware::device_memory_gb;
use crate::timezone_matches_country;
use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

/// The container's one identity: `personas/<name>.toml`, picked by TABOOM_PERSONA at boot.
/// Every field is applied to the runtime by `boot` or verified against it; none is metadata.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Persona {
    pub name: String,
    /// ISO 3166 alpha-2. Fills unset timezone and identity fields, and is the exit country the
    /// route must show.
    pub country: String,
    pub timezone: String,
    /// Logical CPUs Chrome sees (set with Compose `cpuset`); Fortress may map this to its
    /// documented hardware-concurrency override.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cpus: Option<u32>,
    /// Physical memory in MB used for Fortress's device-memory override. Chrome cannot change
    /// this surface; when declared there, it must match Chrome's rounded host-reported value.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ram_mb: Option<u64>,
    pub identity: Identity,
    #[serde(default)]
    pub screen: ScreenSpec,
    #[serde(default)]
    pub browser: Browser,
    #[serde(default)]
    pub route: Route,
    #[serde(default)]
    pub humanizer: Humanizer,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Identity {
    pub locale: String,
    pub languages: Vec<String>,
    pub keyboard_layout: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ScreenSpec {
    pub width: u32,
    pub height: u32,
    #[serde(default = "unit_scale")]
    pub scale: f64,
}

fn unit_scale() -> f64 {
    1.0
}

impl Default for ScreenSpec {
    fn default() -> Self {
        Self { width: 1920, height: 1080, scale: 1.0 }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Browser {
    #[serde(default)]
    pub engine: Engine,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Engine {
    #[default]
    Chrome,
    Fortress,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum Route {
    #[default]
    Direct,
    Socks5(Proxy),
    Http(Proxy),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Proxy {
    /// `host:port`, optionally with the scheme matching `type`. Never with credentials.
    pub url: String,
    /// `vault:<secret name>`; the secret's value is `user:password`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auth: Option<String>,
    /// Accept an exit IP in a known datacenter ASN (refused by default).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub allow_datacenter: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Humanizer {
    /// Stable motor habits. Unset: derived from the name, so they never change between boots.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seed: Option<u64>,
}

/// Shared humanizer speed class for runtime configuration and the motion model.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum SpeedClass {
    Slow,
    Medium,
    Fast,
}

/// Host facts collected by the daemon or CLI adapter and passed to the shared validator.
pub struct System {
    pub zone_tab: String,
    /// Layout names discovered by the host adapter, or None when the table is unavailable.
    pub keyboard_layouts: Option<HashSet<String>>,
    pub cpus: u32,
    /// Physical memory visible to the container, when the host adapter can read it.
    pub ram_mb: Option<u64>,
    /// Runtime architecture, for engines that ship on a subset of Linux targets.
    pub architecture: String,
    /// Architecture of the Docker host, forwarded by Compose to catch emulated x86 images.
    pub host_architecture: String,
    /// Fortress is an opt-in image component, so architecture support alone is insufficient.
    pub fortress_available: bool,
}

/// Docker writes this manifest at image build time to tell persona validation which engines
/// were included in the image. The default image only contains Chrome.
pub const ENGINE_MANIFEST: &str = "/usr/local/share/taboom/engines";

pub fn fortress_is_installed() -> bool {
    std::fs::read_to_string(ENGINE_MANIFEST)
        .is_ok_and(|engines| engines.lines().any(|engine| engine == "fortress"))
}

impl Persona {
    /// Parses a persona file, filling unset timezone and identity fields from `country`.
    pub fn parse(text: &str) -> Result<Self> {
        let mut table: toml::Table = text.parse()?;
        let country = table
            .get("country")
            .and_then(|v| v.as_str())
            .context("`country` is required, an ISO 3166 code such as \"DE\"")?
            .to_string();
        if let Some(d) = country_defaults(&country) {
            table.entry("timezone").or_insert(d.timezone.into());
            let identity = table
                .entry("identity")
                .or_insert(toml::Table::new().into())
                .as_table_mut()
                .context("[identity] must be a table")?;
            identity.entry("locale").or_insert(d.locale.into());
            identity.entry("languages").or_insert(d.languages.to_vec().into());
            identity.entry("keyboard_layout").or_insert(d.keyboard_layout.into());
        }
        toml::Value::Table(table).try_into().with_context(|| {
            format!("persona fields are invalid (country {country} has no built-in defaults, so set timezone and [identity] in full)")
        })
    }

    pub fn seed(&self) -> u64 {
        // FNV-1a: std's hasher is not stable across releases, and habits must survive upgrades
        self.humanizer.seed.unwrap_or_else(|| {
            self.name.bytes().fold(0xcbf29ce484222325, |h, b| (h ^ b as u64).wrapping_mul(0x100000001b3))
        })
    }

    /// Accept-Language as Chrome sends it.
    pub fn accept_languages(&self) -> String {
        self.identity.languages.join(",")
    }

    /// Language preferences exported to Unix programs at boot.
    pub fn language_environment(&self) -> String {
        self.identity
            .languages
            .iter()
            .map(|language| language.replace('-', "_"))
            .collect::<Vec<_>>()
            .join(":")
    }

    /// Every problem at once, so one boot shows the whole fix list.
    pub fn validate(&self, sys: &System) -> Result<()> {
        let mut problems = Vec::new();
        let mut check = |ok: bool, msg: String| {
            if !ok {
                problems.push(msg);
            }
        };
        check(check_name(&self.name).is_ok(), format!("name {:?} must be 1-64 of A-Z a-z 0-9 _ -", self.name));
        let cc = self.country.as_str();
        check(cc.len() == 2 && cc.bytes().all(|b| b.is_ascii_uppercase()), format!("country {cc:?} must be an ISO 3166 code like \"DE\""));
        check(
            timezone_matches_country(&sys.zone_tab, &self.timezone, cc),
            format!("timezone {:?} is not a zone of country {cc} (see /usr/share/zoneinfo/zone.tab)", self.timezone),
        );

        let id = &self.identity;
        let (locale_lang, locale_region) = id
            .locale
            .strip_suffix(".UTF-8")
            .and_then(|l| l.split_once('_'))
            .unwrap_or_default();
        check(
            locale_lang.len() >= 2 && locale_region == cc,
            format!("locale {:?} must look like ll_{cc}.UTF-8 for country {cc}", id.locale),
        );
        check(!id.languages.is_empty(), "identity.languages must not be empty".into());
        for lang in &id.languages {
            let ok = lang.split('-').enumerate().all(|(i, part)| {
                (if i == 0 { 2..=3 } else { 2..=8 }).contains(&part.len()) && part.bytes().all(|b| b.is_ascii_alphanumeric())
            });
            check(ok, format!("language {lang:?} is not a tag like de-DE"));
        }
        if let Some(first) = id.languages.first() {
            let mut parts = first.split('-');
            check(
                parts.next() == Some(locale_lang) && parts.all(|p| p.len() != 2 || p == cc),
                format!("first language {first:?} must be {locale_lang} or {locale_lang}-{cc}, matching locale and country"),
            );
        }
        let layout_ok = !id.keyboard_layout.is_empty()
            && id.keyboard_layout.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_');
        check(
            layout_ok && sys.keyboard_layouts.as_ref().map_or(true, |layouts| layouts.contains(&id.keyboard_layout)),
            format!("keyboard_layout {:?} is not an XKB layout", id.keyboard_layout),
        );

        let s = &self.screen;
        check(
            (800..=7680).contains(&s.width) && (600..=4320).contains(&s.height) && (1.0..=3.0).contains(&s.scale),
            format!("screen {}x{} scale {} is out of range (800-7680 x 600-4320, scale 1-3)", s.width, s.height, s.scale),
        );
        if self.browser.engine == Engine::Fortress {
            check(
                sys.architecture == "x86_64",
                format!("Fortress requires a Linux x86_64 runtime; this runtime architecture is {}", sys.architecture),
            );
            check(
                sys.host_architecture == "x86_64",
                format!("Fortress requires an x86_64 Docker host; detected host architecture is {} (set TABOOM_HOST_ARCH when using a remote Docker host)", sys.host_architecture),
            );
            check(
                sys.fortress_available,
                "Fortress is not included in this image; rebuild with TABOOM_ENGINES=chrome,fortress".into(),
            );
        }
        if let Some(cpus) = self.cpus {
            if self.browser.engine == Engine::Chrome {
                check(
                    cpus == sys.cpus,
                    format!("cpus = {cpus} but Chrome would see {}; set Compose `cpuset` to {cpus} CPUs or remove cpus", sys.cpus),
                );
            } else {
                check(cpus > 0, "cpus must be greater than zero for Fortress".into());
            }
        }
        if let Some(ram_mb) = self.ram_mb {
            check(ram_mb > 0, "ram_mb must be greater than zero".into());
            if self.browser.engine == Engine::Chrome {
                match sys.ram_mb {
                    Some(actual_mb) => {
                        let declared = device_memory_gb(ram_mb);
                        let actual = device_memory_gb(actual_mb);
                        check(
                            declared == actual,
                            format!("ram_mb is read-only for Chrome: {ram_mb} MB maps to {declared} GB but the host reports {actual} GB; use a matching value or remove ram_mb"),
                        );
                    }
                    None => check(
                        false,
                        "ram_mb cannot be checked because host physical memory is unavailable".into(),
                    ),
                }
            }
        }
        if let Route::Socks5(p) | Route::Http(p) = &self.route {
            if let Err(e) = self.route.endpoint() {
                check(false, format!("{e:#}"));
            }
            if let Some(auth) = &p.auth {
                check(
                    auth.strip_prefix("vault:").is_some_and(|n| !n.is_empty()),
                    format!("route.auth {auth:?} must be vault:<secret name>"),
                );
            }
        }

        if !problems.is_empty() {
            bail!("persona {:?} is invalid:\n  - {}", self.name, problems.join("\n  - "));
        }
        Ok(())
    }
}

impl Route {
    pub fn kind(&self) -> &'static str {
        match self {
            Route::Direct => "direct",
            Route::Socks5(_) => "socks5",
            Route::Http(_) => "http",
        }
    }

    pub fn proxy(&self) -> Option<&Proxy> {
        match self {
            Route::Direct => None,
            Route::Socks5(p) | Route::Http(p) => Some(p),
        }
    }

    /// The upstream proxy's host and port.
    pub fn endpoint(&self) -> Result<(String, u16)> {
        let Some(p) = self.proxy() else { bail!("a direct route has no proxy") };
        let rest = match p.url.split_once("://") {
            Some((scheme, rest)) if scheme == self.kind() => rest,
            Some((scheme, _)) => bail!("route.url scheme {scheme} does not match type = {:?}", self.kind()),
            None => p.url.as_str(),
        };
        if rest.contains('@') {
            bail!("route.url must not carry credentials; put them in the vault and set auth = \"vault:<name>\"");
        }
        let (host, port) = rest
            .trim_end_matches('/')
            .rsplit_once(':')
            .context("route.url must be host:port")?;
        let host = host.trim_start_matches('[').trim_end_matches(']');
        let port = port.parse().ok().filter(|p| *p > 0).context("route.url port must be 1-65535")?;
        if host.is_empty() {
            bail!("route.url host is empty");
        }
        Ok((host.to_string(), port))
    }
}

struct CountryDefaults {
    timezone: &'static str,
    locale: &'static str,
    languages: &'static [&'static str],
    keyboard_layout: &'static str,
}

fn country_defaults(country: &str) -> Option<CountryDefaults> {
    let (timezone, locale, languages, keyboard_layout): (_, _, &'static [&'static str], _) = match country {
        "US" => ("America/New_York", "en_US.UTF-8", &["en-US", "en"], "us"),
        "GB" => ("Europe/London", "en_GB.UTF-8", &["en-GB", "en"], "gb"),
        "DE" => ("Europe/Berlin", "de_DE.UTF-8", &["de-DE", "de", "en"], "de"),
        "FR" => ("Europe/Paris", "fr_FR.UTF-8", &["fr-FR", "fr", "en"], "fr"),
        "JP" => ("Asia/Tokyo", "ja_JP.UTF-8", &["ja", "en"], "jp"),
        "BR" => ("America/Sao_Paulo", "pt_BR.UTF-8", &["pt-BR", "pt", "en"], "br"),
        "IN" => ("Asia/Kolkata", "en_IN.UTF-8", &["en-IN", "en", "hi"], "us"),
        _ => return None,
    };
    Some(CountryDefaults { timezone, locale, languages, keyboard_layout })
}

/// A complete persona for `country`: its defaults plus a screen drawn from common hardware.
pub fn derive_persona(name: &str, country: &str, seed: u64) -> Result<Persona> {
    use rand::SeedableRng;
    let d = country_defaults(country).with_context(|| format!("no built-in defaults for country {country}"))?;
    let hw = crate::hardware::draw_hardware(&mut rand::rngs::StdRng::seed_from_u64(seed));
    Ok(Persona {
        name: name.into(),
        country: country.into(),
        timezone: d.timezone.into(),
        cpus: None,
        ram_mb: None,
        identity: Identity {
            locale: d.locale.into(),
            languages: d.languages.iter().map(|s| s.to_string()).collect(),
            keyboard_layout: d.keyboard_layout.into(),
        },
        screen: ScreenSpec { width: hw.screen_width, height: hw.screen_height, scale: hw.dpr },
        browser: Browser::default(),
        route: Route::Direct,
        humanizer: Humanizer { seed: Some(seed) },
    })
}

/// Persona names become file and folder names, so only [A-Za-z0-9_-] is allowed.
pub fn check_name(name: &str) -> Result<()> {
    let ok = (1..=64).contains(&name.len())
        && name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-');
    if !ok {
        bail!("persona name must be 1-64 characters of A-Z, a-z, 0-9, _ or -, got {name:?}");
    }
    Ok(())
}

#[cfg(test)]
pub mod tests {
    use super::*;

    pub const ZONE_TAB: &str = "# comment\nDE\t+5230+01322\tEurope/Berlin\nUS\t+404251-0740023\tAmerica/New_York\n\
        US\t+340308-1181434\tAmerica/Los_Angeles\nGB\t+513030-0000731\tEurope/London\n";

    pub fn system() -> System {
        System {
            zone_tab: ZONE_TAB.into(), keyboard_layouts: None, cpus: 4, ram_mb: Some(8192),
            architecture: "x86_64".into(), host_architecture: "x86_64".into(), fortress_available: true,
        }
    }

    const SHOP: &str = r#"
        name = "shop"
        country = "DE"
        timezone = "Europe/Berlin"

        [identity]
        locale = "de_DE.UTF-8"
        languages = ["de-DE", "de", "en"]
        keyboard_layout = "de"

        [screen]
        width = 2560
        height = 1440
        scale = 1.25

        [browser]
        engine = "chrome"

        [route]
        type = "socks5"
        url = "socks5://proxy.example:1080"
        auth = "vault:shop-proxy"

        [humanizer]
        seed = 81723
    "#;

    #[test]
    fn roadmap_example_parses_and_validates() {
        let p = Persona::parse(SHOP).unwrap();
        p.validate(&system()).unwrap();
        assert_eq!(p.screen, ScreenSpec { width: 2560, height: 1440, scale: 1.25 });
        assert_eq!(p.route.endpoint().unwrap(), ("proxy.example".to_string(), 1080));
        assert_eq!(p.route.proxy().unwrap().auth.as_deref(), Some("vault:shop-proxy"));
        assert_eq!(p.seed(), 81723);
        assert_eq!(p.accept_languages(), "de-DE,de,en");
    }

    #[test]
    fn country_fills_unset_fields() {
        let p = Persona::parse("name = \"a\"\ncountry = \"DE\"\n[identity]\nkeyboard_layout = \"us\"\n").unwrap();
        assert_eq!(p.timezone, "Europe/Berlin");
        assert_eq!(p.identity.locale, "de_DE.UTF-8");
        assert_eq!(p.identity.keyboard_layout, "us");
        assert_eq!(p.screen, ScreenSpec::default());
        assert_eq!(p.route, Route::Direct);
        assert_eq!(p.seed(), Persona::parse("name = \"a\"\ncountry = \"US\"").unwrap().seed());
        assert!(Persona::parse("name = \"a\"\ncountry = \"ZZ\"").is_err(), "no defaults for ZZ");
        assert!(Persona::parse("name = \"a\"").is_err(), "country is required");
    }

    #[test]
    fn stale_or_plaintext_fields_are_refused() {
        for bad in [
            "[hardware]\nscreen_width = 1",
            "[route]\ntype = \"socks5\"\nurl = \"h:1\"\npassword = \"hunter2\"",
            "[route]\ntype = \"tor\"",
        ] {
            let text = format!("name = \"a\"\ncountry = \"US\"\n{bad}");
            assert!(Persona::parse(&text).is_err(), "{bad}");
        }
        let creds_in_url = Persona::parse("name = \"a\"\ncountry = \"US\"\n[route]\ntype = \"socks5\"\nurl = \"socks5://u:p@h:1\"").unwrap();
        assert!(creds_in_url.validate(&system()).unwrap_err().to_string().contains("credentials"));
    }

    #[test]
    fn chrome_ram_is_read_only_while_fortress_can_map_declared_ram() {
        let mut p = Persona::parse("name = \"a\"\ncountry = \"US\"\nram_mb = 8192").unwrap();
        assert!(p.validate(&system()).is_ok());

        p.ram_mb = Some(4096);
        assert!(p.validate(&system()).unwrap_err().to_string().contains("read-only for Chrome"));

        p.browser.engine = Engine::Fortress;
        assert!(p.validate(&system()).is_ok(), "Fortress applies the declared device-memory value");
        p.ram_mb = Some(0);
        assert!(p.validate(&system()).unwrap_err().to_string().contains("ram_mb must be greater than zero"));

        p.ram_mb = Some(8192);
        let mut unknown_ram = system();
        unknown_ram.ram_mb = None;
        p.browser.engine = Engine::Chrome;
        assert!(p.validate(&unknown_ram).unwrap_err().to_string().contains("physical memory is unavailable"));
    }

    #[test]
    fn mismatches_are_all_reported() {
        let mut p = Persona::parse(SHOP).unwrap();
        p.timezone = "America/New_York".into();
        p.identity.locale = "en_US.UTF-8".into();
        p.identity.keyboard_layout = "../x".into();
        p.screen.scale = 5.0;
        p.browser.engine = Engine::Chrome;
        p.cpus = Some(8);
        p.route = Route::Http(Proxy { url: "socks5://h:1".into(), auth: Some("plain".into()), allow_datacenter: false });
        let err = p.validate(&system()).unwrap_err().to_string();
        for needle in ["timezone", "locale", "first language", "keyboard_layout", "screen", "cpus = 8", "scheme", "vault:"] {
            assert!(err.contains(needle), "{needle} missing from:\n{err}");
        }
    }

    #[test]
    fn fortress_requires_supported_architecture_and_opt_in_image() {
        let mut p = Persona::parse(SHOP).unwrap();
        p.browser.engine = Engine::Fortress;
        p.cpus = Some(8);
        let mut sys = system();
        sys.architecture = "aarch64".into();
        sys.fortress_available = true;
        let err = p.validate(&sys).unwrap_err().to_string();
        assert!(err.contains("Fortress requires a Linux x86_64 runtime"), "{err}");

        sys.architecture = "x86_64".into();
        sys.host_architecture = "aarch64".into();
        let err = p.validate(&sys).unwrap_err().to_string();
        assert!(err.contains("x86_64 Docker host"), "{err}");

        sys.host_architecture = "x86_64".into();
        sys.fortress_available = false;
        let err = p.validate(&sys).unwrap_err().to_string();
        assert!(err.contains("TABOOM_ENGINES=chrome,fortress"), "{err}");

        sys.fortress_available = true;
        assert!(p.validate(&sys).is_ok(), "Fortress maps configured cpus rather than requiring a cpuset");
    }

    #[test]
    fn names_are_path_safe() {
        for good in ["default", "work", "a", "Alice_2-b", &"x".repeat(64)] {
            assert!(check_name(good).is_ok(), "{good}");
        }
        for bad in ["", ".", "..", "../x", "a/b", "a b", "a.b", "~", "émile", &"x".repeat(65)] {
            assert!(check_name(bad).is_err(), "{bad:?}");
        }
    }
}
