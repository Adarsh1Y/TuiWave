use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Gauge, List, ListItem, Paragraph, Wrap};

use crate::art;
use crate::tui::app::{Mode, RepeatMode, ViewData};

pub fn draw(frame: &mut Frame, data: &ViewData, mode: Mode) {
    match mode {
        Mode::Search => draw_search(frame, data),
        Mode::NowPlaying => draw_now_playing(frame, data),
        Mode::Queue => draw_queue(frame, data),
        Mode::Help => draw_help(frame),
    }
    if let Some(toast) = &data.toast {
        draw_toast(frame, toast);
    }
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
        Paragraph::new("Enter: play   a: add to queue   z: shuffle   ?: help")
            .style(Style::default().fg(Color::DarkGray)),
        chunks[1],
    );

    if data.results.is_empty() {
        frame.render_widget(
            Paragraph::new("type to search — matches appear here")
                .style(Style::default().fg(Color::DarkGray))
                .alignment(Alignment::Center),
            chunks[2],
        );
    } else {
        let items: Vec<ListItem> = data
            .results
            .iter()
            .map(|t| {
                ListItem::new(Line::from(vec![
                    Span::styled(
                        t.title.clone(),
                        Style::default().add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(
                        format!("  —  {}", t.artist),
                        Style::default().fg(Color::DarkGray),
                    ),
                    Span::styled(
                        format!("  {}", format_duration(t.duration)),
                        Style::default().fg(Color::Gray),
                    ),
                ]))
            })
            .collect();

        let list = List::new(items)
            .block(Block::default().borders(Borders::ALL).title(" Results "))
            .highlight_style(Style::default().bg(Color::Rgb(40, 44, 52)))
            .highlight_symbol("▶ ");
        frame.render_stateful_widget(
            list,
            chunks[2],
            &mut ratatui::widgets::ListState::default().with_selected(Some(
                data.selected.min(data.results.len().saturating_sub(1)),
            )),
        );
    }

    frame.render_widget(
        Paragraph::new("lastwave").style(Style::default().fg(Color::DarkGray)),
        chunks[3],
    );
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

    let media = vec![
        Line::from(vec![
            Span::styled("◉ ", Style::default().fg(Color::Green)),
            Span::styled(
                track.title.clone(),
                Style::default().bold().fg(Color::White),
            ),
        ]),
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
        Span::styled("n/p next prev  ", Style::default().fg(Color::Blue)),
        Span::styled("s search  ", Style::default().fg(Color::Blue)),
        Span::styled("z shuffle  ", Style::default().fg(Color::Blue)),
        Span::styled("r repeat  ", Style::default().fg(Color::Blue)),
        Span::styled("q quit", Style::default().fg(Color::Blue)),
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
            ListItem::new(Line::from(vec![
                Span::raw(prefix),
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
                .highlight_style(Style::default().bg(Color::Rgb(40, 44, 52)))
                .highlight_symbol(""),
            chunks[1],
            &mut ratatui::widgets::ListState::default()
                .with_selected(Some(data.cursor.min(data.queue.len().saturating_sub(1)))),
        );
    }

    frame.render_widget(
        Paragraph::new("enter: play   d: remove   j/k: move   o/esc: back")
            .style(Style::default().fg(Color::DarkGray)),
        chunks[2],
    );
}

fn draw_help(frame: &mut Frame) {
    let text = concat!(
        " LastWave — bindings\n\n",
        "   s / /            search\n",
        "   enter            play selection\n",
        "   a                add selection to queue\n",
        "   space            play / pause\n",
        "   ← / →            seek 10s (shift: 60s)\n",
        "   , / .            volume down / up\n",
        "   n / p            next / previous\n",
        "   j / k            move down / up (lists)\n",
        "   o / t            open queue\n",
        "   z                toggle shuffle\n",
        "   r                cycle repeat\n",
        "   q                quit\n\n",
        "   press any key to close",
    );
    let area = centered(70, 20, frame.area());
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
