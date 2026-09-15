//! Radio station fetching and generic browse-tab track collection.

use std::collections::HashSet;

use anyhow::Context;
use reqwest::Client;
use serde_json::{Value, json};

use crate::model::Track;

use super::config::InnertubeConfig;
use super::search::parse_duration;
use super::{context, post_json};

const MAX_CONTINUATION_PAGES: usize = 12;

/// Build a track radio station id for a video.
pub fn track_radio_id(video_id: &str) -> String {
    format!("RDAMVM{video_id}")
}

/// Fetch a track radio station's playlist via the `next` endpoint.
///
/// The `browse` endpoint rejects `RDAMVM...` ids with INVALID_ARGUMENT; the
/// working call is `next` with `{"playlistId": "RDAMVM<videoId>"}`, returning
/// a `playlistPanelRenderer` of `playlistPanelVideoRenderer` items, extended
/// through `nextRadioContinuationData` continuations.
pub async fn fetch_radio(
    client: &Client,
    cfg: &InnertubeConfig,
    video_id: &str,
) -> anyhow::Result<Vec<Track>> {
    let station = track_radio_id(video_id);
    let mut tracks: Vec<Track> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    let mut token: Option<String> = None;
    let mut pages: usize = 0;

    loop {
        let body = if let Some(t) = &token {
            json!({
                "context": context(cfg, cfg.client_name, cfg.client_name_id, &cfg.client_version, &cfg.visitor_data),
                "continuation": t,
            })
        } else {
            json!({
                "context": context(cfg, cfg.client_name, cfg.client_name_id, &cfg.client_version, &cfg.visitor_data),
                "playlistId": station,
            })
        };
        let root = post_json(client, cfg, "next", body)
            .await
            .with_context(|| format!("radio failed for {station}"))?;
        for item in collect_panel_items(&root) {
            if let Some(track) = parse_radio_item(item)
                && seen.insert(track.key())
            {
                tracks.push(track);
            }
        }
        token = radio_continuation(&root);
        pages += 1;
        if token.is_none() || tracks.len() >= 100 || pages >= MAX_CONTINUATION_PAGES {
            break;
        }
    }
    Ok(tracks)
}

/// Fetch the songs of any music browse page (album, artist, station...).
/// Handles `musicResponsiveListItemRenderer` items, following continuations
/// until the page is exhausted or `max` tracks are collected.
pub async fn fetch_browse_tracks(
    client: &Client,
    cfg: &InnertubeConfig,
    browse_id: &str,
    max: usize,
) -> anyhow::Result<Vec<Track>> {
    let root =
        browse(client, cfg, browse_id, None)
            .await
            .with_context(|| format!("browse failed for {browse_id}"))?;

    let mut tracks = collect_shelf_tracks(&root);
    let mut token = continuation_token(&root);
    let mut pages = 0;
    while let Some(t) = token {
        if pages >= MAX_CONTINUATION_PAGES || tracks.len() >= max {
            break;
        }
        match browse(client, cfg, browse_id, Some(&t)).await {
            Ok(page) => {
                let more = collect_shelf_tracks(&page);
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
            }
            Err(_) => break,
        }
        pages += 1;
    }
    Ok(tracks)
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

fn continuation_token(root: &Value) -> Option<String> {
    let cont = root.pointer("/contents/singleColumnBrowseResultsRenderer/tabs/0/tabRenderer/content/sectionListRenderer/contents")?;
    let arr = cont.as_array()?;
    for item in arr {
        if let Some(shelf) = item.get("musicPlaylistShelfRenderer")
            && let Some(c) = shelf.get("continuations").and_then(Value::as_array)
        {
            for c in c {
                if let Some(t) = c
                    .pointer("/nextContinuationData/continuation")
                    .and_then(Value::as_str)
                {
                    return Some(t.to_string());
                }
            }
        }
        if let Some(t) = item
            .pointer("/musicShelfRenderer/continuations/0/nextContinuationData/continuation")
            .and_then(Value::as_str)
        {
            return Some(t.to_string());
        }
    }
    None
}

/// Collect playable tracks from every shelf in a browse response.
pub fn collect_shelf_tracks(root: &Value) -> Vec<Track> {
    super::search::collect_list_items(root)
        .into_iter()
        .filter_map(super::search::parse_list_item)
        .collect()
}

/// Every `playlistPanelVideoRenderer` object in a radio `next` response, in
/// document order.
fn collect_panel_items(root: &Value) -> Vec<&Value> {
    let mut out = Vec::new();
    fn walk<'a>(node: &'a Value, out: &mut Vec<&'a Value>) {
        match node {
            Value::Object(map) => {
                if let Some(item) = map.get("playlistPanelVideoRenderer") {
                    out.push(item);
                }
                for v in map.values() {
                    walk(v, out);
                }
            }
            Value::Array(arr) => {
                for v in arr {
                    walk(v, out);
                }
            }
            _ => {}
        }
    }
    walk(root, &mut out);
    out
}

/// First `nextRadioContinuationData.continuation`, wherever it appears.
fn radio_continuation(root: &Value) -> Option<String> {
    fn walk(node: &Value, out: &mut Option<String>) {
        match node {
            Value::Object(map) => {
                if let Some(data) = map.get("nextRadioContinuationData")
                    && let Some(t) = data.get("continuation").and_then(Value::as_str)
                {
                    *out = Some(t.to_string());
                    return;
                }
                for v in map.values() {
                    if out.is_some() {
                        return;
                    }
                    walk(v, out);
                }
            }
            Value::Array(arr) => {
                for v in arr {
                    if out.is_some() {
                        return;
                    }
                    walk(v, out);
                }
            }
            _ => {}
        }
    }
    let mut out = None;
    walk(root, &mut out);
    out
}

/// Parse one `playlistPanelVideoRenderer` into a playable track.
fn parse_radio_item(renderer: &Value) -> Option<Track> {
    let video_id = renderer
        .get("videoId")
        .and_then(Value::as_str)
        .or_else(|| {
            renderer
                .pointer("/navigationEndpoint/watchEndpoint/videoId")
                .and_then(Value::as_str)
        })?
        .to_string();
    let title = renderer
        .pointer("/title/runs/0/text")
        .and_then(Value::as_str)
        .map(str::to_string)
        .unwrap_or_else(|| "Unknown title".to_string());

    let mut artist = None;
    let mut artist_id = None;
    if let Some(runs) = renderer.pointer("/longBylineText/runs").and_then(Value::as_array) {
        for run in runs {
            let text = run.get("text").and_then(Value::as_str).unwrap_or("");
            let is_artist = run
                .pointer("/navigationEndpoint/browseEndpoint/browseId")
                .and_then(Value::as_str)
                .is_some_and(|id| id.starts_with("UC"));
            if is_artist && artist.is_none() {
                artist = Some(text.trim().to_string());
                artist_id = run
                    .pointer("/navigationEndpoint/browseEndpoint/browseId")
                    .and_then(Value::as_str)
                    .map(str::to_string);
            }
        }
        if artist.is_none()
            && let Some(first) = runs.first()
        {
            artist = first
                .get("text")
                .and_then(Value::as_str)
                .map(|s| s.trim().to_string());
        }
    }

    let duration = renderer
        .pointer("/lengthText/runs/0/text")
        .and_then(Value::as_str)
        .and_then(parse_duration);
    let thumbnail_url = renderer
        .pointer("/thumbnail/thumbnails/0/url")
        .and_then(Value::as_str)
        .map(|s| {
            if s.starts_with("//") {
                format!("https:{s}")
            } else {
                s.to_string()
            }
        });

    Some(Track {
        video_id,
        title,
        artist: artist.unwrap_or_else(|| "Unknown artist".to_string()),
        album: None,
        thumbnail_url,
        duration,
        category: None,
        artist_id,
        album_id: None,
        browse_id: None,
        source: Default::default(),
    })
}

#[cfg(test)]
mod tests {
    use super::super::json_of;
    use super::*;

    #[test]
    fn builds_radio_ids() {
        assert_eq!(track_radio_id("dQw4w9WgXcQ"), "RDAMVMdQw4w9WgXcQ");
    }

    #[test]
    fn reads_shelf_continuations() {
        let root = json_of(r#"{
            "contents": {"singleColumnBrowseResultsRenderer": {"tabs": [{"tabRenderer": {
                "content": {"sectionListRenderer": {"contents": [
                    {"musicShelfRenderer": {
                        "contents": [{"musicResponsiveListItemRenderer": {"flexColumns": [
                            {"musicResponsiveListItemFlexColumnRenderer": {"text": {
                                "runs": [{"text": "Radio Track", "navigationEndpoint": {"watchEndpoint": {"videoId": "AAAAAAAAAAA"}}}]
                            }}},
                            {"musicResponsiveListItemFlexColumnRenderer": {"text": {"runs": [{"text": "Artist X"}]}}}
                        ]}}],
                        "continuations": [{"nextContinuationData": {"continuation": "CONT1"}}]
                    }}
                ]}}
            }}]}}
        }"#);
        assert_eq!(continuation_token(&root).as_deref(), Some("CONT1"));
        let tracks = collect_shelf_tracks(&root);
        assert_eq!(tracks.len(), 1);
        assert_eq!(tracks[0].video_id, "AAAAAAAAAAA");
    }

    #[test]
    fn parses_radio_panel_items() {
        let root = json_of(r#"{
            "foo": {
                "bar": {
                    "playlistPanelRenderer": {
                        "contents": [
                            {"playlistPanelVideoRenderer": {
                                "videoId": "AAAAAAAAAAA",
                                "title": {"runs": [{"text": "Cheri Cheri Lady"}]},
                                "longBylineText": {"runs": [
                                    {"text": "Modern Talking"},
                                    {"text": " • "},
                                    {"text": "406M views"}
                                ]},
                                "lengthText": {"runs": [{"text": "3:48"}]},
                                "thumbnail": {"thumbnails": [{"url": "//lh3.example/x.jpg"}]}
                            }},
                            {"playlistPanelVideoRenderer": {
                                "title": {"runs": [{"text": "No video id"}]}
                            }}
                        ]
                    }
                }
            }
        }"#);
        let items = collect_panel_items(&root);
        assert_eq!(items.len(), 2);
        let tracks: Vec<Track> = items.iter().filter_map(|i| parse_radio_item(i)).collect();
        assert_eq!(tracks.len(), 1);
        assert_eq!(tracks[0].video_id, "AAAAAAAAAAA");
        assert_eq!(tracks[0].artist, "Modern Talking");
        assert_eq!(tracks[0].duration, Some(228));
        assert_eq!(
            tracks[0].thumbnail_url.as_deref(),
            Some("https://lh3.example/x.jpg")
        );
        assert_eq!(radio_continuation(&root), None);
    }

    #[test]
    fn reads_next_radio_continuation() {
        let root = json_of(r#"{
            "continuationContents": {"playlistPanelContinuation": {
                "contents": [{"playlistPanelVideoRenderer": {"videoId": "a"}}],
                "continuations": [{"nextRadioContinuationData": {"continuation": "ABCDEF", "timeoutMs": "10000"}}]
            }}
        }"#);
        assert_eq!(radio_continuation(&root).as_deref(), Some("ABCDEF"));
    }
}