pub mod admin;
pub mod consistency;
pub mod hardware;
pub mod persona;

pub use crate::persona::check_name;

/// Whether `tz` is one of `country`'s zones in tzdata's zone.tab
/// (`CC<tab>coordinates<tab>Zone`).
pub fn timezone_matches_country(zone_tab: &str, tz: &str, country: &str) -> bool {
    zone_tab
        .lines()
        .filter(|line| !line.starts_with('#'))
        .map(|line| line.split('\t'))
        .any(|mut fields| fields.next() == Some(country) && fields.nth(1) == Some(tz))
}

#[cfg(test)]
mod tests {
    use super::timezone_matches_country;

    #[test]
    fn zone_tab_match_requires_exact_country_and_zone() {
        let zones = "# comment\nDE\t+5230+01322\tEurope/Berlin\nUS\t+404251-0740023\tAmerica/New_York\n";
        assert!(timezone_matches_country(zones, "Europe/Berlin", "DE"));
        assert!(!timezone_matches_country(zones, "America/New_York", "DE"));
    }
}
