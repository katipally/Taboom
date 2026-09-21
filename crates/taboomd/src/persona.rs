use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::RwLock;
use taboom_proto::{BrowserConfig, ProxyAuth, ProxyProtocol, Route};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PersonaConfig {
    pub name: String,
    pub cpus: u32,
    pub ram_mb: u32,
    #[serde(default = "default_disk_gb")]
    pub disk_gb: u32,
    #[serde(default = "default_timezone")]
    pub timezone: String,
    #[serde(default)]
    pub route: RouteConfig,
    #[serde(default)]
    pub browser: BrowserSettings,
    #[serde(default)]
    pub hardware: Option<HardwareConfig>,
    #[serde(default)]
    pub identity: Option<IdentityConfig>,
    #[serde(default)]
    pub humanizer_seed: Option<u64>,
    #[serde(default)]
    pub humanizer_style: Option<HumanizerStyle>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HardwareConfig {
    pub screen_width: u32,
    pub screen_height: u32,
    #[serde(default = "default_dpr")]
    pub dpr: f64,
}

fn default_dpr() -> f64 {
    1.0
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IdentityConfig {
    #[serde(default = "default_keyboard")]
    pub keyboard_layout: String,
    #[serde(default = "default_locale")]
    pub locale: String,
    #[serde(default)]
    pub languages: Vec<String>,
    #[serde(default)]
    pub fonts_packages: Vec<String>,
}

fn default_keyboard() -> String {
    "us".into()
}
fn default_locale() -> String {
    "en_US.UTF-8".into()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HumanizerStyle {
    #[serde(default = "default_speed")]
    pub speed: SpeedClass,
    #[serde(default)]
    pub typo_rate: f64,
    #[serde(default)]
    pub overshoot_tendency: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SpeedClass {
    Slow,
    Medium,
    Fast,
}

fn default_speed() -> SpeedClass {
    SpeedClass::Medium
}

fn default_disk_gb() -> u32 {
    20
}

fn default_timezone() -> String {
    "America/New_York".into()
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum RouteConfig {
    #[default]
    Direct,
    Proxy {
        address: String,
        port: u16,
        #[serde(default)]
        username: Option<String>,
        #[serde(default)]
        password: Option<String>,
        #[serde(default = "default_proxy_protocol")]
        protocol: ProxyProtocolConfig,
    },
}


#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ProxyProtocolConfig {
    Socks5,
    Http,
}

fn default_proxy_protocol() -> ProxyProtocolConfig {
    ProxyProtocolConfig::Socks5
}

impl RouteConfig {
    pub fn to_proto_route(&self) -> Route {
        match self {
            RouteConfig::Direct => Route::Direct,
            RouteConfig::Proxy {
                address,
                port,
                username,
                password,
                protocol,
            } => Route::Proxy {
                address: address.clone(),
                port: *port,
                auth: match (username, password) {
                    (Some(u), Some(p)) => Some(ProxyAuth {
                        username: u.clone(),
                        password: p.clone(),
                    }),
                    _ => None,
                },
                protocol: match protocol {
                    ProxyProtocolConfig::Socks5 => ProxyProtocol::Socks5,
                    ProxyProtocolConfig::Http => ProxyProtocol::Http,
                },
            },
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BrowserSettings {
    #[serde(default = "default_accept_languages")]
    pub accept_languages: String,
    #[serde(default = "default_download_dir")]
    pub download_dir: String,
}

impl Default for BrowserSettings {
    fn default() -> Self {
        Self {
            accept_languages: default_accept_languages(),
            download_dir: default_download_dir(),
        }
    }
}

fn default_accept_languages() -> String {
    "en-US,en".into()
}

fn default_download_dir() -> String {
    "/home/taboom/Downloads".into()
}

impl BrowserSettings {
    pub fn to_proto_config(&self) -> BrowserConfig {
        BrowserConfig {
            accept_languages: self.accept_languages.clone(),
            download_dir: self.download_dir.clone(),
        }
    }
}

pub struct PersonaRegistry {
    home: PathBuf,
    personas: RwLock<HashMap<String, PersonaConfig>>,
}

impl PersonaRegistry {
    pub fn load(home: &Path) -> Result<Self> {
        let dir = home.join("personas");
        let mut personas = HashMap::new();
        let registry = |personas| Self { home: home.to_path_buf(), personas: RwLock::new(personas) };

        if !dir.exists() {
            return Ok(registry(personas));
        }

        for entry in std::fs::read_dir(&dir)? {
            let entry = entry?;
            let path = entry.path();

            if path.extension().map(|e| e == "toml").unwrap_or(false) {
                let content = std::fs::read_to_string(&path)
                    .with_context(|| format!("reading {}", path.display()))?;
                let config: PersonaConfig = toml::from_str(&content)
                    .with_context(|| format!("parsing {}", path.display()))?;
                personas.insert(config.name.clone(), config);
            }
        }

        Ok(registry(personas))
    }

    pub fn home(&self) -> &Path {
        &self.home
    }

    pub fn count(&self) -> usize {
        self.personas.read().unwrap().len()
    }

    pub fn get(&self, name: &str) -> Option<PersonaConfig> {
        self.personas.read().unwrap().get(name).cloned()
    }

    pub fn list(&self) -> Vec<PersonaConfig> {
        let mut v: Vec<_> = self.personas.read().unwrap().values().cloned().collect();
        v.sort_by(|a, b| a.name.cmp(&b.name));
        v
    }

    /// Writes `personas/<name>.toml` and makes the persona visible at once. Never overwrites.
    pub fn create(&self, config: PersonaConfig) -> Result<PathBuf> {
        check_name(&config.name)?;
        let mut personas = self.personas.write().unwrap();
        if personas.contains_key(&config.name) {
            bail!("persona {:?} already exists", config.name);
        }
        let dir = self.home.join("personas");
        std::fs::create_dir_all(&dir)?;
        let path = dir.join(format!("{}.toml", config.name));
        let mut file = match std::fs::OpenOptions::new().write(true).create_new(true).open(&path) {
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                bail!("{} already exists", path.display())
            }
            other => other.with_context(|| format!("creating {}", path.display()))?,
        };
        file.write_all(toml::to_string(&config)?.as_bytes())?;
        personas.insert(config.name.clone(), config);
        Ok(path)
    }
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

/// The persona's own Chrome profile folder (`--user-data-dir`), kept in the data volume.
pub fn profile_dir(home: &Path, name: &str) -> Result<PathBuf> {
    check_name(name)?;
    Ok(home.join("profiles").join(name))
}

/// A new persona using Taboom's built-in default settings.
pub fn template(name: &str) -> PersonaConfig {
    PersonaConfig {
        name: name.into(),
        cpus: 2,
        ram_mb: 4096,
        disk_gb: default_disk_gb(),
        timezone: default_timezone(),
        route: RouteConfig::Direct,
        browser: BrowserSettings::default(),
        hardware: Some(HardwareConfig { screen_width: 1920, screen_height: 1080, dpr: default_dpr() }),
        identity: Some(IdentityConfig {
            keyboard_layout: default_keyboard(),
            locale: default_locale(),
            languages: vec!["en-US".into(), "en".into()],
            fonts_packages: vec![],
        }),
        humanizer_seed: None,
        humanizer_style: Some(HumanizerStyle { speed: SpeedClass::Medium, typo_rate: 0.02, overshoot_tendency: 0.15 }),
    }
}

pub fn derive_persona(
    exit_country: Option<&str>,
    seed: u64,
) -> (String, IdentityConfig, Vec<String>, crate::hardware::HardwareProfile) {
    use rand::SeedableRng;
    let mut rng = rand::rngs::StdRng::seed_from_u64(seed);

    let (tz, locale, langs) = match exit_country {
        Some("US") => (
            "America/New_York",
            "en_US.UTF-8",
            vec!["en-US", "en"],
        ),
        Some("GB") => (
            "Europe/London",
            "en_GB.UTF-8",
            vec!["en-GB", "en"],
        ),
        Some("DE") => (
            "Europe/Berlin",
            "de_DE.UTF-8",
            vec!["de", "en"],
        ),
        Some("FR") => (
            "Europe/Paris",
            "fr_FR.UTF-8",
            vec!["fr", "en"],
        ),
        Some("JP") => (
            "Asia/Tokyo",
            "ja_JP.UTF-8",
            vec!["ja", "en"],
        ),
        Some("BR") => (
            "America/Sao_Paulo",
            "pt_BR.UTF-8",
            vec!["pt-BR", "pt", "en"],
        ),
        Some("IN") => (
            "Asia/Kolkata",
            "en_IN.UTF-8",
            vec!["en-IN", "en", "hi"],
        ),
        _ => (
            "America/New_York",
            "en_US.UTF-8",
            vec!["en-US", "en"],
        ),
    };

    let hw = crate::hardware::draw_hardware(&mut rng);

    let identity = IdentityConfig {
        keyboard_layout: match exit_country {
            Some("DE") => "de".into(),
            Some("FR") => "fr".into(),
            Some("JP") => "jp".into(),
            Some("BR") => "br".into(),
            _ => "us".into(),
        },
        locale: locale.into(),
        languages: langs.iter().map(|s| s.to_string()).collect(),
        fonts_packages: vec![],
    };

    let accept = langs.join(",");

    (tz.into(), identity, vec![accept], hw)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_path_safe() {
        for good in ["default", "work", "a", "Alice_2-b", &"x".repeat(64)] {
            assert!(check_name(good).is_ok(), "{good}");
        }
        for bad in ["", ".", "..", "../x", "a/b", "a b", "a.b", "~", "émile", &"x".repeat(65)] {
            assert!(check_name(bad).is_err(), "{bad:?}");
        }
        let home = Path::new("/h");
        assert_eq!(profile_dir(home, "work").unwrap(), Path::new("/h/profiles/work"));
        assert!(profile_dir(home, "../etc").is_err());
    }

    #[test]
    fn create_then_list_and_reload() {
        let home = tempfile::tempdir().unwrap();
        let reg = PersonaRegistry::load(home.path()).unwrap();
        assert_eq!(reg.count(), 0);
        let path = reg.create(template("work")).unwrap();
        assert_eq!(path, home.path().join("personas/work.toml"));
        assert_eq!(reg.list().iter().map(|p| p.name.as_str()).collect::<Vec<_>>(), ["work"]);

        let reloaded = PersonaRegistry::load(home.path()).unwrap();
        let work = reloaded.get("work").unwrap();
        assert_eq!(work.timezone, "America/New_York");
        assert_eq!(work.identity.unwrap().languages, ["en-US", "en"]);
    }

    #[test]
    fn duplicates_and_bad_names_refused() {
        let home = tempfile::tempdir().unwrap();
        let reg = PersonaRegistry::load(home.path()).unwrap();
        reg.create(template("work")).unwrap();
        assert!(reg.create(template("work")).is_err());
        // a file on disk the registry does not know by that name is not overwritten either
        std::fs::write(home.path().join("personas/other.toml"), "name = \"x\"\ncpus = 1\nram_mb = 1\n").unwrap();
        assert!(reg.create(template("other")).is_err());
        assert!(reg.create(template("../evil")).is_err());
        assert!(!home.path().join("evil.toml").exists());
        assert_eq!(reg.count(), 1);
    }
}
