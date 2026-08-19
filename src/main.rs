mod theme;
mod player;

use std::path::PathBuf;
use std::time::Duration;
use clap::Parser;
use crossterm::{
    event::{self, Event, KeyCode, KeyEventKind, MediaKeyCode, MouseButton, MouseEventKind, EnableMouseCapture, DisableMouseCapture},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{
    backend::CrosstermBackend,
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph},
    Terminal,
};
use souvlaki::{MediaControlEvent, MediaControls, MediaMetadata, MediaPlayback, MediaPosition, PlatformConfig, SeekDirection};

use theme::*;
use player::{Player, TrackInfo};

fn get_default_music_dir() -> PathBuf {
    if let Some(user_dirs) = std::env::var_os("HOME") {
        let music_path = PathBuf::from(user_dirs).join("Music");
        if music_path.exists() {
            return music_path;
        }
    }
    PathBuf::from(".")
}

#[derive(Parser, Debug)]
#[command(author, version, about = "Modern Rust Music Player with Nord Palette")]
struct Args {
    /// Directory containing music files (MP3, FLAC, WAV, OGG, M4A)
    #[arg(short, long, default_value_os_t = get_default_music_dir())]
    music_dir: PathBuf,
}

#[derive(PartialEq, Eq)]
enum ActiveFocus {
    ArtistColumn,
    AlbumColumn,
}

/// Helper function to create centered modal dialog Rect using exact width and height
fn centered_rect_fixed(width: u16, height: u16, r: Rect) -> Rect {
    let x = r.x + r.width.saturating_sub(width) / 2;
    let y = r.y + r.height.saturating_sub(height) / 2;
    Rect::new(x, y, width.min(r.width), height.min(r.height))
}

fn update_mpris_state(controls: &mut MediaControls, player: &Player) {
    if let Some(track) = player.current_track() {
        let _ = controls.set_metadata(MediaMetadata {
            title: Some(&track.title),
            album: Some(&track.album),
            artist: Some(&track.artist),
            duration: Some(track.duration),
            cover_url: None,
        });
        let progress = Some(MediaPosition(player.current_elapsed_duration()));
        if player.is_paused {
            let _ = controls.set_playback(MediaPlayback::Paused { progress });
        } else {
            let _ = controls.set_playback(MediaPlayback::Playing { progress });
        }
    } else {
        let _ = controls.set_playback(MediaPlayback::Stopped);
    }
}

fn main() -> color_eyre::Result<()> {
    color_eyre::install()?;
    let args = Args::parse();

    // Terminal setup with Mouse Capture enabled
    enable_raw_mode()?;
    let mut stdout = std::io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    // Initialize player and load music directory
    let mut player = Player::new()?;
    let _ = player.load_directory(&args.music_dir);

    // Initialize OS-level Media Controls (MPRIS on Linux for laptop media keys)
    let (mpris_tx, mpris_rx) = std::sync::mpsc::channel();
    let mut media_controls = {
        let config = PlatformConfig {
            dbus_name: "music_rust",
            display_name: "music-rust",
            hwnd: None,
        };
        let mut controls = MediaControls::new(config).ok();
        if let Some(c) = controls.as_mut() {
            let _ = c.attach(move |event| {
                let _ = mpris_tx.send(event);
            });
        }
        controls
    };

    let mut active_focus = ActiveFocus::ArtistColumn;
    let mut artist_list_state = ListState::default();
    let mut album_list_state = ListState::default();
    let mut show_about_modal = false;

    if !player.artists.is_empty() {
        artist_list_state.select(Some(0)); // Select "All Artists"
    }
    if !player.artists.is_empty() {
        album_list_state.select(Some(1));
    }

    let mut running = true;
    let mut last_artist_rect = Rect::default();
    let mut last_album_rect = Rect::default();
    let mut last_progress_bar_x: u16 = 0;
    let mut last_progress_bar_width: u16 = 0;
    let mut last_progress_bar_y: u16 = 0;
    let mut last_vol_bar_x: u16 = 0;
    let mut last_vol_bar_width: u16 = 10;
    let mut last_now_playing_y: u16 = 0;

    let mut last_click_time = std::time::Instant::now() - Duration::from_secs(10);
    let mut last_clicked_track_idx: Option<usize> = None;

    let mut last_mpris_track_idx: Option<usize> = None;
    let mut last_mpris_paused: bool = false;

    while running {
        // Process MPRIS OS-level media control events (laptop media keys, playerctl, GNOME shell)
        while let Ok(mpris_event) = mpris_rx.try_recv() {
            match mpris_event {
                MediaControlEvent::Play => {
                    if player.is_paused {
                        player.toggle_pause();
                    } else if player.current_track_index.is_none() && !player.flat_playlist.is_empty() {
                        let _ = player.play_index(0);
                    }
                }
                MediaControlEvent::Pause => {
                    if !player.is_paused && player.current_track_index.is_some() {
                        player.toggle_pause();
                    }
                }
                MediaControlEvent::Toggle => {
                    if player.current_track_index.is_some() {
                        player.toggle_pause();
                    } else if !player.flat_playlist.is_empty() {
                        let _ = player.play_index(0);
                    }
                }
                MediaControlEvent::Next => {
                    let _ = player.next();
                }
                MediaControlEvent::Previous => {
                    let _ = player.previous();
                }
                MediaControlEvent::Stop => {
                    if !player.is_paused && player.current_track_index.is_some() {
                        player.toggle_pause();
                    }
                }
                MediaControlEvent::Seek(direction) => {
                    let cur = player.current_elapsed_duration();
                    let delta = Duration::from_secs(5);
                    match direction {
                        SeekDirection::Forward => player.seek_to(cur + delta),
                        SeekDirection::Backward => player.seek_to(cur.saturating_sub(delta)),
                    }
                }
                MediaControlEvent::SetPosition(pos) => {
                    player.seek_to(pos.0);
                }
                MediaControlEvent::SetVolume(vol) => {
                    player.set_volume(vol as f32);
                }
                MediaControlEvent::Quit => {
                    running = false;
                }
                _ => {}
            }
        }

        // Sync MPRIS state on track change or pause/resume
        let current_idx = player.current_track_index;
        let current_paused = player.is_paused;
        if current_idx != last_mpris_track_idx || current_paused != last_mpris_paused {
            last_mpris_track_idx = current_idx;
            last_mpris_paused = current_paused;
            if let Some(controls) = media_controls.as_mut() {
                update_mpris_state(controls, &player);
            }
        }

        // 1. Build Left Column (Artists List)
        let mut artist_items: Vec<ListItem> = Vec::new();
        artist_items.push(ListItem::new(" All Artists").style(Style::default().fg(NORD8).add_modifier(Modifier::BOLD)));
        for artist in &player.artists {
            artist_items.push(ListItem::new(format!(" {}", artist.name)).style(Style::default().fg(NORD4)));
        }

        // Selected Artist Filter
        let selected_artist_idx = artist_list_state.selected().unwrap_or(0);
        let current_albums = if selected_artist_idx == 0 {
            player.artists.iter().flat_map(|a| a.albums.clone()).collect::<Vec<_>>()
        } else if let Some(artist) = player.artists.get(selected_artist_idx - 1) {
            artist.albums.clone()
        } else {
            Vec::new()
        };

        // 2. Build Right Column (Albums & Tracks for selected artist)
        let mut album_items: Vec<ListItem> = Vec::new();
        let mut row_to_track: Vec<Option<TrackInfo>> = Vec::new();

        for album in &current_albums {
            // Album Header row
            album_items.push(ListItem::new(Line::from(vec![
                Span::styled(" ", Style::default()),
                Span::styled(&album.name, Style::default().fg(NORD8).add_modifier(Modifier::BOLD)),
                Span::styled(format!(" • {}", album.artist), Style::default().fg(NORD4)),
            ])).style(Style::default().bg(NORD1)));
            row_to_track.push(None);

            // Album Tracks
            for track in &album.tracks {
                let is_current = player
                    .current_track_index
                    .and_then(|idx| player.flat_playlist.get(idx))
                    .map(|curr| curr.path == track.path)
                    .unwrap_or(false);

                let prefix = if is_current { "  ► " } else { "    " };
                let track_style = if is_current {
                    Style::default().fg(NORD14).add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(NORD4)
                };

                album_items.push(ListItem::new(Line::from(vec![
                    Span::styled(prefix, Style::default().fg(NORD14)),
                    Span::styled(&track.title, track_style),
                ])));
                row_to_track.push(Some(track.clone()));
            }
        }

        let total_artist_rows = artist_items.len();
        let total_album_rows = album_items.len();

        terminal.draw(|f| {
            let chunks = Layout::default()
                .direction(Direction::Vertical)
                .constraints([
                    Constraint::Length(3),  // Header / Status bar
                    Constraint::Min(6),     // 2-Column Split View (Artists left, Albums right)
                    Constraint::Length(4),  // Now Playing & Playback Controls Box
                ])
                .split(f.area());

            let header_p = Paragraph::new(Line::from(vec![
                Span::styled("MUSIC-RUST", Style::default().fg(NORD8).add_modifier(Modifier::BOLD)),
            ]))
            .alignment(Alignment::Center)
            .block(Block::default().borders(Borders::ALL).border_style(Style::default().fg(NORD9)));
            f.render_widget(header_p, chunks[0]);

            // 2-Column Layout Split
            let main_columns = Layout::default()
                .direction(Direction::Horizontal)
                .constraints([
                    Constraint::Percentage(30), // Left: Artists Column
                    Constraint::Percentage(70), // Right: Albums & Tracks Column
                ])
                .split(chunks[1]);

            last_artist_rect = main_columns[0];
            last_album_rect = main_columns[1];

            // Left Column: Artists
            let artist_border_color = if active_focus == ActiveFocus::ArtistColumn { NORD8 } else { NORD3 };
            let artist_widget = List::new(artist_items)
                .block(Block::default().borders(Borders::ALL).border_style(Style::default().fg(artist_border_color)).title(" Artists "))
                .highlight_style(Style::default().bg(NORD2).fg(NORD6).add_modifier(Modifier::BOLD));
            f.render_stateful_widget(artist_widget, main_columns[0], &mut artist_list_state);

            // Right Column: Albums & Tracks
            let album_border_color = if active_focus == ActiveFocus::AlbumColumn { NORD8 } else { NORD3 };
            let album_widget = List::new(album_items)
                .block(Block::default().borders(Borders::ALL).border_style(Style::default().fg(album_border_color)).title(" Albums & Tracks "))
                .highlight_style(Style::default().bg(NORD2).fg(NORD6).add_modifier(Modifier::BOLD));
            f.render_stateful_widget(album_widget, main_columns[1], &mut album_list_state);

            // 3. Now Playing & Playback Controls Box
            let (status_icon, status_color) = if player.is_paused {
                (" ⏸ ", NORD13)
            } else if player.current_track_index.is_some() {
                (" ▶ ", NORD14)
            } else {
                (" ⏹ ", NORD3)
            };

            let now_playing_text = if let Some(track) = player.current_track() {
                vec![
                    Span::styled(status_icon, Style::default().fg(status_color).add_modifier(Modifier::BOLD)),
                    Span::styled(&track.title, Style::default().fg(NORD6).add_modifier(Modifier::BOLD)),
                    Span::styled(format!(" by {}", track.artist), Style::default().fg(NORD8)),
                    Span::styled(format!("  from {}", track.album), Style::default().fg(NORD7)),
                ]
            } else {
                vec![
                    Span::styled(status_icon, Style::default().fg(status_color)),
                    Span::styled("No track selected", Style::default().fg(NORD3).add_modifier(Modifier::ITALIC)),
                ]
            };

            // Playback Progress Bar & Volume Slider Layout
            let elapsed = player.current_elapsed_duration();
            let total_dur = player.current_track().map(|t| t.duration).unwrap_or(std::time::Duration::ZERO);
            
            let elapsed_sec = elapsed.as_secs();
            let total_sec = total_dur.as_secs();
            let remaining_sec = total_sec.saturating_sub(elapsed_sec);

            let elapsed_str = format!("{}:{:02}", elapsed_sec / 60, elapsed_sec % 60);
            let total_str = format!("{}:{:02}", total_sec / 60, total_sec % 60);
            let remaining_str = format!("(-{}:{:02})", remaining_sec / 60, remaining_sec % 60);

            // Volume Indicator Bar (Vol: 100% [──────────])
            let vol_pct = (player.volume * 100.0).round() as u32;
            let vol_blocks = ((player.volume * 10.0).round() as usize).clamp(0, 10);
            let vol_filled = "─".repeat(vol_blocks);
            let vol_empty = "─".repeat(10 - vol_blocks);

            let vol_prefix = format!("Vol: {:3}% [", vol_pct);
            let vol_suffix = "]";

            let fixed_meta_len = (elapsed_str.len() + 2)
                + (total_str.len() + 2)
                + (remaining_str.len() + 2)
                + (vol_prefix.len() + 2)
                + 10
                + 1;

            let inner_width = chunks[2].width.saturating_sub(2) as usize;
            let progress_width = inner_width.saturating_sub(fixed_meta_len);
            
            let elapsed_label_len = (elapsed_str.len() + 2) as u16;
            last_now_playing_y = chunks[2].y + 1;
            last_progress_bar_y = chunks[2].y + 2;
            last_progress_bar_x = chunks[2].x + 1 + elapsed_label_len;
            last_progress_bar_width = progress_width as u16;

            last_vol_bar_x = last_progress_bar_x
                + last_progress_bar_width
                + (total_str.len() + 2) as u16
                + (remaining_str.len() + 2) as u16
                + 2
                + vol_prefix.len() as u16;
            last_vol_bar_width = 10;

            let ratio = if total_sec > 0 {
                (elapsed_sec as f32 / total_sec as f32).clamp(0.0, 1.0)
            } else {
                0.0
            };

            let thumb_pos = (ratio * progress_width.saturating_sub(1) as f32).round() as usize;
            let mut bar_chars = Vec::new();
            for i in 0..progress_width {
                if i == thumb_pos && player.current_track_index.is_some() {
                    bar_chars.push(Span::styled("█", Style::default().fg(NORD4)));
                } else if i < thumb_pos {
                    bar_chars.push(Span::styled("─", Style::default().fg(NORD3)));
                } else {
                    bar_chars.push(Span::styled("─", Style::default().fg(NORD1)));
                }
            }

            let mut progress_spans = vec![
                Span::styled(format!(" {} ", elapsed_str), Style::default().fg(NORD3).add_modifier(Modifier::BOLD)),
            ];
            progress_spans.extend(bar_chars);
            progress_spans.push(Span::styled(format!(" {} ", total_str), Style::default().fg(NORD3)));
            progress_spans.push(Span::styled(format!(" {} ", remaining_str), Style::default().fg(NORD2)));
            progress_spans.push(Span::styled(format!("  {}", vol_prefix), Style::default().fg(NORD3)));
            progress_spans.push(Span::styled(vol_filled, Style::default().fg(NORD3).add_modifier(Modifier::BOLD)));
            progress_spans.push(Span::styled(vol_empty, Style::default().fg(NORD1)));
            progress_spans.push(Span::styled(vol_suffix, Style::default().fg(NORD3)));

            let playback_border_color = if player.current_track_index.is_some() && !player.is_paused {
                NORD8
            } else {
                NORD3
            };

            let playback_paragraph = Paragraph::new(vec![
                Line::from(now_playing_text).alignment(Alignment::Center),
                Line::from(progress_spans),
            ])
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(playback_border_color))
                    .title(" Now Playing "),
            );
            f.render_widget(playback_paragraph, chunks[2]);

            // 4. Render Modal About & Shortcuts Overlay if Active
            if show_about_modal {
                let area = centered_rect_fixed(66, 16, f.area());
                f.render_widget(Clear, area);

                let about_shortcuts = [
                    ("Tab, ←, →, h, l", "Switch Column"),
                    ("Space, Media Play", "Play / Pause"),
                    ("Enter, Double-Click", "Play Selected Song"),
                    ("N / P, Media Next", "Next / Previous Track"),
                    ("+ / -, Mouse Wheel", "Volume Up / Down"),
                    ("j / k, ↑ / ↓", "Navigate Lists"),
                    ("Mouse Click / Drag", "Seek & Select Track"),
                    ("A", "About & Shortcuts"),
                    ("Q, Esc", "Quit Application"),
                ];

                let mut about_text = vec![
                    Line::from(vec![
                        Span::styled("MUSIC-RUST v0.1.0", Style::default().fg(NORD10).add_modifier(Modifier::BOLD)),
                    ]).alignment(Alignment::Center),
                    Line::from(vec![
                        Span::styled("Author: ", Style::default().fg(NORD9).add_modifier(Modifier::BOLD)),
                        Span::styled("iapizarro", Style::default().fg(NORD5)),
                        Span::styled("   •   ", Style::default().fg(NORD10)),
                        Span::styled("Design: ", Style::default().fg(NORD9).add_modifier(Modifier::BOLD)),
                        Span::styled("Antigravity", Style::default().fg(NORD5)),
                    ]).alignment(Alignment::Center),
                    Line::from(""),
                    Line::from(vec![
                        Span::styled("──────────── ", Style::default().fg(NORD10)),
                        Span::styled("Keyboard & Mouse Shortcuts", Style::default().fg(NORD7).add_modifier(Modifier::BOLD)),
                        Span::styled(" ────────────", Style::default().fg(NORD10)),
                    ]).alignment(Alignment::Center),
                    Line::from(""),
                ];

                for (keys, desc) in about_shortcuts {
                    about_text.push(Line::from(vec![
                        Span::styled("      ", Style::default()),
                        Span::styled(format!("{:<25}", keys), Style::default().fg(NORD10).add_modifier(Modifier::BOLD)),
                        Span::styled(format!("{:<24}", desc), Style::default().fg(NORD5)),
                    ]));
                }

                let about_popup = Paragraph::new(about_text)
                    .block(
                        Block::default()
                            .borders(Borders::ALL)
                            .border_style(Style::default().fg(NORD7).add_modifier(Modifier::BOLD))
                            .style(Style::default().bg(NORD0)),
                    );

                f.render_widget(about_popup, area);
            }
        })?;

        // Check for track completion and auto-advance
        player.tick();

        // Handle Events
        if event::poll(Duration::from_millis(100))? {
            match event::read()? {
                Event::Mouse(mouse_event) => {
                    if show_about_modal {
                        if mouse_event.kind == MouseEventKind::Down(MouseButton::Left) {
                            show_about_modal = false;
                        }
                    } else {
                        let mx = mouse_event.column;
                        let my = mouse_event.row;

                        match mouse_event.kind {
                            MouseEventKind::Down(MouseButton::Left) => {
                                // 1. Check if click is inside Artists Column inner content area
                                if last_artist_rect.width > 2
                                    && last_artist_rect.height > 2
                                    && mx > last_artist_rect.x
                                    && mx < last_artist_rect.x + last_artist_rect.width - 1
                                    && my > last_artist_rect.y
                                    && my < last_artist_rect.y + last_artist_rect.height - 1
                                {
                                    active_focus = ActiveFocus::ArtistColumn;
                                    let rel_y = (my - (last_artist_rect.y + 1)) as usize;
                                    let target_idx = artist_list_state.offset() + rel_y;
                                    if target_idx < total_artist_rows {
                                        if artist_list_state.selected() != Some(target_idx) {
                                            artist_list_state.select(Some(target_idx));
                                            album_list_state.select(Some(0));
                                        }
                                    }
                                }
                                // 2. Check if click is inside Albums & Tracks Column inner content area
                                else if last_album_rect.width > 2
                                    && last_album_rect.height > 2
                                    && mx > last_album_rect.x
                                    && mx < last_album_rect.x + last_album_rect.width - 1
                                    && my > last_album_rect.y
                                    && my < last_album_rect.y + last_album_rect.height - 1
                                {
                                    active_focus = ActiveFocus::AlbumColumn;
                                    let rel_y = (my - (last_album_rect.y + 1)) as usize;
                                    let target_idx = album_list_state.offset() + rel_y;
                                    if target_idx < total_album_rows {
                                        let now = std::time::Instant::now();
                                        let is_double_click = last_clicked_track_idx == Some(target_idx)
                                            && now.duration_since(last_click_time) < Duration::from_millis(500);
                                        let was_already_selected = album_list_state.selected() == Some(target_idx);

                                        last_click_time = now;
                                        last_clicked_track_idx = Some(target_idx);
                                        album_list_state.select(Some(target_idx));

                                        // Play track if clicking an already selected song or on double click
                                        if is_double_click || was_already_selected {
                                            if let Some(Some(track)) = row_to_track.get(target_idx) {
                                                let _ = player.play_track(track);
                                            }
                                        }
                                    }
                                }
                                // 3. Check if click is on Progress / Track Bar
                                else if my == last_progress_bar_y
                                    && last_progress_bar_width > 0
                                    && mx >= last_progress_bar_x
                                    && mx < last_progress_bar_x + last_progress_bar_width
                                {
                                    if let Some(track) = player.current_track() {
                                        let total_sec = track.duration.as_secs_f32();
                                        if total_sec > 0.0 {
                                            let rel_x = (mx - last_progress_bar_x) as f32;
                                            let ratio = (rel_x / (last_progress_bar_width as f32 - 1.0).max(1.0)).clamp(0.0, 1.0);
                                            let target_dur = Duration::from_secs_f32(ratio * total_sec);
                                            player.seek_to(target_dur);
                                        }
                                    }
                                }
                                // 4. Check if click is on Volume Slider
                                else if my == last_progress_bar_y
                                    && last_vol_bar_width > 0
                                    && mx >= last_vol_bar_x
                                    && mx <= last_vol_bar_x + last_vol_bar_width
                                {
                                    let rel_x = (mx - last_vol_bar_x) as f32;
                                    let vol_ratio = (rel_x / last_vol_bar_width as f32).clamp(0.0, 1.0);
                                    player.set_volume(vol_ratio);
                                }
                                // 5. Check if click is on Now Playing info bar
                                else if my == last_now_playing_y {
                                    player.toggle_pause();
                                }
                            }
                            MouseEventKind::Drag(MouseButton::Left) => {
                                // Allow scrubbing progress bar or dragging volume slider
                                if my == last_progress_bar_y {
                                    if last_progress_bar_width > 0
                                        && mx >= last_progress_bar_x
                                        && mx < last_progress_bar_x + last_progress_bar_width
                                    {
                                        if let Some(track) = player.current_track() {
                                            let total_sec = track.duration.as_secs_f32();
                                            if total_sec > 0.0 {
                                                let rel_x = (mx - last_progress_bar_x) as f32;
                                                let ratio = (rel_x / (last_progress_bar_width as f32 - 1.0).max(1.0)).clamp(0.0, 1.0);
                                                let target_dur = Duration::from_secs_f32(ratio * total_sec);
                                                player.seek_to(target_dur);
                                            }
                                        }
                                    } else if last_vol_bar_width > 0
                                        && mx >= last_vol_bar_x
                                        && mx <= last_vol_bar_x + last_vol_bar_width
                                    {
                                        let rel_x = (mx - last_vol_bar_x) as f32;
                                        let vol_ratio = (rel_x / last_vol_bar_width as f32).clamp(0.0, 1.0);
                                        player.set_volume(vol_ratio);
                                    }
                                }
                            }
                            MouseEventKind::ScrollDown => {
                                let over_artist = mx >= last_artist_rect.x
                                    && mx < last_artist_rect.x + last_artist_rect.width
                                    && my >= last_artist_rect.y
                                    && my < last_artist_rect.y + last_artist_rect.height;
                                let over_album = mx >= last_album_rect.x
                                    && mx < last_album_rect.x + last_album_rect.width
                                    && my >= last_album_rect.y
                                    && my < last_album_rect.y + last_album_rect.height;
                                let over_vol_or_prog = my == last_progress_bar_y || my == last_now_playing_y;

                                if over_vol_or_prog {
                                    player.volume_down();
                                } else if over_artist || (!over_album && active_focus == ActiveFocus::ArtistColumn) {
                                    active_focus = ActiveFocus::ArtistColumn;
                                    if total_artist_rows > 0 {
                                        let curr = artist_list_state.selected().unwrap_or(0);
                                        let next = (curr + 1).min(total_artist_rows - 1);
                                        if next != curr {
                                            artist_list_state.select(Some(next));
                                            album_list_state.select(Some(0));
                                        }
                                    }
                                } else {
                                    active_focus = ActiveFocus::AlbumColumn;
                                    if total_album_rows > 0 {
                                        let curr = album_list_state.selected().unwrap_or(0);
                                        let next = (curr + 1).min(total_album_rows - 1);
                                        album_list_state.select(Some(next));
                                    }
                                }
                            }
                            MouseEventKind::ScrollUp => {
                                let over_artist = mx >= last_artist_rect.x
                                    && mx < last_artist_rect.x + last_artist_rect.width
                                    && my >= last_artist_rect.y
                                    && my < last_artist_rect.y + last_artist_rect.height;
                                let over_album = mx >= last_album_rect.x
                                    && mx < last_album_rect.x + last_album_rect.width
                                    && my >= last_album_rect.y
                                    && my < last_album_rect.y + last_album_rect.height;
                                let over_vol_or_prog = my == last_progress_bar_y || my == last_now_playing_y;

                                if over_vol_or_prog {
                                    player.volume_up();
                                } else if over_artist || (!over_album && active_focus == ActiveFocus::ArtistColumn) {
                                    active_focus = ActiveFocus::ArtistColumn;
                                    if total_artist_rows > 0 {
                                        let curr = artist_list_state.selected().unwrap_or(0);
                                        let prev = curr.saturating_sub(1);
                                        if prev != curr {
                                            artist_list_state.select(Some(prev));
                                            album_list_state.select(Some(0));
                                        }
                                    }
                                } else {
                                    active_focus = ActiveFocus::AlbumColumn;
                                    if total_album_rows > 0 {
                                        let curr = album_list_state.selected().unwrap_or(0);
                                        let prev = curr.saturating_sub(1);
                                        album_list_state.select(Some(prev));
                                    }
                                }
                            }
                            _ => {}
                        }
                    }
                }
                Event::Key(key) => {
                    if key.kind == KeyEventKind::Press {
                        if show_about_modal {
                            match key.code {
                                KeyCode::Esc | KeyCode::Char('a') | KeyCode::Char('A') | KeyCode::Enter | KeyCode::Char('q') => {
                                    show_about_modal = false;
                                }
                                KeyCode::Media(media_key) => match media_key {
                                    MediaKeyCode::Play
                                    | MediaKeyCode::Pause
                                    | MediaKeyCode::PlayPause => {
                                        if player.current_track_index.is_some() {
                                            player.toggle_pause();
                                        } else if !player.flat_playlist.is_empty() {
                                            let _ = player.play_index(0);
                                        }
                                    }
                                    MediaKeyCode::TrackNext | MediaKeyCode::FastForward => {
                                        let _ = player.next();
                                    }
                                    MediaKeyCode::TrackPrevious | MediaKeyCode::Reverse => {
                                        let _ = player.previous();
                                    }
                                    MediaKeyCode::RaiseVolume => {
                                        player.volume_up();
                                    }
                                    MediaKeyCode::LowerVolume => {
                                        player.volume_down();
                                    }
                                    MediaKeyCode::Stop => {
                                        if !player.is_paused && player.current_track_index.is_some() {
                                            player.toggle_pause();
                                        }
                                    }
                                    _ => {}
                                },
                                _ => {}
                            }
                        } else {
                            match key.code {
                                KeyCode::Char('q') | KeyCode::Esc => running = false,
                                KeyCode::Char('a') | KeyCode::Char('A') => show_about_modal = true,
                                KeyCode::Char('+') | KeyCode::Char('=') => player.volume_up(),
                                KeyCode::Char('-') | KeyCode::Char('_') => player.volume_down(),
                                KeyCode::Tab | KeyCode::Right | KeyCode::Left | KeyCode::Char('h') | KeyCode::Char('l') => {

                                    active_focus = match active_focus {
                                        ActiveFocus::ArtistColumn => ActiveFocus::AlbumColumn,
                                        ActiveFocus::AlbumColumn => ActiveFocus::ArtistColumn,
                                    };
                                }
                                KeyCode::Char(' ') => player.toggle_pause(),
                                KeyCode::Char('n') => {
                                    let _ = player.next();
                                }
                                KeyCode::Char('p') => {
                                    let _ = player.previous();
                                }
                                KeyCode::Enter => {
                                    if active_focus == ActiveFocus::AlbumColumn {
                                        if let Some(selected_row) = album_list_state.selected() {
                                            if let Some(Some(track)) = row_to_track.get(selected_row) {
                                                let _ = player.play_track(track);
                                            }
                                        }
                                    }
                                }
                                KeyCode::Down | KeyCode::Char('j') => match active_focus {
                                    ActiveFocus::ArtistColumn => {
                                        if total_artist_rows > 0 {
                                            let next = match artist_list_state.selected() {
                                                Some(i) => (i + 1) % total_artist_rows,
                                                None => 0,
                                            };
                                            artist_list_state.select(Some(next));
                                            album_list_state.select(Some(0));
                                        }
                                    }
                                    ActiveFocus::AlbumColumn => {
                                        if total_album_rows > 0 {
                                            let next = match album_list_state.selected() {
                                                Some(i) => (i + 1) % total_album_rows,
                                                None => 0,
                                            };
                                            album_list_state.select(Some(next));
                                        }
                                    }
                                },
                                KeyCode::Up | KeyCode::Char('k') => match active_focus {
                                    ActiveFocus::ArtistColumn => {
                                        if total_artist_rows > 0 {
                                            let prev = match artist_list_state.selected() {
                                                Some(i) => if i == 0 { total_artist_rows - 1 } else { i - 1 },
                                                None => 0,
                                            };
                                            artist_list_state.select(Some(prev));
                                            album_list_state.select(Some(0));
                                        }
                                    }
                                    ActiveFocus::AlbumColumn => {
                                        if total_album_rows > 0 {
                                            let prev = match album_list_state.selected() {
                                                Some(i) => if i == 0 { total_album_rows - 1 } else { i - 1 },
                                                None => 0,
                                            };
                                            album_list_state.select(Some(prev));
                                        }
                                    }
                                },
                                _ => {}
                            }
                        }
                    }
                }
                _ => {}
            }
        }
    }

    // Terminal cleanup
    disable_raw_mode()?;
    execute!(terminal.backend_mut(), DisableMouseCapture, LeaveAlternateScreen)?;
    terminal.show_cursor()?;

    Ok(())
}
