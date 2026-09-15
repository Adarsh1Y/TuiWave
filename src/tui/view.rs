use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Gauge, List, ListItem, Paragraph, Wrap};

use crate::art;
use crate::tui::app::{Mode, PromptKind, RepeatMode, ViewData};

const HIGHLIGHT: Color = Color::Rgb(40, 44, 52);

fn footer<'a>(text: &'a str) -> Paragraph<'a> {
    Paragraph::new(text).style(Style::default().fg(Color::DarkGray))
}

pub fn draw(frame: &mut Frame, data: &ViewData, mode: Mode) {
    match mode {
        Mode::Search => draw_search(frame, data),
        Mode::NowPlaying => draw_now_playing(frame, data),
        Mode::Queue => draw_queue(frame, data),
        Mode::Help => draw_help(frame),
        Mode::Prompt => draw_prompt(frame, data),
        Mode::Playlists => draw_playlists(frame, data),
        Mode::PlaylistDetail => draw_playlist_detail(frame, data),
        Mode::Local => draw_local(frame, data),
    }
    if let Some(toast) = &data.toast {
        draw_toast(frame, toast);
    }
}

fn heart(track_liked: bool) -> &'static str {
    if track_liked {
        "♥ "
    } else {
        "  "
    }
}

fn track_line<'a>(t: &'a crate::model::Track, liked: bool, show_like: bool) -> Line<'a> {
    let mut spans = Vec::new();
    if show_like {
        spans.push(Span::styled(
            heart(liked),
            Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
        ));
    }
    spans.push(Span::styled(
        t.title.clone(),
        Style::default().add_modifier(Modifier::BOLD),
    ));
    spans.push(Span::styled(
        format!("  —  {}", t.artist),
        Style::default().fg(Color::DarkGray),
    ));
    if let Some(album) = &t.album {
        spans.push(Span::styled(
            format!("  [{album}]"),
            Style::default().fg(Color::Gray),
        ));
    }
    spans.push(Span::styled(
        format!("  {}", format_duration(t.duration)),
        Style::default().fg(Color::Gray),
    ));
    Line::from(spans)
}

fn draw_search(frame: &mut Frame, data: &ViewData) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Length(1),
            Constraint::Min(0),
            Constraint::Length(1),
        ])
        .split(frame.area());

    frame.render_widget(
        Paragraph::new(data.query.as_str())
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(Line::from(" Search ").centered()),
            )
            .style(Style::default().fg(Color::Cyan)),
        chunks[0],
    );
    frame.set_cursor_position((chunks[0].x + 2 + data.query.len() as u16, chunks[0].y + 1));

    frame.render_widget(
        footer("type: query   enter: play   alt+l: like   esc: back   ctrl+c: quit"),
        chunks[1],
    );

    if data.results.is_empty() {
        frame.render_widget(
            Paragraph::new("type to search — songs appear here")
                .style(Style::default().fg(Color::DarkGray))
                .alignment(Alignment::Center),
            chunks[2],
        );
    } else {
        let items: Vec<ListItem> = data
            .results
            .iter()
            .map(|t| {
                ListItem::new(track_line(
                    t,
                    data.liked_keys.contains(&t.key()),
                    true,
                ))
            })
            .collect();

        let list = List::new(items)
            .block(Block::default().borders(Borders::ALL).title(" Results "))
            .highlight_style(Style::default().bg(HIGHLIGHT))
            .highlight_symbol("▶ ");
        frame.render_stateful_widget(
            list,
            chunks[2],
            &mut ratatui::widgets::ListState::default().with_selected(Some(
                data.selected.min(data.results.len().saturating_sub(1)),
            )),
        );
    }

    frame.render_widget(footer("lastwave — bare letters type"), chunks[3]);
}

fn draw_now_playing(frame: &mut Frame, data: &ViewData) {
    let outer = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(0),
            Constraint::Length(2),
            Constraint::Length(1),
        ])
        .split(frame.area());

    let inner = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Min(0), Constraint::Length(24)])
        .split(outer[0]);

    let Some(track) = data.current.as_ref() else {
        frame.render_widget(
            Paragraph::new("Nothing playing yet — press s to search")
                .style(Style::default().fg(Color::DarkGray))
                .alignment(Alignment::Center),
            outer[0],
        );
        return;
    };
    let state = &data.playback;

    let info = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(4),
            Constraint::Length(3),
            Constraint::Min(0),
        ])
        .split(inner[0]);

    let liked = data.liked_keys.contains(&track.key());
    let mut title_spans = vec![Span::styled(
        "◉ ",
        Style::default().fg(Color::Green),
    )];
    if liked {
        title_spans.push(Span::styled(
            "♥ ",
            Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
        ));
    }
    title_spans.push(Span::styled(
        track.title.clone(),
        Style::default().bold().fg(Color::White),
    ));
    if let Some(codec) = &data.current_codec {
        title_spans.push(Span::styled(
            format!("   [{codec}]"),
            Style::default().fg(Color::Green),
        ));
    }
    let media = vec![
        Line::from(title_spans),
        Line::from(vec![
            Span::styled(track.artist.clone(), Style::default().fg(Color::Cyan)),
            Span::styled(" · ", Style::default().fg(Color::DarkGray)),
            Span::styled(
                track.album.clone().unwrap_or_default(),
                Style::default().fg(Color::DarkGray),
            ),
        ]),
    ];
    frame.render_widget(
        Paragraph::new(media).block(
            Block::default()
                .borders(Borders::ALL)
                .title(" Now Playing "),
        ),
        info[0],
    );

    frame.render_widget(progress(state), info[1]);

    let mut line = vec![
        Span::styled("space pause  ", Style::default().fg(Color::Blue)),
        Span::styled("← → seek  ", Style::default().fg(Color::Blue)),
        Span::styled(", . vol  ", Style::default().fg(Color::Blue)),
        Span::styled("alt+n/p next prev  ", Style::default().fg(Color::Blue)),
        Span::styled("alt+l like  ", Style::default().fg(Color::Blue)),
        Span::styled("alt+r repeat  ", Style::default().fg(Color::Blue)),
        Span::styled("alt+z shuffle  ", Style::default().fg(Color::Blue)),
        Span::styled("alt+e/alt+x eq  ", Style::default().fg(Color::Blue)),
        Span::styled("s search  ", Style::default().fg(Color::Blue)),
        Span::styled("q quit", Style::default().fg(Color::Blue)),
        Span::styled("  ", Style::default().fg(Color::DarkGray)),
        Span::styled(data.eq.clone(), Style::default().fg(Color::Cyan)),
    ];
    line.push(Span::raw(if data.shuffle { "  🔀" } else { "  ·" }));
    line.push(Span::raw(match data.repeat {
        RepeatMode::Off => "",
        RepeatMode::All => " 🔁",
        RepeatMode::One => " 🔂",
    }));
    frame.render_widget(Paragraph::new(Line::from(line)), outer[1]);

    render_art_area(frame, inner[1], data);
}

fn draw_queue(frame: &mut Frame, data: &ViewData) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(0),
            Constraint::Length(1),
        ])
        .split(frame.area());

    frame.render_widget(
        Paragraph::new(format!(
            "Now: {} / Queue ({} tracks)",
            data.current
                .as_ref()
                .map(|t| t.title.clone())
                .unwrap_or_else(|| "—".to_string()),
            data.queue.len()
        ))
        .block(Block::default().borders(Borders::ALL).title(" Queue ")),
        chunks[0],
    );

    let items: Vec<ListItem> = data
        .queue
        .iter()
        .map(|t| {
            let is_current = data.current.as_ref().map(|c| &c.video_id) == Some(&t.video_id);
            let prefix = if is_current { "▶" } else { " " };
            let liked = data.liked_keys.contains(&t.key());
            ListItem::new(Line::from(vec![
                Span::raw(prefix),
                Span::styled(
                    heart(liked),
                    Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    t.title.clone(),
                    Style::default().add_modifier(if is_current {
                        Modifier::BOLD
                    } else {
                        Modifier::empty()
                    }),
                ),
                Span::styled(
                    format!(" — {}", t.artist),
                    Style::default().fg(Color::DarkGray),
                ),
                Span::styled(
                    format!(" {}", format_duration(t.duration)),
                    Style::default().fg(Color::Gray),
                ),
            ]))
        })
        .collect();

    if data.queue.is_empty() {
        frame.render_widget(
            Paragraph::new("empty — search for something first (s)")
                .style(Style::default().fg(Color::DarkGray))
                .alignment(Alignment::Center),
            chunks[1],
        );
    } else {
        frame.render_stateful_widget(
            List::new(items)
                .block(Block::default().borders(Borders::ALL).title(" Tracks "))
                .highlight_style(Style::default().bg(HIGHLIGHT)),
            chunks[1],
            &mut ratatui::widgets::ListState::default()
                .with_selected(Some(data.cursor.min(data.queue.len().saturating_sub(1)))),
        );
    }

    frame.render_widget(
        footer("enter: play   alt+l: like   alt+d: remove   alt+o/alt+t: back   j/k: move   esc: back"),
        chunks[2],
    );
}

fn draw_prompt(frame: &mut Frame, data: &ViewData) {
    let label = match data.prompt_kind {
        PromptKind::SaveQueue => "Save queue as playlist",
        PromptKind::LoadYtPlaylist => "Paste YouTube Music playlist URL or id",
    };
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(0),
            Constraint::Length(1),
        ])
        .split(frame.area());

    frame.render_widget(
        Paragraph::new(data.prompt.as_str())
            .block(Block::default().borders(Borders::ALL).title(label)),
        chunks[0],
    );
    frame.set_cursor_position((chunks[0].x + 1 + data.prompt.len() as u16, chunks[0].y + 1));
    frame.render_widget(
        Paragraph::new("enter: confirm   esc: cancel")
            .style(Style::default().fg(Color::DarkGray)),
        chunks[2],
    );
}

fn draw_playlists(frame: &mut Frame, data: &ViewData) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Min(0),
            Constraint::Length(1),
        ])
        .split(frame.area());

    frame.render_widget(
        Paragraph::new("Playlists — alt+L opens Liked Songs")
            .style(Style::default().fg(Color::Cyan)),
        chunks[0],
    );

    let mut items = vec![ListItem::new(Line::from(vec![
        Span::styled("♥ ", Style::default().fg(Color::Red).add_modifier(Modifier::BOLD)),
        Span::styled(
            format!("Liked Songs ({})", data.liked_keys.len()),
            Style::default().bold(),
        ),
    ]))];
    for name in &data.playlist_names {
        items.push(ListItem::new(Line::from(vec![
            Span::styled("♪ ", Style::default().fg(Color::Green)),
            Span::styled(name.clone(), Style::default()),
        ])));
    }

    let list = List::new(items)
        .block(Block::default().borders(Borders::ALL).title(" Playlists "))
        .highlight_style(Style::default().bg(HIGHLIGHT))
        .highlight_symbol("▶ ");
    let selected = if data.playlist_cursor == 0 && data.playlist_names.is_empty() {
        0
    } else {
        data.playlist_cursor.min(data.playlist_names.len())
    };
    frame.render_stateful_widget(
        list,
        chunks[1],
        &mut ratatui::widgets::ListState::default().with_selected(Some(selected)),
    );

    frame.render_widget(
        footer("enter: open   alt+L: liked   alt+P: back   j/k: move   esc: back"),
        chunks[2],
    );
}

fn draw_playlist_detail(frame: &mut Frame, data: &ViewData) {
    let title = match &data.active_playlist {
        Some(name) => format!(" Playlist: {name} "),
        None => " Liked Songs ".to_string(),
    };
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(0),
            Constraint::Length(1),
        ])
        .split(frame.area());

    frame.render_widget(
        Paragraph::new(format!("{} tracks", data.playlist_tracks.len()))
            .block(Block::default().borders(Borders::ALL).title(title)),
        chunks[0],
    );

    let items: Vec<ListItem> = data
        .playlist_tracks
        .iter()
        .map(|t| {
            ListItem::new(track_line(
                t,
                data.liked_keys.contains(&t.key()),
                true,
            ))
        })
        .collect();

    if data.playlist_tracks.is_empty() {
        frame.render_widget(
            Paragraph::new("empty")
                .style(Style::default().fg(Color::DarkGray))
                .alignment(Alignment::Center),
            chunks[1],
        );
    } else {
        frame.render_stateful_widget(
            List::new(items)
                .block(Block::default().borders(Borders::ALL).title(" Tracks "))
                .highlight_style(Style::default().bg(HIGHLIGHT))
                .highlight_symbol("▶ "),
            chunks[1],
            &mut ratatui::widgets::ListState::default().with_selected(Some(
                data.playlist_cursor.min(data.playlist_tracks.len().saturating_sub(1)),
            )),
        );
    }

    let hint = if data.active_playlist.is_some() {
        "enter: play   alt+l: like   alt+d: remove   alt+x: delete playlist   esc: back"
    } else {
        "enter: play   alt+l: like   alt+d: unlike   esc: back"
    };
    frame.render_widget(
        footer(hint),
        chunks[2],
    );
}

fn draw_local(frame: &mut Frame, data: &ViewData) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(0),
            Constraint::Length(1),
        ])
        .split(frame.area());

    frame.render_widget(
        Paragraph::new(format!("{} local files", data.local_tracks.len()))
            .block(Block::default().borders(Borders::ALL).title(" Local Files (FLAC/Opus/MP3) ")),
        chunks[0],
    );

    let items: Vec<ListItem> = data
        .local_tracks
        .iter()
        .map(|t| {
            let ext = t
                .source_path_extension()
                .map(|e| e.to_uppercase())
                .unwrap_or_default();
            ListItem::new(Line::from(vec![
                Span::styled(
                    heart(data.liked_keys.contains(&t.key())),
                    Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
                ),
                Span::styled(t.title.clone(), Style::default().add_modifier(Modifier::BOLD)),
                Span::styled(
                    format!("  [{}]", ext),
                    Style::default().fg(Color::Cyan),
                ),
                Span::styled(
                    format!("  {}", t.album.clone().unwrap_or_default()),
                    Style::default().fg(Color::DarkGray),
                ),
            ]))
        })
        .collect();

    if data.local_tracks.is_empty() {
        frame.render_widget(
            Paragraph::new(format!(
                "no audio files found — set local_dirs in {}",
                crate::config::Config::path()
                    .map(|p| p.display().to_string())
                    .unwrap_or_default()
            ))
            .style(Style::default().fg(Color::DarkGray))
            .alignment(Alignment::Center),
            chunks[1],
        );
    } else {
        frame.render_stateful_widget(
            List::new(items)
                .block(Block::default().borders(Borders::ALL).title(" Files "))
                .highlight_style(Style::default().bg(HIGHLIGHT))
                .highlight_symbol("▶ "),
            chunks[1],
            &mut ratatui::widgets::ListState::default().with_selected(Some(
                data.local_cursor.min(data.local_tracks.len().saturating_sub(1)),
            )),
        );
    }

    frame.render_widget(
        footer("enter: play   alt+l: like   j/k: move   esc: back"),
        chunks[2],
    );
}

fn draw_help(frame: &mut Frame) {
    let text = concat!(
        " LastWave — bindings\n\n",
        "   s / /            search\n",
        "   enter            play selection\n",
        "   space            play / pause\n",
        "   ← / →            seek 10s (shift: 60s)\n",
        "   , / .            volume down / up\n",
        "   j / k            move down / up (lists)\n",
        "   alt+n / alt+p    next / previous\n",
        "   alt+l            like / unlike (heart)\n",
        "   alt+L            open Liked Songs\n",
        "   alt+P            open playlists\n",
        "   alt+o / alt+t    open queue\n",
        "   alt+y            load a YouTube Music playlist\n",
        "   alt+S            save current queue as playlist\n",
        "   alt+u            local files (FLAC/Opus/MP3)\n",
        "   alt+e            toggle EQ preset (deep bass)\n",
        "   alt+x            reset EQ to clean\n",
        "   alt+z            toggle shuffle\n",
        "   alt+r            cycle repeat\n",
        "   q                quit\n\n",
        "   bare letters type in search; alt+letter acts\n",
        "   press any key to close",
    );
    let area = centered(70, 27, frame.area());
    frame.render_widget(Clear, area);
    frame.render_widget(
        Paragraph::new(text)
            .block(Block::default().borders(Borders::ALL).title(" Help "))
            .style(Style::default().fg(Color::White))
            .wrap(Wrap { trim: true }),
        area,
    );
}

fn progress(state: &crate::mpv::PlaybackState) -> Gauge<'static> {
    let duration = state.duration.unwrap_or(0.0);
    let position = state.time_pos.unwrap_or(0.0);
    let ratio = if duration > 0.0 && position <= duration {
        (position / duration).clamp(0.0, 1.0)
    } else {
        0.0
    };
    Gauge::default()
        .block(Block::default().borders(Borders::ALL).title(" progress "))
        .ratio(ratio)
        .label(format!(
            "{} / {}",
            format_time(position),
            format_time(duration)
        ))
        .gauge_style(Style::default().fg(Color::Green).bg(Color::Rgb(30, 30, 30)))
}

fn render_art_area(frame: &mut Frame, area: Rect, data: &ViewData) {
    let art_area = Rect {
        x: area.x + 1,
        y: area.y + 1,
        width: area.width.saturating_sub(2),
        height: area.height.saturating_sub(2),
    };
    if let Some(art) = &data.art {
        let bounded = Rect {
            width: art.cols.min(art_area.width),
            height: art.rows.min(art_area.height),
            ..art_area
        };
        frame.render_widget(Clear, bounded);
        art::render_into(frame.buffer_mut(), bounded, art);
    } else {
        frame.render_widget(
            Paragraph::new(" no cover ")
                .style(Style::default().fg(Color::DarkGray))
                .alignment(Alignment::Center),
            art_area,
        );
    }
}

fn draw_toast(frame: &mut Frame, msg: &str) {
    let width = (msg.len() as u16 + 4).clamp(4, frame.area().width.saturating_sub(2));
    let area = Rect {
        x: frame.area().width.saturating_sub(width + 2),
        y: frame.area().height.saturating_sub(2),
        width,
        height: 1,
    };
    frame.render_widget(Clear, area);
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            format!(" {msg} "),
            Style::default().fg(Color::Black).bg(Color::Yellow),
        ))),
        area,
    );
}

fn centered(width: u16, height: u16, area: Rect) -> Rect {
    let x = area.x + area.width.saturating_sub(width) / 2;
    let y = area.y + area.height.saturating_sub(height) / 2;
    Rect {
        x,
        y,
        width: width.min(area.width),
        height: height.min(area.height),
    }
}

fn format_time(secs: f64) -> String {
    if !secs.is_finite() || secs < 0.0 {
        return "0:00".to_string();
    }
    let total = secs as u64;
    let h = total / 3600;
    let m = (total % 3600) / 60;
    let s = total % 60;
    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m}:{s:02}")
    }
}

fn format_duration(secs: Option<u32>) -> String {
    secs.map(|s| format_time(s as f64)).unwrap_or_default()
}