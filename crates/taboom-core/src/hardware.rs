use rand::distributions::WeightedIndex;
use rand::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
/// A common display setup; CPU and RAM are not drawn because Chrome reports the host's.
pub struct HardwareProfile {
    pub screen_width: u32,
    pub screen_height: u32,
    pub dpr: f64,
}

struct HardwareEntry {
    profile: HardwareProfile,
    weight: u32,
}

const TABLE: &[HardwareEntry] = &[
    HardwareEntry {
        profile: HardwareProfile {
            screen_width: 1366,
            screen_height: 768,
            dpr: 1.0,
        },
        weight: 15,
    },
    HardwareEntry {
        profile: HardwareProfile {
            screen_width: 1920,
            screen_height: 1080,
            dpr: 1.0,
        },
        weight: 30,
    },
    HardwareEntry {
        profile: HardwareProfile {
            screen_width: 1920,
            screen_height: 1080,
            dpr: 1.25,
        },
        weight: 10,
    },
    HardwareEntry {
        profile: HardwareProfile {
            screen_width: 1920,
            screen_height: 1080,
            dpr: 1.0,
        },
        weight: 12,
    },
    HardwareEntry {
        profile: HardwareProfile {
            screen_width: 2560,
            screen_height: 1440,
            dpr: 1.0,
        },
        weight: 8,
    },
    HardwareEntry {
        profile: HardwareProfile {
            screen_width: 2560,
            screen_height: 1440,
            dpr: 1.25,
        },
        weight: 5,
    },
    HardwareEntry {
        profile: HardwareProfile {
            screen_width: 3840,
            screen_height: 2160,
            dpr: 1.5,
        },
        weight: 3,
    },
    HardwareEntry {
        profile: HardwareProfile {
            screen_width: 2560,
            screen_height: 1600,
            dpr: 2.0,
        },
        weight: 7,
    },
    HardwareEntry {
        profile: HardwareProfile {
            screen_width: 1920,
            screen_height: 1200,
            dpr: 1.0,
        },
        weight: 3,
    },
    HardwareEntry {
        profile: HardwareProfile {
            screen_width: 1920,
            screen_height: 1080,
            dpr: 1.0,
        },
        weight: 7,
    },
];

pub fn draw_hardware(rng: &mut impl Rng) -> HardwareProfile {
    let weights: Vec<u32> = TABLE.iter().map(|e| e.weight).collect();
    let dist = WeightedIndex::new(&weights).expect("non-empty hardware table");
    TABLE[rng.sample(dist)].profile.clone()
}

/// `navigator.deviceMemory` as current desktop Chrome reports it: RAM in GiB rounded to the
/// nearest power of two (ties round down), clamped to 2..=32.
pub fn device_memory_gb(ram_mb: u64) -> f64 {
    let gb = ram_mb as f64 / 1024.0;
    if gb <= 0.0 {
        return 2.0;
    }
    let lower = 2f64.powi(gb.log2().floor() as i32);
    let upper = lower * 2.0;
    // Chromium's comparison is <=, so exact midpoints select the lower bucket.
    let nearest = if gb - lower <= upper - gb { lower } else { upper };
    nearest.clamp(2.0, 32.0)
}

/// Physical RAM in MiB from Linux's `MemTotal` field, or None when unavailable.
pub fn physical_memory_mb() -> Option<u64> {
    let info = std::fs::read_to_string("/proc/meminfo").ok()?;
    info.lines()
        .find_map(|line| line.strip_prefix("MemTotal:"))?
        .trim()
        .strip_suffix("kB")?
        .trim()
        .parse::<u64>()
        .ok()
        .map(|kb| kb / 1024)
}

/// Architecture of the Docker host, forwarded by Compose to distinguish it from an emulated
/// container architecture. Returns `unknown` when Compose does not provide a recognizable value.
pub fn host_architecture() -> String {
    normalize_architecture(&std::env::var("TABOOM_HOST_ARCH").unwrap_or_default()).to_owned()
}

fn normalize_architecture(raw: &str) -> &'static str {
    match raw.trim().to_ascii_lowercase().as_str() {
        "amd64" | "x86_64" | "x64" => "x86_64",
        "arm64" | "aarch64" => "aarch64",
        _ => "unknown",
    }
}

/// "0-3,6,8-9" -> 7
pub fn count_cpu_list(list: &str) -> u32 {
    list.split(',')
        .filter_map(|part| match part.split_once('-') {
            Some((a, b)) => Some(b.parse::<u32>().ok()? + 1 - a.parse::<u32>().ok()?),
            None => part.parse::<u32>().ok().map(|_| 1),
        })
        .sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn draw_returns_valid_profile() {
        let mut rng = rand::rngs::StdRng::seed_from_u64(42);
        let hw = draw_hardware(&mut rng);
        assert!(hw.screen_width >= 1366);
        assert!(hw.dpr >= 1.0);
    }

    #[test]
    fn deterministic_with_same_seed() {
        let mut r1 = rand::rngs::StdRng::seed_from_u64(99);
        let mut r2 = rand::rngs::StdRng::seed_from_u64(99);
        let a = draw_hardware(&mut r1);
        let b = draw_hardware(&mut r2);
        assert_eq!((a.screen_width, a.screen_height, a.dpr), (b.screen_width, b.screen_height, b.dpr));
    }

    #[test]
    fn desktop_chrome_device_memory_uses_2026_limits_and_rounding() {
        assert_eq!(device_memory_gb(0), 2.0);
        assert_eq!(device_memory_gb(1024), 2.0);
        assert_eq!(device_memory_gb(2048), 2.0);
        assert_eq!(device_memory_gb(3000), 2.0);
        assert_eq!(device_memory_gb(3072), 2.0); // exact midpoint rounds down
        assert_eq!(device_memory_gb(3073), 4.0);
        assert_eq!(device_memory_gb(3500), 4.0);
        assert_eq!(device_memory_gb(15_900), 16.0);
        assert_eq!(device_memory_gb(24_576), 16.0); // exact midpoint rounds down
        assert_eq!(device_memory_gb(24_577), 32.0);
        assert_eq!(device_memory_gb(32_768), 32.0);
        assert_eq!(device_memory_gb(65_536), 32.0);
    }

    #[test]
    fn cpu_lists_count() {
        assert_eq!(count_cpu_list("0-3,6,8-9"), 7);
        assert_eq!(count_cpu_list("0"), 1);
    }

    #[test]
    fn host_architecture_names_are_normalized() {
        for (raw, expected) in [("amd64", "x86_64"), ("x86_64", "x86_64"), ("arm64", "aarch64"), ("aarch64", "aarch64"), ("mips", "unknown")] {
            assert_eq!(normalize_architecture(raw), expected);
        }
    }
}
