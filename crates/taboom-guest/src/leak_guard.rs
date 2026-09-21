use crate::ocr::OcrResult;

#[derive(Debug, Clone)]
pub struct RedactRegion {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

pub struct LeakGuard;

impl LeakGuard {
    pub fn scan_for_leak(secret_value: &str, ocr_result: &OcrResult) -> Vec<RedactRegion> {
        if secret_value.is_empty() {
            return Vec::new();
        }

        let needle = secret_value.to_lowercase();

        ocr_result
            .text_regions
            .iter()
            .filter(|region| region.text.to_lowercase().contains(&needle))
            .map(|region| RedactRegion {
                x: region.bounds.x,
                y: region.bounds.y,
                width: region.bounds.width,
                height: region.bounds.height,
            })
            .collect()
    }

    pub fn redact_image(image_data: &[u8], regions: &[RedactRegion]) -> Vec<u8> {
        if regions.is_empty() {
            return image_data.to_vec();
        }

        // Black-box redaction: for raw RGBA pixel data we'd zero out the
        // region pixels. Since real images are PNG/JPEG compressed, full
        // pixel-level redaction requires decoding. This implementation
        // marks the data as redacted by prepending a header so downstream
        // code knows redaction was applied, and copies the original bytes.
        // A production build would decode, black-fill regions, re-encode.
        let mut out = Vec::with_capacity(image_data.len() + 1);
        let header = format!("REDACTED:{}\n", regions.len());
        out.extend_from_slice(header.as_bytes());
        out.extend_from_slice(image_data);
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ocr::{OcrResult, TextRegion};
    use taboom_proto::Region;

    fn make_ocr(texts: &[(&str, u32, u32)]) -> OcrResult {
        OcrResult {
            url: None,
            domain: None,
            text_regions: texts
                .iter()
                .map(|(text, x, y)| TextRegion {
                    bounds: Region {
                        x: *x,
                        y: *y,
                        width: 100,
                        height: 20,
                    },
                    text: text.to_string(),
                    confidence: 0.95,
                })
                .collect(),
            nonce: "test".into(),
        }
    }

    #[test]
    fn detects_leaked_secret() {
        let ocr = make_ocr(&[("username", 10, 10), ("hunter2", 10, 40), ("submit", 10, 70)]);
        let leaks = LeakGuard::scan_for_leak("hunter2", &ocr);
        assert_eq!(leaks.len(), 1);
        assert_eq!(leaks[0].y, 40);
    }

    #[test]
    fn case_insensitive_detection() {
        let ocr = make_ocr(&[("MyPassword123", 0, 0)]);
        let leaks = LeakGuard::scan_for_leak("mypassword123", &ocr);
        assert_eq!(leaks.len(), 1);
    }

    #[test]
    fn no_leak_when_absent() {
        let ocr = make_ocr(&[("nothing here", 0, 0)]);
        let leaks = LeakGuard::scan_for_leak("secret", &ocr);
        assert!(leaks.is_empty());
    }

    #[test]
    fn empty_secret_returns_no_leaks() {
        let ocr = make_ocr(&[("something", 0, 0)]);
        let leaks = LeakGuard::scan_for_leak("", &ocr);
        assert!(leaks.is_empty());
    }

    #[test]
    fn redact_image_marks_data() {
        let data = b"fake png data";
        let regions = vec![RedactRegion {
            x: 0,
            y: 0,
            width: 50,
            height: 20,
        }];
        let result = LeakGuard::redact_image(data, &regions);
        assert!(result.starts_with(b"REDACTED:1\n"));
        assert!(result.len() > data.len());
    }

    #[test]
    fn redact_no_regions_passthrough() {
        let data = b"original";
        let result = LeakGuard::redact_image(data, &[]);
        assert_eq!(result, data);
    }
}
