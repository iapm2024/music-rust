use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
use lofty::file::{AudioFile, TaggedFileExt};
use lofty::probe::Probe;
use lofty::tag::Accessor;
use rayon::prelude::*;
use rodio::{Decoder, OutputStream, OutputStreamHandle, Sink, Source};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CachedTrackMeta {
    pub mtime: u64,
    pub size: u64,
    pub title: String,
    pub artist: String,
    pub album: String,
    pub duration_secs: u64,
    pub duration_nanos: u32,
    pub track_number: Option<u32>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct LibraryCache {
    pub entries: HashMap<PathBuf, CachedTrackMeta>,
}

impl LibraryCache {
    pub fn cache_file_path() -> Option<PathBuf> {
        if let Ok(xdg_cache) = std::env::var("XDG_CACHE_HOME") {
            return Some(PathBuf::from(xdg_cache).join("music-rust").join("library_cache.json"));
        }
        if let Ok(home) = std::env::var("HOME") {
            return Some(PathBuf::from(home).join(".cache").join("music-rust").join("library_cache.json"));
        }
        None
    }

    pub fn load() -> Self {
        if let Some(path) = Self::cache_file_path() {
            if let Ok(file) = fs::File::open(&path) {
                let reader = std::io::BufReader::new(file);
                if let Ok(cache) = serde_json::from_reader(reader) {
                    return cache;
                }
            }
        }
        Self::default()
    }

    pub fn save(&self) {
        if let Some(path) = Self::cache_file_path() {
            if let Some(parent) = path.parent() {
                let _ = fs::create_dir_all(parent);
            }
            let tmp_path = path.with_extension("tmp");
            if let Ok(file) = fs::File::create(&tmp_path) {
                let writer = std::io::BufWriter::new(file);
                if serde_json::to_writer(writer, self).is_ok() {
                    let _ = fs::rename(&tmp_path, &path);
                } else {
                    let _ = fs::remove_file(&tmp_path);
                }
            }
        }
    }
}

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
    pub previous_volume: Option<f32>,
}

struct RawFileEntry {
    path: PathBuf,
    mtime: u64,
    size: u64,
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
            previous_volume: Some(volume),
        })
    }

    pub fn set_volume(&mut self, vol: f32) {
        self.volume = ((vol * 100.0).round() / 100.0).clamp(0.0, 1.0);
        if self.volume > 0.0 {
            self.previous_volume = Some(self.volume);
        }
        self.sink.set_volume(self.volume);
    }

    pub fn toggle_mute(&mut self) {
        if self.volume > 0.0 {
            self.previous_volume = Some(self.volume);
            self.volume = 0.0;
            self.sink.set_volume(0.0);
        } else {
            let restore = self.previous_volume.unwrap_or(0.5);
            let target = if restore > 0.0 { restore } else { 0.5 };
            self.set_volume(target);
        }
    }

    pub fn stop(&mut self) {
        self.sink.stop();
        self.current_track_index = None;
        self.is_paused = false;
        self.track_start_time = None;
        self.elapsed_paused_duration = Duration::ZERO;
        self.pause_start_time = None;
    }

    pub fn restart_track(&mut self) {
        if self.current_track_index.is_some() {
            self.seek_to(Duration::ZERO);
        }
    }

    pub fn volume_up(&mut self) {
        self.set_volume(self.volume + 0.05);
    }

    pub fn volume_down(&mut self) {
        self.set_volume(self.volume - 0.05);
    }

    pub fn load_directory<P: AsRef<Path>>(&mut self, path: P) -> color_eyre::Result<()> {
        let currently_playing_path = self.current_track().map(|t| t.path.clone());

        self.artists.clear();
        self.flat_playlist.clear();

        #[inline]
        fn is_audio_extension(ext: &str) -> bool {
            matches!(
                ext.to_ascii_lowercase().as_str(),
                "mp3" | "flac" | "wav" | "ogg" | "m4a" | "aac" | "opus" | "wma"
            )
        }

        let mut file_entries = Vec::new();
        let mut visited_dirs = std::collections::HashSet::new();

        fn scan_dir(
            dir: &Path,
            paths: &mut Vec<RawFileEntry>,
            visited: &mut std::collections::HashSet<PathBuf>,
            depth: usize,
        ) {
            if depth > 16 {
                return;
            }
            if let Ok(canonical) = fs::canonicalize(dir) {
                if !visited.insert(canonical) {
                    return; // Circular symlink detected, skip
                }
            }
            if let Ok(entries) = fs::read_dir(dir) {
                for entry in entries.flatten() {
                    let file_type = match entry.file_type() {
                        Ok(ft) => ft,
                        Err(_) => continue,
                    };
                    let path = entry.path();
                    if file_type.is_dir() {
                        scan_dir(&path, paths, visited, depth + 1);
                    } else if file_type.is_file() {
                        if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
                            if is_audio_extension(ext) {
                                if let Ok(meta) = entry.metadata() {
                                    let mtime = meta
                                        .modified()
                                        .ok()
                                        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                                        .map(|d| d.as_secs())
                                        .unwrap_or(0);
                                    let size = meta.len();
                                    paths.push(RawFileEntry { path, mtime, size });
                                }
                            }
                        }
                    }
                }
            }
        }

        if path.as_ref().is_dir() {
            scan_dir(path.as_ref(), &mut file_entries, &mut visited_dirs, 0);
        }

        let cache = LibraryCache::load();

        let (parsed_tracks, new_entries): (Vec<TrackInfo>, Vec<Option<(PathBuf, CachedTrackMeta)>>) = file_entries
            .into_par_iter()
            .map(|entry| {
                if let Some(cached) = cache.entries.get(&entry.path) {
                    if cached.mtime == entry.mtime && cached.size == entry.size {
                        let track = TrackInfo {
                            path: entry.path,
                            title: cached.title.clone(),
                            artist: cached.artist.clone(),
                            album: cached.album.clone(),
                            duration: Duration::new(cached.duration_secs, cached.duration_nanos),
                            track_number: cached.track_number,
                            flat_index: 0,
                        };
                        return (track, None);
                    }
                }

                let mut title = entry.path
                    .file_stem()
                    .and_then(|n| n.to_str())
                    .unwrap_or("Unknown Title")
                    .to_string();
                let mut artist = "Unknown Artist".to_string();
                let mut album = entry.path
                    .parent()
                    .and_then(|p| p.file_name())
                    .and_then(|n| n.to_str())
                    .unwrap_or("Unknown Album")
                    .to_string();
                let mut duration = Duration::ZERO;
                let mut track_number = None;

                if let Ok(tagged_file) = Probe::open(&entry.path).and_then(|p| p.read()) {
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

                let cached_meta = CachedTrackMeta {
                    mtime: entry.mtime,
                    size: entry.size,
                    title: title.clone(),
                    artist: artist.clone(),
                    album: album.clone(),
                    duration_secs: duration.as_secs(),
                    duration_nanos: duration.subsec_nanos(),
                    track_number,
                };

                let track = TrackInfo {
                    path: entry.path.clone(),
                    title,
                    artist,
                    album,
                    duration,
                    track_number,
                    flat_index: 0,
                };

                (track, Some((entry.path, cached_meta)))
            })
            .unzip();

        let mut updated_cache = cache;
        let mut cache_modified = false;
        for new_entry in new_entries.into_iter().flatten() {
            updated_cache.entries.insert(new_entry.0, new_entry.1);
            cache_modified = true;
        }

        let initial_cache_len = updated_cache.entries.len();
        updated_cache.entries.retain(|p, _| p.exists());
        if updated_cache.entries.len() != initial_cache_len {
            cache_modified = true;
        }

        if cache_modified {
            updated_cache.save();
        }

        let mut artist_map: BTreeMap<String, BTreeMap<String, Vec<TrackInfo>>> = BTreeMap::new();
        for track in parsed_tracks {
            artist_map
                .entry(track.artist.clone())
                .or_default()
                .entry(track.album.clone())
                .or_default()
                .push(track);
        }

        for (artist_name, album_map) in artist_map {
            let mut album_groups = Vec::new();
            for (album_name, mut tracks) in album_map {
                tracks.sort_by(|a, b| {
                    match (a.track_number, b.track_number) {
                        (Some(num_a), Some(num_b)) if num_a != num_b => num_a.cmp(&num_b),
                        (Some(_), None) => std::cmp::Ordering::Less,
                        (None, Some(_)) => std::cmp::Ordering::Greater,
                        _ => a.path.file_name().cmp(&b.path.file_name()),
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

        if let Some(ref current_path) = currently_playing_path {
            self.current_track_index = self.flat_playlist.iter().position(|t| &t.path == current_path);
        } else {
            self.current_track_index = None;
        }

        Ok(())
    }

    pub fn rescan_directory<P: AsRef<Path>>(&mut self, path: P) -> color_eyre::Result<()> {
        self.load_directory(path)
    }

    pub fn update_track_metadata(
        &mut self,
        track_path: &Path,
        title: &str,
        artist: &str,
        album: &str,
        genre: &str,
        year: Option<u32>,
        track_num: Option<u32>,
    ) -> color_eyre::Result<()> {
        use lofty::tag::TagExt;

        let mut tagged_file = Probe::open(track_path)?.read()?;
        let tag = match tagged_file.primary_tag_mut() {
            Some(t) => t,
            None => {
                if let Some(t) = tagged_file.first_tag_mut() {
                    t
                } else {
                    let tag_type = tagged_file.primary_tag_type();
                    tagged_file.insert_tag(lofty::tag::Tag::new(tag_type));
                    tagged_file.primary_tag_mut().ok_or_else(|| color_eyre::eyre::eyre!("Failed to get or create tag"))?
                }
            }
        };

        if title.trim().is_empty() {
            tag.remove_title();
        } else {
            tag.set_title(title.trim().to_string());
        }

        if artist.trim().is_empty() {
            tag.remove_artist();
        } else {
            tag.set_artist(artist.trim().to_string());
        }

        if album.trim().is_empty() {
            tag.remove_album();
        } else {
            tag.set_album(album.trim().to_string());
        }

        if genre.trim().is_empty() {
            tag.remove_genre();
        } else {
            tag.set_genre(genre.trim().to_string());
        }

        if let Some(y) = year {
            tag.set_year(y);
        } else {
            tag.remove_year();
        }

        if let Some(t) = track_num {
            tag.set_track(t);
        } else {
            tag.remove_track();
        }

        tag.save_to_path(track_path, lofty::config::WriteOptions::default())?;

        // Invalidate entry in library cache so rescan picks up new mtime and tags
        let mut cache = LibraryCache::load();
        cache.entries.remove(track_path);
        cache.save();

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
        let is_opus = track
            .path
            .extension()
            .and_then(|e| e.to_str())
            .map(|ext| ext.eq_ignore_ascii_case("opus"))
            .unwrap_or(false);

        self.sink.stop();
        if is_opus {
            let file = std::fs::File::open(&track.path)?;
            let source = OpusSource::new(file, track.duration)?;
            self.sink.append(source);
        } else {
            let file = std::fs::File::open(&track.path)?;
            let source = Decoder::new(std::io::BufReader::new(file))?;
            self.sink.append(source);
        }
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
        if let Some(track) = self.current_track() {
            let clamped_time = if track.duration > Duration::ZERO {
                target_time.min(track.duration)
            } else {
                target_time
            };
            if self.sink.try_seek(clamped_time).is_ok() {
                let now = Instant::now();
                self.track_start_time = now.checked_sub(clamped_time).or(Some(now));
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
            let elapsed = total_elapsed.saturating_sub(self.elapsed_paused_duration);
            if let Some(track) = self.current_track() {
                if track.duration > Duration::ZERO {
                    return elapsed.min(track.duration);
                }
            }
            elapsed
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
        let next_idx = match self.current_track_index {
            Some(current) => (current + 1) % self.flat_playlist.len(),
            None => 0,
        };
        let mut target = next_idx;
        let mut attempts = 0;
        while attempts < self.flat_playlist.len() {
            if self.play_index(target).is_ok() {
                return Ok(());
            }
            target = (target + 1) % self.flat_playlist.len();
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
        // If current track has played for more than 3 seconds, restart it instead of going back
        if self.current_elapsed_duration() > Duration::from_secs(3) {
            self.restart_track();
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

pub struct OpusSource {
    reader: Option<opus_pure::OggOpusReader<std::io::BufReader<std::fs::File>>>,
    decoder: OpusDecoderWrapper,
    trim: opus_pure::Trim,
    sample_rate: u32,
    channels: u16,
    duration: Option<Duration>,
    buffer: Vec<i16>,
    buffer_idx: usize,
    raw_block: Vec<i16>,
}

enum OpusDecoderWrapper {
    Single(opus_pure::OpusDecoder),
    Multi(opus_pure::OpusMSDecoder),
}

impl OpusDecoderWrapper {
    fn reset_state(&mut self) {
        match self {
            OpusDecoderWrapper::Single(dec) => {
                let _ = dec.reset_state();
            }
            OpusDecoderWrapper::Multi(dec) => {
                let _ = dec.reset_state();
            }
        }
    }
}

impl OpusSource {
    pub fn new(file: std::fs::File, duration: Duration) -> color_eyre::Result<Self> {
        let buf_reader = std::io::BufReader::with_capacity(128 * 1024, file);
        let reader = opus_pure::OggOpusReader::new(buf_reader)?;
        let head = reader.head().clone();
        let channels = head.channel_count as usize;
        let sample_rate = 48_000u32;

        let decoder = if head.mapping_family == 0 {
            OpusDecoderWrapper::Single(head.decoder(sample_rate as i32)?)
        } else {
            let mut ms_dec = opus_pure::OpusMSDecoder::new(
                sample_rate as i32,
                channels,
                head.mapping_family,
            )?;
            for d in ms_dec.streams_mut() {
                d.gain_q8 = head.output_gain_q8 as i32;
            }
            OpusDecoderWrapper::Multi(ms_dec)
        };

        let trim = opus_pure::Trim::new(&head, sample_rate as i32, channels)?;
        let raw_block = vec![0i16; opus_pure::MAX_PACKET_SAMPLES * channels];
        let total_dur = if duration > Duration::ZERO { Some(duration) } else { None };

        let mut source = Self {
            reader: Some(reader),
            decoder,
            trim,
            sample_rate,
            channels: channels as u16,
            duration: total_dur,
            buffer: Vec::with_capacity(32 * 1024),
            buffer_idx: 0,
            raw_block,
        };

        // Pre-fill buffer with initial audio frames (e.g. ~100ms: ~4800 samples per channel)
        // to prevent ALSA buffer underruns at track onset or under thread scheduling jitter.
        let target_prefill = (sample_rate as usize * channels) / 10;
        while source.buffer.len() < target_prefill {
            if !source.decode_next_packet() {
                break;
            }
        }

        Ok(source)
    }

    /// Helper to read and decode a single packet into `self.buffer`.
    /// Returns true if a packet was processed (even if 0 kept samples), false on EOF/error.
    fn decode_next_packet(&mut self) -> bool {
        let Some(reader) = self.reader.as_mut() else {
            return false;
        };
        let packet = match reader.read_packet() {
            Ok(Some(pkt)) => pkt,
            _ => return false,
        };

        let decoded_samples = match &mut self.decoder {
            OpusDecoderWrapper::Single(dec) => {
                match dec.decode_s16(&packet.data, opus_pure::MAX_PACKET_SAMPLES, &mut self.raw_block) {
                    Ok(n) => n,
                    Err(_) => return true,
                }
            }
            OpusDecoderWrapper::Multi(dec) => {
                match dec.decode_s16(&packet.data, opus_pure::MAX_PACKET_SAMPLES, &mut self.raw_block) {
                    Ok(n) => n,
                    Err(_) => return true,
                }
            }
        };

        let total_samples = decoded_samples * (self.channels as usize);
        let kept_range = self.trim.keep_range(&packet, total_samples);

        if !kept_range.is_empty() && kept_range.end <= self.raw_block.len() {
            self.buffer.extend_from_slice(&self.raw_block[kept_range]);
        }

        true
    }

    /// Seek to a target timestamp in the Opus stream.
    pub fn seek(&mut self, target_time: Duration) -> Result<(), rodio::source::SeekError> {
        use std::io::Seek;

        let ch = self.channels as usize;
        let target_frames = (target_time.as_secs_f64() * self.sample_rate as f64).round() as u64;

        let buf_remaining_frames = (self.buffer.len().saturating_sub(self.buffer_idx)) / ch;
        let current_frame = self.trim.samples_emitted().saturating_sub(buf_remaining_frames as u64);

        // If target is behind current position, rewind and reinitialize reader & trim
        if target_frames < current_frame {
            let reader = self.reader.take().ok_or_else(|| {
                rodio::source::SeekError::Other(Box::new(std::io::Error::new(
                    std::io::ErrorKind::Other,
                    "Opus reader unavailable for seek",
                )))
            })?;

            let mut file = reader.into_inner();
            file.rewind().map_err(|e| rodio::source::SeekError::Other(Box::new(e)))?;

            let new_reader = opus_pure::OggOpusReader::new(file).map_err(|e| {
                rodio::source::SeekError::Other(Box::new(std::io::Error::new(
                    std::io::ErrorKind::Other,
                    e.to_string(),
                )))
            })?;
            let head = new_reader.head().clone();
            let new_trim = opus_pure::Trim::new(&head, self.sample_rate as i32, ch).map_err(|e| {
                rodio::source::SeekError::Other(Box::new(std::io::Error::new(
                    std::io::ErrorKind::Other,
                    e.to_string(),
                )))
            })?;

            self.decoder.reset_state();
            self.reader = Some(new_reader);
            self.trim = new_trim;
            self.buffer.clear();
            self.buffer_idx = 0;
        }

        // If target_frames falls within the currently buffered decoded samples:
        let buf_frames = self.buffer.len() / ch;
        let buf_start_frame = self.trim.samples_emitted().saturating_sub(buf_frames as u64);
        if target_frames >= buf_start_frame && target_frames <= self.trim.samples_emitted() {
            let offset_frames = (target_frames - buf_start_frame) as usize;
            self.buffer_idx = (offset_frames * ch).min(self.buffer.len());
            return Ok(());
        }

        // Clear buffer since we will seek forward beyond it
        self.buffer.clear();
        self.buffer_idx = 0;

        // Fast-forward through packets without full decoding until ~80ms before target (RFC 7845 §4.2 preroll)
        let preroll_frames = (self.sample_rate as u64 * 80) / 1000;
        let fast_forward_target = target_frames.saturating_sub(preroll_frames);

        while self.trim.samples_emitted() < fast_forward_target {
            let Some(reader) = self.reader.as_mut() else {
                return Ok(());
            };
            match reader.read_packet() {
                Ok(Some(packet)) => {
                    let nb_samples = opus_pure::packet::samples(&packet.data, self.sample_rate as i32).unwrap_or(960);
                    let total_samples = nb_samples * ch;
                    let _ = self.trim.keep_range(&packet, total_samples);
                }
                Ok(None) => return Ok(()), // EOF reached
                Err(e) => {
                    return Err(rodio::source::SeekError::Other(Box::new(std::io::Error::new(
                        std::io::ErrorKind::Other,
                        e.to_string(),
                    ))));
                }
            }
        }

        // Decode remaining packets through the preroll window until target frame is reached
        while self.trim.samples_emitted() < target_frames {
            if !self.decode_next_packet() {
                return Ok(()); // EOF reached
            }
        }

        // Adjust buffer_idx to align with the exact target frame
        if self.trim.samples_emitted() >= target_frames {
            let overshoot_frames = (self.trim.samples_emitted() - target_frames) as usize;
            let overshoot_samples = overshoot_frames * ch;
            self.buffer_idx = self.buffer.len().saturating_sub(overshoot_samples);
        }

        // Top up buffer with ~100ms of decoded samples ahead to prevent ALSA underruns immediately after seek
        let target_prefill = (self.sample_rate as usize * ch) / 10;
        while self.buffer.len().saturating_sub(self.buffer_idx) < target_prefill {
            if !self.decode_next_packet() {
                break;
            }
        }

        Ok(())
    }
}

impl Iterator for OpusSource {
    type Item = i16;

    #[inline]
    fn next(&mut self) -> Option<Self::Item> {
        loop {
            if self.buffer_idx < self.buffer.len() {
                let sample = self.buffer[self.buffer_idx];
                self.buffer_idx += 1;
                return Some(sample);
            }

            // Compact buffer before refilling
            self.buffer.clear();
            self.buffer_idx = 0;

            // Keep decoding until we have samples or reach EOF.
            // Cap consecutive empty/corrupt packets to 128 to prevent spinning on corrupted streams.
            let mut got_packets = false;
            let mut empty_packet_attempts = 0;
            while self.buffer.is_empty() && empty_packet_attempts < 128 {
                if !self.decode_next_packet() {
                    return None;
                }
                got_packets = true;
                empty_packet_attempts += 1;
            }

            if !got_packets || self.buffer.is_empty() {
                return None;
            }
        }
    }
}

impl Source for OpusSource {
    #[inline]
    fn current_frame_len(&self) -> Option<usize> {
        None
    }

    #[inline]
    fn channels(&self) -> u16 {
        self.channels
    }

    #[inline]
    fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    #[inline]
    fn total_duration(&self) -> Option<Duration> {
        self.duration
    }

    #[inline]
    fn try_seek(&mut self, pos: Duration) -> Result<(), rodio::source::SeekError> {
        self.seek(pos)
    }
}


pub fn format_duration(d: Duration) -> String {
    let total_secs = d.as_secs();
    let hours = total_secs / 3600;
    let minutes = (total_secs % 3600) / 60;
    let seconds = total_secs % 60;
    if hours > 0 {
        format!("{}:{:02}:{:02}", hours, minutes, seconds)
    } else {
        format!("{}:{:02}", minutes, seconds)
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

    #[test]
    fn test_library_cache_serde() {
        let mut cache = LibraryCache::default();
        let meta = CachedTrackMeta {
            mtime: 12345678,
            size: 1048576,
            title: "Cached Song".to_string(),
            artist: "Cached Artist".to_string(),
            album: "Cached Album".to_string(),
            duration_secs: 210,
            duration_nanos: 500000,
            track_number: Some(3),
        };
        let path = PathBuf::from("/music/song.mp3");
        cache.entries.insert(path.clone(), meta);

        let json = serde_json::to_string(&cache).expect("Failed to serialize cache");
        let deserialized: LibraryCache = serde_json::from_str(&json).expect("Failed to deserialize cache");

        let entry = deserialized.entries.get(&path).expect("Entry not found in deserialized cache");
        assert_eq!(entry.title, "Cached Song");
        assert_eq!(entry.artist, "Cached Artist");
        assert_eq!(entry.album, "Cached Album");
        assert_eq!(entry.duration_secs, 210);
        assert_eq!(entry.track_number, Some(3));
    }

    #[test]
    fn test_benchmark_music_dir_performance() {
        let music_dir = std::env::var_os("HOME")
            .map(|h| PathBuf::from(h).join("Music"))
            .filter(|p| p.exists());

        if let Some(dir) = music_dir {
            // First load (populating or using cache)
            let start = Instant::now();
            let mut player = Player::new().expect("Failed to initialize player");
            player.load_directory(&dir).expect("Failed to load directory");
            let cold_time = start.elapsed();
            let track_count = player.flat_playlist.len();

            // Second load (warm cache hit)
            let warm_start = Instant::now();
            let mut player2 = Player::new().expect("Failed to initialize player");
            player2.load_directory(&dir).expect("Failed to load directory");
            let warm_time = warm_start.elapsed();

            println!(
                "\n🚀 Performance Benchmark ({} tracks in {}):\n  - First load: {:?}\n  - Warm cache load: {:?}\n",
                track_count,
                dir.display(),
                cold_time,
                warm_time
            );
        }
    }

    #[test]
    fn test_volume_and_mute() {
        let mut player = Player::new().expect("Failed to initialize player");
        player.set_volume(0.8);
        assert!((player.volume - 0.8).abs() < 0.01);
        assert_eq!(player.previous_volume, Some(0.8));

        // Toggle mute
        player.toggle_mute();
        assert_eq!(player.volume, 0.0);
        assert_eq!(player.previous_volume, Some(0.8));

        // Toggle unmute restores 0.8
        player.toggle_mute();
        assert!((player.volume - 0.8).abs() < 0.01);

        // Volume up un-mutes automatically if muted
        player.toggle_mute();
        assert_eq!(player.volume, 0.0);
        player.volume_up();
        assert!((player.volume - 0.05).abs() < 0.01);
    }

    #[test]
    fn test_player_stop() {
        let mut player = Player::new().expect("Failed to initialize player");
        player.current_track_index = Some(2);
        player.is_paused = true;
        player.stop();
        assert_eq!(player.current_track_index, None);
        assert!(!player.is_paused);
        assert_eq!(player.track_start_time, None);
    }

    #[test]
    fn test_format_duration() {
        assert_eq!(format_duration(Duration::from_secs(0)), "0:00");
        assert_eq!(format_duration(Duration::from_secs(9)), "0:09");
        assert_eq!(format_duration(Duration::from_secs(65)), "1:05");
        assert_eq!(format_duration(Duration::from_secs(3599)), "59:59");
        assert_eq!(format_duration(Duration::from_secs(3600)), "1:00:00");
        assert_eq!(format_duration(Duration::from_secs(3665)), "1:01:05");
        assert_eq!(format_duration(Duration::from_secs(7325)), "2:02:05");
    }

    #[test]
    fn test_next_and_previous_empty() {
        let mut player = Player::new().expect("Failed to initialize player");
        assert!(player.next().is_ok());
        assert!(player.previous().is_ok());
    }

    #[test]
    fn test_rescan_directory_preserves_playing_track() {
        let mut player = Player::new().expect("Failed to initialize player");
        let temp_dir = std::env::temp_dir().join(format!("music_rust_test_{}", std::process::id()));
        let _ = fs::create_dir_all(&temp_dir);

        let song_path = temp_dir.join("test_song.mp3");
        let _ = fs::write(&song_path, b"dummy audio content");

        // Load directory
        let _ = player.load_directory(&temp_dir);
        if !player.flat_playlist.is_empty() {
            player.current_track_index = Some(0);
            let playing_path = player.current_track().unwrap().path.clone();

            // Rescan
            let _ = player.rescan_directory(&temp_dir);
            assert_eq!(player.current_track().map(|t| &t.path), Some(&playing_path));
        }

        let _ = fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_play_opus_track() {
        let sample_opus = PathBuf::from("/home/iapizarro/Music/Sirenia/Sirenia - The Enigma of Life (Full Album) [5Frbt6Z_rO8].opus");
        if sample_opus.exists() {
            let mut player = Player::new().expect("Failed to initialize player");
            let track = TrackInfo {
                path: sample_opus,
                title: "Sirenia Opus Test".to_string(),
                artist: "Sirenia".to_string(),
                album: "Test Album".to_string(),
                duration: Duration::from_secs(3857),
                track_number: Some(1),
                flat_index: 0,
            };
            player.flat_playlist.push(track.clone());
            assert!(player.play_track(&track).is_ok());
            assert_eq!(player.current_track_index, Some(0));
            assert!(!player.is_paused);
            player.stop();
        }
    }

    #[test]
    fn test_seek_opus_track() {
        let sample_opus = PathBuf::from("/home/iapizarro/Music/Sirenia/Sirenia - The Enigma of Life (Full Album) [5Frbt6Z_rO8].opus");
        if sample_opus.exists() {
            let mut player = Player::new().expect("Failed to initialize player");
            let track = TrackInfo {
                path: sample_opus,
                title: "Sirenia Opus Test".to_string(),
                artist: "Sirenia".to_string(),
                album: "Test Album".to_string(),
                duration: Duration::from_secs(3857),
                track_number: Some(1),
                flat_index: 0,
            };
            player.flat_playlist.push(track.clone());
            assert!(player.play_track(&track).is_ok());

            // Seek forward to 60s
            player.seek_to(Duration::from_secs(60));
            let elapsed_60 = player.current_elapsed_duration();
            assert!(elapsed_60 >= Duration::from_secs(59) && elapsed_60 <= Duration::from_secs(62));

            // Seek further forward to 120s
            player.seek_to(Duration::from_secs(120));
            let elapsed_120 = player.current_elapsed_duration();
            assert!(elapsed_120 >= Duration::from_secs(119) && elapsed_120 <= Duration::from_secs(122));

            // Seek backward to 15s
            player.seek_to(Duration::from_secs(15));
            let elapsed_15 = player.current_elapsed_duration();
            assert!(elapsed_15 >= Duration::from_secs(14) && elapsed_15 <= Duration::from_secs(17));

            // Seek back to start (0s)
            player.seek_to(Duration::ZERO);
            let elapsed_0 = player.current_elapsed_duration();
            assert!(elapsed_0 <= Duration::from_secs(2));

            player.stop();
        }
    }

    #[test]
    fn test_update_track_metadata() {
        let sample_opus = PathBuf::from("/home/iapizarro/Music/Sirenia/Sirenia - The Enigma of Life (Full Album) [5Frbt6Z_rO8].opus");
        if sample_opus.exists() {
            let temp_dir = std::env::temp_dir().join(format!("music_rust_meta_test_{}", std::process::id()));
            let _ = fs::create_dir_all(&temp_dir);
            let test_file = temp_dir.join("test_song.opus");
            let _ = fs::copy(&sample_opus, &test_file);

            let mut player = Player::new().expect("Failed to initialize player");
            let update_res = player.update_track_metadata(
                &test_file,
                "Updated Opus Title",
                "Updated Opus Artist",
                "Updated Opus Album",
                "Gothic Rock",
                Some(2025),
                Some(7),
            );
            assert!(update_res.is_ok());

            // Read back with Lofty to verify persistence
            let tagged = lofty::probe::Probe::open(&test_file).unwrap().read().unwrap();
            let tag = tagged.primary_tag().unwrap();
            use lofty::tag::Accessor;
            assert_eq!(tag.title().as_deref(), Some("Updated Opus Title"));
            assert_eq!(tag.artist().as_deref(), Some("Updated Opus Artist"));
            assert_eq!(tag.album().as_deref(), Some("Updated Opus Album"));
            assert_eq!(tag.genre().as_deref(), Some("Gothic Rock"));
            assert_eq!(tag.year(), Some(2025));
            assert_eq!(tag.track(), Some(7));

            let _ = fs::remove_dir_all(&temp_dir);
        }
    }
}



