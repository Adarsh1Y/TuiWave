pub mod config;
pub mod nsig;
pub mod player;
pub mod search;
pub mod ytdl;

use std::time::Duration;

use anyhow::Context;
use reqwest::Client;
use serde_json::Value;

use config::{InnertubeConfig, context};

pub const SEARCH_FILTER_SONGS: &str = "EgWKAQIIAWoKEAoQCRADEAA%3D";
const MUSIC_BASE: &str = "https://music.youtube.com/youtubei/v1";

pub fn http_client() -> Client {
    Client::builder()
        .timeout(Duration::from_secs(20))
        .user_agent("Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/130.0.0.0 Safari/537.36")
        .build()
        .expect("reqwest client builder cannot fail")
}

/// Send a POST to a youtubei endpoint and return the raw JSON body.
async fn post_json(
    client: &Client,
    cfg: &InnertubeConfig,
    endpoint: &str,
    body: Value,
) -> anyhow::Result<Value> {
    let url = format!(
        "{MUSIC_BASE}/{endpoint}?key={}&prettyPrint=false",
        cfg.api_key
    );
    let res = client
        .post(&url)
        .header("X-Goog-Api-Format-Version", "1")
        .header("X-YouTube-Client-Name", cfg.client_name_id.to_string())
        .header("X-YouTube-Client-Version", &cfg.client_version)
        .header("X-Goog-Visitor-Id", &cfg.visitor_data)
        .header("Origin", "https://music.youtube.com")
        .header("Referer", "https://music.youtube.com/")
        .json(&body)
        .send()
        .await
        .context("innertube request failed")?;

    let status = res.status();
    let text = res.text().await.context("read innertube response")?;
    let value: Value = serde_json::from_str(&text)
        .with_context(|| format!("innertube returned non-JSON (HTTP {status})"))?;
    Ok(value)
}

/// Detect the "valid-looking but logged-out" canary that indicates the
/// visitor data has gone stale (HTTP 200 with LOGIN_REQUIRED / empty data).
fn is_login_required(value: &Value) -> bool {
    value
        .pointer("/playabilityStatus/status")
        .and_then(Value::as_str)
        .map(|s| s == "LOGIN_REQUIRED")
        .unwrap_or(false)
}

/// Safe text extraction: concatenates `.text` runs of a renderer field.
fn run_text(node: &Value) -> Option<String> {
    let runs = node.pointer("/runs")?.as_array()?;
    let mut out = String::new();
    for run in runs {
        if let Some(text) = run.get("text").and_then(Value::as_str) {
            out.push_str(text);
        }
    }
    if out.is_empty() { None } else { Some(out) }
}

pub use player::StreamFormat;

/// Resolve a direct, playable stream URL for a track. Tries the VISIONOS
/// client first (pre-signed URLs, verified with a ranged GET), then the other
/// clients, then nsig deciphering, and finally the `yt-dlp` binary.
pub async fn resolve_stream(
    client: &Client,
    cfg: &mut InnertubeConfig,
    video_id: &str,
) -> anyhow::Result<StreamFormat> {
    player::resolve_stream(client, cfg, video_id)
        .await
        .with_context(|| format!("failed to resolve stream for {video_id}"))
}

mod test_helpers {
    use super::*;

    #[allow(dead_code)]
    pub fn json_of(s: &str) -> Value {
        serde_json::from_str(s).unwrap()
    }
}

#[cfg(test)]
pub use test_helpers::json_of;

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn login_required_detection_works() {
        let bad = json!({"playabilityStatus": {"status": "LOGIN_REQUIRED"}});
        assert!(is_login_required(&bad));
        let ok = json!({"playabilityStatus": {"status": "OK"}});
        assert!(!is_login_required(&ok));
    }

    #[test]
    fn run_text_extracts_first_run() {
        let node = json!({"runs": [{"text": "Hello"}, {"text": "World"}]});
        assert_eq!(run_text(&node).as_deref(), Some("HelloWorld"));
    }
}
