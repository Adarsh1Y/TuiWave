//! Best-effort `n`-signature (nsig) deciphering.
//!
//! YouTube signs its stream URLs with an `n` parameter. For some clients the
//! player enforces that the `n` value has been passed through a transformation
//! function living in the player script (base.js). This module:
//!
//! 1. downloads and caches the player script for the current player id,
//! 2. scans its top-level functions for the `n` transformer by *behaviour*:
//!    each candidate is executed in an embedded QuickJS interpreter with a
//!    fixed test string, and the first function that maps it to a same-length,
//!    URL-safe, different string is treated as the `n` function,
//! 3. runs that function on a live challenge and splices the result back into
//!    the original stream URL.
//!
//! Modern players obfuscate the `n` function heavily and may not yield a
//! candidate (YouTube currently does not enforce nsig for the VISIONOS
//! pre-signed URLs). Every failure path returns `None` so callers fall back
//! to the yt-dlp resolver.

use anyhow::{Context, Result};
use std::path::PathBuf;
use std::sync::OnceLock;

/// Result of inspecting a live `n` challenge.
pub struct Deciphered {
    /// URL with the `n` parameter replaced by its deciphered value.
    pub url: String,
}

const TEST_INPUT: &str = "nzX9kWm4TpRqYs7Ef2vCx8hLgB6dJHa5SuQ3oM";
const CANDIDATE_MAX_BODY: usize = 25_000;
const MIN_OPS: usize = 3;
const WATCH_URL: &str = "https://www.youtube.com/watch?v=3JZ_D3ELwOQ";

/// Whether a URL carries an `n` challenge parameter at all.
pub fn has_n(url: &str) -> bool {
    n_param(url).is_some()
}

/// Extract the raw `n` value from a stream URL's query string.
fn n_param(url: &str) -> Option<String> {
    let query = url.split('?').nth(1)?;
    for pair in query.split('&') {
        if let Some(v) = pair.strip_prefix("n=") {
            return Some(v.to_string());
        }
    }
    None
}

/// Attempt to decipher and replace the `n` parameter of `url`.
pub async fn maybe_decipher(client: &reqwest::Client, url: &str) -> Result<Option<Deciphered>> {
    let Some(challenge) = n_param(url) else {
        return Ok(None);
    };
    let script = player_script(client).await?;
    let Some(deciphered) = run_n_function(&script.js, &challenge)? else {
        return Ok(None);
    };
    Ok(Some(Deciphered {
        url: replace_n_param(url, &deciphered),
    }))
}

fn replace_n_param(url: &str, new_n: &str) -> String {
    let Some((path, query)) = url.split_once('?') else {
        return url.to_string();
    };
    let mut parts: Vec<String> = Vec::new();
    for pair in query.split('&') {
        if pair.starts_with("n=") {
            parts.push(format!("n={}", percent_encode(new_n)));
        } else {
            parts.push(pair.to_string());
        }
    }
    format!("{path}?{}", parts.join("&"))
}

fn percent_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z'
            | b'a'..=b'z'
            | b'0'..=b'9'
            | b'-'
            | b'_'
            | b'.'
            | b'~'
            | b'!'
            | b'*'
            | b'\''
            | b'('
            | b')' => out.push(b as char),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Player script fetch + cache
// ---------------------------------------------------------------------------

pub struct PlayerScript {
    pub id: String,
    pub js: String,
}

static CACHED: OnceLock<Option<PlayerScript>> = OnceLock::new();

/// Fetch (caching in `~/.cache/lastwave/nsig/<id>.js`) the player script for
/// the current player.
pub async fn player_script(client: &reqwest::Client) -> Result<PlayerScript> {
    if let Some(Some(script)) = CACHED.get() {
        return Ok(PlayerScript {
            id: script.id.clone(),
            js: script.js.clone(),
        });
    }
    let dir = nsig_cache_dir();
    if let Some((cached_id, cached_js)) = read_cached(&dir) {
        let _ = CACHED.set(Some(PlayerScript {
            id: cached_id.clone(),
            js: cached_js.clone(),
        }));
        return Ok(PlayerScript {
            id: cached_id,
            js: cached_js,
        });
    }

    let js_url = player_js_url(client).await?;
    let id = js_url.rsplit('/').nth(2).unwrap_or("unknown").to_string();
    let js = client
        .get(&js_url)
        .send()
        .await
        .context("download player script")?
        .text()
        .await
        .context("read player script")?;

    let script = PlayerScript { id, js };
    let _ = write_cache(&dir, &script);
    let _ = CACHED.set(Some(PlayerScript {
        id: script.id.clone(),
        js: script.js.clone(),
    }));
    Ok(script)
}

fn nsig_cache_dir() -> PathBuf {
    dirs::cache_dir()
        .unwrap_or_else(|| PathBuf::from("/tmp"))
        .join("lastwave")
        .join("nsig")
}

fn read_cached(dir: &PathBuf) -> Option<(String, String)> {
    let entry = std::fs::read_dir(dir).ok()?.next()?;
    let path = entry.ok()?.path();
    let js = std::fs::read_to_string(&path).ok()?;
    let id = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("unknown")
        .to_string();
    Some((id, js))
}

fn write_cache(dir: &PathBuf, script: &PlayerScript) -> Result<()> {
    std::fs::create_dir_all(dir)?;
    std::fs::write(dir.join(format!("{}.js", script.id)), &script.js).context("cache player script")
}

/// Find the absolute player script URL embedded in the watch page ytcfg.
async fn player_js_url(client: &reqwest::Client) -> Result<String> {
    let html = client
        .get(WATCH_URL)
        .header("User-Agent", "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/26.0 Safari/605.1.15")
        .send()
        .await
        .context("fetch watch page")?
        .text()
        .await?;
    let marker = "\"PLAYER_JS_URL\":\"";
    let start = html
        .find(marker)
        .context("PLAYER_JS_URL not found in watch page")?
        + marker.len();
    let rest = &html[start..];
    let end = rest.find('"').context("malformed PLAYER_JS_URL")?;
    let path = &rest[..end];
    if path.is_empty() {
        anyhow::bail!("empty PLAYER_JS_URL");
    }
    Ok(format!(
        "{}{}",
        if path.starts_with("http") {
            ""
        } else {
            "https://www.youtube.com"
        },
        path
    ))
}

// ---------------------------------------------------------------------------
// n-function detection + execution via embedded QuickJS
// ---------------------------------------------------------------------------

/// Extract top-level function bodies: `NAME=function(ARG){...}` and
/// `function NAME(ARG){...}`. Returns (name, arg, body) triples.
fn top_level_functions(js: &str) -> Vec<(String, String, String)> {
    let bytes = js.as_bytes();
    fn is_ident(b: u8) -> bool {
        b.is_ascii_alphanumeric() || b == b'_' || b == b'$'
    }
    fn skip_ws(src: &[u8], mut i: usize) -> usize {
        while i < src.len() && (src[i] == b' ' || src[i] == b'\t' || src[i] == b'\n') {
            i += 1;
        }
        i
    }
    // find matching brace for src[i] == '{', respecting string literals.
    fn match_brace(src: &[u8], mut i: usize) -> Option<usize> {
        let mut depth = 0u32;
        let mut in_str: Option<u8> = None;
        let mut esc = false;
        while i < src.len() {
            let c = src[i];
            if let Some(q) = in_str {
                if esc {
                    esc = false;
                } else if c == b'\\' {
                    esc = true;
                } else if c == q {
                    in_str = None;
                }
            } else {
                match c {
                    b'"' | b'\'' | b'`' => in_str = Some(c),
                    b'{' => depth += 1,
                    b'}' => {
                        depth -= 1;
                        if depth == 0 {
                            return Some(i);
                        }
                    }
                    _ => {}
                }
            }
            i += 1;
        }
        None
    }

    let mut out: Vec<(String, String, String)> = Vec::new();
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    let s = js;

    // Pass 1: var/let/const NAME=function(ARG){...} and NAME=function(ARG){...}
    let mut i = 0usize;
    while i < s.len() {
        let c = s.as_bytes()[i];
        if !(c.is_ascii_alphabetic() || c == b'_' || c == b'$') {
            i += 1;
            continue;
        }
        let start = i;
        while i < s.len() && is_ident(s.as_bytes()[i]) {
            i += 1;
        }
        let first = s[start..i].to_string();
        let name_end = i;
        let mut j = skip_ws(bytes, name_end);
        if j >= s.len() || bytes[j] != b'=' {
            if !matches!(first.as_str(), "var" | "let" | "const") {
                i = name_end;
                continue;
            }
            // Handle var/let/const NAME=... where the first ident was the keyword.
            let nstart = j;
            while j < s.len() && is_ident(bytes[j]) {
                j += 1;
            }
            let actual = s[nstart..j].to_string();
            let jk = skip_ws(bytes, j);
            if jk >= s.len() || bytes[jk] != b'=' {
                i = j;
                continue;
            }
            let k2 = skip_ws(bytes, jk + 1);
            if !s[k2..].starts_with("function") {
                i = j;
                continue;
            }
            let k3 = skip_ws(bytes, k2 + 8);
            if k3 >= s.len() || bytes[k3] != b'(' {
                i = j;
                continue;
            }
            if let Some(close_pos) = s[k3 + 1..].find(')') {
                let close = k3 + 1 + close_pos;
                let arg = s[k3 + 1..close].trim().to_string();
                let m = skip_ws(bytes, close + 1);
                if m < s.len()
                    && bytes[m] == b'{'
                    && let Some(end) = match_brace(bytes, m)
                {
                    let body = s[m + 1..end].to_string();
                    if seen.insert(actual.clone()) {
                        out.push((actual, arg, body));
                    }
                    i = end + 1;
                    continue;
                }
            }
            i = j;
            continue;
        }
        if !s[j + 1..].starts_with("function") {
            i = name_end;
            continue;
        }
        let k = skip_ws(bytes, j + 8);
        if k >= s.len() || bytes[k] != b'(' {
            i = name_end;
            continue;
        }
        if let Some(close_pos) = s[k + 1..].find(')') {
            let close = k + 1 + close_pos;
            let arg = s[k + 1..close].trim().to_string();
            let m = skip_ws(bytes, close + 1);
            if m < s.len()
                && bytes[m] == b'{'
                && let Some(end) = match_brace(bytes, m)
            {
                let body = s[m + 1..end].to_string();
                if seen.insert(first.clone()) {
                    out.push((first, arg, body));
                }
                i = end + 1;
                continue;
            }
        }
        i = name_end;
    }

    // Pass 2: function NAME(ARG){...}
    let needle = "function ";
    let mut search_from = 0usize;
    while let Some(pos) = s[search_from..].find(needle) {
        let p = search_from + pos;
        let mut j = skip_ws(bytes, p + needle.len());
        if j >= s.len() || !is_ident(bytes[j]) {
            search_from = j;
            continue;
        }
        let name_start = j;
        while j < s.len() && is_ident(bytes[j]) {
            j += 1;
        }
        let name = s[name_start..j].to_string();
        if seen.contains(&name) {
            search_from = name_start;
            continue;
        }
        let k = skip_ws(bytes, j);
        if k >= s.len() || bytes[k] != b'(' {
            search_from = k;
            continue;
        }
        let close = match s[k + 1..].find(')') {
            Some(d) => k + 1 + d,
            None => {
                search_from = k;
                continue;
            }
        };
        let arg = s[k + 1..close].trim().to_string();
        let m = skip_ws(bytes, close + 1);
        if m < s.len()
            && bytes[m] == b'{'
            && let Some(end) = match_brace(bytes, m)
        {
            let body = s[m + 1..end].to_string();
            seen.insert(name.clone());
            out.push((name, arg, body));
            search_from = end + 1;
            continue;
        }
        search_from = k;
    }
    out
}

/// Structural + behavioural pre-filter for candidate n-functions.
fn plausible(body: &str) -> bool {
    if body.len() > CANDIDATE_MAX_BODY {
        return false;
    }
    if body.contains("document")
        || body.contains("window")
        || body.contains(".location")
        || body.contains("XMLHttpRequest")
        || body.contains("fetch(")
        || body.contains("JSON.parse")
        || body.contains("atob(")
        || body.contains("decodeURIComponent")
        || body.contains("encodeURIComponent")
        || body.contains("addEventListener")
        || body.contains("localStorage")
    {
        return false;
    }
    let counts = [
        body.matches("charCodeAt").count(),
        body.matches("splice").count(),
        body.matches(".reverse").count(),
        body.matches(".join(").count(),
        body.matches(".split(").count(),
        body.matches("String.fromCharCode").count(),
        body.matches("fromCharCode").count(),
        body.matches(".length)").count(),
        body.matches("for (").count(),
        body.matches("while (").count(),
        body.split(">>>").count() - 1,
    ];
    counts.iter().sum::<usize>() >= MIN_OPS
}

fn url_safe(s: &str) -> bool {
    !s.is_empty()
        && s.bytes().all(|b| {
            b.is_ascii_alphanumeric()
                || matches!(b, b'-' | b'_' | b'!' | b'*' | b'\'' | b'(' | b')' | b'~')
        })
}

/// Find and run the n-function over `challenge`, iff a candidate exists.
///
/// The "n function" is located behaviourally: every plausible top-level
/// function is executed on a fixed test string inside QuickJS and the first
/// function that produces a same-length URL-safe transform is used.
fn run_n_function(js: &str, challenge: &str) -> Result<Option<String>> {
    let winner = find_n_function(js)?;
    let Some(body) = winner else {
        return Ok(None);
    };
    let out = execute_body(&body, challenge)?;
    Ok(Some(out))
}

fn execute_body(body: &str, input: &str) -> Result<String> {
    use rquickjs::{Context, Runtime, Value};
    let rt = Runtime::new().context("create quickjs runtime")?;
    // `Context::base` registers only the base objects (no eval/statement
    // support), so the full context is required to actually run candidates.
    let ctx = Context::full(&rt).context("create quickjs context")?;
    ctx.with(|ctx| -> Result<String> {
        let src = format!("((a) => {{ {body}; return a; }})");
        let value: Value = ctx
            .eval::<Value, _>(src.as_str())
            .map_err(|e| anyhow::anyhow!("quickjs parse: {e}"))?;
        let f = value
            .into_function()
            .ok_or_else(|| anyhow::anyhow!("candidate is not callable"))?;
        let out: String = f
            .call((input,))
            .map_err(|e| anyhow::anyhow!("quickjs call: {e}"))?;
        Ok(out)
    })
}

/// Returns the body of the first plausible, behaviourally-matching function.
fn find_n_function(js: &str) -> Result<Option<String>> {
    for (_name, _arg, body) in top_level_functions(js) {
        if !plausible(&body) {
            continue;
        }
        match execute_body(&body, TEST_INPUT) {
            Ok(o) if output_matches(&o, TEST_INPUT) => return Ok(Some(body)),
            // ReferenceError / runtime errors -> not a self-contained n function
            _ => {}
        }
    }
    Ok(None)
}

fn output_matches(out: &str, input: &str) -> bool {
    let len_ok =
        out.len() >= input.len().saturating_sub(4) && out.len() <= input.len().saturating_add(8);
    len_ok && out != input && url_safe(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_n_value() {
        let url = "https://rr.example/x.m4a?a=1&n=AbC123&b=2";
        assert_eq!(n_param(url).as_deref(), Some("AbC123"));
        assert!(has_n(url));
    }

    #[test]
    fn replaces_n_value() {
        let url = "https://rr.example/x.m4a?a=1&n=AbC&b=2";
        let out = replace_n_param(url, "XYZ");
        assert!(out.contains("&n=XYZ&"));
        assert!(out.contains("a=1"));
        assert!(out.contains("b=2"));
    }

    #[test]
    fn percent_encodes_non_urlsafe() {
        assert_eq!(percent_encode("a+b/c=1"), "a%2Bb%2Fc%3D1");
        assert_eq!(percent_encode("AbC-_.~"), "AbC-_.~");
    }

    #[test]
    fn collects_top_level_functions() {
        let js = r#"
            var foo = function(x){ return x + 1 };
            function bar(a) { return a * 2 }
            var z = 5;
        "#;
        let fns = top_level_functions(js);
        let names: Vec<&str> = fns.iter().map(|(n, _, _)| n.as_str()).collect();
        assert!(names.contains(&"foo"));
        assert!(names.contains(&"bar"));
    }

    #[test]
    fn detects_behavioural_n_function() {
        // classic n-like transform: reverse and toggle chars, URL-safe, same len
        let js = r#"
            var ncode = function(a){
                var chars = a.split("");
                var out = [];
                for (var i = chars.length - 1; i >= 0; i--) {
                    var c = chars[i];
                    if (c >= 'a' && c <= 'z') {
                        c = String.fromCharCode(c.charCodeAt(0) - 32);
                    } else if (c >= 'A' && c <= 'Z') {
                        c = String.fromCharCode(c.charCodeAt(0) + 32);
                    }
                    out.push(c);
                }
                return out.join("");
            };
            var notIt = function(a){ return a + "zzz" };
        "#;
        let body = find_n_function(js).unwrap().unwrap();
        assert!(body.contains("fromCharCode"));
        // Run on a real challenge
        let out = run_n_function(js, "Ab4").unwrap().unwrap();
        assert_eq!(out, "4Ba");
    }

    #[test]
    fn returns_none_when_no_candidate() {
        let js = r#"var x = function(a){ return a + "extra" };"#;
        assert!(find_n_function(js).unwrap().is_none());
    }
}
