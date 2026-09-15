use reqwest::Client;
use serde_json::{Value, json};

use super::{config::InnertubeConfig, is_login_required};

/// A resolved, directly playable audio stream.
#[derive(Debug, Clone)]
pub struct StreamFormat {
    pub url: String,
    pub itag: Option<u32>,
    pub bitrate: Option<u32>,
}

/// A YouTube innerTube player client we are willing to resolve streams with.
struct ClientDef {
    name: &'static str,
    id: u8,
    version: &'static str,
    base: &'static str,
    ua: &'static str,
    device_make: &'static str,
    device_model: &'static str,
    os_name: &'static str,
    os_version: &'static str,
}

/// Resolution order. VISIONOS is today's reliable pre-signed client (the one
/// yt-dlp defaults to upstream); IOS and ANDROID_VR cover older/restricted
/// content and are tried afterwards.
const CLIENTS: [ClientDef; 3] = [
    ClientDef {
        name: "VISIONOS",
        id: 101,
        version: "1.02",
        base: "https://www.youtube.com",
        ua: "Mozilla/5.0 (Macintosh; Intel Mac OS X 15_7_3) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/26.0 Safari/605.1.15",
        device_make: "Apple",
        device_model: "RealityDevice17,1",
        os_name: "visionOS",
        os_version: "26.5.23O471",
    },
    ClientDef {
        name: "IOS",
        id: 5,
        version: "21.26.4",
        base: "https://music.youtube.com",
        ua: "com.google.ios.youtube/21.26.4 (iPhone16,2; U; CPU iOS 18_3_2 like Mac OS X;)",
        device_make: "Apple",
        device_model: "iPhone16,2",
        os_name: "iPhone",
        os_version: "18.3.2.22D82",
    },
    ClientDef {
        name: "ANDROID_VR",
        id: 28,
        version: "1.65.10",
        base: "https://music.youtube.com",
        ua: "com.google.android.apps.youtube.vr.oculus/1.65.10 (Linux; U; Android 12L; eureka-user Build/SQ3A.220605.009.A1) gzip",
        device_make: "Oculus",
        device_model: "Quest 3",
        os_name: "Android",
        os_version: "12L",
    },
];

/// Preferred audio itags (highest quality first). VISIONOS serves opus (251)
/// in addition to the m4a family that the mobile clients expose.
const PREFERRED_ITAGS: [u32; 4] = [251, 140, 141, 139];

/// Resolve a playable, download-verified stream URL for a video id.
///
/// Strategy (validated against live YouTube Music in 2026):
/// - WEB-family clients return UNPLAYABLE for essentially all music; the
///   `VISIONOS` client (yt-dlp's own default) returns playable audio with
///   pre-signed URLs for every test video.
/// - Every candidate URL is *verified* with a small ranged GET before it is
///   accepted; dead URLs (bot-gate) are skipped.
/// - If a URL carries an `n` challenge, we attempt nsig deciphering.
/// - If every client fails, fall back to the `yt-dlp` binary, which bundles
///   its own JS challenge solvers and keeps working when the direct path
///   drifts.
/// - On LOGIN_REQUIRED the visitor data is stale: re-scrape and retry once.
pub async fn resolve_stream(
    client: &Client,
    cfg: &mut InnertubeConfig,
    video_id: &str,
) -> anyhow::Result<StreamFormat> {
    for def in CLIENTS {
        for attempt in 0..2 {
            let value = player_request(client, cfg, video_id, &def).await?;
            if is_login_required(&value) {
                if attempt == 0 {
                    *cfg = super::config::scrape().await;
                    continue;
                }
                break;
            }
            if let Some(fmt) = best_audio(&value) {
                if verify_url(client, &fmt.url).await {
                    return Ok(fmt);
                }
                if let Some(deciphered) = super::nsig::maybe_decipher(client, &fmt.url).await? {
                    let fixed = StreamFormat {
                        url: deciphered.url,
                        ..fmt.clone()
                    };
                    if verify_url(client, &fixed.url).await {
                        return Ok(fixed);
                    }
                }
            }
            break;
        }
    }
    super::ytdl::resolve("yt-dlp", video_id).await
}

async fn player_request(
    client: &Client,
    cfg: &InnertubeConfig,
    video_id: &str,
    def: &ClientDef,
) -> anyhow::Result<Value> {
    let context = json!({
        "client": {
            "clientName": def.name,
            "clientVersion": def.version,
            "deviceMake": def.device_make,
            "deviceModel": def.device_model,
            "osName": def.os_name,
            "osVersion": def.os_version,
            "hl": cfg.hl,
            "gl": cfg.gl,
            "visitorData": cfg.visitor_data,
            "userAgent": def.ua,
        }
    });
    let body = json!({
        "context": context,
        "videoId": video_id,
        "contentCheckOk": true,
        "racyCheckOk": true,
    });
    let url = format!(
        "{}/youtubei/v1/player?key={}&prettyPrint=false",
        def.base, cfg.api_key
    );
    let res = client
        .post(&url)
        .header("X-Goog-Api-Format-Version", "1")
        .header("X-YouTube-Client-Name", def.id.to_string())
        .header("X-YouTube-Client-Version", def.version)
        .header("X-Goog-Visitor-Id", &cfg.visitor_data)
        .header("Origin", def.base)
        .header("Referer", format!("{}/", def.base))
        .header("User-Agent", def.ua)
        .json(&body)
        .send()
        .await?;
    let status = res.status();
    let text = res.text().await?;
    let value: Value = serde_json::from_str(&text)
        .map_err(|e| anyhow::anyhow!("player returned non-JSON (HTTP {status}): {e}"))?;
    Ok(value)
}

/// Confirm a stream URL actually serves audio with a small ranged GET.
async fn verify_url(client: &Client, url: &str) -> bool {
    let res = match client.get(url).header("Range", "bytes=0-2047").send().await {
        Ok(r) => r,
        Err(_) => return false,
    };
    let status = res.status();
    status.is_success() || status == reqwest::StatusCode::PARTIAL_CONTENT
}

/// Pick the best audio-only format from an adaptiveFormats list.
/// Formats behind a `signatureCipher` are ignored for now.
fn best_audio(response: &Value) -> Option<StreamFormat> {
    let formats = response
        .pointer("/streamingData/adaptiveFormats")
        .and_then(Value::as_array)?;

    let mut usable: Vec<StreamFormat> = Vec::new();
    for f in formats {
        let mime = f.get("mimeType").and_then(Value::as_str).unwrap_or("");
        if !mime.starts_with("audio/") {
            continue;
        }
        let url = match f.get("url").and_then(Value::as_str) {
            Some(u) if u.starts_with("http") => u.to_string(),
            _ => continue,
        };
        usable.push(StreamFormat {
            url,
            itag: f.get("itag").and_then(Value::as_u64).map(|v| v as u32),
            bitrate: f.get("bitrate").and_then(Value::as_u64).map(|v| v as u32),
        });
    }
    if usable.is_empty() {
        return None;
    }

    usable.sort_by_key(|f| {
        let pref = f
            .itag
            .and_then(|itag| PREFERRED_ITAGS.iter().position(|&x| x == itag))
            .unwrap_or(4);
        // Prefer lower index (higher itag priority), then higher bitrate.
        let anti_bitrate = i32::MAX - f.bitrate.unwrap_or(0) as i32;
        (pref, anti_bitrate)
    });

    usable.into_iter().next()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::innertube::json_of;

    fn sample_formats(audiourl: bool) -> Value {
        let audio_field = if audiourl {
            r#""url": "https://rr.example/audio140""#
        } else {
            r#""signatureCipher": "s=abc&sp=sig&url=https%3A%2F%2Frr.example%2Fciphered140""#
        };
        let opus_field = if audiourl {
            r#""url": "https://rr.example/audio251""#
        } else {
            r#""signatureCipher": "s=abc&sp=sig&url=https%3A%2F%2Frr.example%2Fciphered251""#
        };
        json_of(&format!(
            r#"{{
                "streamingData": {{
                    "adaptiveFormats": [
                        {{
                            "itag": 397,
                            "mimeType": "video/mp4",
                            "bitrate": 2000000,
                            "url": "https://rr.example/video"
                        }},
                        {{
                            "itag": 140,
                            "mimeType": "audio/mp4",
                            "bitrate": 129000,
                            {audio_field}
                        }},
                        {{
                            "itag": 251,
                            "mimeType": "audio/webm",
                            "bitrate": 160000,
                            {opus_field}
                        }}
                    ]
                }}
            }}"#
        ))
    }

    #[test]
    fn picks_preferred_audio() {
        let fmt = best_audio(&sample_formats(true)).unwrap();
        // itag 251 is preferred over 140.
        assert_eq!(fmt.itag, Some(251));
        assert_eq!(fmt.url, "https://rr.example/audio251");
    }

    #[test]
    fn returns_none_when_only_ciphered() {
        assert!(best_audio(&sample_formats(false)).is_none());
        assert!(best_audio(&json_of(r#"{"streamingData":{}}"#)).is_none());
    }
}
