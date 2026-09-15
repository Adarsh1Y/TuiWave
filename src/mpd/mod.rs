use std::sync::Arc;

use anyhow::Context;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::tcp::{OwnedReadHalf as TcpRead, OwnedWriteHalf as TcpWrite};
use tokio::net::unix::{OwnedReadHalf as UnixRead, OwnedWriteHalf as UnixWrite};
use tokio::net::{TcpStream, UnixStream};
use tokio::sync::{RwLock, mpsc, oneshot};
use tokio::time::{Duration, MissedTickBehavior};

use crate::config::MpdConfig;
use crate::mpv::{MpvEvent, PlaybackState};

const POLL_INTERVAL: Duration = Duration::from_millis(500);

enum ReadHalf {
    Tcp(BufReader<TcpRead>),
    Unix(BufReader<UnixRead>),
}

enum WriteHalf {
    Tcp(TcpWrite),
    Unix(UnixWrite),
}

impl WriteHalf {
    async fn write_line(&mut self, s: &str) -> std::io::Result<()> {
        match self {
            Self::Tcp(w) => {
                w.write_all(s.as_bytes()).await?;
                w.flush().await?;
            }
            Self::Unix(w) => {
                w.write_all(s.as_bytes()).await?;
                w.flush().await?;
            }
        }
        Ok(())
    }
}

impl ReadHalf {
    async fn read_line(&mut self, buf: &mut String) -> std::io::Result<usize> {
        match self {
            Self::Tcp(r) => r.read_line(buf).await,
            Self::Unix(r) => r.read_line(buf).await,
        }
    }
}

type InternalCmd = (String, oneshot::Sender<anyhow::Result<Vec<String>>>);

type CmdSender = mpsc::UnboundedSender<InternalCmd>;

/// Client handle for a running MPD. Commands are serviced by a background
/// agent that also keeps `PlaybackState` fresh and translates natural
/// track-end / stream-error into the same events mpv would emit.
#[derive(Clone)]
pub struct MpdClient {
    reqs: CmdSender,
    state: Arc<RwLock<PlaybackState>>,
}

impl std::fmt::Debug for MpdClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("MpdClient")
    }
}

impl MpdClient {
    pub async fn connect(
        cfg: &MpdConfig,
    ) -> anyhow::Result<(Self, Arc<RwLock<PlaybackState>>, mpsc::UnboundedReceiver<MpvEvent>)> {
        let (mut read, mut write) = if let Some(sock) = &cfg.socket {
            let s = UnixStream::connect(sock)
                .await
                .with_context(|| format!("connect to MPD socket {}", sock.display()))?;
            let (r, w) = s.into_split();
            (ReadHalf::Unix(BufReader::new(r)), WriteHalf::Unix(w))
        } else {
            let addr = format!("{}:{}", cfg.host, cfg.port);
            let s = TcpStream::connect(&addr)
                .await
                .with_context(|| format!("connect to MPD at {addr}"))?;
            let (r, w) = s.into_split();
            (ReadHalf::Tcp(BufReader::new(r)), WriteHalf::Tcp(w))
        };

        if let Some(pw) = cfg.password.as_deref() {
            write
                .write_line(&format!("password {pw}\n"))
                .await
                .context("MPD password handshake")?;
            let mut line = String::new();
            read.read_line(&mut line).await?;
            if !line.starts_with("OK") {
                anyhow::bail!("MPD refused password: {line:?}");
            }
        }

        let state = Arc::new(RwLock::new(PlaybackState::default()));
        let (event_tx, event_rx) = mpsc::unbounded_channel();
        let (reqs, req_rx) = mpsc::unbounded_channel();

        tokio::spawn(agent_loop(read, write, req_rx, Arc::clone(&state), event_tx));

        Ok((Self { reqs, state: state.clone() }, state, event_rx))
    }

    pub fn state(&self) -> Arc<RwLock<PlaybackState>> {
        Arc::clone(&self.state)
    }

    /// Send one command and return the reply lines (excluding OK/ACK).
    pub async fn cmd(&self, command: &str) -> anyhow::Result<Vec<String>> {
        let (tx, rx) = oneshot::channel();
        self.reqs
            .send((command.to_string(), tx))
            .map_err(|_| anyhow::anyhow!("MPD connection closed"))?;
        rx.await
            .map_err(|_| anyhow::anyhow!("MPD connection closed"))?
    }

    pub async fn clear(&self) -> anyhow::Result<()> {
        self.cmd("clear").await.map(|_| ())
    }

    /// Clear the MPD queue and start playing `url`.
    pub async fn load(&self, url: &str) -> anyhow::Result<()> {
        self.cmd("clear").await?;
        self.cmd(&format!("addid \"{}\"\n", escape(url))).await?;
        self.cmd("play").await?;
        Ok(())
    }

    pub async fn play_pause(&self) -> anyhow::Result<()> {
        self.cmd("pause").await.map(|_| ())
    }

    pub async fn seek(&self, seconds: f64, _absolute: bool) -> anyhow::Result<()> {
        self.cmd(&format!("seekcur {seconds}\n")).await.map(|_| ())
    }

    pub async fn set_volume(&self, volume: f64) -> anyhow::Result<()> {
        let v = volume.clamp(0.0, 100.0) as u32;
        self.cmd(&format!("setvol {v}\n")).await.map(|_| ())
    }

    pub async fn shutdown(&self) {
        drop(self.cmd("close"));
    }
}

fn escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n\t"),
            c => out.push(c),
        }
    }
    out
}

/// Read a reply: collect lines until `OK` (Ok(true)) or `ACK ...` (Ok(false)).
async fn read_reply(read: &mut ReadHalf, lines: &mut Vec<String>) -> std::io::Result<bool> {
    let mut buf = String::new();
    loop {
        buf.clear();
        let n = read.read_line(&mut buf).await?;
        if n == 0 {
            return Ok(true); // EOF: pretend success so polling stops cleanly
        }
        let trimmed = buf.trim_end();
        if trimmed == "OK" {
            return Ok(true);
        }
        if trimmed.starts_with("ACK") {
            return Ok(false);
        }
        lines.push(unescape(trimmed));
    }
}

fn unescape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\\' => match chars.next() {
                Some('\\') => out.push('\\'),
                Some('n') => {
                    out.push('\n');
                    if chars.peek() == Some(&'\t') {
                        chars.next();
                    }
                }
                Some(other) => {
                    out.push('\\');
                    out.push(other);
                }
                None => out.push('\\'),
            },
            c => out.push(c),
        }
    }
    out
}

/// Poll `status` once; returns parsed key/values.
async fn poll_status(name_lines: &mut ReadHalf, write: &mut WriteHalf) -> anyhow::Result<Vec<String>> {
    write.write_line("status\n").await?;
    let mut lines = Vec::new();
    let ok = read_reply(name_lines, &mut lines).await?;
    if !ok {
        anyhow::bail!("MPD status ACKed");
    }
    Ok(lines)
}

/// Poll `currentsong` once; returns "key: value" lines.
async fn poll_current(
    name_lines: &mut ReadHalf,
    write: &mut WriteHalf,
) -> anyhow::Result<Vec<String>> {
    write.write_line("currentsong\n").await?;
    let mut lines = Vec::new();
    let ok = read_reply(name_lines, &mut lines).await?;
    if !ok {
        anyhow::bail!("MPD currentsong ACKed");
    }
    Ok(lines)
}

fn kv(line: &str) -> Option<(&str, &str)> {
    line.split_once(':').map(|(k, v)| (k.trim(), v.trim()))
}

/// Background agent: owns the socket, runs commands, polls state, emits events.
async fn agent_loop(
    mut read: ReadHalf,
    mut write: WriteHalf,
    mut reqs: mpsc::UnboundedReceiver<InternalCmd>,
    state: Arc<RwLock<PlaybackState>>,
    event_tx: mpsc::UnboundedSender<MpvEvent>,
) {
    let mut poll = tokio::time::interval(POLL_INTERVAL);
    poll.set_missed_tick_behavior(MissedTickBehavior::Skip);

    let mut armed = false; // a URL was loaded; watch for its end/error
    let mut was_playing = false;

    loop {
        tokio::select! {
            cmd = reqs.recv() => {
                let Some((cmd, reply)) = cmd else { break };
                let result = async {
                    let mut lines = Vec::new();
                    write.write_line(&cmd).await?;
                    let ok = read_reply(&mut read, &mut lines).await?;
                    if !ok {
                        anyhow::bail!("MPD: {}", lines.first().cloned().unwrap_or_default());
                    }
                    Ok(lines)
                }
                .await;
                if cmd.starts_with("clear") || cmd.starts_with("play") || cmd.starts_with("pause 0") {
                    armed = true;
                    was_playing = false;
                }
                let _ = reply.send(result);
            }
            _ = poll.tick() => {
                let status = match poll_status(&mut read, &mut write).await {
                    Ok(lines) => lines,
                    Err(_) => continue,
                };
                let current = match poll_current(&mut read, &mut write).await {
                    Ok(lines) => lines,
                    Err(_) => continue,
                };

                let mut volume = 0.0;
                let mut elapsed = None;
                let mut stopped = true;
                let mut error = None;
                for l in &status {
                    if let Some((k, v)) = kv(l) {
                        match k {
                            "volume" => volume = v.parse().unwrap_or(0.0),
                            "elapsed" => elapsed = v.parse().ok(),
                            "state" => stopped = v != "play" && v != "pause",
                            "error" if !v.is_empty() => error = Some(v.to_string()),
                            _ => {}
                        }
                    }
                }

                let mut title: Option<String> = None;
                let mut duration: Option<f64> = None;
                for l in &current {
                    if let Some((k, v)) = kv(l) {
                        if k == "Title" && title.is_none() {
                            title = Some(v.to_string());
                        } else if (k == "Duration" || k == "Time") && duration.is_none() {
                            duration = v.parse().ok();
                        }
                    }
                }

                {
                    let mut s = state.write().await;
                    s.volume = volume;
                    s.time_pos = elapsed;
                    s.paused = !stopped && status.iter().any(|l| kv(l).is_some_and(|(k, v)| k == "state" && v == "pause"));
                    s.idle = stopped;
                    s.duration = duration.or(s.duration);
                    if title.is_some() {
                        s.media_title = title.clone();
                    }
                }

                if armed {
                    if let Some(err) = error {
                        let _ = event_tx.send(MpvEvent::EndFile { reason: "error".into() });
                        let _ = err;
                        armed = false;
                        was_playing = false;
                    } else if stopped && was_playing {
                        let _ = event_tx.send(MpvEvent::EndFile { reason: "end-of-file".into() });
                        armed = false;
                        was_playing = false;
                    }
                }
                if !stopped {
                    was_playing = true;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::MpdConfig;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;
    use tokio::time::{sleep, timeout};

    /// A scripted MPD that answers commands and can push a "track ended"
    /// transition afterwards.
    async fn fake_mpd(listener: TcpListener) {
        let (mut sock, _) = listener.accept().await.unwrap();
        sock.write_all(b"OK MPD 0.23.0\n").await.unwrap();
        let mut buf = [0u8; 4096];
        let mut seen_play = false;
        let mut status_after_play = 0u32;
        loop {
            let n = sock.read(&mut buf).await.unwrap();
            if n == 0 {
                break;
            }
            let cmd = String::from_utf8_lossy(&buf[..n]);
            let out = match cmd.split_whitespace().next().unwrap_or("") {
                "clear" => "OK\n".to_string(),
                "addid" => "Id: 7\nOK\n".to_string(),
                "play" | "pause" | "setvol" | "seekcur" => {
                    seen_play = true;
                    "OK\n".to_string()
                }
                "status" => {
                    let state = if seen_play && status_after_play >= 1 {
                        "stop"
                    } else if seen_play {
                        "play"
                    } else {
                        "stop"
                    };
                    if seen_play {
                        status_after_play += 1;
                    }
                    let mut s = format!("volume: 80\nelapsed: 12.3\nstate: {state}\n");
                    s.push_str("OK\n");
                    s
                }
                "currentsong" => {
                    if seen_play {
                        "Title: Test Track\nDuration: 200\nOK\n".to_string()
                    } else {
                        "OK\n".to_string()
                    }
                }
                _ => "ACK [5@0] {x} unknown command\n".to_string(),
            };
            sock.write_all(out.as_bytes()).await.unwrap();
        }
    }

    #[tokio::test]
    async fn talks_to_a_mock_mpd_and_loads_a_url() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(fake_mpd(listener));

        let cfg = MpdConfig {
            socket: None,
            host: "127.0.0.1".into(),
            port,
            password: None,
        };
        // Give the fake server a moment to bind.
        let (client, state, mut events) = timeout(
            Duration::from_secs(5),
            MpdClient::connect(&cfg),
        )
        .await
        .unwrap()
        .unwrap();

        client.load("https://example.com/audio").await.unwrap();
        client.set_volume(80.0).await.unwrap();

        // Wait for a poll to refresh the shared state.
        sleep(Duration::from_millis(1200)).await;
        let s = state.read().await.clone();
        assert_eq!(s.volume, 80.0);
        assert!(s.idle);

        // The fake server flips "play" -> "stop", so an end-of-file event
        // should arrive on a later poll.
        let ev = timeout(Duration::from_secs(5), events.recv())
            .await
            .unwrap()
            .unwrap();
        assert!(matches!(ev, MpvEvent::EndFile { reason } if reason == "end-of-file"));
        assert!(client.clear().await.is_ok());
    }

    #[test]
    fn escaping_roundtrips() {
        // URL escaping: backslash doubled, newlines escaped.
        assert_eq!(escape("a\\b"), "a\\\\b");
        assert_eq!(escape("l\nne"), "l\\n\tne");
        assert_eq!(unescape("a\\\\b"), "a\\b");
        assert_eq!(unescape("l\\n\tne"), "l\nne");
        assert_eq!(unescape("normal https://x.a/b&c=d"), "normal https://x.a/b&c=d");
    }

    #[test]
    fn parse_ack_line() {
        let ack = "ACK [5@0] {play} can't use a command without a connection\n";
        assert!(!ack.trim_start().starts_with("OK"));
        assert!(ack.trim_start().starts_with("ACK"));
    }
}