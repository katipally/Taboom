use std::time::{Duration, Instant};

#[derive(Debug, Clone)]
pub struct SettleDetector {
    pub threshold_pct: f64,
    pub stable_duration: Duration,
    pub max_wait: Duration,
}

#[derive(Debug, Clone, PartialEq)]
pub enum SettleResult {
    Settled { frame_count: u32 },
    StillChanging { frame_count: u32 },
    TimedOut { frame_count: u32 },
}

impl Default for SettleDetector {
    fn default() -> Self {
        Self {
            threshold_pct: 0.5,
            stable_duration: Duration::from_millis(200),
            max_wait: Duration::from_secs(3),
        }
    }
}

impl SettleDetector {
    pub fn check_settled(&self, diffs: &[f64], timestamps: &[Instant]) -> SettleResult {
        if diffs.is_empty() {
            return SettleResult::Settled { frame_count: 0 };
        }

        let frame_count = diffs.len() as u32;

        if let (Some(&first_ts), Some(&last_ts)) = (timestamps.first(), timestamps.last()) {
            if last_ts.duration_since(first_ts) >= self.max_wait {
                return SettleResult::TimedOut { frame_count };
            }
        }

        let mut stable_since: Option<Instant> = None;

        for (i, &diff) in diffs.iter().enumerate() {
            if diff < self.threshold_pct {
                if stable_since.is_none() {
                    stable_since = Some(timestamps[i]);
                }
                if let Some(since) = stable_since {
                    if timestamps[i].duration_since(since) >= self.stable_duration {
                        return SettleResult::Settled { frame_count };
                    }
                }
            } else {
                stable_since = None;
            }
        }

        SettleResult::StillChanging { frame_count }
    }
}

pub fn pixel_diff_pct(a: &[u8], b: &[u8]) -> f64 {
    if a.len() != b.len() || a.is_empty() {
        return 100.0;
    }
    let changed = a.iter().zip(b.iter()).filter(|(x, y)| x != y).count();
    (changed as f64 / a.len() as f64) * 100.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_diffs_is_settled() {
        let sd = SettleDetector::default();
        assert_eq!(sd.check_settled(&[], &[]), SettleResult::Settled { frame_count: 0 });
    }

    #[test]
    fn stable_frames_settle() {
        let sd = SettleDetector {
            threshold_pct: 1.0,
            stable_duration: Duration::from_millis(100),
            max_wait: Duration::from_secs(3),
        };
        let now = Instant::now();
        let timestamps = vec![
            now,
            now + Duration::from_millis(50),
            now + Duration::from_millis(120),
        ];
        let diffs = vec![0.1, 0.05, 0.02];
        assert_eq!(
            sd.check_settled(&diffs, &timestamps),
            SettleResult::Settled { frame_count: 3 }
        );
    }

    #[test]
    fn changing_frames_not_settled() {
        let sd = SettleDetector {
            threshold_pct: 1.0,
            stable_duration: Duration::from_millis(500),
            max_wait: Duration::from_secs(3),
        };
        let now = Instant::now();
        let timestamps = vec![
            now,
            now + Duration::from_millis(50),
        ];
        let diffs = vec![5.0, 3.0];
        assert_eq!(
            sd.check_settled(&diffs, &timestamps),
            SettleResult::StillChanging { frame_count: 2 }
        );
    }

    #[test]
    fn timeout_fires() {
        let sd = SettleDetector {
            threshold_pct: 0.1,
            stable_duration: Duration::from_millis(200),
            max_wait: Duration::from_secs(1),
        };
        let now = Instant::now();
        let timestamps = vec![
            now,
            now + Duration::from_millis(500),
            now + Duration::from_millis(1100),
        ];
        let diffs = vec![5.0, 5.0, 5.0];
        assert_eq!(
            sd.check_settled(&diffs, &timestamps),
            SettleResult::TimedOut { frame_count: 3 }
        );
    }

    #[test]
    fn pixel_diff_identical() {
        let a = vec![0u8; 100];
        assert!(pixel_diff_pct(&a, &a) < 0.001);
    }

    #[test]
    fn pixel_diff_totally_different() {
        let a = vec![0u8; 100];
        let b = vec![255u8; 100];
        assert!((pixel_diff_pct(&a, &b) - 100.0).abs() < 0.001);
    }
}
