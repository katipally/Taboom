use maxminddb::Reader;
use serde::{Deserialize, Serialize};
use std::net::IpAddr;
use std::path::Path;
use crate::persona::Persona;
use std::collections::HashSet;
use std::sync::OnceLock;
use tracing::info;

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

    /// (country database, ASN database) loaded.
    pub fn databases(&self) -> (bool, bool) {
        (self.country_reader.is_some(), self.asn_reader.is_some())
    }
}

/// Hosting and cloud ASNs, from `data/datacenter-asns.txt`.
fn datacenter_asns() -> &'static HashSet<u32> {
    static SET: OnceLock<HashSet<u32>> = OnceLock::new();
    SET.get_or_init(|| {
        include_str!("../data/datacenter-asns.txt")
            .lines()
            .filter_map(|l| l.split('#').next()?.trim().parse().ok())
            .collect()
    })
}

pub fn check_datacenter_asn(asn: Option<u32>) -> bool {
    asn.is_some_and(|a| datacenter_asns().contains(&a))
}

pub const GEOLITE_REQUIRED: &str =
    "a proxy route needs GeoLite2-Country.mmdb and GeoLite2-ASN.mmdb in the data volume";

/// Whether the exit IP fits the persona. Proxy routes fail closed: both GeoLite2 databases are
/// required, the exit country must be the persona's and a datacenter ASN is refused unless
/// allowed. Direct routes only check the country, and only when the database is there.
pub fn judge_exit(exit: &GeoInfo, dbs: (bool, bool), persona: &Persona) -> Result<(), String> {
    let proxy = persona.route.proxy();
    if proxy.is_some() && !(dbs.0 && dbs.1) {
        return Err(GEOLITE_REQUIRED.into());
    }
    match (&exit.country, proxy) {
        (Some(c), _) if *c != persona.country => {
            return Err(format!("exit IP {} is in {c}, but the persona's country is {}", exit.ip, persona.country));
        }
        (None, Some(_)) => return Err(format!("exit IP {} has no country in GeoLite2", exit.ip)),
        _ => {}
    }
    if let Some(proxy) = proxy {
        let Some(asn) = exit.asn else {
            return Err(format!("exit IP {} has no ASN in GeoLite2", exit.ip));
        };
        if !proxy.allow_datacenter && check_datacenter_asn(Some(asn)) {
            return Err(format!(
                "exit IP {} is in datacenter AS{asn} ({}); set route.allow_datacenter = true to accept it",
                exit.ip,
                exit.asn_org.as_deref().unwrap_or("unknown")
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::persona::{tests::ZONE_TAB, Persona, Proxy, Route};
    use taboom_core::timezone_matches_country;

    #[test]
    fn datacenter_asn_detected() {
        assert!(check_datacenter_asn(Some(16509)));
        assert!(!check_datacenter_asn(Some(7922)));
        assert!(!check_datacenter_asn(None));
    }

    #[test]
    fn timezone_country_match() {
        assert!(timezone_matches_country(ZONE_TAB, "America/New_York", "US"));
        assert!(timezone_matches_country(ZONE_TAB, "America/Los_Angeles", "US"));
        assert!(!timezone_matches_country(ZONE_TAB, "Europe/London", "US"));
        assert!(timezone_matches_country(ZONE_TAB, "Europe/Berlin", "DE"));
        assert!(!timezone_matches_country(ZONE_TAB, "Anything/Goes", "ZZ"), "unknown is a mismatch, not a pass");
    }

    fn exit(country: Option<&str>, asn: u32) -> GeoInfo {
        GeoInfo { ip: "203.0.113.7".parse().unwrap(), country: country.map(str::to_string), asn: Some(asn), asn_org: None }
    }

    #[test]
    fn proxy_exit_fails_closed() {
        let mut p = Persona::parse("name = \"a\"\ncountry = \"DE\"").unwrap();
        let de = exit(Some("DE"), 3320);
        assert!(judge_exit(&de, (true, true), &p).is_ok(), "direct route");
        assert!(judge_exit(&exit(Some("US"), 3320), (true, true), &p).is_err(), "direct but wrong country");
        assert!(judge_exit(&exit(None, 0), (false, false), &p).is_ok(), "direct works without GeoLite");

        p.route = Route::Socks5(Proxy { url: "h:1".into(), auth: None, allow_datacenter: false });
        assert!(judge_exit(&de, (true, true), &p).is_ok());
        assert!(judge_exit(&de, (true, false), &p).unwrap_err().contains("GeoLite2"));
        assert!(judge_exit(&exit(None, 3320), (true, true), &p).is_err());
        let mut no_asn = de.clone();
        no_asn.asn = None;
        assert!(judge_exit(&no_asn, (true, true), &p).unwrap_err().contains("no ASN"));
        assert!(judge_exit(&exit(Some("NL"), 3320), (true, true), &p).unwrap_err().contains("NL"));
        assert!(judge_exit(&exit(Some("DE"), 24940), (true, true), &p).unwrap_err().contains("datacenter"));
        p.route = Route::Socks5(Proxy { url: "h:1".into(), auth: None, allow_datacenter: true });
        assert!(judge_exit(&exit(Some("DE"), 24940), (true, true), &p).is_ok());
        assert!(judge_exit(&no_asn, (true, true), &p).unwrap_err().contains("no ASN"));
    }
}
