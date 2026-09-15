use serde_json::{Value, json};

/// Static fallback values used when live scraping from music.youtube.com fails.
/// Mirrors the approach of LastWave, which scrapes + hardcodes fallbacks.
pub const FALLBACK_API_KEY: &str = "AIzaSyAO_FJ2SlqU8Q4STEHLGCilw_Y9_11qcW8";
pub const FALLBACK_CLIENT_VERSION: &str = "1.20250605.01.00";
/// Known-good visitor data (base64) used by InnerTune when none can be scraped.
pub const FALLBACK_VISITOR_DATA: &str = "CgtsZG1ySnZiQWtSbyiMjuGSBg%3D%3D";

#[derive(Debug, Clone)]
pub struct InnertubeConfig {
    pub api_key: String,
    pub client_version: String,
    pub visitor_data: String,
    pub client_name: &'static str,
    pub client_name_id: u8,
    pub hl: String,
    pub gl: String,
}

impl Default for InnertubeConfig {
    fn default() -> Self {
        Self {
            api_key: FALLBACK_API_KEY.to_string(),
            client_version: FALLBACK_CLIENT_VERSION.to_string(),
            visitor_data: FALLBACK_VISITOR_DATA.to_string(),
            client_name: "WEB_REMIX",
            client_name_id: 67,
            hl: "en".to_string(),
            gl: "US".to_string(),
        }
    }
}

/// Scrape the web client's live config JSON from the music.youtube.com page
/// so we always send the current API key / client version. Falls back to
/// hardcoded values on any failure.
pub async fn scrape() -> InnertubeConfig {
    let mut cfg = InnertubeConfig::default();
    let client = crate::innertube::http_client();
    let Ok(res) = client.get("https://music.youtube.com").send().await else {
        return cfg;
    };
    let Ok(html) = res.text().await else {
        return cfg;
    };
    if let Some(key) = extract("\"INNERTUBE_API_KEY\":\"", &html) {
        cfg.api_key = key;
    }
    if let Some(ver) = extract("\"INNERTUBE_CONTEXT_CLIENT_VERSION\":\"", &html) {
        cfg.client_version = ver;
    }
    if let Some(visitor) = extract("\"VISITOR_DATA\":\"", &html) {
        cfg.visitor_data = visitor;
    }
    cfg
}

fn extract(before: &str, html: &str) -> Option<String> {
    let start = html.find(before)? + before.len();
    let rest = &html[start..];
    let end = rest.find('"')?;
    let val = &rest[..end];
    if val.is_empty() {
        None
    } else {
        Some(val.to_string())
    }
}

/// Build the `context` object sent in every youtubei request body.
pub fn context(
    cfg: &InnertubeConfig,
    client_name: &'static str,
    client_name_id: u8,
    client_version: &str,
    visitor_data: &str,
) -> Value {
    json!({
        "client": {
            "clientName": client_name,
            "clientVersion": client_version,
            "androidSdkVersion": 30,
            "hl": cfg.hl,
            "gl": cfg.gl,
            "visitorData": visitor_data,
            "userAgent": user_agent(client_name_id),
            "osName": "Linux",
            "osVersion": "6",
            "timeZone": "UTC",
            "platform": "DESKTOP"
        }
    })
}

pub fn user_agent(_client_name_id: u8) -> String {
    "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/130.0.0.0 Safari/537.36"
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_quoted_field() {
        let html = r#"var ytcfg = {"INNERTUBE_API_KEY":"AIzaSyABC123","VISITOR_DATA":"CgtsZG1ySnZiQWtSbyiMjuGSBg%3D%3D"};"#;
        assert_eq!(
            extract("\"INNERTUBE_API_KEY\":\"", html).as_deref(),
            Some("AIzaSyABC123")
        );
        assert_eq!(
            extract("\"VISITOR_DATA\":\"", html).as_deref(),
            Some("CgtsZG1ySnZiQWtSbyiMjuGSBg%3D%3D")
        );
    }

    #[test]
    fn falls_back_when_missing() {
        assert_eq!(extract("\"MISSING\":\"", r#"no such"#), None);
    }
}
