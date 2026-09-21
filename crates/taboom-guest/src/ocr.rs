use serde::{Deserialize, Serialize};
use taboom_proto::Region;
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OcrResult {
    pub url: Option<String>,
    pub domain: Option<String>,
    pub text_regions: Vec<TextRegion>,
    pub nonce: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TextRegion {
    pub bounds: Region,
    pub text: String,
    pub confidence: f64,
}

pub trait OcrBackend: Send + Sync {
    fn recognize(&self, image_data: &[u8]) -> anyhow::Result<Vec<TextRegion>>;
}

pub struct TesseractBackend;

impl OcrBackend for TesseractBackend {
    fn recognize(&self, _image_data: &[u8]) -> anyhow::Result<Vec<TextRegion>> {
        anyhow::bail!("tesseract OCR requires tesseract binary installed")
    }
}

pub fn perform_ocr(
    image_data: &[u8],
    backend: &dyn OcrBackend,
) -> anyhow::Result<OcrResult> {
    let nonce = Uuid::new_v4().to_string();
    let regions = backend.recognize(image_data)?;

    let url = regions.iter().find_map(|r| extract_url(&r.text));
    let domain = url.as_ref().and_then(|u| extract_domain(u));

    Ok(OcrResult {
        url,
        domain,
        text_regions: regions,
        nonce,
    })
}

const SECRET_PATTERNS: &[&str] = &[
    "password",
    "secret",
    "api_key",
    "apikey",
    "token",
    "private_key",
    "-----BEGIN",
];

pub fn scan_for_secrets(text: &str) -> Vec<String> {
    let lower = text.to_lowercase();
    SECRET_PATTERNS
        .iter()
        .filter(|p| lower.contains(*p))
        .map(|p| format!("potential secret pattern: {p}"))
        .collect()
}

fn extract_url(text: &str) -> Option<String> {
    for word in text.split_whitespace() {
        if word.starts_with("http://") || word.starts_with("https://") {
            return Some(word.to_string());
        }
    }
    None
}

fn extract_domain(url: &str) -> Option<String> {
    let stripped = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))?;
    Some(stripped.split('/').next()?.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_url_finds_https() {
        assert_eq!(
            extract_url("visit https://example.com/page for info"),
            Some("https://example.com/page".into())
        );
    }

    #[test]
    fn extract_domain_works() {
        assert_eq!(
            extract_domain("https://sub.example.com/path"),
            Some("sub.example.com".into())
        );
    }

    #[test]
    fn secret_scan_detects_patterns() {
        let hits = scan_for_secrets("my API_KEY is abc123");
        assert!(!hits.is_empty());
        let clean = scan_for_secrets("nothing suspicious here");
        assert!(clean.is_empty());
    }
}
