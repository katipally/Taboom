use maxminddb::Reader;
use serde::{Deserialize, Serialize};
use std::net::IpAddr;
use std::path::Path;
use crate::persona::RouteConfig;
use tracing::{info, warn};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GeoInfo {
    pub ip: IpAddr,
    pub country: Option<String>,
    pub asn: Option<u32>,
    pub asn_org: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
struct MaxmindCountry {
    country: Option<MaxmindCountryInner>,
}

#[derive(Debug, Serialize, Deserialize)]
struct MaxmindCountryInner {
    iso_code: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
struct MaxmindAsn {
    autonomous_system_number: Option<u32>,
    autonomous_system_organization: Option<String>,
}

pub struct GeoLookup {
    country_reader: Option<Reader<Vec<u8>>>,
    asn_reader: Option<Reader<Vec<u8>>>,
}

impl GeoLookup {
    pub fn open(data_dir: &Path) -> Self {
        let country_path = data_dir.join("GeoLite2-Country.mmdb");
        let asn_path = data_dir.join("GeoLite2-ASN.mmdb");

        let country_reader = Reader::open_readfile(&country_path)
            .map_err(|e| {
                info!(path = %country_path.display(), "GeoIP country DB not available: {e}");
                e
            })
            .ok();

        let asn_reader = Reader::open_readfile(&asn_path)
            .map_err(|e| {
                info!(path = %asn_path.display(), "GeoIP ASN DB not available: {e}");
                e
            })
            .ok();

        Self {
            country_reader,
            asn_reader,
        }
    }

    pub fn lookup(&self, ip: IpAddr) -> GeoInfo {
        let country = self
            .country_reader
            .as_ref()
            .and_then(|r| r.lookup::<MaxmindCountry>(ip).ok())
            .and_then(|c| c.country)
            .and_then(|c| c.iso_code);

        let (asn, asn_org) = self
            .asn_reader
            .as_ref()
            .and_then(|r| r.lookup::<MaxmindAsn>(ip).ok())
            .map(|a| (a.autonomous_system_number, a.autonomous_system_organization))
            .unwrap_or((None, None));

        GeoInfo {
            ip,
            country,
            asn,
            asn_org,
        }
    }

    pub fn available(&self) -> bool {
        self.country_reader.is_some() || self.asn_reader.is_some()
    }
}

const DATACENTER_ASNS: &[u32] = &[
    14618, 16509, 15169, 8075, 396982, // AWS, GCP, Azure, Oracle
    13335, 20473, 63949, 24940, 16276, // Cloudflare, Vultr, Linode, Hetzner, OVH
];

pub fn check_datacenter_asn(asn: Option<u32>) -> bool {
    asn.map(|a| DATACENTER_ASNS.contains(&a)).unwrap_or(false)
}

pub struct RouteChecker {
    geo: GeoLookup,
}

impl RouteChecker {
    pub fn new(geo: GeoLookup) -> Self {
        Self { geo }
    }

    pub fn geo_available(&self) -> bool {
        self.geo.available()
    }

    pub fn validate_at_creation(
        &self,
        route: &RouteConfig,
        _timezone: &str,
    ) -> Vec<String> {
        let mut warnings = Vec::new();

        match route {
            RouteConfig::Direct => {
                warn!("persona using direct route; datacenter detection possible in cloud");
                warnings.push("direct route: datacenter IP detection possible if running in cloud".into());
            }
            RouteConfig::Proxy { address, port, .. } => {
                if let Ok(ip) = address.parse::<IpAddr>() {
                    let geo = self.geo.lookup(ip);
                    if check_datacenter_asn(geo.asn) {
                        warnings.push(format!(
                            "proxy {address}:{port} resolves to datacenter ASN {}",
                            geo.asn.unwrap_or(0)
                        ));
                    }
                }
            }
        }

        warnings
    }

    pub fn check_exit_geo(&self, ip: IpAddr, expected_tz: &str) -> RouteHealthResult {
        let geo = self.geo.lookup(ip);

        let tz_mismatch = match (&geo.country, expected_tz) {
            (Some(country), tz) => !timezone_matches_country(tz, country),
            _ => false,
        };

        if tz_mismatch {
            warn!(
                exit_ip = %ip,
                country = ?geo.country,
                expected_tz = expected_tz,
                "exit geo does not match persona timezone"
            );
        }

        let is_datacenter = check_datacenter_asn(geo.asn);

        RouteHealthResult {
            geo,
            tz_mismatch,
            is_datacenter,
        }
    }
}

pub struct RouteHealthResult {
    pub geo: GeoInfo,
    pub tz_mismatch: bool,
    pub is_datacenter: bool,
}

fn timezone_matches_country(tz: &str, country_code: &str) -> bool {
    let prefix = match country_code {
        "US" => &["America/"][..],
        "GB" => &["Europe/London"],
        "DE" => &["Europe/Berlin"],
        "FR" => &["Europe/Paris"],
        "JP" => &["Asia/Tokyo"],
        "AU" => &["Australia/"],
        "CA" => &["America/"],
        "IN" => &["Asia/Kolkata", "Asia/Calcutta"],
        "BR" => &["America/Sao_Paulo", "America/Fortaleza", "America/Manaus"],
        _ => return true,
    };

    prefix.iter().any(|p| tz.starts_with(p))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn datacenter_asn_detected() {
        assert!(check_datacenter_asn(Some(16509)));
        assert!(!check_datacenter_asn(Some(7922)));
        assert!(!check_datacenter_asn(None));
    }

    #[test]
    fn timezone_country_match() {
        assert!(timezone_matches_country("America/New_York", "US"));
        assert!(!timezone_matches_country("Europe/London", "US"));
        assert!(timezone_matches_country("Europe/London", "GB"));
        assert!(timezone_matches_country("Asia/Tokyo", "JP"));
        assert!(timezone_matches_country("Anything/Goes", "ZZ"));
    }
}
