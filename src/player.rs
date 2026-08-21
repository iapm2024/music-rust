use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
use lofty::file::{AudioFile, TaggedFileExt};
use lofty::probe::Probe;
use lofty::tag::Accessor;
use rodio::{Decoder, OutputStream, OutputStreamHandle, Sink};

#[derive(Debug, Clone)]
pub struct TrackInfo {
    pub path: PathBuf,
    pub title: String,
    pub artist: String,
    pub album: String,
    pub duration: Duration,
    pub track_number: Option<u32>,
    pub flat_index: usize,
}

#[derive(Debug, Clone)]
pub struct AlbumGroup {
    pub name: String,
    pub artist: String,
    pub tracks: Vec<TrackInfo>,
}

#[derive(Debug, Clone)]
pub struct ArtistGroup {
    pub name: String,
    pub albums: Vec<AlbumGroup>,
}

pub struct Player {
    _stream: OutputStream,
    _stream_handle: OutputStreamHandle,
    sink: Sink,
    pub artists: Vec<ArtistGroup>,
    pub flat_playlist: Vec<TrackInfo>,
    pub current_track_index: Option<usize>,
    pub is_paused: bool,
    pub track_start_time: Option<Instant>,
    pub elapsed_paused_duration: Duration,
    pub pause_start_time: Option<Instant>,
    pub volume: f32, // Volume range: 0.0 to 1.0 (0% - 100%)
}

impl Player {
    pub fn new() -> color_eyre::Result<Self> {
        let (_stream, _stream_handle) = OutputStream::try_default()?;
        let sink = Sink::try_new(&_stream_handle)?;
        let volume = 1.0; // Default 100% volume
        sink.set_volume(volume);

        Ok(Self {
            _stream,
            _stream_handle,
            sink,
            artists: Vec::new(),
            flat_playlist: Vec::new(),
            current_track_index: None,
            is_paused: false,
            track_start_time: None,
            elapsed_paused_duration: Duration::ZERO,
            pause_start_time: None,
            volume,
        })
    }

    pub fn set_volume(&mut self, vol: f32) {
        self.volume = vol.clamp(0.0, 1.0);
        self.sink.set_volume(self.volume);
    }

    pub fn volume_up(&mut self) {
        self.set_volume(self.volume + 0.05);
    }

    pub fn volume_down(&mut self) {
        self.set_volume(self.volume - 0.05);
    }

    pub fn load_directory<P: AsRef<Path>>(&mut self, path: P) -> color_eyre::Result<()> {
        self.artists.clear();
        self.flat_playlist.clear();

        let mut file_paths = Vec::new();
        fn scan_dir(dir: &Path, paths: &mut Vec<PathBuf>, depth: usize) {
            if depth > 16 {
                return;
            }
            if let Ok(entries) = fs::read_dir(dir) {
                for entry in entries.flatten() {
                    let path = entry.path();
                    if path.is_dir() {
                        scan_dir(&path, paths, depth + 1);
                    } else if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
                        let valid_exts = ["mp3", "flac", "wav", "ogg", "m4a", "aac", "opus", "wma"];
                        if valid_exts.iter().any(|&e| ext.eq_ignore_ascii_case(e)) {
                            paths.push(path);
                        }
                    }
                }
            }
        }

        if path.as_ref().is_dir() {
            scan_dir(path.as_ref(), &mut file_paths, 0);
        }

        let mut artist_map: BTreeMap<String, BTreeMap<String, Vec<TrackInfo>>> = BTreeMap::new();

        for file_path in file_paths {
            let mut title = file_path
                .file_stem()
                .and_then(|n| n.to_str())
                .unwrap_or("Unknown Title")
                .to_string();
            let mut artist = "Unknown Artist".to_string();
            let mut album = file_path
                .parent()
                .and_then(|p| p.file_name())
                .and_then(|n| n.to_str())
                .unwrap_or("Unknown Album")
                .to_string();
            let mut duration = Duration::ZERO;

            let mut track_number: Option<u32> = None;

            if let Ok(tagged_file) = Probe::open(&file_path).and_then(|p| p.read()) {
                duration = tagged_file.properties().duration();
                if let Some(tag) = tagged_file.primary_tag().or_else(|| tagged_file.first_tag()) {
                    if let Some(t) = tag.title() {
                        if !t.trim().is_empty() {
                            title = t.trim().to_string();
                        }
                    }
                    if let Some(a) = tag.artist() {
                        if !a.trim().is_empty() {
                            artist = a.trim().to_string();
                        }
                    }
                    if let Some(al) = tag.album() {
                        if !al.trim().is_empty() {
                            album = al.trim().to_string();
                        }
                    }
                    track_number = tag.track();
                }
            }

            let track = TrackInfo {
                path: file_path,
                title,
                artist: artist.clone(),
                album: album.clone(),
                duration,
                track_number,
                flat_index: 0,
            };

            artist_map
                .entry(artist)
                .or_default()
                .entry(album)
                .or_default()
                .push(track);
        }

        for (artist_name, album_map) in artist_map {
            let mut album_groups = Vec::new();
            for (album_name, mut tracks) in album_map {
                tracks.sort_by(|a, b| {
                    match (a.track_number, b.track_number) {
                        (Some(num_a), Some(num_b)) => num_a.cmp(&num_b),
                        (Some(_), None) => std::cmp::Ordering::Less,
                        (None, Some(_)) => std::cmp::Ordering::Greater,
                        (None, None) => a.path.file_name().cmp(&b.path.file_name()),
                    }
                });
                album_groups.push(AlbumGroup {
                    name: album_name,
                    artist: artist_name.clone(),
                    tracks,
                });
            }

            self.artists.push(ArtistGroup {
                name: artist_name,
                albums: album_groups,
            });
        }

        self.rebuild_flat_playlist();
        Ok(())
    }

    pub fn rebuild_flat_playlist(&mut self) {
        self.flat_playlist.clear();
        let mut idx = 0;
        for artist in &mut self.artists {
            for album in &mut artist.albums {
                for track in &mut album.tracks {
                    track.flat_index = idx;
                    self.flat_playlist.push(track.clone());
                    idx += 1;
                }
            }
        }
    }

    pub fn play_track(&mut self, track: &TrackInfo) -> color_eyre::Result<()> {
        let file = std::fs::File::open(&track.path)?;
        let source = Decoder::new(std::io::BufReader::new(file))?;

        self.sink.stop();
        self.sink.append(source);
        self.sink.set_volume(self.volume);
        self.sink.play();
        
        self.current_track_index = Some(track.flat_index);
        self.is_paused = false;
        self.track_start_time = Some(Instant::now());
        self.elapsed_paused_duration = Duration::ZERO;
        self.pause_start_time = None;
        Ok(())
    }

    pub fn play_index(&mut self, index: usize) -> color_eyre::Result<()> {
        if index < self.flat_playlist.len() {
            let track = self.flat_playlist[index].clone();
            self.play_track(&track)?;
        }
        Ok(())
    }

    pub fn seek_to(&mut self, target_time: Duration) {
        if self.current_track_index.is_some() {
            if self.sink.try_seek(target_time).is_ok() {
                let now = Instant::now();
                self.track_start_time = now.checked_sub(target_time).or(Some(now));
                self.elapsed_paused_duration = Duration::ZERO;
                if self.is_paused {
                    self.pause_start_time = Some(now);
                } else {
                    self.pause_start_time = None;
                }
            }
        }
    }

    pub fn toggle_pause(&mut self) {
        if self.sink.is_paused() {
            if let Some(pause_start) = self.pause_start_time {
                self.elapsed_paused_duration += pause_start.elapsed();
                self.pause_start_time = None;
            }
            self.sink.play();
            self.is_paused = false;
        } else {
            self.pause_start_time = Some(Instant::now());
            self.sink.pause();
            self.is_paused = true;
        }
    }

    pub fn current_elapsed_duration(&self) -> Duration {
        if let Some(start) = self.track_start_time {
            let total_elapsed = if let Some(pause_start) = self.pause_start_time {
                pause_start.saturating_duration_since(start)
            } else {
                Instant::now().saturating_duration_since(start)
            };
            total_elapsed.saturating_sub(self.elapsed_paused_duration)
        } else {
            Duration::ZERO
        }
    }

    pub fn current_track(&self) -> Option<&TrackInfo> {
        self.current_track_index.and_then(|i| self.flat_playlist.get(i))
    }

    pub fn next(&mut self) -> color_eyre::Result<()> {
        if self.flat_playlist.is_empty() {
            return Ok(());
        }
        let current = self.current_track_index.unwrap_or(0);
        let mut next_idx = (current + 1) % self.flat_playlist.len();
        let mut attempts = 0;
        while attempts < self.flat_playlist.len() {
            if self.play_index(next_idx).is_ok() {
                return Ok(());
            }
            next_idx = (next_idx + 1) % self.flat_playlist.len();
            attempts += 1;
        }
        self.sink.stop();
        self.current_track_index = None;
        self.is_paused = false;
        Ok(())
    }

    pub fn previous(&mut self) -> color_eyre::Result<()> {
        if self.flat_playlist.is_empty() {
            return Ok(());
        }
        let current = self.current_track_index.unwrap_or(0);
        let mut prev_idx = if current == 0 { self.flat_playlist.len() - 1 } else { current - 1 };
        let mut attempts = 0;
        while attempts < self.flat_playlist.len() {
            if self.play_index(prev_idx).is_ok() {
                return Ok(());
            }
            prev_idx = if prev_idx == 0 { self.flat_playlist.len() - 1 } else { prev_idx - 1 };
            attempts += 1;
        }
        self.sink.stop();
        self.current_track_index = None;
        self.is_paused = false;
        Ok(())
    }

    pub fn tick(&mut self) {
        if !self.is_paused && self.current_track_index.is_some() {
            // Check if track has finished playing via sink emptiness
            if self.sink.empty() {
                let _ = self.next();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_track_info_instantiation() {
        let track = TrackInfo {
            path: PathBuf::from("/music/song.mp3"),
            title: "Test Title".to_string(),
            artist: "Test Artist".to_string(),
            album: "Test Album".to_string(),
            duration: Duration::from_secs(180),
            track_number: Some(1),
            flat_index: 0,
        };
        assert_eq!(track.title, "Test Title");
        assert_eq!(track.duration.as_secs(), 180);
        assert_eq!(track.flat_index, 0);
    }

    #[test]
    fn test_album_and_artist_group_hierarchy() {
        let track1 = TrackInfo {
            path: PathBuf::from("/music/01.mp3"),
            title: "Track 1".to_string(),
            artist: "Artist A".to_string(),
            album: "Album 1".to_string(),
            duration: Duration::from_secs(120),
            track_number: Some(1),
            flat_index: 0,
        };
        let track2 = TrackInfo {
            path: PathBuf::from("/music/02.mp3"),
            title: "Track 2".to_string(),
            artist: "Artist A".to_string(),
            album: "Album 1".to_string(),
            duration: Duration::from_secs(200),
            track_number: Some(2),
            flat_index: 1,
        };

        let album = AlbumGroup {
            name: "Album 1".to_string(),
            artist: "Artist A".to_string(),
            tracks: vec![track1, track2],
        };

        let artist = ArtistGroup {
            name: "Artist A".to_string(),
            albums: vec![album],
        };

        assert_eq!(artist.albums.len(), 1);
        assert_eq!(artist.albums[0].tracks.len(), 2);
        assert_eq!(artist.albums[0].tracks[0].title, "Track 1");
    }
}
