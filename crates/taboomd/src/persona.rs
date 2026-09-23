pub use taboom_core::persona::*;

use anyhow::{bail, Context, Result};
use std::io::Write;
use std::path::{Path, PathBuf};

/// Container-specific inputs used to validate shared persona data.
pub fn probe_system() -> anyhow::Result<System> {
    let zone_tab = std::fs::read_to_string("/usr/share/zoneinfo/zone.tab")
        .context("reading /usr/share/zoneinfo/zone.tab (is tzdata installed?)")?;
    let xkb = std::path::PathBuf::from("/usr/share/X11/xkb/symbols");
    let keyboard_layouts = std::fs::read_dir(&xkb).ok().map(|entries| {
        entries.flatten()
            .filter_map(|entry| entry.file_type().ok().filter(|kind| kind.is_file()).map(|_| entry.file_name().to_string_lossy().into_owned()))
            .collect()
    });
    Ok(System {
        zone_tab,
        keyboard_layouts,
        cpus: crate::hardware::visible_cpus(),
        ram_mb: crate::hardware::mem_total_mb(),
        architecture: std::env::consts::ARCH.into(),
        host_architecture: crate::hardware::host_architecture(),
        fortress_available: fortress_is_installed(),
    })
}

/// Reads `personas/<name>.toml`, writing the US direct-route default on first boot.
pub fn load_or_create(home: &Path, name: &str) -> Result<(Persona, bool)> {
    check_name(name)?;
    let path = home.join("personas").join(format!("{name}.toml"));
    let created = !path.exists();
    if created {
        let seed = rand::random::<u64>() & i64::MAX as u64;
        let persona = derive_persona(name, "US", seed)?;
        std::fs::create_dir_all(path.parent().context("persona path has no folder")?)?;
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .with_context(|| format!("creating {}", path.display()))?;
        file.write_all(toml::to_string(&persona)?.as_bytes())?;
    }
    let text = std::fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
    let persona = Persona::parse(&text).with_context(|| format!("parsing {}", path.display()))?;
    if persona.name != name {
        bail!("{} declares name = {:?}; it must match the file name", path.display(), persona.name);
    }
    Ok((persona, created))
}

/// The persona's Chrome profile folder (`--user-data-dir`), kept in the data volume.
pub fn profile_dir(home: &Path, name: &str) -> Result<PathBuf> {
    check_name(name)?;
    Ok(home.join("profiles").join(name))
}

#[cfg(test)]
pub mod tests {
    use taboom_core::persona::System;
    use super::{load_or_create, profile_dir};
    use std::path::Path;

    pub const ZONE_TAB: &str = "# comment\nDE\t+5230+01322\tEurope/Berlin\nUS\t+404251-0740023\tAmerica/New_York\n\
        US\t+340308-1181434\tAmerica/Los_Angeles\nGB\t+513030-0000731\tEurope/London\n";

    pub fn system() -> System {
        System {
            zone_tab: ZONE_TAB.into(), keyboard_layouts: None, cpus: 4, ram_mb: Some(8192),
            architecture: "x86_64".into(), host_architecture: "x86_64".into(), fortress_available: true,
        }
    }

    #[test]
    fn first_boot_writes_a_valid_default_once() {
        let home = tempfile::tempdir().unwrap();
        let (persona, created) = load_or_create(home.path(), "default").unwrap();
        assert!(created);
        assert!(persona.humanizer.seed.unwrap() <= i64::MAX as u64);
        persona.validate(&system()).unwrap();
        let (again, created) = load_or_create(home.path(), "default").unwrap();
        assert!(!created);
        assert_eq!(again, persona, "the seed and screen are fixed on disk, not redrawn");
        assert!(load_or_create(home.path(), "../evil").is_err());
        std::fs::write(home.path().join("personas/other.toml"), "name = \"x\"\ncountry = \"US\"").unwrap();
        assert!(load_or_create(home.path(), "other").unwrap_err().to_string().contains("file name"));
    }

    #[test]
    fn profile_paths_reject_traversal() {
        assert_eq!(profile_dir(Path::new("/h"), "work").unwrap(), Path::new("/h/profiles/work"));
        assert!(profile_dir(Path::new("/h"), "../etc").is_err());
    }
}
