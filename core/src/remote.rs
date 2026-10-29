//! The PC's view of the phone's library (PLAN.md §1, §6): the index kept on disk so browsing
//! never waits for the phone, cover art, and tracks fetched on demand. A track being played
//! is fetched into memory from the playhead to the end; once complete it is written to the
//! track cache, which keeps whole files within a size limit (U1; the segment-file cache of
//! PLAN.md §6 replaces this in Track B).

use std::{
    collections::{HashMap, HashSet},
    io::{Read, Seek, SeekFrom},
    path::PathBuf,
    sync::{
        Arc, Condvar, Mutex, Weak,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::{Duration, SystemTime},
};

use anyhow::{Context, Result};
use serde::Serialize;
use tokio::sync::{Notify, broadcast};

use crate::{
    Event, IndexReply, Node, ReadEvent,
    library::{ArtHash, Index, Track, TrackId, hex},
    proto::BLOCK,
    store::write_atomic,
};

/// The error message of a read released by the reader's interrupt flag.
pub const INTERRUPTED: &str = "interrupted";

pub const DEFAULT_CACHE_LIMIT: u64 = 4 * 1024 * 1024 * 1024;
const REFRESH_INTERVAL: Duration = Duration::from_secs(30);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum RemoteEvent {
    IndexChanged,
    CacheChanged,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoteStatus {
    /// The device serving the library, when connected.
    pub source: Option<String>,
    pub tracks: usize,
    pub generation: u64,
    pub error: Option<String>,
    pub cache_bytes: u64,
    pub cache_limit: u64,
}

struct Cached {
    hash: [u8; 32],
    index: Arc<Index>,
}

struct Inner {
    node: Node,
    dir: PathBuf,
    runtime: tokio::runtime::Handle,
    index: Mutex<Option<Cached>>,
    events: broadcast::Sender<RemoteEvent>,
    fetches: Mutex<HashMap<(TrackId, u64), Weak<Fetch>>>,
    /// Prefetches kept alive until they finish.
    keep: Mutex<Vec<Arc<Fetch>>>,
    cache_limit: AtomicU64,
    refresh: Notify,
    error: Mutex<Option<String>>,
}

#[derive(Clone)]
pub struct Remote(Arc<Inner>);

impl Remote {
    /// Must be called inside a Tokio runtime; `dir` holds the index, art and track cache.
    pub fn open(node: Node, dir: PathBuf) -> Result<Self> {
        std::fs::create_dir_all(dir.join("art"))?;
        std::fs::create_dir_all(dir.join("tracks"))?;
        let index = match (std::fs::read(dir.join("index.bin")), std::fs::read(dir.join("index.hash"))) {
            (Ok(bytes), Ok(hash)) => match (Index::decode(&bytes), <[u8; 32]>::try_from(hash.as_slice())) {
                (Ok(index), Ok(hash)) => Some(Cached { hash, index: Arc::new(index) }),
                _ => None,
            },
            _ => None,
        };
        let remote = Remote(Arc::new(Inner {
            node,
            dir,
            runtime: tokio::runtime::Handle::current(),
            index: Mutex::new(index),
            events: broadcast::channel(64).0,
            fetches: Mutex::default(),
            keep: Mutex::default(),
            cache_limit: AtomicU64::new(DEFAULT_CACHE_LIMIT),
            refresh: Notify::new(),
            error: Mutex::new(None),
        }));
        tokio::spawn(remote.clone().refresh_loop());
        Ok(remote)
    }

    pub fn subscribe(&self) -> broadcast::Receiver<RemoteEvent> {
        self.0.events.subscribe()
    }

    pub fn index(&self) -> Arc<Index> {
        self.0.index.lock().unwrap().as_ref().map(|c| c.index.clone()).unwrap_or_default()
    }

    pub fn status(&self) -> RemoteStatus {
        let index = self.index();
        RemoteStatus {
            source: self.0.node.library_peer().map(|(name, _)| name),
            tracks: index.tracks.len(),
            generation: index.generation,
            error: self.0.error.lock().unwrap().clone(),
            cache_bytes: self.cache_files().iter().map(|(_, _, size, _)| size).sum(),
            cache_limit: self.0.cache_limit.load(Ordering::Relaxed),
        }
    }

    pub fn set_cache_limit(&self, bytes: u64) {
        self.0.cache_limit.store(bytes, Ordering::Relaxed);
        self.evict();
    }

    /// Asks the phone for a newer index now.
    pub fn refresh(&self) {
        self.0.refresh.notify_one();
    }

    async fn refresh_loop(self) {
        let mut events = self.0.node.subscribe();
        loop {
            if self.0.node.library_peer().is_some() {
                match self.refresh_once().await {
                    Ok(()) => *self.0.error.lock().unwrap() = None,
                    Err(e) => {
                        tracing::debug!("index refresh: {e:#}");
                        *self.0.error.lock().unwrap() = Some(format!("{e:#}"));
                    }
                }
            }
            tokio::select! {
                _ = tokio::time::sleep(REFRESH_INTERVAL) => {}
                _ = self.0.refresh.notified() => {}
                e = events.recv() => match e {
                    Ok(Event::StatusChanged | Event::LibraryChanged) | Err(broadcast::error::RecvError::Lagged(_)) => {
                        // Coalesce the burst of status events a new connection raises.
                        tokio::time::sleep(Duration::from_millis(300)).await;
                    }
                    Err(broadcast::error::RecvError::Closed) => return,
                },
            }
        }
    }

    async fn refresh_once(&self) -> Result<()> {
        let have = self.0.index.lock().unwrap().as_ref().map(|c| c.hash);
        match self.0.node.fetch_index(have).await? {
            IndexReply::Same => Ok(()),
            IndexReply::Snapshot { hash, bytes } => {
                let index = Index::decode(&bytes)?;
                write_atomic(&self.0.dir.join("index.bin"), &bytes)?;
                write_atomic(&self.0.dir.join("index.hash"), &hash)?;
                tracing::info!(tracks = index.tracks.len(), generation = index.generation, "library index updated");
                *self.0.index.lock().unwrap() = Some(Cached { hash, index: Arc::new(index) });
                self.drop_stale();
                let _ = self.0.events.send(RemoteEvent::IndexChanged);
                Ok(())
            }
        }
    }

    // ---- Art -----------------------------------------------------------------------------

    pub async fn art(&self, hash: ArtHash) -> Result<Option<Vec<u8>>> {
        let path = self.0.dir.join("art").join(hex(&hash));
        if let Ok(bytes) = tokio::fs::read(&path).await {
            return Ok(Some(bytes));
        }
        let bytes = self.0.node.fetch_art(hash).await?;
        if let Some(bytes) = &bytes {
            if *blake3::hash(bytes).as_bytes() == hash {
                write_atomic(&path, bytes)?;
            }
        }
        Ok(bytes)
    }

    // ---- Tracks ----------------------------------------------------------------------------

    fn cache_path(&self, id: &TrackId, rev: u64) -> PathBuf {
        self.0.dir.join("tracks").join(format!("{}-{rev:016x}", hex(id)))
    }

    /// Tracks complete in the cache, as (id, rev).
    pub fn cached(&self) -> HashSet<(TrackId, u64)> {
        self.cache_files().into_iter().map(|(id, rev, _, _)| (id, rev)).collect()
    }

    fn cache_files(&self) -> Vec<(TrackId, u64, u64, SystemTime)> {
        let Ok(entries) = std::fs::read_dir(self.0.dir.join("tracks")) else { return vec![] };
        entries
            .flatten()
            .filter_map(|e| {
                let name = e.file_name().into_string().ok()?;
                let (id, rev) = name.split_once('-')?;
                let id = crate::library::parse_hex::<16>(id).ok()?;
                let rev = u64::from_str_radix(rev, 16).ok()?;
                let meta = e.metadata().ok()?;
                Some((id, rev, meta.len(), meta.modified().ok()?))
            })
            .collect()
    }

    /// Deletes cached tracks the index no longer has at that rev.
    fn drop_stale(&self) {
        let index = self.index();
        let current: HashSet<(TrackId, u64)> = index.tracks.iter().map(|t| (t.id, t.rev)).collect();
        for (id, rev, _, _) in self.cache_files() {
            if !current.contains(&(id, rev)) {
                let _ = std::fs::remove_file(self.cache_path(&id, rev));
            }
        }
    }

    /// Deletes the least recently used tracks beyond the limit.
    fn evict(&self) {
        let limit = self.0.cache_limit.load(Ordering::Relaxed);
        let mut files = self.cache_files();
        let mut total: u64 = files.iter().map(|f| f.2).sum();
        files.sort_by_key(|f| f.3);
        for (id, rev, size, _) in files {
            if total <= limit {
                break;
            }
            if std::fs::remove_file(self.cache_path(&id, rev)).is_ok() {
                total -= size;
            }
        }
        let _ = self.0.events.send(RemoteEvent::CacheChanged);
    }

    /// Opens a track for reading: from the cache if it's complete there, otherwise fetched
    /// from the phone as it's read. Reads wait for missing bytes, unless `interrupt` is set.
    pub fn open_track(&self, track: &Track, interrupt: Arc<AtomicBool>) -> Result<TrackSource> {
        let path = self.cache_path(&track.id, track.rev);
        if let Ok(file) = std::fs::File::open(&path) {
            if file.metadata().is_ok_and(|m| m.len() == track.size()) {
                // Recently used, for eviction.
                let _ = std::fs::File::options().write(true).open(&path).and_then(|f| f.set_modified(SystemTime::now()));
                return Ok(TrackSource { inner: SourceInner::File(file), pos: 0, len: track.size(), interrupt });
            }
        }
        Ok(TrackSource { inner: SourceInner::Fetch(self.fetch(track)), pos: 0, len: track.size(), interrupt })
    }

    /// Starts fetching a track that will be needed soon (the next in the queue).
    pub fn prefetch(&self, track: &Track) {
        if std::fs::metadata(self.cache_path(&track.id, track.rev)).is_ok_and(|m| m.len() == track.size()) {
            return;
        }
        let fetch = self.fetch(track);
        let mut keep = self.0.keep.lock().unwrap();
        keep.retain(|f| !f.state.lock().unwrap().finished());
        if !keep.iter().any(|f| Arc::ptr_eq(f, &fetch)) {
            keep.push(fetch);
        }
        while keep.len() > 2 {
            keep.remove(0);
        }
    }

    fn fetch(&self, track: &Track) -> Arc<Fetch> {
        let key = (track.id, track.rev);
        let mut fetches = self.0.fetches.lock().unwrap();
        fetches.retain(|_, f| f.strong_count() > 0);
        if let Some(f) = fetches.get(&key).and_then(Weak::upgrade) {
            return f;
        }
        let blocks = track.size().div_ceil(BLOCK) as usize;
        let fetch = Arc::new(Fetch {
            id: track.id,
            rev: track.rev,
            size: track.size(),
            state: Mutex::new(FetchState { data: vec![0; track.size() as usize], have: vec![false; blocks], missing: blocks, want: 0, want_gen: 0, error: None, done: false }),
            cond: Condvar::new(),
            wake: Notify::new(),
        });
        fetches.insert(key, Arc::downgrade(&fetch));
        self.0.runtime.spawn(self.clone().run_fetch(Arc::downgrade(&fetch)));
        fetch
    }

    async fn run_fetch(self, fetch: Weak<Fetch>) {
        let mut failures = 0u32;
        loop {
            let Some(f) = fetch.upgrade() else { return };
            let Some((start, end, asked)) = f.next_range() else {
                self.finish(&f).await;
                return;
            };
            let wake = f.wake.notified();
            let stream = self.0.node.read_range(f.id, f.rev, start * BLOCK, (end * BLOCK).min(f.size)).await;
            let mut stream = match stream {
                Ok(s) => s,
                Err(e) => {
                    // No phone right now: wait for it, or for a reader to ask again.
                    tracing::debug!("fetch: {e:#}");
                    tokio::select! {
                        _ = tokio::time::sleep(Duration::from_secs(1)) => {}
                        _ = wake => {}
                    }
                    continue;
                }
            };
            let mut next = start;
            loop {
                let wake = f.wake.notified();
                tokio::select! {
                    ev = stream.next() => match ev {
                        Ok(ReadEvent::Block { offset, bytes }) => {
                            failures = 0;
                            if f.store(offset, &bytes) {
                                // Complete: cache it now, even if every reader is already gone.
                                self.finish(&f).await;
                                return;
                            }
                            next = (offset + bytes.len() as u64).div_ceil(BLOCK);
                        }
                        Ok(ReadEvent::End) => break,
                        Ok(ReadEvent::Changed { .. }) => {
                            f.fail("The file changed on the phone.");
                            self.refresh();
                            return;
                        }
                        Ok(ReadEvent::Missing) => {
                            f.fail("The file is no longer on the phone.");
                            self.refresh();
                            return;
                        }
                        Err(e) => {
                            tracing::debug!("fetch interrupted: {e:#}");
                            failures += 1;
                            tokio::time::sleep(Duration::from_millis(200 * failures.min(10) as u64)).await;
                            break;
                        }
                    },
                    _ = wake => {}
                }
                // A reader wants a block this stream won't bring soon: start over from there.
                let (want, want_gen, have_want) = f.want();
                if want_gen != asked && !have_want && !(next <= want && want < end && want < next + 8) {
                    break;
                }
            }
        }
    }

    async fn finish(&self, f: &Arc<Fetch>) {
        let path = self.cache_path(&f.id, f.rev);
        let data = {
            let st = f.state.lock().unwrap();
            if st.done {
                return;
            }
            st.data.clone()
        };
        let written = tokio::task::spawn_blocking(move || write_atomic(&path, &data)).await;
        if !matches!(written, Ok(Ok(()))) {
            tracing::warn!("could not write a track to the cache: {written:?}");
        }
        f.state.lock().unwrap().done = true;
        self.evict();
    }
}

struct FetchState {
    data: Vec<u8>,
    have: Vec<bool>,
    missing: usize,
    /// The block a reader is waiting for, and how many times readers have asked.
    want: u64,
    want_gen: u64,
    error: Option<String>,
    done: bool,
}

impl FetchState {
    fn finished(&self) -> bool {
        self.done || self.error.is_some()
    }
}

struct Fetch {
    id: TrackId,
    rev: u64,
    size: u64,
    state: Mutex<FetchState>,
    cond: Condvar,
    wake: Notify,
}

impl Fetch {
    /// The next run of missing blocks, starting at the wanted block and wrapping around.
    fn next_range(&self) -> Option<(u64, u64, u64)> {
        let st = self.state.lock().unwrap();
        if st.missing == 0 || st.error.is_some() {
            return None;
        }
        let n = st.have.len() as u64;
        let first = (0..n).map(|i| (st.want + i) % n).find(|&b| !st.have[b as usize])?;
        let end = (first..n).find(|&b| st.have[b as usize]).unwrap_or(n);
        Some((first, end, st.want_gen))
    }

    fn want(&self) -> (u64, u64, bool) {
        let st = self.state.lock().unwrap();
        (st.want, st.want_gen, st.have.get(st.want as usize).copied().unwrap_or(true))
    }

    /// Stores a block; returns whether the track is now complete.
    fn store(&self, offset: u64, bytes: &[u8]) -> bool {
        if offset % BLOCK != 0 || offset + bytes.len() as u64 > self.size {
            return false;
        }
        let mut st = self.state.lock().unwrap();
        let b = (offset / BLOCK) as usize;
        if !st.have[b] {
            st.data[offset as usize..offset as usize + bytes.len()].copy_from_slice(bytes);
            st.have[b] = true;
            st.missing -= 1;
        }
        self.cond.notify_all();
        st.missing == 0
    }

    fn fail(&self, error: &str) {
        self.state.lock().unwrap().error = Some(error.into());
        self.cond.notify_all();
    }
}

enum SourceInner {
    File(std::fs::File),
    Fetch(Arc<Fetch>),
}

/// A track as a seekable byte source for the decoder.
pub struct TrackSource {
    inner: SourceInner,
    pos: u64,
    len: u64,
    interrupt: Arc<AtomicBool>,
}

impl TrackSource {
    pub fn len(&self) -> u64 {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }
}

impl Read for TrackSource {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        if self.pos >= self.len || buf.is_empty() {
            return Ok(0);
        }
        match &mut self.inner {
            SourceInner::File(file) => {
                let n = (buf.len() as u64).min(self.len - self.pos) as usize;
                crate::library::read_exact_at(file, &mut buf[..n], self.pos)?;
                self.pos += n as u64;
                Ok(n)
            }
            SourceInner::Fetch(f) => {
                let block = self.pos / BLOCK;
                let mut st = f.state.lock().unwrap();
                let mut asked = false;
                loop {
                    if st.have[block as usize] {
                        let block_end = ((block + 1) * BLOCK).min(self.len);
                        let n = (buf.len() as u64).min(block_end - self.pos) as usize;
                        buf[..n].copy_from_slice(&st.data[self.pos as usize..self.pos as usize + n]);
                        self.pos += n as u64;
                        return Ok(n);
                    }
                    if let Some(e) = &st.error {
                        return Err(std::io::Error::other(e.clone()));
                    }
                    if self.interrupt.load(Ordering::Acquire) {
                        // Not `ErrorKind::Interrupted`: readers retry that one.
                        return Err(std::io::Error::other(INTERRUPTED));
                    }
                    if !asked || st.want != block {
                        st.want = block;
                        st.want_gen += 1;
                        asked = true;
                        f.wake.notify_one();
                    }
                    st = f.cond.wait_timeout(st, Duration::from_millis(50)).unwrap().0;
                }
            }
        }
    }
}

impl Seek for TrackSource {
    fn seek(&mut self, pos: SeekFrom) -> std::io::Result<u64> {
        let next = match pos {
            SeekFrom::Start(p) => p as i128,
            SeekFrom::End(d) => self.len as i128 + d as i128,
            SeekFrom::Current(d) => self.pos as i128 + d as i128,
        };
        if next < 0 {
            return Err(std::io::ErrorKind::InvalidInput.into());
        }
        self.pos = next as u64;
        Ok(self.pos)
    }
}

/// Reads a whole track (tests and the localhost server).
pub fn read_all(source: &mut TrackSource) -> Result<Vec<u8>> {
    let mut out = Vec::with_capacity(source.len() as usize);
    source.read_to_end(&mut out).context("reading the track")?;
    Ok(out)
}
