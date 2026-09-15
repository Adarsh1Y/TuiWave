use std::path::PathBuf;

use anyhow::Context;
use image::imageops::FilterType;
use reqwest::Client;

use crate::config::Config;
use crate::model::Track;

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

        let px_cols = cols.max(1) as u32 * 2;
        let px_rows = rows.max(1) as u32 * 2;
        let small = image::imageops::resize(&img.to_rgb8(), px_cols, px_rows, FilterType::Triangle);

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
}
