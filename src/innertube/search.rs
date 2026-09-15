use anyhow::Context;
use reqwest::Client;
use serde_json::Value;

use super::{
    SEARCH_FILTER_SONGS, config::InnertubeConfig, context, http_client, post_json, run_text,
};
use crate::model::Track;

/// Search YouTube Music for songs and parse them into `Track`s.
pub async fn search_songs(cfg: &InnertubeConfig, query: &str) -> anyhow::Result<Vec<Track>> {
    let client = http_client();
    let mut ctx = cfg.clone();
    let mut parsed = search_with(&client, &mut ctx, query, Some(SEARCH_FILTER_SONGS)).await?;
    if parsed.is_empty() {
        // Some clients drop the shelf with certain filters; retry unfiltered.
        parsed = search_with(&client, &mut ctx, query, None).await?;
    }
    Ok(parsed)
}

async fn search_with(
    client: &Client,
    ctx: &mut InnertubeConfig,
    query: &str,
    params: Option<&str>,
) -> anyhow::Result<Vec<Track>> {
    let body = serde_json::json!({
        "context": context(ctx, ctx.client_name, ctx.client_name_id, &ctx.client_version, &ctx.visitor_data),
        "query": query,
        "params": params,
    });
    let value = post_json(client, ctx, "search", body)
        .await
        .context("search failed")?;
    let items = collect_list_items(&value);
    let tracks: Vec<Track> = items.iter().filter_map(|it| parse_list_item(it)).collect();
    Ok(tracks)
}

/// Recursively collect every `musicResponsiveListItemRenderer` object in the
/// response, regardless of which shelf section it appears under.
fn collect_list_items(root: &Value) -> Vec<&Value> {
    let mut out = Vec::new();
    fn walk<'a>(node: &'a Value, out: &mut Vec<&'a Value>) {
        match node {
            Value::Object(map) => {
                if let Some(item) = map.get("musicResponsiveListItemRenderer") {
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

fn parse_list_item(renderer: &Value) -> Option<Track> {
    let flex = renderer.get("flexColumns")?.as_array()?;

    let title_col = flex.first()?;
    let title_node = title_col.pointer("/musicResponsiveListItemFlexColumnRenderer/text")?;
    let title = parse_title(title_node)?;

    let mut video_id = None;
    if let Some(id) = title_node
        .pointer("/runs/0/navigationEndpoint/watchEndpoint/videoId")
        .and_then(Value::as_str)
    {
        video_id = Some(id.to_string());
    }

    // Secondary flex column holds "artist · album" or just "artist".
    let mut artist = String::new();
    let mut album = None;
    if let Some(col) = flex.get(1) {
        let text = col
            .pointer("/musicResponsiveListItemFlexColumnRenderer/text")
            .and_then(run_text)?;
        if let Some((a, b)) = text.rsplit_once(" · ") {
            artist = a.to_string();
            album = Some(b.to_string());
        } else {
            artist = text;
        }
    }

    // Try the overlay thumbnail, then the direct thumbnail slot.
    let thumbnail = renderer
        .pointer("/thumbnail/musicThumbnailRenderer/thumbnail/thumbnails")
        .or_else(|| renderer.pointer("/overlay/musicItemThumbnailRenderer/content/musicThumbnailRenderer/thumbnail/thumbnails"))
        .and_then(Value::as_array)
        .and_then(|thumbs| {
            // Prefer the largest thumbnail.
            thumbs
                .iter()
                .flat_map(|t| t.get("url").and_then(Value::as_str))
                .next()
                .map(|url| url.to_string())
        });

    let duration_text = renderer
        .pointer("/fixedColumns/0/musicResponsiveListItemFlexColumnRenderer/text/runs/0/text")
        .and_then(Value::as_str)
        .and_then(parse_duration);

    let video_id = video_id.or_else(|| {
        renderer
            .pointer("/playlistItemData/videoId")
            .and_then(Value::as_str)
            .map(str::to_string)
    })?;

    Some(Track {
        video_id,
        title,
        artist: clean_secondary(artist),
        album,
        thumbnail_url: clean_thumbnail(thumbnail),
        duration: duration_text,
    })
}

/// A title node may carry the videoId via navigationEndpoint directly on the run.
fn parse_title(node: &Value) -> Option<String> {
    node.pointer("/runs/0/text")
        .and_then(Value::as_str)
        .map(|s| s.to_string())
}

fn clean_secondary(s: String) -> String {
    // YT Music separates artist and album with " · " (U+00B7) or "-".
    s
}

fn clean_thumbnail(u: Option<String>) -> Option<String> {
    u.map(|mut s| {
        if s.starts_with("//") {
            s = format!("https:{}", s);
        }
        s
    })
}

/// Parse a duration string like "4:05", "1:04:32", "1 hour 5 min" into seconds.
pub fn parse_duration(s: &str) -> Option<u32> {
    let s = s.trim();
    if s.is_empty() {
        return None;
    }
    if s.contains(':') {
        let parts: Vec<&str> = s.split(':').collect();
        let mut secs: u32 = parts.last()?.parse().ok()?;
        if parts.len() >= 2 {
            secs += parts[parts.len() - 2].parse::<u32>().ok()? * 60;
        }
        if parts.len() >= 3 {
            secs += parts[parts.len() - 3].parse::<u32>().ok()? * 3600;
        }
        return Some(secs);
    }
    // "4 min", "1 hour 30 min", "45 sec"
    let words: Vec<&str> = s.split_whitespace().collect();
    let mut secs: u32 = 0;
    let mut i = 0;
    while i + 1 < words.len() {
        let num: u32 = words[i].parse().ok()?;
        match words[i + 1] {
            w if w.starts_with("hour") => secs += num * 3600,
            w if w.starts_with("min") => secs += num * 60,
            w if w.starts_with("sec") => secs += num,
            _ => return None,
        }
        i += 2;
    }
    if secs == 0 { None } else { Some(secs) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::innertube::json_of;

    #[test]
    fn parses_durations() {
        assert_eq!(parse_duration("4:05"), Some(245));
        assert_eq!(parse_duration("1:04:32"), Some(3872));
        assert_eq!(parse_duration("1 hour 5 min"), Some(3900));
        assert_eq!(parse_duration("45 sec"), Some(45));
        assert_eq!(parse_duration("4 min"), Some(240));
        assert_eq!(parse_duration(""), None);
        assert_eq!(parse_duration("abc"), None);
    }

    #[test]
    fn collects_and_parses_list_items() {
        let resp = json_of(
            r#"{
            "contents": {
                "tabbedSearchResultsRenderer": {
                    "tabs": [{
                        "tabRenderer": {
                            "content": {
                                "sectionListRenderer": {
                                    "contents": [{
                                        "musicShelfRenderer": {
                                            "contents": [{
                                                "musicResponsiveListItemRenderer": {
                                                    "flexColumns": [
                                                        {
                                                            "musicResponsiveListItemFlexColumnRenderer": {
                                                                "text": {
                                                                    "runs": [{
                                                                        "text": "Bohemian Rhapsody",
                                                                        "navigationEndpoint": {
                                                                            "watchEndpoint": {
                                                                                "videoId": "fJ9rUzIMcZQ"
                                                                            }
                                                                        }
                                                                    }]
                                                                }
                                                            }
                                                        },
                                                        {
                                                            "musicResponsiveListItemFlexColumnRenderer": {
                                                                "text": {
                                                                    "runs": [{"text": "Queen"}, {"text": " · A Night at the Opera"}]
                                                                }
                                                            }
                                                        }
                                                    ],
                                                    "fixedColumns": [{
                                                        "musicResponsiveListItemFlexColumnRenderer": {
                                                            "text": { "runs": [{"text": "6:07"}] }
                                                        }
                                                    }],
                                                    "thumbnail": {
                                                        "musicThumbnailRenderer": {
                                                            "thumbnail": {
                                                                "thumbnails": [
                                                                    {"url": "//lh3.googleusercontent.com/art/aaa"},
                                                                    {"url": "//lh3.googleusercontent.com/art/bbb=s2000"}
                                                                ]
                                                            }
                                                        }
                                                    }
                                                }
                                            }]
                                        }
                                    }]
                                }
                            }
                        }
                    }]
                }
            }
        }"#,
        );

        let items = collect_list_items(&resp);
        assert_eq!(items.len(), 1);
        let track = parse_list_item(items[0]).unwrap();
        assert_eq!(track.video_id, "fJ9rUzIMcZQ");
        assert_eq!(track.title, "Bohemian Rhapsody");
        assert_eq!(track.artist, "Queen");
        assert_eq!(track.album.as_deref(), Some("A Night at the Opera"));
        assert_eq!(track.duration, Some(367));
        assert_eq!(
            track.thumbnail_url.as_deref(),
            Some("https://lh3.googleusercontent.com/art/aaa")
        );
    }

    #[test]
    fn ignores_non_music_items() {
        let resp = json_of(
            r#"{"contents":{"sectionListRenderer":{"contents":[{"musicCardShelfRenderer":{"title":"x"}}]}}}"#,
        );
        assert!(collect_list_items(&resp).is_empty());
    }
}
