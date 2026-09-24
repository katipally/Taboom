use anyhow::{Result, bail};
use hmac::{Hmac, Mac};
use sha1::Sha1;
use std::time::{SystemTime, UNIX_EPOCH};

type HmacSha1 = Hmac<Sha1>;

#[derive(Debug, Clone)]
pub struct TotpConfig {
    pub digits: u8,
    pub period_secs: u32,
}

impl Default for TotpConfig {
    fn default() -> Self {
        Self {
            digits: 6,
            period_secs: 30,
        }
    }
}

pub fn generate_totp(seed_base32: &str, time: SystemTime) -> Result<String> {
    generate_totp_with_config(seed_base32, time, &TotpConfig::default())
}

pub fn generate_totp_with_config(
    seed_base32: &str,
    time: SystemTime,
    config: &TotpConfig,
) -> Result<String> {
    // Authenticator apps show seeds in spaced groups ("JBSW Y3DP ...").
    let seed: String = seed_base32.chars().filter(|c| !c.is_whitespace()).collect::<String>().to_uppercase();
    let key = data_encoding::BASE32_NOPAD
        .decode(seed.as_bytes())
        .or_else(|_| data_encoding::BASE32.decode(seed.as_bytes()))
        .map_err(|e| anyhow::anyhow!("invalid base32 seed: {e}"))?;

    if key.is_empty() {
        bail!("empty TOTP seed");
    }

    let elapsed = time
        .duration_since(UNIX_EPOCH)
        .map_err(|_| anyhow::anyhow!("system time before unix epoch"))?;
    let counter = elapsed.as_secs() / config.period_secs as u64;

    let code = hotp(&key, counter, config.digits)?;
    Ok(format!("{:0>width$}", code, width = config.digits as usize))
}

fn hotp(key: &[u8], counter: u64, digits: u8) -> Result<u32> {
    let mut mac =
        HmacSha1::new_from_slice(key).map_err(|e| anyhow::anyhow!("HMAC init failed: {e}"))?;
    mac.update(&counter.to_be_bytes());
    let result = mac.finalize().into_bytes();

    let offset = (result[result.len() - 1] & 0x0f) as usize;
    let truncated = u32::from_be_bytes([
        result[offset] & 0x7f,
        result[offset + 1],
        result[offset + 2],
        result[offset + 3],
    ]);

    Ok(truncated % 10u32.pow(digits as u32))
}

#[cfg(test)]
mod tests {
    use super::*;

    // RFC 6238 test vectors use the ASCII string "12345678901234567890" as seed
    // which is GEZDGNBVGY3TQOJQGEZDGNBVGY3TQOJQ in base32
    const TEST_SEED: &str = "GEZDGNBVGY3TQOJQGEZDGNBVGY3TQOJQ";

    fn time_from_secs(secs: u64) -> SystemTime {
        UNIX_EPOCH + std::time::Duration::from_secs(secs)
    }

    #[test]
    fn rfc6238_published_vectors() {
        let eight = TotpConfig { digits: 8, period_secs: 30 };
        for (secs, expected) in [(59, "94287082"), (1111111109, "07081804"), (1111111111, "14050471"), (1234567890, "89005924")] {
            assert_eq!(generate_totp_with_config(TEST_SEED, time_from_secs(secs), &eight).unwrap(), expected);
            assert_eq!(generate_totp(TEST_SEED, time_from_secs(secs)).unwrap(), expected[2..]);
        }
    }

    #[test]
    fn spaced_lowercase_seed_accepted() {
        let spaced = "gezd gnbv gy3t qojq gezd gnbv gy3t qojq";
        assert_eq!(generate_totp(spaced, time_from_secs(59)).unwrap(), "287082");
    }

    #[test]
    fn totp_deterministic() {
        let t = time_from_secs(1234567890);
        let a = generate_totp(TEST_SEED, t).unwrap();
        let b = generate_totp(TEST_SEED, t).unwrap();
        assert_eq!(a, b);
    }

    #[test]
    fn totp_changes_across_periods() {
        let a = generate_totp(TEST_SEED, time_from_secs(0)).unwrap();
        let b = generate_totp(TEST_SEED, time_from_secs(30)).unwrap();
        assert_ne!(a, b);
    }

    #[test]
    fn totp_same_within_period() {
        let a = generate_totp(TEST_SEED, time_from_secs(10)).unwrap();
        let b = generate_totp(TEST_SEED, time_from_secs(20)).unwrap();
        assert_eq!(a, b);
    }

    #[test]
    fn invalid_base32_rejected() {
        assert!(generate_totp("!!!invalid!!!", time_from_secs(0)).is_err());
    }

    #[test]
    fn custom_config_8_digits() {
        let config = TotpConfig {
            digits: 8,
            period_secs: 30,
        };
        let code =
            generate_totp_with_config(TEST_SEED, time_from_secs(59), &config).unwrap();
        assert_eq!(code.len(), 8);
    }
}
