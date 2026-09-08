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
use lofty::file::TaggedFileExt;
use ratatui::{
    backend::CrosstermBackend,
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, Clear, List, ListItem, ListState, Paragraph},
    Terminal,
};
use souvlaki::{MediaControlEvent, MediaControls, MediaMetadata, MediaPlayback, MediaPosition, PlatformConfig, SeekDirection};

use theme::*;
use player::Player;

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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MetaField {
    Title,
    Artist,
    Album,
    Genre,
    Year,
    TrackNumber,
}

impl MetaField {
    const ALL: [MetaField; 6] = [
        MetaField::Title,
        MetaField::Artist,
        MetaField::Album,
        MetaField::Genre,
        MetaField::Year,
        MetaField::TrackNumber,
    ];
}

#[derive(Debug, Clone)]
struct MetadataEditState {
    track_path: PathBuf,
    field_idx: usize,
    is_editing: bool,
    cursor_pos: usize,
    title: String,
    artist: String,
    album: String,
    genre: String,
    year: String,
    track_number: String,
}

impl MetadataEditState {
    fn current_field_str(&self) -> &str {
        match MetaField::ALL[self.field_idx] {
            MetaField::Title => &self.title,
            MetaField::Artist => &self.artist,
            MetaField::Album => &self.album,
            MetaField::Genre => &self.genre,
            MetaField::Year => &self.year,
            MetaField::TrackNumber => &self.track_number,
        }
    }

    fn current_field_str_mut(&mut self) -> &mut String {
        match MetaField::ALL[self.field_idx] {
            MetaField::Title => &mut self.title,
            MetaField::Artist => &mut self.artist,
            MetaField::Album => &mut self.album,
            MetaField::Genre => &mut self.genre,
            MetaField::Year => &mut self.year,
            MetaField::TrackNumber => &mut self.track_number,
        }
    }
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

struct TerminalCleanup;

impl Drop for TerminalCleanup {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
        let _ = execute!(std::io::stdout(), DisableMouseCapture, LeaveAlternateScreen, crossterm::cursor::Show);
    }
}

fn install_panic_hook() {
    let original_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |panic_info| {
        let _ = std::panic::catch_unwind(|| {
            let _ = disable_raw_mode();
            let _ = execute!(std::io::stdout(), DisableMouseCapture, LeaveAlternateScreen, crossterm::cursor::Show);
        });
        original_hook(panic_info);
    }));
}

/// Redirect stderr to /dev/null to prevent ALSA (and other C-level libraries)
/// from printing underrun, buffer, or driver diagnostic messages directly to the
/// terminal, which corrupts ratatui's raw-mode TUI screen.
fn suppress_alsa_and_stderr_noise() {
    #[cfg(unix)]
    unsafe {
        use std::fs::OpenOptions;
        use std::os::unix::io::AsRawFd;

        if let Ok(dev_null) = OpenOptions::new().write(true).open("/dev/null") {
            let _ = libc::dup2(dev_null.as_raw_fd(), libc::STDERR_FILENO);
        }
    }
}

fn main() -> color_eyre::Result<()> {
    install_panic_hook();
    color_eyre::install()?;
    suppress_alsa_and_stderr_noise();
    let args = Args::parse();

    // Terminal setup with Mouse Capture enabled
    enable_raw_mode()?;
    let mut stdout = std::io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let _cleanup = TerminalCleanup;
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
    let mut metadata_edit_modal: Option<MetadataEditState> = None;

    if !player.artists.is_empty() {
        artist_list_state.select(Some(0)); // Select "All Artists"
    }
    if !player.artists.is_empty() {
        album_list_state.select(Some(1));
    }

    let mut running = true;
    let app_start_time = std::time::Instant::now();
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

    let mut status_message: Option<(String, std::time::Instant)> = None;
    let mut artist_labels: Vec<String> = player.artists.iter().map(|artist| format!(" {}", artist.name)).collect();

    let mut last_rendered_artist_idx: Option<usize> = None;
    let mut last_rendered_track_idx: Option<usize> = None;
    let mut cached_album_items: Vec<ListItem> = Vec::new();
    let mut cached_row_to_track: Vec<Option<usize>> = Vec::new();

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
                    player.stop();
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

        // Selected Artist Filter
        let selected_artist_idx = artist_list_state.selected().unwrap_or(0);

        // 2. Build Right Column (Albums & Tracks for selected artist) only when selection/track changes
        if last_rendered_artist_idx != Some(selected_artist_idx) || last_rendered_track_idx != current_idx {
            last_rendered_artist_idx = Some(selected_artist_idx);
            last_rendered_track_idx = current_idx;
            cached_album_items.clear();
            cached_row_to_track.clear();

            if player.artists.is_empty() {
                cached_album_items.push(ListItem::new("  No music files found in music directory.").style(Style::default().fg(NORD3).add_modifier(Modifier::ITALIC)));
                cached_row_to_track.push(None);
            } else {
                let album_iter: Box<dyn Iterator<Item = &player::AlbumGroup>> = if selected_artist_idx == 0 {
                    Box::new(player.artists.iter().flat_map(|a| &a.albums))
                } else if let Some(artist) = player.artists.get(selected_artist_idx.saturating_sub(1)) {
                    Box::new(artist.albums.iter())
                } else {
                    Box::new(std::iter::empty())
                };

                for album in album_iter {
                    // Album Header row
                    cached_album_items.push(ListItem::new(Line::from(vec![
                        Span::styled(" ", Style::default()),
                        Span::styled(album.name.clone(), Style::default().fg(NORD8).add_modifier(Modifier::BOLD)),
                        Span::styled(format!(" • {}", album.artist), Style::default().fg(NORD4)),
                    ])).style(Style::default().bg(NORD1)));
                    cached_row_to_track.push(None);

                    // Album Tracks
                    for track in &album.tracks {
                        let flat_idx = track.flat_index;
                        let is_current = current_idx == Some(flat_idx);

                        let prefix = if is_current { "  ▶ " } else { "    " };
                        let track_style = if is_current {
                            Style::default().fg(NORD6).add_modifier(Modifier::BOLD)
                        } else {
                            Style::default().fg(NORD4)
                        };

                        cached_album_items.push(ListItem::new(Line::from(vec![
                            Span::styled(prefix, Style::default().fg(NORD6)),
                            Span::styled(track.title.clone(), track_style),
                        ])));
                        cached_row_to_track.push(Some(flat_idx));
                    }
                }
            }
        }

        let total_artist_rows = artist_labels.len() + 1;
        let total_album_rows = cached_album_items.len();
        if total_album_rows > 0 {
            if let Some(selected) = album_list_state.selected() {
                if selected >= total_album_rows {
                    album_list_state.select(Some(total_album_rows - 1));
                }
            } else {
                album_list_state.select(Some(0));
            }
        } else {
            album_list_state.select(None);
        }
        let row_to_track = &cached_row_to_track;

        terminal.draw(|f| {
            let chunks = Layout::default()
                .direction(Direction::Vertical)
                .constraints([
                    Constraint::Length(3),  // Header / Status bar
                    Constraint::Min(6),     // 2-Column Split View (Artists left, Albums right)
                    Constraint::Length(4),  // Now Playing & Playback Controls Box
                ])
                .split(f.area());

            let header_line = if let Some((ref msg, time)) = status_message {
                if time.elapsed() < Duration::from_secs(3) {
                    Line::from(vec![
                        Span::styled("MUSIC-RUST  ", Style::default().fg(NORD8).add_modifier(Modifier::BOLD)),
                        Span::styled(format!("•  {}", msg), Style::default().fg(NORD14).add_modifier(Modifier::BOLD)),
                    ])
                } else {
                    Line::from(vec![
                        Span::styled("MUSIC-RUST", Style::default().fg(NORD8).add_modifier(Modifier::BOLD)),
                    ])
                }
            } else {
                Line::from(vec![
                    Span::styled("MUSIC-RUST", Style::default().fg(NORD8).add_modifier(Modifier::BOLD)),
                ])
            };

            let header_p = Paragraph::new(header_line)
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
            let artist_items_iter = std::iter::once(
                ListItem::new(" All Artists").style(Style::default().fg(NORD8).add_modifier(Modifier::BOLD)),
            )
            .chain(artist_labels.iter().map(|name| {
                ListItem::new(name.as_str()).style(Style::default().fg(NORD4))
            }));
            let artist_widget = List::new(artist_items_iter)
                .block(Block::default().borders(Borders::ALL).border_style(Style::default().fg(artist_border_color)).title(" Artists "))
                .highlight_style(Style::default().bg(NORD2).fg(NORD6).add_modifier(Modifier::BOLD));
            f.render_stateful_widget(artist_widget, main_columns[0], &mut artist_list_state);

            // Right Column: Albums & Tracks
            let album_border_color = if active_focus == ActiveFocus::AlbumColumn { NORD8 } else { NORD3 };
            let album_widget = List::new(cached_album_items.iter().cloned())
                .block(Block::default().borders(Borders::ALL).border_style(Style::default().fg(album_border_color)).title(" Albums & Tracks "))
                .highlight_style(Style::default().bg(NORD2).fg(NORD6).add_modifier(Modifier::BOLD));
            f.render_stateful_widget(album_widget, main_columns[1], &mut album_list_state);

            // 3. Now Playing & Playback Controls Box
            let (status_icon, status_color) = if player.is_paused {
                ("❚❚ ", NORD6)
            } else if player.current_track_index.is_some() {
                ("▶ ", NORD6)
            } else {
                ("■ ", NORD3)
            };

            let now_playing_text = if let Some(track) = player.current_track() {
                vec![
                    Span::styled(status_icon, Style::default().fg(status_color)),
                    Span::styled(&track.title, Style::default().fg(NORD6).add_modifier(Modifier::BOLD)),
                    Span::styled(" • ", Style::default().fg(NORD3)),
                    Span::styled(&track.artist, Style::default().fg(NORD8)),
                    Span::styled(" • ", Style::default().fg(NORD3)),
                    Span::styled(&track.album, Style::default().fg(NORD7)),
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
            let remaining_dur = total_dur.saturating_sub(elapsed);

            let elapsed_str = player::format_duration(elapsed);
            let total_str = player::format_duration(total_dur);
            let remaining_str = format!("(-{})", player::format_duration(remaining_dur));

            let total_sec = total_dur.as_secs();
            let elapsed_sec = elapsed.as_secs();

            // Volume Indicator Bar (Vol: 100% [━━━━━━━━━━])
            const VOL_FILLED: [&str; 11] = ["", "━", "━━", "━━━", "━━━━", "━━━━━", "━━━━━━", "━━━━━━━", "━━━━━━━━", "━━━━━━━━━", "━━━━━━━━━━"];
            const VOL_EMPTY: [&str; 11] = ["──────────", "─────────", "────────", "───────", "──────", "─────", "────", "───", "──", "─", ""];

            let vol_pct = (player.volume * 100.0).round() as u32;
            let vol_blocks = ((player.volume * 10.0).round() as usize).clamp(0, 10);
            let vol_filled = VOL_FILLED[vol_blocks];
            let vol_empty = VOL_EMPTY[vol_blocks];

            let vol_prefix = if player.volume == 0.0 {
                "Vol: MUTE [".to_string()
            } else {
                format!("Vol: {:3}% [", vol_pct)
            };
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
            let mut bar_chars = Vec::with_capacity(progress_width);
            for i in 0..progress_width {
                if i == thumb_pos && player.current_track_index.is_some() {
                    bar_chars.push(Span::styled("█", Style::default().fg(NORD6)));
                } else if i < thumb_pos {
                    bar_chars.push(Span::styled("━", Style::default().fg(NORD8)));
                } else {
                    bar_chars.push(Span::styled("─", Style::default().fg(NORD2)));
                }
            }

            let mut progress_spans = vec![
                Span::styled(format!(" {} ", elapsed_str), Style::default().fg(NORD4).add_modifier(Modifier::BOLD)),
            ];
            progress_spans.extend(bar_chars);
            progress_spans.push(Span::styled(format!(" {} ", total_str), Style::default().fg(NORD4)));
            progress_spans.push(Span::styled(format!(" {} ", remaining_str), Style::default().fg(NORD3)));
            progress_spans.push(Span::styled(format!("  {}", vol_prefix), Style::default().fg(NORD4)));
            progress_spans.push(Span::styled(vol_filled, Style::default().fg(NORD8)));
            progress_spans.push(Span::styled(vol_empty, Style::default().fg(NORD2)));
            progress_spans.push(Span::styled(vol_suffix, Style::default().fg(NORD4)));

            let is_playing = player.current_track_index.is_some() && !player.is_paused;
            let (playback_border_color, playback_border_type) = if is_playing {
                let elapsed_secs = app_start_time.elapsed().as_secs_f32();
                (theme::get_breathing_playback_color(elapsed_secs), BorderType::Thick)
            } else if player.is_paused && player.current_track_index.is_some() {
                (NORD9, BorderType::Plain)
            } else {
                (NORD3, BorderType::Plain)
            };

            let playback_paragraph = Paragraph::new(vec![
                Line::from(now_playing_text).alignment(Alignment::Center),
                Line::from(progress_spans),
            ])
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_type(playback_border_type)
                    .border_style(Style::default().fg(playback_border_color))
                    .title(" Now Playing "),
            );
            f.render_widget(playback_paragraph, chunks[2]);

            // 4. Render Modal About & Shortcuts Overlay if Active
            if show_about_modal {
                let area = centered_rect_fixed(68, 19, f.area());
                f.render_widget(Clear, area);

                let about_shortcuts = [
                    ("↑ / ↓", "Move Selection"),
                    ("Tab / Esc", "Switch Column Focus"),
                    ("Space", "Play / Pause"),
                    ("Enter", "Play Track / Artist"),
                    ("n / p", "Next / Previous Track"),
                    ("[ / ]", "Seek -5s / +5s"),
                    ("+ / -", "Volume Up / Down"),
                    ("m", "Mute / Unmute"),
                    ("s / r / F5", "Scan Music Folder"),
                    ("x", "Stop Playback"),
                    ("e", "Edit Track Metadata"),
                    ("q", "Quit Application"),
                ];

                let mut about_text = vec![
                    Line::from(vec![
                        Span::styled(format!("MUSIC-RUST v{}", env!("CARGO_PKG_VERSION")), Style::default().fg(NORD10).add_modifier(Modifier::BOLD)),
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
                        Span::styled("Keyboard Shortcuts", Style::default().fg(NORD7).add_modifier(Modifier::BOLD)),
                        Span::styled(" ────────────", Style::default().fg(NORD10)),
                    ]).alignment(Alignment::Center),
                    Line::from(""),
                ];

                for (keys, desc) in about_shortcuts {
                    about_text.push(Line::from(vec![
                        Span::styled("      ", Style::default()),
                        Span::styled(format!("{:<20}", keys), Style::default().fg(NORD10).add_modifier(Modifier::BOLD)),
                        Span::styled(format!("{:<26}", desc), Style::default().fg(NORD5)),
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

            // 5. Render Metadata Edit Modal if Active
            if let Some(ref edit_state) = metadata_edit_modal {
                let area = centered_rect_fixed(84, 16, f.area());
                f.render_widget(Clear, area);

                let block = Block::default()
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(META_ROSE_PINK))
                    .style(Style::default().bg(NORD0));
                f.render_widget(block, area);

                let modal_chunks = Layout::default()
                    .direction(Direction::Vertical)
                    .constraints([
                        Constraint::Length(3), // Header
                        Constraint::Length(8), // 6 Metadata Fields
                        Constraint::Length(2), // Help / Instructions footer
                    ])
                    .margin(1)
                    .split(area);

                // Header
                let file_name = edit_state
                    .track_path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("Track");

                let max_name_len = 38;
                let display_file_name = if file_name.chars().count() > max_name_len {
                    let mut s: String = file_name.chars().take(max_name_len - 3).collect();
                    s.push_str("...");
                    s
                } else {
                    file_name.to_string()
                };

                let header_lines = vec![
                    Line::from(vec![
                        Span::styled("EDIT METADATA", Style::default().fg(META_LIGHT_LILAC).add_modifier(Modifier::BOLD)),
                    ]).alignment(Alignment::Center),
                    Line::from(vec![
                        Span::styled("File: ", Style::default().fg(META_ROSE_PINK).add_modifier(Modifier::BOLD)),
                        Span::styled(display_file_name, Style::default().fg(META_SNOW_MID)),
                        Span::styled("   •   ", Style::default().fg(META_DEEP_MAUVE)),
                        Span::styled("App: ", Style::default().fg(META_ROSE_PINK).add_modifier(Modifier::BOLD)),
                        Span::styled(format!("music-rust v{}", env!("CARGO_PKG_VERSION")), Style::default().fg(META_SNOW_MID)),
                    ]).alignment(Alignment::Center),
                    Line::from(vec![
                        Span::styled("──────────── ", Style::default().fg(META_DEEP_MAUVE)),
                        Span::styled("Audio Tags", Style::default().fg(META_LIGHT_LILAC).add_modifier(Modifier::BOLD)),
                        Span::styled(" ────────────", Style::default().fg(META_DEEP_MAUVE)),
                    ]).alignment(Alignment::Center),
                ];
                let header_p = Paragraph::new(header_lines).style(Style::default().bg(NORD0));
                f.render_widget(header_p, modal_chunks[0]);

                // Fields List
                let mut field_lines = Vec::new();
                for (idx, field) in MetaField::ALL.iter().enumerate() {
                    let is_selected = edit_state.field_idx == idx;
                    let num_str = format!("{}. ", idx + 1);
                    let row_bg = if is_selected { META_BG_ACTIVE } else { NORD0 };

                    let label_style = if is_selected {
                        Style::default().bg(row_bg).fg(META_SNOW_BRIGHT).add_modifier(Modifier::BOLD)
                    } else {
                        Style::default().bg(row_bg).fg(META_SNOW_MID)
                    };

                    let val_style = if is_selected {
                        Style::default().bg(row_bg).fg(META_SNOW_BRIGHT).add_modifier(Modifier::BOLD)
                    } else {
                        Style::default().bg(row_bg).fg(META_SNOW_MAIN)
                    };

                    let cursor_str = if is_selected { "  > " } else { "    " };
                    let cursor_style = if is_selected {
                        Style::default().bg(row_bg).fg(META_LIGHT_LILAC).add_modifier(Modifier::BOLD)
                    } else {
                        Style::default().bg(row_bg).fg(NORD3)
                    };

                    let marker_style = Style::default().bg(row_bg).fg(META_ROSE_PINK).add_modifier(Modifier::BOLD);
                    let num_style = Style::default().bg(row_bg).fg(if is_selected { META_ROSE_PINK } else { META_MUTED_PURPLE });

                    let (label, val_str) = match field {
                        MetaField::Title => ("Track Title:    ", edit_state.title.as_str()),
                        MetaField::Artist => ("Track Artist:   ", edit_state.artist.as_str()),
                        MetaField::Album => ("Album Name:     ", edit_state.album.as_str()),
                        MetaField::Genre => ("Genre:          ", edit_state.genre.as_str()),
                        MetaField::Year => ("Release Year:   ", edit_state.year.as_str()),
                        MetaField::TrackNumber => ("Track Number:   ", edit_state.track_number.as_str()),
                    };

                    let max_val_len = 54;
                    let display_val = if is_selected && edit_state.is_editing {
                        let chars: Vec<char> = val_str.chars().collect();
                        let pos = edit_state.cursor_pos.min(chars.len());
                        let mut with_cursor = String::new();
                        with_cursor.extend(chars[..pos].iter());
                        with_cursor.push('_');
                        with_cursor.extend(chars[pos..].iter());

                        if with_cursor.chars().count() > max_val_len {
                            let s: String = with_cursor.chars().skip(with_cursor.chars().count() - max_val_len).collect();
                            s
                        } else {
                            with_cursor
                        }
                    } else if val_str.is_empty() {
                        "---".to_string()
                    } else if val_str.chars().count() > max_val_len {
                        let mut s: String = val_str.chars().take(max_val_len - 3).collect();
                        s.push_str("...");
                        s
                    } else {
                        val_str.to_string()
                    };

                    field_lines.push(Line::from(vec![
                        Span::styled(cursor_str, cursor_style),
                        Span::styled("§ ", marker_style),
                        Span::styled(num_str, num_style),
                        Span::styled(label, label_style),
                        Span::styled(display_val, val_style),
                    ]));
                }

                let fields_p = Paragraph::new(field_lines).style(Style::default().bg(NORD0));
                f.render_widget(fields_p, modal_chunks[1]);

                // Footer instructions
                let footer_line = if edit_state.is_editing {
                    Line::from(vec![
                        Span::styled("← / →: ", Style::default().fg(META_ROSE_PINK).add_modifier(Modifier::BOLD)),
                        Span::styled("Move cursor", Style::default().fg(META_SNOW_MID)),
                        Span::styled("  •  ", Style::default().fg(META_DEEP_MAUVE)),
                        Span::styled("Enter: ", Style::default().fg(META_ROSE_PINK).add_modifier(Modifier::BOLD)),
                        Span::styled("Done field", Style::default().fg(META_SNOW_MID)),
                        Span::styled("  •  ", Style::default().fg(META_DEEP_MAUVE)),
                        Span::styled("Esc: ", Style::default().fg(META_ROSE_PINK).add_modifier(Modifier::BOLD)),
                        Span::styled("Save & Close", Style::default().fg(META_LIGHT_LILAC).add_modifier(Modifier::BOLD)),
                    ]).alignment(Alignment::Center)
                } else {
                    Line::from(vec![
                        Span::styled("↑ / ↓: ", Style::default().fg(META_ROSE_PINK).add_modifier(Modifier::BOLD)),
                        Span::styled("Navigate", Style::default().fg(META_SNOW_MID)),
                        Span::styled("  •  ", Style::default().fg(META_DEEP_MAUVE)),
                        Span::styled("Enter: ", Style::default().fg(META_ROSE_PINK).add_modifier(Modifier::BOLD)),
                        Span::styled("Edit field", Style::default().fg(META_SNOW_MID)),
                        Span::styled("  •  ", Style::default().fg(META_DEEP_MAUVE)),
                        Span::styled("Ctrl+S / Esc: ", Style::default().fg(META_ROSE_PINK).add_modifier(Modifier::BOLD)),
                        Span::styled("Auto-Save & Close", Style::default().fg(META_LIGHT_LILAC).add_modifier(Modifier::BOLD)),
                    ]).alignment(Alignment::Center)
                };
                let footer_p = Paragraph::new(vec![Line::from(""), footer_line]).style(Style::default().bg(NORD0));
                f.render_widget(footer_p, modal_chunks[2]);
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
                    } else if metadata_edit_modal.is_some() {
                        // In edit modal, keep mouse clicks from messing with background selection
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
                                            album_list_state.select(Some(1));
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
                                            if let Some(Some(track_idx)) = row_to_track.get(target_idx) {
                                                let _ = player.play_index(*track_idx);
                                            } else if is_double_click {
                                                if let Some(Some(track_idx)) = row_to_track.get(target_idx + 1) {
                                                    let _ = player.play_index(*track_idx);
                                                    album_list_state.select(Some(target_idx + 1));
                                                }
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
                                            if let Some(controls) = media_controls.as_mut() {
                                                update_mpris_state(controls, &player);
                                            }
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
                                    if player.current_track_index.is_some() {
                                        player.toggle_pause();
                                    } else if !player.flat_playlist.is_empty() {
                                        let _ = player.play_index(0);
                                    }
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
                                                if let Some(controls) = media_controls.as_mut() {
                                                    update_mpris_state(controls, &player);
                                                }
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
                                            album_list_state.select(Some(1));
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
                                            album_list_state.select(Some(1));
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
                                KeyCode::Esc | KeyCode::Char('a') | KeyCode::Char('A') | KeyCode::Char('?') | KeyCode::Enter | KeyCode::Char('q') | KeyCode::Char(' ') => {
                                    show_about_modal = false;
                                }
                                KeyCode::Char('c') if key.modifiers.contains(crossterm::event::KeyModifiers::CONTROL) => {
                                    running = false;
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
                                        player.stop();
                                    }
                                    _ => {}
                                },
                                _ => {}
                            }
                        } else if let Some(ref mut edit_state) = metadata_edit_modal {
                            let mut should_save_and_close = false;
                            let mut should_discard_and_close = false;

                            if edit_state.is_editing {
                                match key.code {
                                    KeyCode::Enter => {
                                        edit_state.is_editing = false;
                                    }
                                    KeyCode::Esc => {
                                        edit_state.is_editing = false;
                                        should_save_and_close = true;
                                    }
                                    KeyCode::Left => {
                                        if key.modifiers.contains(crossterm::event::KeyModifiers::CONTROL) {
                                            // Jump word left
                                            let field_str = edit_state.current_field_str();
                                            let chars: Vec<char> = field_str.chars().collect();
                                            let mut pos = edit_state.cursor_pos.min(chars.len());
                                            while pos > 0 && chars[pos - 1].is_whitespace() {
                                                pos -= 1;
                                            }
                                            while pos > 0 && !chars[pos - 1].is_whitespace() {
                                                pos -= 1;
                                            }
                                            edit_state.cursor_pos = pos;
                                        } else {
                                            edit_state.cursor_pos = edit_state.cursor_pos.saturating_sub(1);
                                        }
                                    }
                                    KeyCode::Right => {
                                        let field_str = edit_state.current_field_str();
                                        let total_chars = field_str.chars().count();
                                        if key.modifiers.contains(crossterm::event::KeyModifiers::CONTROL) {
                                            // Jump word right
                                            let chars: Vec<char> = field_str.chars().collect();
                                            let mut pos = edit_state.cursor_pos.min(chars.len());
                                            while pos < total_chars && !chars[pos].is_whitespace() {
                                                pos += 1;
                                            }
                                            while pos < total_chars && chars[pos].is_whitespace() {
                                                pos += 1;
                                            }
                                            edit_state.cursor_pos = pos;
                                        } else if edit_state.cursor_pos < total_chars {
                                            edit_state.cursor_pos += 1;
                                        }
                                    }
                                    KeyCode::Home => {
                                        edit_state.cursor_pos = 0;
                                    }
                                    KeyCode::End => {
                                        let field_str = edit_state.current_field_str();
                                        edit_state.cursor_pos = field_str.chars().count();
                                    }
                                    KeyCode::Backspace => {
                                        let pos = edit_state.cursor_pos;
                                        if pos > 0 {
                                            let field_str = edit_state.current_field_str_mut();
                                            let mut chars: Vec<char> = field_str.chars().collect();
                                            if pos <= chars.len() {
                                                chars.remove(pos - 1);
                                                *field_str = chars.into_iter().collect();
                                                edit_state.cursor_pos = pos - 1;
                                            }
                                        }
                                    }
                                    KeyCode::Delete => {
                                        let pos = edit_state.cursor_pos;
                                        let field_str = edit_state.current_field_str_mut();
                                        let mut chars: Vec<char> = field_str.chars().collect();
                                        if pos < chars.len() {
                                            chars.remove(pos);
                                            *field_str = chars.into_iter().collect();
                                        }
                                    }
                                    KeyCode::Char(c) => {
                                        let is_numeric = matches!(
                                            MetaField::ALL[edit_state.field_idx],
                                            MetaField::Year | MetaField::TrackNumber
                                        );
                                        if !is_numeric || c.is_ascii_digit() {
                                            let pos = edit_state.cursor_pos;
                                            let field_str = edit_state.current_field_str_mut();
                                            let mut chars: Vec<char> = field_str.chars().collect();
                                            let insert_idx = pos.min(chars.len());
                                            chars.insert(insert_idx, c);
                                            *field_str = chars.into_iter().collect();
                                            edit_state.cursor_pos = insert_idx + 1;
                                        }
                                    }
                                    _ => {}
                                }
                            } else {
                                match key.code {
                                    KeyCode::Esc => {
                                        should_save_and_close = true;
                                    }
                                    KeyCode::Char('q') => {
                                        should_discard_and_close = true;
                                    }
                                    KeyCode::Up | KeyCode::Char('k') => {
                                        if edit_state.field_idx == 0 {
                                            edit_state.field_idx = MetaField::ALL.len() - 1;
                                        } else {
                                            edit_state.field_idx -= 1;
                                        }
                                    }
                                    KeyCode::Down | KeyCode::Char('j') => {
                                        edit_state.field_idx = (edit_state.field_idx + 1) % MetaField::ALL.len();
                                    }
                                    KeyCode::Enter => {
                                        edit_state.is_editing = true;
                                        edit_state.cursor_pos = edit_state.current_field_str().chars().count();
                                    }
                                    KeyCode::Char('s') if key.modifiers.contains(crossterm::event::KeyModifiers::CONTROL) => {
                                        should_save_and_close = true;
                                    }
                                    _ => {}
                                }
                            }

                            if should_save_and_close {
                                let path = edit_state.track_path.clone();
                                let title = edit_state.title.clone();
                                let artist = edit_state.artist.clone();
                                let album = edit_state.album.clone();
                                let genre = edit_state.genre.clone();
                                let year = edit_state.year.trim().parse::<u32>().ok();
                                let track_num = edit_state.track_number.trim().parse::<u32>().ok();

                                match player.update_track_metadata(
                                    &path,
                                    &title,
                                    &artist,
                                    &album,
                                    &genre,
                                    year,
                                    track_num,
                                ) {
                                    Ok(_) => {
                                        let prev_artist_idx = artist_list_state.selected().unwrap_or(0);
                                        let prev_artist_name = if prev_artist_idx == 0 {
                                            None
                                        } else {
                                            player.artists.get(prev_artist_idx.saturating_sub(1)).map(|a| a.name.clone())
                                        };

                                        let _ = player.rescan_directory(&args.music_dir);
                                        artist_labels = player.artists.iter().map(|artist| format!(" {}", artist.name)).collect();

                                        if let Some(ref name) = prev_artist_name {
                                            let new_idx = player.artists.iter().position(|a| &a.name == name).map(|i| i + 1).unwrap_or(0);
                                            artist_list_state.select(Some(new_idx));
                                        } else if !player.artists.is_empty() {
                                            artist_list_state.select(Some(0));
                                        } else {
                                            artist_list_state.select(None);
                                        }

                                        last_rendered_artist_idx = None;
                                        last_rendered_track_idx = None;

                                        status_message = Some((
                                            format!("Saved metadata for {}", title),
                                            std::time::Instant::now(),
                                        ));
                                        metadata_edit_modal = None;
                                    }
                                    Err(err) => {
                                        status_message = Some((
                                            format!("Error saving tags: {}", err),
                                            std::time::Instant::now(),
                                        ));
                                    }
                                }
                            } else if should_discard_and_close {
                                metadata_edit_modal = None;
                            }
                        } else {
                            match key.code {
                                KeyCode::Char('q') => running = false,
                                KeyCode::Char('c') if key.modifiers.contains(crossterm::event::KeyModifiers::CONTROL) => running = false,
                                KeyCode::Esc => {
                                    active_focus = match active_focus {
                                        ActiveFocus::ArtistColumn => ActiveFocus::AlbumColumn,
                                        ActiveFocus::AlbumColumn => ActiveFocus::ArtistColumn,
                                    };
                                }
                                KeyCode::Char('a') | KeyCode::Char('A') | KeyCode::Char('?') => show_about_modal = true,
                                KeyCode::Char('+') | KeyCode::Char('=') => player.volume_up(),
                                KeyCode::Char('-') | KeyCode::Char('_') => player.volume_down(),
                                KeyCode::Char('m') | KeyCode::Char('M') => player.toggle_mute(),
                                KeyCode::Char('x') | KeyCode::Char('X') => player.stop(),
                                KeyCode::Char('s') | KeyCode::Char('S') | KeyCode::Char('r') | KeyCode::Char('R') | KeyCode::F(5) => {
                                    let prev_artist_idx = artist_list_state.selected().unwrap_or(0);
                                    let prev_artist_name = if prev_artist_idx == 0 {
                                        None
                                    } else {
                                        player.artists.get(prev_artist_idx.saturating_sub(1)).map(|a| a.name.clone())
                                    };

                                    let _ = player.rescan_directory(&args.music_dir);
                                    artist_labels = player.artists.iter().map(|artist| format!(" {}", artist.name)).collect();

                                    if let Some(ref name) = prev_artist_name {
                                        let new_idx = player.artists.iter().position(|a| &a.name == name).map(|i| i + 1).unwrap_or(0);
                                        artist_list_state.select(Some(new_idx));
                                    } else if !player.artists.is_empty() {
                                        artist_list_state.select(Some(0));
                                    } else {
                                        artist_list_state.select(None);
                                    }

                                    last_rendered_artist_idx = None;
                                    last_rendered_track_idx = None;

                                    status_message = Some((
                                        format!("Scanned music folder ({} tracks)", player.flat_playlist.len()),
                                        std::time::Instant::now(),
                                    ));
                                }
                                KeyCode::Char('0') => player.restart_track(),
                                KeyCode::Char('e') | KeyCode::Char('E') => {
                                    // Open metadata editor for selected track
                                    let target_track_idx = match active_focus {
                                        ActiveFocus::AlbumColumn => {
                                            album_list_state.selected().and_then(|row| {
                                                row_to_track.get(row).copied().flatten().or_else(|| {
                                                    // If selecting an album header, select the first track under that album
                                                    row_to_track.get(row + 1).copied().flatten()
                                                })
                                            })
                                        }
                                        ActiveFocus::ArtistColumn => {
                                            // Fallback to currently playing, or first track of the current view
                                            player.current_track_index.or_else(|| {
                                                row_to_track.iter().flatten().copied().next()
                                            })
                                        }
                                    }.or(player.current_track_index);

                                    if let Some(track_idx) = target_track_idx {
                                        if let Some(track) = player.flat_playlist.get(track_idx) {
                                            let mut genre = String::new();
                                            let mut year_str = String::new();
                                            if let Ok(tagged) = lofty::probe::Probe::open(&track.path).and_then(|p| p.read()) {
                                                if let Some(tag) = tagged.primary_tag().or_else(|| tagged.first_tag()) {
                                                    use lofty::tag::Accessor;
                                                    if let Some(g) = tag.genre() {
                                                        genre = g.trim().to_string();
                                                    }
                                                    if let Some(y) = tag.year() {
                                                        year_str = y.to_string();
                                                    }
                                                }
                                            }

                                            metadata_edit_modal = Some(MetadataEditState {
                                                track_path: track.path.clone(),
                                                field_idx: 0,
                                                is_editing: false,
                                                cursor_pos: 0,
                                                title: track.title.clone(),
                                                artist: track.artist.clone(),
                                                album: track.album.clone(),
                                                genre,
                                                year: year_str,
                                                track_number: track.track_number.map(|n| n.to_string()).unwrap_or_default(),
                                            });
                                        }
                                    }
                                }
                                KeyCode::Tab => {
                                    active_focus = match active_focus {
                                        ActiveFocus::ArtistColumn => ActiveFocus::AlbumColumn,
                                        ActiveFocus::AlbumColumn => ActiveFocus::ArtistColumn,
                                    };
                                }
                                KeyCode::Left | KeyCode::Char('h') => {
                                    active_focus = ActiveFocus::ArtistColumn;
                                }
                                KeyCode::Right | KeyCode::Char('l') => {
                                    active_focus = ActiveFocus::AlbumColumn;
                                }
                                KeyCode::Char(' ') => {
                                    if player.current_track_index.is_some() {
                                        player.toggle_pause();
                                    } else if !player.flat_playlist.is_empty() {
                                        let _ = player.play_index(0);
                                    }
                                }
                                KeyCode::Char('[') | KeyCode::Char(',') => {
                                    let cur = player.current_elapsed_duration();
                                    player.seek_to(cur.saturating_sub(Duration::from_secs(5)));
                                    if let Some(controls) = media_controls.as_mut() {
                                        update_mpris_state(controls, &player);
                                    }
                                }
                                KeyCode::Char(']') | KeyCode::Char('.') => {
                                    let cur = player.current_elapsed_duration();
                                    player.seek_to(cur + Duration::from_secs(5));
                                    if let Some(controls) = media_controls.as_mut() {
                                        update_mpris_state(controls, &player);
                                    }
                                }
                                KeyCode::Char('{') | KeyCode::Char('<') => {
                                    let cur = player.current_elapsed_duration();
                                    player.seek_to(cur.saturating_sub(Duration::from_secs(30)));
                                    if let Some(controls) = media_controls.as_mut() {
                                        update_mpris_state(controls, &player);
                                    }
                                }
                                KeyCode::Char('}') | KeyCode::Char('>') => {
                                    let cur = player.current_elapsed_duration();
                                    player.seek_to(cur + Duration::from_secs(30));
                                    if let Some(controls) = media_controls.as_mut() {
                                        update_mpris_state(controls, &player);
                                    }
                                }
                                KeyCode::Char('n') => {
                                    let _ = player.next();
                                }
                                KeyCode::Char('p') => {
                                    let _ = player.previous();
                                }
                                KeyCode::Home | KeyCode::Char('g') => match active_focus {
                                    ActiveFocus::ArtistColumn => {
                                        if total_artist_rows > 0 {
                                            artist_list_state.select(Some(0));
                                            album_list_state.select(Some(1));
                                        }
                                    }
                                    ActiveFocus::AlbumColumn => {
                                        if total_album_rows > 0 {
                                            album_list_state.select(Some(0));
                                        }
                                    }
                                },
                                KeyCode::End | KeyCode::Char('G') => match active_focus {
                                    ActiveFocus::ArtistColumn => {
                                        if total_artist_rows > 0 {
                                            artist_list_state.select(Some(total_artist_rows - 1));
                                            album_list_state.select(Some(1));
                                        }
                                    }
                                    ActiveFocus::AlbumColumn => {
                                        if total_album_rows > 0 {
                                            album_list_state.select(Some(total_album_rows - 1));
                                        }
                                    }
                                },
                                KeyCode::PageUp => match active_focus {
                                    ActiveFocus::ArtistColumn => {
                                        if total_artist_rows > 0 {
                                            let curr = artist_list_state.selected().unwrap_or(0);
                                            let prev = curr.saturating_sub(10);
                                            artist_list_state.select(Some(prev));
                                            album_list_state.select(Some(1));
                                        }
                                    }
                                    ActiveFocus::AlbumColumn => {
                                        if total_album_rows > 0 {
                                            let curr = album_list_state.selected().unwrap_or(0);
                                            let prev = curr.saturating_sub(10);
                                            album_list_state.select(Some(prev));
                                        }
                                    }
                                },
                                KeyCode::PageDown => match active_focus {
                                    ActiveFocus::ArtistColumn => {
                                        if total_artist_rows > 0 {
                                            let curr = artist_list_state.selected().unwrap_or(0);
                                            let next = (curr + 10).min(total_artist_rows - 1);
                                            artist_list_state.select(Some(next));
                                            album_list_state.select(Some(1));
                                        }
                                    }
                                    ActiveFocus::AlbumColumn => {
                                        if total_album_rows > 0 {
                                            let curr = album_list_state.selected().unwrap_or(0);
                                            let next = (curr + 10).min(total_album_rows - 1);
                                            album_list_state.select(Some(next));
                                        }
                                    }
                                },
                                KeyCode::Enter => {
                                    match active_focus {
                                        ActiveFocus::AlbumColumn => {
                                            if let Some(selected_row) = album_list_state.selected() {
                                                if let Some(Some(track_idx)) = row_to_track.get(selected_row) {
                                                    let _ = player.play_index(*track_idx);
                                                } else if let Some(None) = row_to_track.get(selected_row) {
                                                    if let Some(Some(track_idx)) = row_to_track.get(selected_row + 1) {
                                                        let _ = player.play_index(*track_idx);
                                                        album_list_state.select(Some(selected_row + 1));
                                                    }
                                                }
                                            }
                                        }
                                        ActiveFocus::ArtistColumn => {
                                            active_focus = ActiveFocus::AlbumColumn;
                                            let first_track_row = row_to_track.iter().position(|r| r.is_some());
                                            if let Some(target_row) = first_track_row {
                                                album_list_state.select(Some(target_row));
                                                if let Some(Some(track_idx)) = row_to_track.get(target_row) {
                                                    let _ = player.play_index(*track_idx);
                                                }
                                            } else if total_album_rows > 0 {
                                                album_list_state.select(Some(0));
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
                                            album_list_state.select(Some(1));
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
                                            album_list_state.select(Some(1));
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
