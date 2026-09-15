//! Fetch a YouTube Music playlist through the InnerTube browse endpoint,
//! following continuation pages until every track is loaded.

use anyhow::Context;
use reqwest::Client;
use serde_json::{Value, json};

use crate::model::Track;

use super::config::InnertubeConfig;
use super::{context, post_json};

const MAX_CONTINUATION_PAGES: usize = 50;

#[derive(Debug, Clone)]
pub struct YtPlaylist {
    pub id: String,
    pub title: String,
    pub track_count: usize,
    pub tracks: Vec<Track>,
}

/// Pull a valid playlist id / browse id out of the user's input.
pub fn extract_playlist_id(input: &str) -> Option<String> {
    let raw = input.trim();
    if raw.is_empty() {
        return None;
    }
    if let Some(rest) = raw.split("list=").nth(1) {
        let end = rest.find('&').unwrap_or(rest.len());
        let id = &rest[..end];
        if !id.is_empty() {
            return Some(id.to_string());
        }
    }
    let candidate: String = raw
        .trim_end_matches('/')
        .rsplit('/')
        .next()
        .unwrap_or(raw)
        .to_string();
    if candidate.len() >= 11 {
        Some(candidate)
    } else {
        None
    }
}

fn browse_id_for(id: &str) -> String {
    if id.starts_with("VL")
        || id.starts_with("RDCLAK")
        || id.starts_with("RDAM")
        || id.starts_with("FE")
        || id.starts_with("OLAK")
        || id.starts_with("MPRE")
    {
        id.to_string()
    } else {
        format!("VL{id}")
    }
}

/// Fetch a playlist's tracks, honoring continuation tokens.
pub async fn fetch_playlist(
    client: &Client,
    cfg: &InnertubeConfig,
    id_or_url: &str,
) -> anyhow::Result<YtPlaylist> {
    let id = extract_playlist_id(id_or_url).context("could not parse playlist id/url")?;
    let browse_id = browse_id_for(&id);

    let root = browse(client, cfg, &browse_id, None)
        .await
        .with_context(|| format!("browse failed for playlist {browse_id}"))?;

    let title = playlist_title(&root).unwrap_or_else(|| "Playlist".to_string());

    let mut tracks = collect_playlist_tracks(&root);
    let mut token = continuation_token(&root);
    let mut pages = 0;
    while let Some(t) = token.as_deref() {
        if pages >= MAX_CONTINUATION_PAGES {
            break;
        }
        let page = browse(client, cfg, &browse_id, Some(t))
            .await
            .with_context(|| format!("continuation failed for playlist {browse_id}"))?;
        let more = collect_playlist_tracks(&page);
        let known: Vec<String> = tracks.iter().map(|tr| tr.key()).collect();
        let fresh: Vec<Track> = more
            .into_iter()
            .filter(|tr| !known.contains(&tr.key()))
            .collect();
        if fresh.is_empty() {
            break;
        }
        tracks.extend(fresh);
        token = continuation_token(&page);
        pages += 1;
    }

    Ok(YtPlaylist {
        id,
        title,
        track_count: tracks.len(),
        tracks,
    })
}

async fn browse(
    client: &Client,
    cfg: &InnertubeConfig,
    browse_id: &str,
    continuation: Option<&str>,
) -> anyhow::Result<Value> {
    let body = if let Some(token) = continuation {
        json!({
            "context": context(cfg, cfg.client_name, cfg.client_name_id, &cfg.client_version, &cfg.visitor_data),
            "continuation": token,
        })
    } else {
        json!({
            "context": context(cfg, cfg.client_name, cfg.client_name_id, &cfg.client_version, &cfg.visitor_data),
            "browseId": browse_id,
        })
    };
    post_json(client, cfg, "browse", body).await
}

/// The `musicPlaylistShelfRenderer` carrying the track list. Playlists return
/// either a single-column (mobile) or two-column (desktop web) layout.
fn shelf(root: &Value) -> Option<&Value> {
    root
        .pointer("/contents/singleColumnBrowseResultsRenderer/tabs/0/tabRenderer/content/sectionListRenderer/contents/0/musicPlaylistShelfRenderer")
        .or_else(|| {
            root.pointer("/contents/twoColumnBrowseResultsRenderer/secondaryContents/sectionListRenderer/contents/0/musicPlaylistShelfRenderer")
        })
}

/// Tracks from the initial `musicPlaylistShelfRenderer` contents.
fn collect_playlist_tracks(root: &Value) -> Vec<Track> {
    let Some(item_renders) = shelf(root)
        .and_then(|s| s.get("contents"))
        .and_then(Value::as_array)
    else {
        return Vec::new();
    };
    item_renders
        .iter()
        .filter_map(|it| {
            let renderer = it.get("musicResponsiveListItemRenderer")?;
            super::search::parse_list_item(renderer)
        })
        .collect()
}

fn continuation_token(root: &Value) -> Option<String> {
    let cont = shelf(root)?.get("continuations")?.as_array()?;
    for c in cont {
        if let Some(t) = c
            .pointer("/nextContinuationData/continuation")
            .and_then(Value::as_str)
        {
            return Some(t.to_string());
        }
        if let Some(t) = c
            .pointer("/continuationCommand/token")
            .and_then(Value::as_str)
        {
            return Some(t.to_string());
        }
    }
    None
}

fn playlist_title(root: &Value) -> Option<String> {
    root.pointer("/header/musicDetailHeaderRenderer/title/runs/0/text")
        .and_then(Value::as_str)
        .map(str::to_string)
        .or_else(|| {
            root.pointer("/header/musicResponsiveHeaderRenderer/title/runs/0/text")
                .and_then(Value::as_str)
                .map(str::to_string)
        })
}

#[cfg(test)]
mod tests {
    use super::super::json_of;
    use super::*;

    #[test]
    fn extracts_ids_from_urls() {
        assert_eq!(
            extract_playlist_id("https://music.youtube.com/playlist?list=PL1234567890&si=x")
                .as_deref(),
            Some("PL1234567890")
        );
        assert_eq!(extract_playlist_id("PL1234567890").as_deref(), Some("PL1234567890"));
        assert_eq!(extract_playlist_id("VLPL1234567890").as_deref(), Some("VLPL1234567890"));
        assert_eq!(extract_playlist_id(""), None);
        assert_eq!(extract_playlist_id("  "), None);
    }

    #[test]
    fn builds_browse_ids() {
        assert_eq!(browse_id_for("PL123"), "VLPL123");
        assert_eq!(browse_id_for("VLPL123"), "VLPL123");
        assert_eq!(browse_id_for("RDAMVMabc"), "RDAMVMabc");
    }

    #[test]
    fn parses_playlist_shelf_and_continuation() {
        let root = json_of(r#"{
            "header": {"musicDetailHeaderRenderer": {"title": {"runs": [{"text": "My Mix"}]}}},
            "contents": {"singleColumnBrowseResultsRenderer": {"tabs": [{"tabRenderer": {
                "content": {"sectionListRenderer": {"contents": [{"musicPlaylistShelfRenderer": {
                    "contents": [{"musicResponsiveListItemRenderer": {"flexColumns": [
                        {"musicResponsiveListItemFlexColumnRenderer": {"text": {
                            "runs": [{"text": "Song A", "navigationEndpoint": {"watchEndpoint": {"videoId": "AAAAAAAAAAA"}}}]
                        }}},
                        {"musicResponsiveListItemFlexColumnRenderer": {"text": {"runs": [{"text": "Artist A · Album A"}]}}}
                    ]}}],
                    "continuations": [{"nextContinuationData": {"continuation": "TOK123", "clickTrackingParams": "x"}}]
                }}]}}
            }}]}}
        }"#);
        let tracks = collect_playlist_tracks(&root);
        assert_eq!(tracks.len(), 1);
        assert_eq!(tracks[0].title, "Song A");
        assert_eq!(continuation_token(&root).as_deref(), Some("TOK123"));
        assert_eq!(playlist_title(&root).as_deref(), Some("My Mix"));
    }

    #[test]
    fn parses_two_column_playlist_layout() {
        let root = json_of(
            r#"{
                "header": {"musicResponsiveHeaderRenderer": {"title": {"runs": [{"text": "Boogie Beats"}]}}},
                "contents": {
                    "twoColumnBrowseResultsRenderer": {
                        "secondaryContents": {
                            "sectionListRenderer": {
                                "contents": [
                                    {
                                        "musicPlaylistShelfRenderer": {
                                            "contents": [
                                                {
                                                    "musicResponsiveListItemRenderer": {
                                                        "flexColumns": [
                                                            {
                                                                "musicResponsiveListItemFlexColumnRenderer": {
                                                                    "text": {
                                                                        "runs": [{
                                                                            "text": "Up Jump",
                                                                            "navigationEndpoint": {
                                                                                "watchEndpoint": {
                                                                                    "videoId": "BBBBBBBBBBB"
                                                                                }
                                                                            }
                                                                        }]
                                                                    }
                                                                }
                                                            },
                                                            {
                                                                "musicResponsiveListItemFlexColumnRenderer": {
                                                                    "text": {
                                                                        "runs": [
                                                                            {
                                                                                "text": "Dalamar",
                                                                                "navigationEndpoint": {
                                                                                    "browseEndpoint": {
                                                                                        "browseId": "UCD1daftz"
                                                                                    }
                                                                                }
                                                                            },
                                                                            {"text": " · Remix"}
                                                                        ]
                                                                    }
                                                                }
                                                            }
                                                        ],
                                                        "fixedColumns": [
                                                            {
                                                                "musicResponsiveListItemFixedColumnRenderer": {
                                                                    "text": {
                                                                        "runs": [{"text": "4:20"}]
                                                                    }
                                                                }
                                                            }
                                                        ]
                                                    }
                                                }
                                            ],
                                            "continuations": [
                                                {"continuationCommand": {"token": "TOK9"}}
                                            ]
                                        }
                                    }
                                ]
                            }
                        }
                    }
                }
            }"#,
        );
        let tracks = collect_playlist_tracks(&root);
        assert_eq!(tracks.len(), 1);
        assert_eq!(tracks[0].title, "Up Jump");
        assert_eq!(tracks[0].video_id, "BBBBBBBBBBB");
        assert_eq!(continuation_token(&root).as_deref(), Some("TOK9"));
        assert_eq!(playlist_title(&root).as_deref(), Some("Boogie Beats"));
    }
}