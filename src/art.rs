use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, LazyLock, Mutex};

use anyhow::Context;
use image::imageops::FilterType;
use reqwest::Client;

use crate::config::Config;
use crate::model::Track;

/// Longest side (pixels) for kitty-transmitted art; keeps the payload small.
const KITTY_MAX_SIDE: u32 = 320;
/// How many kitty payloads to keep ready for re-transmission.
const KITTY_CACHE_LIMIT: usize = 8;

/// Two stacked RGB pixels ready to be rendered as one half-block terminal cell.
pub type CellColors = (RgbColor, RgbColor);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RgbColor {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

#[derive(Debug, Clone)]
pub struct Art {
    pub cols: u16,
    pub rows: u16,
    /// Packed as (top-pixel, bottom-pixel) per cell, row-major.
    pub pixels: Vec<CellColors>,
}

/// True-colour image prepared for the kitty graphics protocol.
#[derive(Debug, Clone)]
pub struct KittyImage {
    /// Stable identity (usually the track key / video id) for deduplication.
    pub key: String,
    pub width: u32,
    pub height: u32,
    /// Hex-encoded 24-bit RGB pixel data.
    pub hex: String,
}

/// The flavour of artwork the TUI renders for the current track.
#[derive(Debug, Clone)]
pub enum ArtRender {
    Half(Art),
    Kitty(KittyImage),
}

/// Ready-to-send payloads, so the TUI loop only transmits each image once.
static KITTY_IMAGES: LazyLock<Mutex<HashMap<String, Arc<KittyImage>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// Remember an image payload for later transmission.
pub fn register_kitty(image: &KittyImage) {
    let mut cache = KITTY_IMAGES.lock().unwrap();
    cache.insert(image.key.clone(), Arc::new(image.clone()));
    while cache.len() > KITTY_CACHE_LIMIT {
        let oldest = cache.keys().next().cloned();
        if let Some(key) = oldest {
            cache.remove(&key);
        }
    }
}

/// Fetch a previously built payload by key, if it is still cached.
pub fn kitty_payload(key: &str) -> Option<Arc<KittyImage>> {
    KITTY_IMAGES.lock().unwrap().get(key).cloned()
}

impl Art {
    /// Fetch the track's cover into the cache, resize to `cols x rows` cells
    /// and convert to half-block colors. Returns None on any failure so the
    /// TUI can show a placeholder instead.
    pub async fn load(
        cfg: &Config,
        track: &Track,
        cols: u16,
        rows: u16,
    ) -> anyhow::Result<Option<Art>> {
        let Some(img) = fetch_cached(cfg, track).await? else {
            return Ok(None);
        };

        let px_cols = cols.max(1) as u32 * 2;
        let px_rows = rows.max(1) as u32 * 2;
        let small = image::imageops::resize(&img, px_cols, px_rows, FilterType::Triangle);

        let mut pixels = Vec::with_capacity((px_cols * px_rows) as usize);
        for y in (0..px_rows).step_by(2) {
            for x in 0..px_cols {
                let top = small.get_pixel(x, y).0;
                let bottom = small.get_pixel(x, y + 1).0;
                pixels.push((rgb(top), rgb(bottom)));
            }
        }

        Ok(Some(Art { cols, rows, pixels }))
    }
}

impl KittyImage {
    /// Fetch the track's cover into the cache, downsample to a sane size and
    /// hex-encode it for the kitty graphics protocol.
    pub async fn load(cfg: &Config, track: &Track) -> anyhow::Result<Option<KittyImage>> {
        let Some(img) = fetch_cached(cfg, track).await? else {
            return Ok(None);
        };

        let (width, height) = fit(img.width(), img.height(), KITTY_MAX_SIDE);
        let resized = image::imageops::resize(&img, width, height, FilterType::Triangle);
        let bytes = resized.as_raw();
        let hex = hex::encode(bytes);

        Ok(Some(KittyImage {
            key: track.key(),
            width,
            height,
            hex,
        }))
    }
}

/// Largest `max_side x max_side` rect preserving aspect ratio.
fn fit(width: u32, height: u32, max_side: u32) -> (u32, u32) {
    let longest = width.max(height).max(1);
    let scale = (max_side as f32 / longest as f32).min(1.0);
    (
        (width as f32 * scale).round().max(1.0) as u32,
        (height as f32 * scale).round().max(1.0) as u32,
    )
}

/// Download (or read from cache) the track cover as an RGB image.
async fn fetch_cached(cfg: &Config, track: &Track) -> anyhow::Result<Option<image::RgbImage>> {
    let Some(url) = &track.thumbnail_url else {
        return Ok(None);
    };
    let path = cache_path(cfg, track, url)?;
    if !path.exists() {
        match download_art(url, &path).await {
            Ok(()) => {}
            Err(_) => return Ok(None),
        }
    }
    let img = match image::open(&path) {
        Ok(img) => img,
        Err(e) => {
            // Corrupt / partially-written cache entry; drop and retry once.
            let _ = std::fs::remove_file(&path);
            download_art(url, &path)
                .await
                .with_context(|| format!("downloading cover art: {e}"))?;
            image::open(&path).map_err(|e| anyhow::anyhow!("decode cover art: {e}"))?
        }
    };
    Ok(Some(img.to_rgb8()))
}

fn rgb(c: [u8; 3]) -> RgbColor {
    RgbColor {
        r: c[0],
        g: c[1],
        b: c[2],
    }
}

fn cache_path(cfg: &Config, track: &Track, url: &str) -> anyhow::Result<PathBuf> {
    let ext = extension_of(url)?;
    Ok(cfg
        .art_cache_dir()
        .join(format!("{}.{}", track.video_id, ext)))
}

fn extension_of(url: &str) -> anyhow::Result<&str> {
    let no_query = url.split('?').next().unwrap_or(url);
    let seg = no_query.rsplit('/').next().unwrap_or("");
    let mut parts = seg.split('.');
    let last = parts.next_back().unwrap_or("");
    if last.is_empty() {
        Ok("webp")
    } else {
        Ok(last)
    }
}

async fn download_art(url: &str, path: &PathBuf) -> anyhow::Result<()> {
    let client = Client::builder()
        .user_agent("Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 Chrome/130.0 Safari/537.36")
        .build()?;
    let bytes = client
        .get(url)
        .send()
        .await
        .context("request cover art")?
        .bytes()
        .await
        .context("read cover art body")?;
    std::fs::write(path, &bytes).context("write cover art cache")?;
    Ok(())
}

/// Render half-block cells into a ratatui buffer.
pub fn render_into(buf: &mut ratatui::buffer::Buffer, area: ratatui::layout::Rect, art: &Art) {
    let cols = art.cols.min(area.width);
    let rows = art.rows.min(area.height);
    for y in 0..rows {
        for x in 0..cols {
            let idx = (y as usize) * art.cols as usize + x as usize;
            let Some((top, bottom)) = art.pixels.get(idx) else {
                continue;
            };
            let cell = &mut buf[(area.x + x, area.y + y)];
            cell.set_symbol("▀")
                .set_fg(ratatui::style::Color::Rgb(top.r, top.g, top.b))
                .set_bg(ratatui::style::Color::Rgb(bottom.r, bottom.g, bottom.b));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extension_parsing() {
        assert_eq!(
            extension_of("https://lh3.googleusercontent.com/art/foo=s2000").unwrap(),
            "foo=s2000"
        );
        assert_eq!(
            extension_of("https://i.ytimg.com/vi/abc/maxresdefault.jpg?sqp=xyz").unwrap(),
            "jpg"
        );
        assert_eq!(
            extension_of("https://example.com/cover.webp").unwrap(),
            "webp"
        );
        assert_eq!(extension_of("https://example.com/noext").unwrap(), "noext");
    }

    #[test]
    fn rgb_stores_channels() {
        let c = rgb([1, 2, 3]);
        assert_eq!(c, RgbColor { r: 1, g: 2, b: 3 });
    }

    #[test]
    fn fit_respects_aspect_ratio() {
        assert_eq!(fit(300, 100, 320), (300, 100));
        assert_eq!(fit(800, 400, 320), (320, 160));
        assert_eq!(fit(400, 1200, 320), (107, 320));
        assert!(fit(100, 100, 320).0 <= 320);
    }

    #[test]
    fn kitty_registry_roundtrips() {
        let img = KittyImage {
            key: "k1".to_owned(),
            width: 2,
            height: 2,
            hex: "aabbcc".to_owned(),
        };
        register_kitty(&img);
        let got = kitty_payload("k1").expect("stored");
        assert_eq!(got.hex, "aabbcc");
        assert!(kitty_payload("missing").is_none());
    }

    #[test]
    fn kitty_registry_evicts_oldest() {
        for i in 0..(KITTY_CACHE_LIMIT + 3) {
            register_kitty(&KittyImage {
                key: format!("k{i}"),
                width: 1,
                height: 1,
                hex: "00".to_owned(),
            });
        }
        let cache = KITTY_IMAGES.lock().unwrap();
        assert!(cache.len() <= KITTY_CACHE_LIMIT);
    }
}