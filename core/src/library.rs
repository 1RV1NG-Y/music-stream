//! The phone's music library (PLAN.md §3): scanning the shared storage folders, file
//! revisions, tags and cover art, aliases for moved files, and the index snapshot the PCs
//! fetch. Only the device that owns the files writes it.

use std::{
    collections::{HashMap, HashSet},
    fs::File,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::Instant,
};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

use crate::store::write_atomic;

pub type TrackId = [u8; 16];
pub type ArtHash = [u8; 32];

/// Bytes hashed at each end of a file for its fingerprint.
const SAMPLE: u64 = 64 * 1024;
/// Longest side of a cover thumbnail.
const ART_SIZE: u32 = 512;
const AUDIO_EXTENSIONS: &[&str] = &["flac", "mp3", "m4a", "mp4", "aac", "ogg", "oga", "wav", "aif", "aiff"];
const FOLDER_ART: &[&str] = &["cover.jpg", "cover.png", "folder.jpg", "folder.png", "front.jpg", "front.png"];

/// A folder tree that counts as library, with the storage volume it lives on (its name is
/// part of every track id, PLAN.md §3).
#[derive(Debug, Clone)]
pub struct Root {
    pub volume: String,
    pub path: PathBuf,
}

/// What a file looked like, from `fstat` plus a hash of its two ends (PLAN.md §3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Fingerprint {
    pub size: u64,
    pub mtime: (i64, u32),
    pub ctime: (i64, u32),
    pub ino: u64,
    pub sample: [u8; 32],
}

impl Fingerprint {
    pub fn rev(&self) -> u64 {
        let bytes = postcard::to_stdvec(self).expect("fingerprint serializes");
        u64::from_le_bytes(blake3::hash(&bytes).as_bytes()[..8].try_into().unwrap())
    }

    /// The parts `fstat` gives, to check a file hasn't changed since its fingerprint.
    fn stat_matches(&self, stat: &Stat) -> bool {
        self.size == stat.size && self.mtime == stat.mtime && self.ctime == stat.ctime && self.ino == stat.ino
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Stat {
    size: u64,
    mtime: (i64, u32),
    ctime: (i64, u32),
    ino: u64,
}

fn stat_of(meta: &std::fs::Metadata) -> Stat {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        Stat { size: meta.len(), mtime: (meta.mtime(), meta.mtime_nsec() as u32), ctime: (meta.ctime(), meta.ctime_nsec() as u32), ino: meta.ino() }
    }
    #[cfg(not(unix))]
    {
        let mtime = meta.modified().ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map_or((0, 0), |d| (d.as_secs() as i64, d.subsec_nanos()));
        Stat { size: meta.len(), mtime, ctime: (0, 0), ino: 0 }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReplayGain {
    pub track_gain: Option<f32>,
    pub track_peak: Option<f32>,
    pub album_gain: Option<f32>,
    pub album_peak: Option<f32>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Track {
    pub id: TrackId,
    pub rev: u64,
    pub volume: String,
    /// Relative to the volume's root, with `/` separators.
    pub path: String,
    pub fp: Fingerprint,
    pub format: String,
    pub duration_ms: u64,
    pub sample_rate: u32,
    pub bit_depth: u8,
    pub channels: u8,
    pub title: String,
    pub artist: String,
    pub album: String,
    pub album_artist: String,
    pub disc: u32,
    pub track: u32,
    pub year: u16,
    pub genre: String,
    pub replay_gain: ReplayGain,
    pub art: Option<ArtHash>,
}

impl Track {
    pub fn size(&self) -> u64 {
        self.fp.size
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Index {
    pub generation: u64,
    pub tracks: Vec<Track>,
    /// `old id → new id` for files that moved with their bytes unchanged (PLAN.md §3).
    pub aliases: Vec<(TrackId, TrackId)>,
}

impl Index {
    pub fn encode(&self) -> Vec<u8> {
        let raw = postcard::to_stdvec(self).expect("index serializes");
        zstd::bulk::compress(&raw, 3).expect("zstd compresses")
    }

    pub fn decode(bytes: &[u8]) -> Result<Self> {
        let raw = zstd::bulk::decompress(bytes, 512 * 1024 * 1024).context("the library index is damaged")?;
        postcard::from_bytes(&raw).context("the library index is damaged")
    }

    /// Where a stored reference points now: the id itself while it's current, else one hop
    /// through the aliases (never more, so a malformed table can't loop).
    pub fn resolve(&self, id: &TrackId) -> Option<&Track> {
        let find = |id: &TrackId| self.tracks.iter().find(|t| &t.id == id);
        find(id).or_else(|| self.aliases.iter().find(|(old, _)| old == id).and_then(|(_, new)| find(new)))
    }
}

/// How playlists, likes and play counts refer to a track (PLAN.md §3): its id, plus tags to
/// find it again if the file was re-ripped under another path.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TrackRef {
    pub id: String,
    pub fallback: Fallback,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Fallback {
    pub album_artist: String,
    pub album: String,
    pub disc: u32,
    pub track: u32,
    pub title: String,
    pub duration_s: u64,
}

impl Track {
    pub fn reference(&self) -> TrackRef {
        TrackRef {
            id: hex(&self.id),
            fallback: Fallback {
                album_artist: if self.album_artist.is_empty() { self.artist.clone() } else { self.album_artist.clone() },
                album: self.album.clone(),
                disc: self.disc,
                track: self.track,
                title: self.title.clone(),
                duration_s: (self.duration_ms + 500) / 1000,
            },
        }
    }
}

impl Index {
    /// Where a stored reference points now (PLAN.md §3): the id while it's current, else one
    /// alias hop, else the one track whose tags match its fallback (none if several do).
    pub fn resolve_ref(&self, r: &TrackRef) -> Option<&Track> {
        if let Some(t) = parse_hex::<16>(&r.id).ok().and_then(|id| self.resolve(&id)) {
            return Some(t);
        }
        let f = &r.fallback;
        if f.title.is_empty() {
            return None;
        }
        let same = |a: &str, b: &str| a.trim().eq_ignore_ascii_case(b.trim());
        let mut matches = self.tracks.iter().filter(|t| {
            let g = t.reference().fallback;
            same(&g.album_artist, &f.album_artist) && same(&g.album, &f.album) && g.disc == f.disc && g.track == f.track && same(&g.title, &f.title) && g.duration_s.abs_diff(f.duration_s) <= 1
        });
        match (matches.next(), matches.next()) {
            (Some(t), None) => Some(t),
            _ => None,
        }
    }
}

pub fn track_id(volume: &str, path: &str) -> TrackId {
    let mut h = blake3::Hasher::new();
    h.update(volume.as_bytes());
    h.update(&[0]);
    h.update(path.as_bytes());
    h.finalize().as_bytes()[..16].try_into().unwrap()
}

pub fn hex(bytes: &[u8]) -> String {
    data_encoding::HEXLOWER.encode(bytes)
}

pub fn parse_hex<const N: usize>(s: &str) -> Result<[u8; N]> {
    let bytes = data_encoding::HEXLOWER_PERMISSIVE.decode(s.as_bytes()).context("not a valid id")?;
    bytes.try_into().map_err(|_| anyhow::anyhow!("not a valid id"))
}

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScanReport {
    pub tracks: usize,
    pub added: usize,
    pub changed: usize,
    pub removed: usize,
    pub moved: usize,
    pub unreadable: usize,
    pub took_ms: u64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LibraryStatus {
    pub tracks: usize,
    pub bytes: u64,
    pub generation: u64,
    pub scanning: bool,
    pub last_scan: Option<ScanReport>,
    pub last_scan_at: Option<String>,
    pub error: Option<String>,
}

struct Snapshot {
    index: Arc<Index>,
    encoded: Arc<Vec<u8>>,
    hash: [u8; 32],
    by_id: HashMap<TrackId, usize>,
}

impl Snapshot {
    fn new(index: Index) -> Self {
        let encoded = index.encode();
        let hash = *blake3::hash(&postcard::to_stdvec(&(&index.tracks, &index.aliases)).unwrap()).as_bytes();
        let by_id = index.tracks.iter().enumerate().map(|(i, t)| (t.id, i)).collect();
        Snapshot { index: Arc::new(index), encoded: Arc::new(encoded), hash, by_id }
    }
}

struct State {
    snapshot: Snapshot,
    scanning: bool,
    last_scan: Option<ScanReport>,
    last_scan_at: Option<String>,
    error: Option<String>,
}

/// Open files being served, keyed by (track, rev), shared between concurrent requests and
/// read only with positional reads (PLAN.md §5).
struct OpenFile {
    file: Arc<File>,
    path: PathBuf,
    fp: Fingerprint,
    used: Instant,
}

pub struct Library {
    dir: PathBuf,
    roots: Vec<Root>,
    state: Mutex<State>,
    open: Mutex<HashMap<(TrackId, u64), OpenFile>>,
    /// One scan at a time.
    scan_lock: Mutex<()>,
}

/// Why a track can't be served at the rev asked for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ServeError {
    Changed { rev: u64, size: u64 },
    Missing,
}

const MAX_OPEN: usize = 8;

impl Library {
    /// `dir` holds the index and the art thumbnails.
    pub fn open(dir: PathBuf, roots: Vec<Root>) -> Result<Self> {
        std::fs::create_dir_all(dir.join("art"))?;
        let index = match std::fs::read(dir.join("index.bin")) {
            Ok(bytes) => Index::decode(&bytes).unwrap_or_else(|e| {
                tracing::warn!("starting a new library index: {e:#}");
                Index::default()
            }),
            Err(_) => Index::default(),
        };
        Ok(Library {
            dir,
            roots,
            state: Mutex::new(State { snapshot: Snapshot::new(index), scanning: false, last_scan: None, last_scan_at: None, error: None }),
            open: Mutex::default(),
            scan_lock: Mutex::new(()),
        })
    }

    pub fn status(&self) -> LibraryStatus {
        let st = self.state.lock().unwrap();
        let index = &st.snapshot.index;
        LibraryStatus {
            tracks: index.tracks.len(),
            bytes: index.tracks.iter().map(Track::size).sum(),
            generation: index.generation,
            scanning: st.scanning,
            last_scan: st.last_scan.clone(),
            last_scan_at: st.last_scan_at.clone(),
            error: st.error.clone(),
        }
    }

    pub fn index(&self) -> Arc<Index> {
        self.state.lock().unwrap().snapshot.index.clone()
    }

    /// The compressed index and its content hash.
    pub fn snapshot(&self) -> ([u8; 32], Arc<Vec<u8>>) {
        let st = self.state.lock().unwrap();
        (st.snapshot.hash, st.snapshot.encoded.clone())
    }

    pub fn track(&self, id: &TrackId) -> Option<Track> {
        let st = self.state.lock().unwrap();
        st.snapshot.by_id.get(id).map(|&i| st.snapshot.index.tracks[i].clone())
    }

    pub fn art_path(&self, hash: &ArtHash) -> PathBuf {
        self.dir.join("art").join(hex(hash))
    }

    /// Where a track's file is on this device.
    pub fn file_path(&self, track: &Track) -> Option<PathBuf> {
        self.full_path(track)
    }

    fn full_path(&self, track: &Track) -> Option<PathBuf> {
        let root = self.roots.iter().find(|r| r.volume == track.volume)?;
        Some(track.path.split('/').fold(root.path.clone(), |p, part| p.join(part)))
    }

    /// Rescans the roots. Unchanged files (same `fstat`) keep their entry without being read.
    /// Blocking; call from a blocking thread.
    pub fn scan(&self) -> Result<ScanReport> {
        let _one = self.scan_lock.lock().unwrap();
        let started = Instant::now();
        let previous = {
            let mut st = self.state.lock().unwrap();
            st.scanning = true;
            st.snapshot.index.clone()
        };
        let result = self.scan_inner(&previous, started);
        let mut st = self.state.lock().unwrap();
        st.scanning = false;
        st.last_scan_at = Some(crate::model::now_iso());
        match &result {
            Ok((report, next)) => {
                st.error = None;
                st.last_scan = Some(report.clone());
                if let Some(next) = next {
                    st.snapshot = Snapshot::new(next.clone());
                }
            }
            Err(e) => st.error = Some(format!("{e:#}")),
        }
        result.map(|(report, _)| report)
    }

    fn scan_inner(&self, previous: &Index, started: Instant) -> Result<(ScanReport, Option<Index>)> {
        let before: HashMap<TrackId, &Track> = previous.tracks.iter().map(|t| (t.id, t)).collect();
        let mut report = ScanReport::default();
        let mut tracks = Vec::new();
        let mut art_cache: HashMap<[u8; 32], Option<ArtHash>> = HashMap::new();
        let mut found_any_root = false;
        for root in &self.roots {
            if !root.path.is_dir() {
                tracing::warn!(path = %root.path.display(), "library folder not found");
                continue;
            }
            found_any_root = true;
            let mut files = Vec::new();
            walk(&root.path, &root.path, &mut files);
            for (path, rel) in files {
                let id = track_id(&root.volume, &rel);
                let Ok(meta) = std::fs::metadata(&path) else {
                    report.unreadable += 1;
                    continue;
                };
                let stat = stat_of(&meta);
                if let Some(old) = before.get(&id).filter(|t| t.fp.stat_matches(&stat)) {
                    tracks.push((*old).clone());
                    continue;
                }
                match read_track(&path, &root.volume, &rel, id, &self.dir, &mut art_cache) {
                    Ok(track) => {
                        match before.get(&id) {
                            Some(old) if old.rev == track.rev => {}
                            Some(_) => report.changed += 1,
                            None => report.added += 1,
                        }
                        tracks.push(track);
                    }
                    Err(e) => {
                        tracing::debug!(path = %path.display(), "unreadable: {e:#}");
                        report.unreadable += 1;
                    }
                }
            }
        }
        if !found_any_root && !self.roots.is_empty() {
            bail!("The music folders can't be read. Check the app's permission to read audio.");
        }
        tracks.sort_by(|a, b| (&a.volume, &a.path).cmp(&(&b.volume, &b.path)));
        let current: HashSet<TrackId> = tracks.iter().map(|t| t.id).collect();
        let vanished: Vec<&Track> = previous.tracks.iter().filter(|t| !current.contains(&t.id)).collect();
        let appeared: Vec<&Track> = tracks.iter().filter(|t| !before.contains_key(&t.id)).collect();
        report.removed = vanished.len();
        let mut aliases = previous.aliases.clone();
        for old in &vanished {
            let candidates: Vec<&&Track> = appeared.iter().filter(|n| n.fp.size == old.fp.size && n.fp.sample == old.fp.sample).collect();
            if let [new] = candidates.as_slice() {
                add_alias(&mut aliases, old.id, new.id);
                report.moved += 1;
            }
        }
        // An id that is current again (a file moved back) is no alias.
        aliases.retain(|(old, _)| !current.contains(old));
        report.tracks = tracks.len();
        report.took_ms = started.elapsed().as_millis() as u64;
        let next = Index { generation: previous.generation, tracks, aliases };
        if next.tracks == previous.tracks && next.aliases == previous.aliases {
            return Ok((report, None));
        }
        let next = Index { generation: previous.generation + 1, ..next };
        write_atomic(&self.dir.join("index.bin"), &next.encode())?;
        self.prune_art(&next);
        tracing::info!(?report, "library rescanned");
        Ok((report, Some(next)))
    }

    fn prune_art(&self, index: &Index) {
        let used: HashSet<String> = index.tracks.iter().filter_map(|t| t.art.map(|a| hex(&a))).collect();
        if let Ok(entries) = std::fs::read_dir(self.dir.join("art")) {
            for e in entries.flatten() {
                if !used.contains(e.file_name().to_string_lossy().as_ref()) {
                    let _ = std::fs::remove_file(e.path());
                }
            }
        }
    }

    /// Opens a track to serve `rev` of it, checking the file is still that revision
    /// (PLAN.md §5). Blocking.
    pub fn open_for_serving(&self, id: &TrackId, rev: u64) -> Result<Served, ServeError> {
        let track = self.track(id).ok_or(ServeError::Missing)?;
        let path = self.full_path(&track).ok_or(ServeError::Missing)?;
        let key = (*id, rev);
        let mut open = self.open.lock().unwrap();
        if let Some(of) = open.get_mut(&key) {
            // A rename-over leaves an open descriptor on the old file: check the path.
            match std::fs::metadata(&of.path) {
                Ok(meta) if of.fp.stat_matches(&stat_of(&meta)) => {
                    of.used = Instant::now();
                    return Ok(Served { file: of.file.clone(), fp: of.fp });
                }
                _ => {
                    open.remove(&key);
                }
            }
        }
        drop(open);
        let file = File::open(&path).map_err(|_| ServeError::Missing)?;
        let fp = fingerprint(&file).map_err(|_| ServeError::Missing)?;
        if fp.rev() != rev {
            return Err(ServeError::Changed { rev: fp.rev(), size: fp.size });
        }
        let file = Arc::new(file);
        let mut open = self.open.lock().unwrap();
        if open.len() >= MAX_OPEN {
            if let Some(oldest) = open.iter().min_by_key(|(_, of)| of.used).map(|(k, _)| *k) {
                open.remove(&oldest);
            }
        }
        open.insert(key, OpenFile { file: file.clone(), path, fp, used: Instant::now() });
        Ok(Served { file, fp })
    }

    /// Closes handles unused for a minute.
    pub fn close_idle(&self) {
        self.open.lock().unwrap().retain(|_, of| of.used.elapsed().as_secs() < 60);
    }
}

/// An open file at a verified revision.
pub struct Served {
    pub file: Arc<File>,
    pub fp: Fingerprint,
}

impl Served {
    /// Reads `buf.len()` bytes at `offset`, then checks the file is still the same revision,
    /// so the bytes returned belong to it (PLAN.md §5).
    pub fn read_block(&self, offset: u64, buf: &mut [u8]) -> Result<(), ServeError> {
        read_exact_at(&self.file, buf, offset).map_err(|_| ServeError::Missing)?;
        let meta = self.file.metadata().map_err(|_| ServeError::Missing)?;
        let stat = stat_of(&meta);
        if !self.fp.stat_matches(&stat) {
            // The new rev isn't known until the file settles; the caller rescans.
            return Err(ServeError::Changed { rev: 0, size: stat.size });
        }
        Ok(())
    }
}

/// Moves files' aliases forward: `old → new`, and anything that pointed at `old` now points
/// at `new`, so every alias is one hop (PLAN.md §3).
fn add_alias(aliases: &mut Vec<(TrackId, TrackId)>, old: TrackId, new: TrackId) {
    for (_, target) in aliases.iter_mut() {
        if *target == old {
            *target = new;
        }
    }
    aliases.retain(|(k, _)| *k != old && *k != new);
    aliases.push((old, new));
}

pub fn fingerprint(file: &File) -> std::io::Result<Fingerprint> {
    let stat = stat_of(&file.metadata()?);
    let mut h = blake3::Hasher::new();
    let head = stat.size.min(SAMPLE);
    let mut buf = vec![0u8; head as usize];
    read_exact_at(file, &mut buf, 0)?;
    h.update(&buf);
    if stat.size > SAMPLE {
        let tail_start = stat.size.saturating_sub(SAMPLE).max(SAMPLE);
        let mut buf = vec![0u8; (stat.size - tail_start) as usize];
        read_exact_at(file, &mut buf, tail_start)?;
        h.update(&buf);
    }
    Ok(Fingerprint { size: stat.size, mtime: stat.mtime, ctime: stat.ctime, ino: stat.ino, sample: *h.finalize().as_bytes() })
}

/// Positional read that never uses or relies on the file cursor (PLAN.md §5).
pub fn read_exact_at(file: &File, buf: &mut [u8], offset: u64) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::FileExt;
        file.read_exact_at(buf, offset)
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::FileExt;
        let (mut buf, mut offset) = (buf, offset);
        while !buf.is_empty() {
            match file.seek_read(buf, offset) {
                Ok(0) => return Err(std::io::ErrorKind::UnexpectedEof.into()),
                Ok(n) => {
                    buf = &mut buf[n..];
                    offset += n as u64;
                }
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
                Err(e) => return Err(e),
            }
        }
        Ok(())
    }
}

fn walk(root: &Path, dir: &Path, out: &mut Vec<(PathBuf, String)>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    let mut entries: Vec<_> = entries.flatten().collect();
    if entries.iter().any(|e| e.file_name() == ".nomedia") {
        return;
    }
    entries.sort_by_key(|e| e.file_name());
    for e in entries {
        let name = e.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') {
            continue;
        }
        let path = e.path();
        let Ok(kind) = e.file_type() else { continue };
        if kind.is_dir() {
            if dir == root && name == "Android" {
                continue;
            }
            walk(root, &path, out);
        } else if kind.is_file() || kind.is_symlink() {
            let ext = path.extension().and_then(|x| x.to_str()).map(str::to_ascii_lowercase);
            if ext.is_some_and(|x| AUDIO_EXTENSIONS.contains(&x.as_str())) {
                if let Ok(rel) = path.strip_prefix(root) {
                    let rel = rel.components().map(|c| c.as_os_str().to_string_lossy()).collect::<Vec<_>>().join("/");
                    out.push((path, rel));
                }
            }
        }
    }
}

fn read_track(path: &Path, volume: &str, rel: &str, id: TrackId, dir: &Path, art_cache: &mut HashMap<[u8; 32], Option<ArtHash>>) -> Result<Track> {
    use lofty::prelude::*;
    let file = File::open(path)?;
    let fp = fingerprint(&file)?;
    let tagged = lofty::read_from_path(path).context("reading tags")?;
    let props = tagged.properties();
    let tag = tagged.primary_tag().or_else(|| tagged.first_tag());
    let text = |key: lofty::tag::ItemKey| tag.and_then(|t| t.get_string(key)).map(|s| s.trim().to_owned()).unwrap_or_default();
    let gain = |key: lofty::tag::ItemKey| parse_gain(&text(key));
    let stem = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    let art_source = tag
        .and_then(|t| t.pictures().iter().find(|p| p.pic_type() == lofty::picture::PictureType::CoverFront).or_else(|| t.pictures().first()))
        .map(|p| p.data().to_vec())
        .or_else(|| folder_art(path));
    let art = art_source.and_then(|bytes| {
        let key = *blake3::hash(&bytes).as_bytes();
        *art_cache.entry(key).or_insert_with(|| make_thumbnail(&bytes, dir).map_err(|e| tracing::debug!("cover art: {e:#}")).ok())
    });
    Ok(Track {
        id,
        rev: fp.rev(),
        volume: volume.to_owned(),
        path: rel.to_owned(),
        fp,
        format: path.extension().and_then(|x| x.to_str()).unwrap_or_default().to_ascii_lowercase(),
        duration_ms: props.duration().as_millis() as u64,
        sample_rate: props.sample_rate().unwrap_or(0),
        bit_depth: props.bit_depth().unwrap_or(0),
        channels: props.channels().unwrap_or(0),
        title: tag.and_then(|t| t.title()).map(|s| s.trim().to_owned()).filter(|s| !s.is_empty()).unwrap_or(stem),
        artist: tag.and_then(|t| t.artist()).map(|s| s.trim().to_owned()).unwrap_or_default(),
        album: tag.and_then(|t| t.album()).map(|s| s.trim().to_owned()).unwrap_or_default(),
        album_artist: text(lofty::tag::ItemKey::AlbumArtist),
        disc: tag.and_then(|t| t.disk()).unwrap_or(0),
        track: tag.and_then(|t| t.track()).unwrap_or(0),
        year: tag.and_then(|t| t.date()).map(|d| d.year).unwrap_or(0),
        genre: tag.and_then(|t| t.genre()).map(|s| s.trim().to_owned()).unwrap_or_default(),
        replay_gain: ReplayGain {
            track_gain: gain(lofty::tag::ItemKey::ReplayGainTrackGain),
            track_peak: gain(lofty::tag::ItemKey::ReplayGainTrackPeak),
            album_gain: gain(lofty::tag::ItemKey::ReplayGainAlbumGain),
            album_peak: gain(lofty::tag::ItemKey::ReplayGainAlbumPeak),
        },
        art,
    })
}

/// "-6.52 dB" → -6.52
fn parse_gain(s: &str) -> Option<f32> {
    let s = s.trim().trim_end_matches(|c: char| c.is_alphabetic() || c.is_whitespace());
    s.parse().ok().filter(|v: &f32| v.is_finite())
}

fn folder_art(track: &Path) -> Option<Vec<u8>> {
    let dir = track.parent()?;
    let entries: Vec<_> = std::fs::read_dir(dir).ok()?.flatten().collect();
    FOLDER_ART.iter().find_map(|want| {
        entries.iter().find(|e| e.file_name().to_string_lossy().eq_ignore_ascii_case(want)).and_then(|e| std::fs::read(e.path()).ok())
    })
}

/// Scales cover art to a JPEG thumbnail stored by its hash; returns the hash.
fn make_thumbnail(bytes: &[u8], dir: &Path) -> Result<ArtHash> {
    let img = image::load_from_memory(bytes)?;
    let img = if img.width() > ART_SIZE || img.height() > ART_SIZE { img.thumbnail(ART_SIZE, ART_SIZE) } else { img };
    let mut out = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, 85).encode_image(&img.to_rgb8())?;
    let hash = *blake3::hash(&out).as_bytes();
    let path = dir.join("art").join(hex(&hash));
    if !path.exists() {
        write_atomic(&path, &out)?;
    }
    Ok(hash)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A minimal valid FLAC: STREAMINFO, a Vorbis comment, and `frames` of silence, so lofty
    /// can read it. Size grows with `frames`.
    pub(crate) fn write_flac(path: &Path, title: &str, frames: usize) {
        let mut f = Vec::new();
        f.extend_from_slice(b"fLaC");
        // STREAMINFO (not last): block sizes 4096, rate 44100, 2 ch, 16 bit, total samples.
        let mut si = vec![0x10, 0x00, 0x10, 0x00, 0, 0, 0, 0, 0, 0];
        let total = (frames * 4096) as u64;
        let packed: u64 = (44100u64 << 44) | (1u64 << 41) | (15u64 << 36) | total;
        si.extend_from_slice(&packed.to_be_bytes());
        si.extend_from_slice(&[0; 16]);
        f.push(0x00);
        f.extend_from_slice(&(si.len() as u32).to_be_bytes()[1..]);
        f.extend_from_slice(&si);
        let mut vc = Vec::new();
        let vendor = b"test";
        vc.extend_from_slice(&(vendor.len() as u32).to_le_bytes());
        vc.extend_from_slice(vendor);
        let comments = [format!("TITLE={title}"), "ARTIST=Tester".into(), "ALBUM=Album".into(), "TRACKNUMBER=1".into()];
        vc.extend_from_slice(&(comments.len() as u32).to_le_bytes());
        for c in &comments {
            vc.extend_from_slice(&(c.len() as u32).to_le_bytes());
            vc.extend_from_slice(c.as_bytes());
        }
        f.push(0x84); // last block, VORBIS_COMMENT
        f.extend_from_slice(&(vc.len() as u32).to_be_bytes()[1..]);
        f.extend_from_slice(&vc);
        // Payload: arbitrary bytes standing in for frames (tags and properties come from headers).
        f.extend((0..frames * 1000).map(|i| (i * 31 % 251) as u8));
        std::fs::write(path, f).unwrap();
    }

    fn lib(dir: &Path, music: &Path) -> Library {
        Library::open(dir.to_owned(), vec![Root { volume: "external_primary".into(), path: music.to_owned() }]).unwrap()
    }

    #[test]
    fn unchanged_rescan_keeps_the_snapshot() {
        let (d, m) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        write_flac(&m.path().join("a.flac"), "A", 50);
        std::fs::create_dir(m.path().join("Album")).unwrap();
        write_flac(&m.path().join("Album/b.flac"), "B", 50);
        std::fs::write(m.path().join("Album/notes.txt"), "x").unwrap();
        let l = lib(d.path(), m.path());
        let r = l.scan().unwrap();
        assert_eq!((r.tracks, r.added), (2, 2));
        let (hash, _) = l.snapshot();
        let generation = l.status().generation;
        let r = l.scan().unwrap();
        assert_eq!((r.added, r.changed, r.removed), (0, 0, 0));
        assert_eq!(l.snapshot().0, hash);
        assert_eq!(l.status().generation, generation);
        let t = &l.index().tracks[0];
        assert_eq!((t.title.as_str(), t.artist.as_str(), t.sample_rate, t.channels), ("B", "Tester", 44100, 2));
        // Reopening reads the saved index.
        drop(l);
        assert_eq!(lib(d.path(), m.path()).snapshot().0, hash);
    }

    #[test]
    fn same_path_on_two_volumes_is_two_ids() {
        assert_ne!(track_id("external_primary", "Music/a.flac"), track_id("1234-ABCD", "Music/a.flac"));
    }

    #[cfg(unix)]
    #[test]
    fn edits_give_new_revs_and_moves_give_aliases() {
        let (d, m) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let a = m.path().join("a.flac");
        write_flac(&a, "A", 200);
        let l = lib(d.path(), m.path());
        l.scan().unwrap();
        let first = l.index().tracks[0].clone();

        // Same-size edit in the middle, mtime restored: ctime still changes.
        std::thread::sleep(std::time::Duration::from_millis(20));
        let mtime = std::fs::metadata(&a).unwrap().modified().unwrap();
        {
            use std::os::unix::fs::FileExt;
            let f = std::fs::OpenOptions::new().write(true).open(&a).unwrap();
            f.write_all_at(b"XXXX", 100_000).unwrap();
            f.set_modified(mtime).unwrap();
        }
        let r = l.scan().unwrap();
        assert_eq!(r.changed, 1);
        let edited = l.index().tracks[0].clone();
        assert_eq!(edited.id, first.id);
        assert_ne!(edited.rev, first.rev);

        // Moved with the same bytes → alias; moved back → alias reversed; deleted → nothing current.
        std::fs::create_dir(m.path().join("New")).unwrap();
        let b = m.path().join("New/a.flac");
        std::fs::rename(&a, &b).unwrap();
        l.scan().unwrap();
        let moved = l.index().tracks[0].clone();
        assert_eq!(l.index().aliases, vec![(first.id, moved.id)]);
        assert_eq!(l.index().resolve(&first.id).unwrap().id, moved.id);
        std::fs::rename(&b, &a).unwrap();
        l.scan().unwrap();
        assert_eq!(l.index().aliases, vec![(moved.id, first.id)], "no current id is ever an alias key");
        assert_eq!(l.index().resolve(&moved.id).unwrap().id, first.id);
        std::fs::remove_file(&a).unwrap();
        l.scan().unwrap();
        assert!(l.index().resolve(&first.id).is_none() && l.index().resolve(&moved.id).is_none());
    }

    #[test]
    fn a_move_with_a_duplicate_gives_no_alias() {
        let (d, m) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let a = m.path().join("a.flac");
        write_flac(&a, "A", 50);
        let l = lib(d.path(), m.path());
        l.scan().unwrap();
        std::fs::copy(&a, m.path().join("copy.flac")).unwrap();
        std::fs::rename(&a, m.path().join("b.flac")).unwrap();
        let r = l.scan().unwrap();
        assert_eq!(r.moved, 0);
        assert!(l.index().aliases.is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn serving_checks_the_revision() {
        let (d, m) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let a = m.path().join("a.flac");
        write_flac(&a, "A", 300);
        let l = lib(d.path(), m.path());
        l.scan().unwrap();
        let t = l.index().tracks[0].clone();
        let served = l.open_for_serving(&t.id, t.rev).unwrap();
        let mut buf = vec![0; 4096];
        served.read_block(8192, &mut buf).unwrap();
        assert_eq!(buf, std::fs::read(&a).unwrap()[8192..8192 + 4096]);
        assert!(matches!(l.open_for_serving(&t.id, t.rev + 1), Err(ServeError::Changed { .. })));
        // Edited while open: the next block is refused.
        std::thread::sleep(std::time::Duration::from_millis(20));
        {
            use std::os::unix::fs::FileExt;
            std::fs::OpenOptions::new().write(true).open(&a).unwrap().write_all_at(b"Y", 50_000).unwrap();
        }
        assert!(matches!(served.read_block(0, &mut buf), Err(ServeError::Changed { .. })));
        // Replaced by rename: a cached handle isn't reused.
        let other = m.path().join("other.flac");
        write_flac(&other, "Other", 300);
        l.scan().unwrap();
        let t = l.index().tracks.iter().find(|t| t.path == "a.flac").unwrap().clone();
        l.open_for_serving(&t.id, t.rev).unwrap();
        std::fs::rename(&other, &a).unwrap();
        assert!(matches!(l.open_for_serving(&t.id, t.rev), Err(ServeError::Changed { .. })));
        assert!(matches!(l.open_for_serving(&[9; 16], 1), Err(ServeError::Missing)));
    }

    #[test]
    fn references_resolve_by_id_alias_or_a_unique_fallback() {
        let (d, m) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        write_flac(&m.path().join("a.flac"), "Song", 50);
        let l = lib(d.path(), m.path());
        l.scan().unwrap();
        let r = l.index().tracks[0].reference();
        assert_eq!(l.index().resolve_ref(&r).unwrap().title, "Song");
        // Re-ripped under another path with other bytes: found by its tags.
        std::fs::remove_file(m.path().join("a.flac")).unwrap();
        write_flac(&m.path().join("b.flac"), "Song", 60);
        l.scan().unwrap();
        assert!(l.index().aliases.is_empty(), "different bytes: no alias");
        assert_eq!(l.index().resolve_ref(&r).unwrap().path, "b.flac");
        // Two matching tracks: no guess.
        write_flac(&m.path().join("c.flac"), "Song", 60);
        l.scan().unwrap();
        assert!(l.index().resolve_ref(&r).is_none());
    }

    #[test]
    fn gains_parse() {
        assert_eq!(parse_gain("-6.52 dB"), Some(-6.52));
        assert_eq!(parse_gain("0.988"), Some(0.988));
        assert_eq!(parse_gain(""), None);
    }
}
