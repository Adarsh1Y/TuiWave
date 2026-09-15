use anyhow::Context;
use reqwest::Client;
use serde_json::Value;

use super::{
    SEARCH_FILTER_SONGS, config::InnertubeConfig, context, http_client, post_json,
};
use crate::model::Track;

/// Search YouTube Music for songs and parse them into `Track`s.
///
/// The songs filter still mixes video/artist/episode shelves into the
/// response, so matching rows are classified individually via their category
/// label and navigation metadata; anything that is clearly not a song is
/// dropped.
pub async fn search_songs(cfg: &InnertubeConfig, query: &str) -> anyhow::Result<Vec<Track>> {
    let client = http_client();
    let mut ctx = cfg.clone();
    let mut parsed = search_with(&client, &mut ctx, query, Some(SEARCH_FILTER_SONGS)).await?;
    if parsed.is_empty() {
        // Some clients drop the shelf with certain filters; retry unfiltered.
        parsed = search_with(&client, &mut ctx, query, None).await?;
    }
    Ok(parsed.into_iter().filter(is_song_item).collect())
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
pub fn collect_list_items(root: &Value) -> Vec<&Value> {
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

/// Category labels that appear as the first secondary-column run of a
/// responsive list item on the web client.
const NON_SONG_CATEGORIES: &[&str] = &[
    "video",
    "episode",
    "podcast",
    "album",
    "artist",
    "playlist",
    "channel",
    "community post",
    "post",
    "station",
];

/// Every category label the row parser recognises, including songs.
const CATEGORY_LABELS: &[&str] = &[
    "song",
    "single",
    "track",
    "video",
    "episode",
    "podcast",
    "album",
    "artist",
    "playlist",
    "channel",
    "station",
    "community post",
    "post",
];

fn is_category_label(text: &str) -> bool {
    let t = text.trim().to_ascii_lowercase();
    CATEGORY_LABELS.contains(&t.as_str())
}

/// Decide whether a parsed row is a playable song rather than a video,
/// episode, artist, album or playlist row.
fn is_song_item(track: &Track) -> bool {
    if let Some(cat) = track.category.as_deref() {
        // "Song"/"Single" labels are unambiguously music.
        if matches!(cat.to_ascii_lowercase().as_str(), "song" | "single") {
            return true;
        }
        return !NON_SONG_CATEGORIES.contains(&cat.to_ascii_lowercase().as_str());
    }
    // No category label (older clients): keep rows that resolve to an artist
    // or an album browse endpoint; those are music, not plain video rows.
    track.album.is_some() || track.artist_id.is_some()
}

/// A parsed flexible column: title row plus secondary-column details.
struct ParsedRow {
    category: Option<String>,
    artist: Option<String>,
    artist_id: Option<String>,
    album: Option<String>,
    duration: Option<u32>,
}

fn parse_secondary(text: &Value) -> ParsedRow {
    let runs = text
        .pointer("/runs")
        .and_then(Value::as_array)
        .map(|arr| arr.iter().collect::<Vec<_>>())
        .unwrap_or_default();
    let mut row = ParsedRow {
        category: None,
        artist: None,
        artist_id: None,
        album: None,
        duration: None,
    };
    for run in &runs {
        let run_text = run.get("text").and_then(Value::as_str).unwrap_or("");
        let browse_id = run
            .pointer("/navigationEndpoint/browseEndpoint/browseId")
            .and_then(Value::as_str);
        if let Some(id) = browse_id {
            if id.starts_with("UC") && row.artist.is_none() {
                row.artist = Some(run_text.to_string());
                row.artist_id = Some(id.to_string());
            } else if id.starts_with("MPRE") && row.album.is_none() {
                row.album = Some(run_text.to_string());
            }
        } else if row.category.is_none() && !run_text.trim().is_empty() {
            // A category label is a plain run whose text is a known label.
            if is_category_label(run_text) {
                row.category = Some(run_text.trim().to_string());
            }
        }
        if row.duration.is_none() {
            row.duration = parse_duration(run_text);
        }
    }
    row
}

/// Fall back to plain text splitting for clients that do not attach
/// navigation to each run ("Artist · Album" or "Song • Artist").
fn secondary_from_text(text: &Value) -> ParsedRow {
    let joined = join_runs(text);
    let mut row = ParsedRow {
        category: None,
        artist: None,
        artist_id: None,
        album: None,
        duration: None,
    };
    let separator = if joined.contains(" • ") { " • " } else { " · " };
    let mut parts: Vec<&str> = joined.split(separator).collect();
    if parts.first().is_some_and(|p| {
        let p = p.trim().to_ascii_lowercase();
        p == "song" || p == "single" || NON_SONG_CATEGORIES.contains(&p.as_str())
    }) {
        row.category = Some(parts.remove(0).trim().to_string());
    }
    if let Some(first) = parts.first() {
        row.artist = Some(first.trim().to_string());
    }
    if parts.len() > 1 {
        row.album = Some(parts[1].trim().to_string());
    }
    row
}

pub fn parse_list_item(renderer: &Value) -> Option<Track> {
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

    // Secondary flex column holds "artist · album", "Song • artist", or a
    // "Video ..."/"Episode ..." category row.
    let detail = flex.get(1).and_then(|col| {
        col.pointer("/musicResponsiveListItemFlexColumnRenderer/text")
    });
    let row = match detail {
        Some(text) => {
            let nav_rows = parse_secondary(text);
            if nav_rows.artist.is_none() {
                secondary_from_text(text)
            } else {
                nav_rows
            }
        }
        None => ParsedRow {
            category: None,
            artist: None,
            artist_id: None,
            album: None,
            duration: None,
        },
    };

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
        .and_then(parse_duration)
        .or(row.duration);

    let video_id = video_id.or_else(|| {
        renderer
            .pointer("/playlistItemData/videoId")
            .and_then(Value::as_str)
            .map(str::to_string)
    })?;

    Some(Track {
        video_id,
        title,
        artist: row.artist.clone().unwrap_or_else(|| {
            "Unknown artist".to_string()
        }),
        album: row.album,
        thumbnail_url: clean_thumbnail(thumbnail),
        duration: duration_text,
        category: row.category,
        artist_id: row.artist_id,
        source: Default::default(),
    })
}

/// A title node may carry the videoId via navigationEndpoint directly on the run.
fn parse_title(node: &Value) -> Option<String> {
    node.pointer("/runs/0/text")
        .and_then(Value::as_str)
        .map(|s| s.to_string())
}

/// Join all run texts in a text node when present, else the simpleText.
fn join_runs(node: &Value) -> String {
    if let Some(runs) = node.get("runs").and_then(Value::as_array) {
        return runs
            .iter()
            .filter_map(|r| r.get("text").and_then(Value::as_str))
            .collect::<Vec<_>>()
            .join("");
    }
    node.get("simpleText")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
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
