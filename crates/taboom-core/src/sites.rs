pub struct SiteEntry {
    pub url: &'static str,
    pub title: &'static str,
    pub weight: u32,
    pub bookmarkable: bool,
}

pub fn sites_for_country(country: &str) -> Vec<&'static SiteEntry> {
    let country_sites: &[SiteEntry] = match country {
        "US" => &SITES_US,
        "GB" => &SITES_GB,
        "DE" => &SITES_DE,
        "FR" => &SITES_FR,
        "JP" => &SITES_JP,
        "BR" => &SITES_BR,
        "IN" => &SITES_IN,
        _ => &[],
    };
    let mut out: Vec<&'static SiteEntry> = SITES_GLOBAL.iter().collect();
    out.extend(country_sites.iter());
    out
}

const SITES_GLOBAL: [SiteEntry; 22] = [
    SiteEntry { url: "https://www.google.com/", title: "Google", weight: 50, bookmarkable: false },
    SiteEntry { url: "https://www.youtube.com/", title: "YouTube", weight: 40, bookmarkable: true },
    SiteEntry { url: "https://en.wikipedia.org/", title: "Wikipedia", weight: 20, bookmarkable: true },
    SiteEntry { url: "https://github.com/", title: "GitHub", weight: 15, bookmarkable: true },
    SiteEntry { url: "https://www.reddit.com/", title: "Reddit", weight: 18, bookmarkable: true },
    SiteEntry { url: "https://www.amazon.com/", title: "Amazon", weight: 22, bookmarkable: true },
    SiteEntry { url: "https://x.com/", title: "X", weight: 14, bookmarkable: true },
    SiteEntry { url: "https://www.facebook.com/", title: "Facebook", weight: 16, bookmarkable: false },
    SiteEntry { url: "https://www.linkedin.com/", title: "LinkedIn", weight: 12, bookmarkable: true },
    SiteEntry { url: "https://www.instagram.com/", title: "Instagram", weight: 15, bookmarkable: false },
    SiteEntry { url: "https://www.netflix.com/", title: "Netflix", weight: 10, bookmarkable: true },
    SiteEntry { url: "https://stackoverflow.com/", title: "Stack Overflow", weight: 14, bookmarkable: true },
    SiteEntry { url: "https://medium.com/", title: "Medium", weight: 8, bookmarkable: true },
    SiteEntry { url: "https://mail.google.com/", title: "Gmail", weight: 30, bookmarkable: true },
    SiteEntry { url: "https://drive.google.com/", title: "Google Drive", weight: 12, bookmarkable: true },
    SiteEntry { url: "https://docs.google.com/", title: "Google Docs", weight: 10, bookmarkable: false },
    SiteEntry { url: "https://calendar.google.com/", title: "Google Calendar", weight: 8, bookmarkable: true },
    SiteEntry { url: "https://maps.google.com/", title: "Google Maps", weight: 12, bookmarkable: false },
    SiteEntry { url: "https://www.twitch.tv/", title: "Twitch", weight: 6, bookmarkable: true },
    SiteEntry { url: "https://discord.com/", title: "Discord", weight: 8, bookmarkable: true },
    SiteEntry { url: "https://www.spotify.com/", title: "Spotify", weight: 7, bookmarkable: true },
    SiteEntry { url: "https://chat.openai.com/", title: "ChatGPT", weight: 10, bookmarkable: true },
];

const SITES_US: [SiteEntry; 20] = [
    SiteEntry { url: "https://www.cnn.com/", title: "CNN", weight: 12, bookmarkable: true },
    SiteEntry { url: "https://www.nytimes.com/", title: "The New York Times", weight: 10, bookmarkable: true },
    SiteEntry { url: "https://www.washingtonpost.com/", title: "Washington Post", weight: 8, bookmarkable: true },
    SiteEntry { url: "https://www.foxnews.com/", title: "Fox News", weight: 8, bookmarkable: false },
    SiteEntry { url: "https://www.espn.com/", title: "ESPN", weight: 10, bookmarkable: true },
    SiteEntry { url: "https://www.walmart.com/", title: "Walmart", weight: 10, bookmarkable: true },
    SiteEntry { url: "https://www.target.com/", title: "Target", weight: 7, bookmarkable: true },
    SiteEntry { url: "https://www.bestbuy.com/", title: "Best Buy", weight: 6, bookmarkable: true },
    SiteEntry { url: "https://www.ebay.com/", title: "eBay", weight: 8, bookmarkable: true },
    SiteEntry { url: "https://www.chase.com/", title: "Chase", weight: 8, bookmarkable: true },
    SiteEntry { url: "https://www.bankofamerica.com/", title: "Bank of America", weight: 6, bookmarkable: true },
    SiteEntry { url: "https://www.weather.com/", title: "The Weather Channel", weight: 7, bookmarkable: false },
    SiteEntry { url: "https://www.hulu.com/", title: "Hulu", weight: 5, bookmarkable: true },
    SiteEntry { url: "https://www.irs.gov/", title: "IRS", weight: 3, bookmarkable: false },
    SiteEntry { url: "https://www.usps.com/", title: "USPS", weight: 5, bookmarkable: true },
    SiteEntry { url: "https://www.zillow.com/", title: "Zillow", weight: 5, bookmarkable: true },
    SiteEntry { url: "https://www.homedepot.com/", title: "The Home Depot", weight: 5, bookmarkable: true },
    SiteEntry { url: "https://www.costco.com/", title: "Costco", weight: 5, bookmarkable: true },
    SiteEntry { url: "https://www.paypal.com/", title: "PayPal", weight: 6, bookmarkable: true },
    SiteEntry { url: "https://www.yelp.com/", title: "Yelp", weight: 5, bookmarkable: false },
];

const SITES_GB: [SiteEntry; 18] = [
    SiteEntry { url: "https://www.bbc.co.uk/", title: "BBC", weight: 18, bookmarkable: true },
    SiteEntry { url: "https://www.theguardian.com/", title: "The Guardian", weight: 10, bookmarkable: true },
    SiteEntry { url: "https://www.dailymail.co.uk/", title: "Daily Mail", weight: 8, bookmarkable: false },
    SiteEntry { url: "https://www.telegraph.co.uk/", title: "The Telegraph", weight: 6, bookmarkable: true },
    SiteEntry { url: "https://www.sky.com/", title: "Sky", weight: 7, bookmarkable: true },
    SiteEntry { url: "https://www.amazon.co.uk/", title: "Amazon UK", weight: 15, bookmarkable: true },
    SiteEntry { url: "https://www.ebay.co.uk/", title: "eBay UK", weight: 8, bookmarkable: true },
    SiteEntry { url: "https://www.argos.co.uk/", title: "Argos", weight: 6, bookmarkable: true },
    SiteEntry { url: "https://www.tesco.com/", title: "Tesco", weight: 7, bookmarkable: true },
    SiteEntry { url: "https://www.sainsburys.co.uk/", title: "Sainsbury's", weight: 5, bookmarkable: true },
    SiteEntry { url: "https://www.gov.uk/", title: "GOV.UK", weight: 8, bookmarkable: true },
    SiteEntry { url: "https://www.nhs.uk/", title: "NHS", weight: 6, bookmarkable: true },
    SiteEntry { url: "https://www.rightmove.co.uk/", title: "Rightmove", weight: 5, bookmarkable: true },
    SiteEntry { url: "https://www.bbc.co.uk/sport", title: "BBC Sport", weight: 8, bookmarkable: true },
    SiteEntry { url: "https://www.natwest.com/", title: "NatWest", weight: 5, bookmarkable: true },
    SiteEntry { url: "https://www.hsbc.co.uk/", title: "HSBC UK", weight: 5, bookmarkable: true },
    SiteEntry { url: "https://www.ocado.com/", title: "Ocado", weight: 4, bookmarkable: true },
    SiteEntry { url: "https://www.asos.com/", title: "ASOS", weight: 5, bookmarkable: true },
];

const SITES_DE: [SiteEntry; 18] = [
    SiteEntry { url: "https://www.spiegel.de/", title: "Der Spiegel", weight: 12, bookmarkable: true },
    SiteEntry { url: "https://www.bild.de/", title: "Bild", weight: 10, bookmarkable: false },
    SiteEntry { url: "https://www.zeit.de/", title: "Die Zeit", weight: 8, bookmarkable: true },
    SiteEntry { url: "https://www.faz.net/", title: "FAZ", weight: 7, bookmarkable: true },
    SiteEntry { url: "https://www.tagesschau.de/", title: "Tagesschau", weight: 10, bookmarkable: true },
    SiteEntry { url: "https://www.amazon.de/", title: "Amazon.de", weight: 18, bookmarkable: true },
    SiteEntry { url: "https://www.ebay.de/", title: "eBay Kleinanzeigen", weight: 8, bookmarkable: true },
    SiteEntry { url: "https://www.otto.de/", title: "Otto", weight: 6, bookmarkable: true },
    SiteEntry { url: "https://www.zalando.de/", title: "Zalando", weight: 7, bookmarkable: true },
    SiteEntry { url: "https://www.mediamarkt.de/", title: "MediaMarkt", weight: 5, bookmarkable: true },
    SiteEntry { url: "https://www.dhl.de/", title: "DHL", weight: 6, bookmarkable: true },
    SiteEntry { url: "https://www.deutsche-bank.de/", title: "Deutsche Bank", weight: 5, bookmarkable: true },
    SiteEntry { url: "https://www.commerzbank.de/", title: "Commerzbank", weight: 4, bookmarkable: true },
    SiteEntry { url: "https://www.check24.de/", title: "CHECK24", weight: 6, bookmarkable: true },
    SiteEntry { url: "https://www.heise.de/", title: "Heise", weight: 7, bookmarkable: true },
    SiteEntry { url: "https://www.mobile.de/", title: "mobile.de", weight: 5, bookmarkable: true },
    SiteEntry { url: "https://www.immobilienscout24.de/", title: "ImmobilienScout24", weight: 4, bookmarkable: true },
    SiteEntry { url: "https://www.wetter.de/", title: "Wetter.de", weight: 5, bookmarkable: false },
];

const SITES_FR: [SiteEntry; 18] = [
    SiteEntry { url: "https://www.lemonde.fr/", title: "Le Monde", weight: 12, bookmarkable: true },
    SiteEntry { url: "https://www.lefigaro.fr/", title: "Le Figaro", weight: 8, bookmarkable: true },
    SiteEntry { url: "https://www.liberation.fr/", title: "Libération", weight: 6, bookmarkable: true },
    SiteEntry { url: "https://www.francetvinfo.fr/", title: "Franceinfo", weight: 8, bookmarkable: true },
    SiteEntry { url: "https://www.bfmtv.com/", title: "BFMTV", weight: 7, bookmarkable: false },
    SiteEntry { url: "https://www.amazon.fr/", title: "Amazon.fr", weight: 15, bookmarkable: true },
    SiteEntry { url: "https://www.leboncoin.fr/", title: "Leboncoin", weight: 10, bookmarkable: true },
    SiteEntry { url: "https://www.cdiscount.com/", title: "Cdiscount", weight: 6, bookmarkable: true },
    SiteEntry { url: "https://www.fnac.com/", title: "Fnac", weight: 5, bookmarkable: true },
    SiteEntry { url: "https://www.laposte.fr/", title: "La Poste", weight: 5, bookmarkable: true },
    SiteEntry { url: "https://www.orange.fr/", title: "Orange", weight: 7, bookmarkable: true },
    SiteEntry { url: "https://www.free.fr/", title: "Free", weight: 5, bookmarkable: true },
    SiteEntry { url: "https://www.boursorama.com/", title: "Boursorama", weight: 6, bookmarkable: true },
    SiteEntry { url: "https://www.service-public.fr/", title: "Service-Public.fr", weight: 5, bookmarkable: true },
    SiteEntry { url: "https://www.lequipe.fr/", title: "L'Équipe", weight: 8, bookmarkable: true },
    SiteEntry { url: "https://www.seloger.com/", title: "SeLoger", weight: 4, bookmarkable: true },
    SiteEntry { url: "https://www.marmiton.org/", title: "Marmiton", weight: 4, bookmarkable: true },
    SiteEntry { url: "https://www.meteofrance.com/", title: "Météo-France", weight: 6, bookmarkable: false },
];

const SITES_JP: [SiteEntry; 18] = [
    SiteEntry { url: "https://www.yahoo.co.jp/", title: "Yahoo! JAPAN", weight: 20, bookmarkable: true },
    SiteEntry { url: "https://www.nhk.or.jp/", title: "NHK", weight: 10, bookmarkable: true },
    SiteEntry { url: "https://www.asahi.com/", title: "朝日新聞", weight: 8, bookmarkable: true },
    SiteEntry { url: "https://www.nikkei.com/", title: "日本経済新聞", weight: 8, bookmarkable: true },
    SiteEntry { url: "https://www.mainichi.jp/", title: "毎日新聞", weight: 6, bookmarkable: true },
    SiteEntry { url: "https://www.amazon.co.jp/", title: "Amazon.co.jp", weight: 18, bookmarkable: true },
    SiteEntry { url: "https://www.rakuten.co.jp/", title: "楽天市場", weight: 15, bookmarkable: true },
    SiteEntry { url: "https://kakaku.com/", title: "価格.com", weight: 8, bookmarkable: true },
    SiteEntry { url: "https://www.mercari.com/jp/", title: "メルカリ", weight: 7, bookmarkable: true },
    SiteEntry { url: "https://www.zozo.jp/", title: "ZOZOTOWN", weight: 5, bookmarkable: true },
    SiteEntry { url: "https://tabelog.com/", title: "食べログ", weight: 6, bookmarkable: true },
    SiteEntry { url: "https://www.hotpepper.jp/", title: "ホットペッパー", weight: 5, bookmarkable: true },
    SiteEntry { url: "https://www.nicovideo.jp/", title: "ニコニコ動画", weight: 6, bookmarkable: true },
    SiteEntry { url: "https://note.com/", title: "note", weight: 5, bookmarkable: true },
    SiteEntry { url: "https://www.smbc.co.jp/", title: "三井住友銀行", weight: 5, bookmarkable: true },
    SiteEntry { url: "https://www.mufg.jp/", title: "三菱UFJ銀行", weight: 5, bookmarkable: true },
    SiteEntry { url: "https://www.jma.go.jp/", title: "気象庁", weight: 5, bookmarkable: false },
    SiteEntry { url: "https://www.cookpad.com/", title: "クックパッド", weight: 5, bookmarkable: true },
];

const SITES_BR: [SiteEntry; 18] = [
    SiteEntry { url: "https://www.globo.com/", title: "Globo", weight: 15, bookmarkable: true },
    SiteEntry { url: "https://www.uol.com.br/", title: "UOL", weight: 12, bookmarkable: true },
    SiteEntry { url: "https://g1.globo.com/", title: "G1", weight: 10, bookmarkable: true },
    SiteEntry { url: "https://www.folha.uol.com.br/", title: "Folha de S.Paulo", weight: 8, bookmarkable: true },
    SiteEntry { url: "https://www.estadao.com.br/", title: "Estadão", weight: 6, bookmarkable: true },
    SiteEntry { url: "https://www.mercadolivre.com.br/", title: "Mercado Livre", weight: 15, bookmarkable: true },
    SiteEntry { url: "https://www.magazineluiza.com.br/", title: "Magazine Luiza", weight: 8, bookmarkable: true },
    SiteEntry { url: "https://www.americanas.com.br/", title: "Americanas", weight: 7, bookmarkable: true },
    SiteEntry { url: "https://www.casasbahia.com.br/", title: "Casas Bahia", weight: 5, bookmarkable: true },
    SiteEntry { url: "https://www.amazon.com.br/", title: "Amazon Brasil", weight: 10, bookmarkable: true },
    SiteEntry { url: "https://www.ifood.com.br/", title: "iFood", weight: 8, bookmarkable: true },
    SiteEntry { url: "https://www.gov.br/", title: "Gov.br", weight: 6, bookmarkable: true },
    SiteEntry { url: "https://www.bb.com.br/", title: "Banco do Brasil", weight: 6, bookmarkable: true },
    SiteEntry { url: "https://www.itau.com.br/", title: "Itaú", weight: 6, bookmarkable: true },
    SiteEntry { url: "https://www.nubank.com.br/", title: "Nubank", weight: 7, bookmarkable: true },
    SiteEntry { url: "https://www.correios.com.br/", title: "Correios", weight: 5, bookmarkable: true },
    SiteEntry { url: "https://www.terra.com.br/", title: "Terra", weight: 5, bookmarkable: false },
    SiteEntry { url: "https://ge.globo.com/", title: "ge", weight: 7, bookmarkable: true },
];

const SITES_IN: [SiteEntry; 18] = [
    SiteEntry { url: "https://www.ndtv.com/", title: "NDTV", weight: 10, bookmarkable: true },
    SiteEntry { url: "https://timesofindia.indiatimes.com/", title: "Times of India", weight: 10, bookmarkable: true },
    SiteEntry { url: "https://www.thehindu.com/", title: "The Hindu", weight: 7, bookmarkable: true },
    SiteEntry { url: "https://www.hindustantimes.com/", title: "Hindustan Times", weight: 7, bookmarkable: true },
    SiteEntry { url: "https://indianexpress.com/", title: "Indian Express", weight: 6, bookmarkable: true },
    SiteEntry { url: "https://www.flipkart.com/", title: "Flipkart", weight: 15, bookmarkable: true },
    SiteEntry { url: "https://www.amazon.in/", title: "Amazon India", weight: 15, bookmarkable: true },
    SiteEntry { url: "https://www.myntra.com/", title: "Myntra", weight: 7, bookmarkable: true },
    SiteEntry { url: "https://www.snapdeal.com/", title: "Snapdeal", weight: 4, bookmarkable: true },
    SiteEntry { url: "https://www.swiggy.com/", title: "Swiggy", weight: 8, bookmarkable: true },
    SiteEntry { url: "https://www.zomato.com/", title: "Zomato", weight: 8, bookmarkable: true },
    SiteEntry { url: "https://www.hotstar.com/", title: "Hotstar", weight: 8, bookmarkable: true },
    SiteEntry { url: "https://www.irctc.co.in/", title: "IRCTC", weight: 6, bookmarkable: true },
    SiteEntry { url: "https://www.sbi.co.in/", title: "State Bank of India", weight: 6, bookmarkable: true },
    SiteEntry { url: "https://www.hdfcbank.com/", title: "HDFC Bank", weight: 6, bookmarkable: true },
    SiteEntry { url: "https://www.icicibank.com/", title: "ICICI Bank", weight: 5, bookmarkable: true },
    SiteEntry { url: "https://paytm.com/", title: "Paytm", weight: 7, bookmarkable: true },
    SiteEntry { url: "https://www.cricbuzz.com/", title: "Cricbuzz", weight: 8, bookmarkable: true },
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn supported_countries_return_more_than_global() {
        let global_count = SITES_GLOBAL.len();
        for country in ["US", "GB", "DE", "FR", "JP", "BR", "IN"] {
            let sites = sites_for_country(country);
            assert!(
                sites.len() > global_count,
                "{country} returned only {}, expected more than global ({global_count})",
                sites.len()
            );
        }
    }

    #[test]
    fn unsupported_country_falls_back_to_global() {
        let sites = sites_for_country("ZZ");
        assert_eq!(sites.len(), SITES_GLOBAL.len());
    }

    #[test]
    fn all_urls_are_https() {
        for country in ["US", "GB", "DE", "FR", "JP", "BR", "IN", "ZZ"] {
            for site in sites_for_country(country) {
                assert!(
                    site.url.starts_with("https://"),
                    "{} does not start with https://",
                    site.url
                );
            }
        }
    }

    #[test]
    fn all_weights_are_positive() {
        for country in ["US", "GB", "DE", "FR", "JP", "BR", "IN"] {
            for site in sites_for_country(country) {
                assert!(site.weight > 0, "{} has zero weight", site.url);
            }
        }
    }
}
