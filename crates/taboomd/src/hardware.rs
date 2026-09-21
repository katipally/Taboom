use rand::distributions::WeightedIndex;
use rand::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HardwareProfile {
    pub label: &'static str,
    pub cores: u32,
    pub ram_mb: u32,
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
            label: "budget-laptop",
            cores: 2,
            ram_mb: 4096,
            screen_width: 1366,
            screen_height: 768,
            dpr: 1.0,
        },
        weight: 15,
    },
    HardwareEntry {
        profile: HardwareProfile {
            label: "mid-laptop-1080p",
            cores: 4,
            ram_mb: 8192,
            screen_width: 1920,
            screen_height: 1080,
            dpr: 1.0,
        },
        weight: 30,
    },
    HardwareEntry {
        profile: HardwareProfile {
            label: "mid-laptop-hidpi",
            cores: 4,
            ram_mb: 8192,
            screen_width: 1920,
            screen_height: 1080,
            dpr: 1.25,
        },
        weight: 10,
    },
    HardwareEntry {
        profile: HardwareProfile {
            label: "desktop-1080p",
            cores: 4,
            ram_mb: 16384,
            screen_width: 1920,
            screen_height: 1080,
            dpr: 1.0,
        },
        weight: 12,
    },
    HardwareEntry {
        profile: HardwareProfile {
            label: "desktop-1440p",
            cores: 8,
            ram_mb: 16384,
            screen_width: 2560,
            screen_height: 1440,
            dpr: 1.0,
        },
        weight: 8,
    },
    HardwareEntry {
        profile: HardwareProfile {
            label: "desktop-1440p-scaled",
            cores: 8,
            ram_mb: 16384,
            screen_width: 2560,
            screen_height: 1440,
            dpr: 1.25,
        },
        weight: 5,
    },
    HardwareEntry {
        profile: HardwareProfile {
            label: "high-end-desktop",
            cores: 8,
            ram_mb: 32768,
            screen_width: 3840,
            screen_height: 2160,
            dpr: 1.5,
        },
        weight: 3,
    },
    HardwareEntry {
        profile: HardwareProfile {
            label: "ultrabook-13",
            cores: 4,
            ram_mb: 8192,
            screen_width: 2560,
            screen_height: 1600,
            dpr: 2.0,
        },
        weight: 7,
    },
    HardwareEntry {
        profile: HardwareProfile {
            label: "workstation",
            cores: 16,
            ram_mb: 32768,
            screen_width: 1920,
            screen_height: 1200,
            dpr: 1.0,
        },
        weight: 3,
    },
    HardwareEntry {
        profile: HardwareProfile {
            label: "entry-desktop",
            cores: 2,
            ram_mb: 8192,
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

pub fn device_memory_bucket(ram_mb: u32) -> f64 {
    let gb = ram_mb as f64 / 1024.0;
    if gb <= 2.0 {
        2.0
    } else if gb <= 4.0 {
        4.0
    } else {
        8.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn draw_returns_valid_profile() {
        let mut rng = rand::rngs::StdRng::seed_from_u64(42);
        let hw = draw_hardware(&mut rng);
        assert!(hw.cores >= 2);
        assert!(hw.ram_mb >= 4096);
        assert!(hw.screen_width >= 1366);
        assert!(hw.dpr >= 1.0);
    }

    #[test]
    fn deterministic_with_same_seed() {
        let mut r1 = rand::rngs::StdRng::seed_from_u64(99);
        let mut r2 = rand::rngs::StdRng::seed_from_u64(99);
        let a = draw_hardware(&mut r1);
        let b = draw_hardware(&mut r2);
        assert_eq!(a.label, b.label);
    }

    #[test]
    fn device_memory_buckets() {
        assert_eq!(device_memory_bucket(2048), 2.0);
        assert_eq!(device_memory_bucket(4096), 4.0);
        assert_eq!(device_memory_bucket(8192), 8.0);
        assert_eq!(device_memory_bucket(16384), 8.0);
    }
}
