//! Durable storage for a Loro document.
//!
//! Layout inside the data directory:
//! - `library.snapshot`: a full Loro snapshot, replaced atomically during compaction.
//! - `library.log`: an append-only log of Loro update blobs written since that snapshot.
//!
//! Each log record is `[len: u32 LE][crc32: u32 LE][payload]` and is fsynced before
//! [`DocStore::persist`] returns, so callers may acknowledge a change only after `persist`
//! succeeds. A torn record at the end of the log (a crash during an append) is dropped on
//! open; the original log is kept beside it as `library.log.damaged-<ms>` first.

use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result, bail};
use loro::{ExportMode, LoroDoc, VersionVector};

const LOG_MAGIC: &[u8; 8] = b"MUSTLOG1";
const SNAPSHOT_MAGIC: &[u8; 8] = b"MUSTSNP1";
const MAX_RECORD: usize = 256 * 1024 * 1024;
/// Compact into a fresh snapshot once the log grows past this size.
const COMPACT_AT: u64 = 4 * 1024 * 1024;

pub struct DocStore {
    dir: PathBuf,
    /// `None` after a failed compaction left no usable handle; reopened before the next append.
    log: Option<File>,
    log_len: u64,
    persisted: VersionVector,
    /// Test hook: make appends fail as if the disk were full.
    #[cfg(test)]
    pub(crate) fail_writes: bool,
}

impl DocStore {
    /// Loads the snapshot and log from `dir` into `doc` (which should be empty).
    pub fn open(dir: &Path, doc: &LoroDoc) -> Result<Self> {
        fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
        let snapshot = dir.join("library.snapshot");
        match fs::read(&snapshot) {
            Ok(bytes) => {
                let body = unwrap_snapshot(&bytes).context("library.snapshot is damaged")?;
                doc.import(body).context("library.snapshot could not be loaded")?;
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e).context("reading library.snapshot"),
        }

        let log_path = dir.join("library.log");
        let (records, valid_len, total_len) = read_log(&log_path)?;
        for record in &records {
            doc.import(record).context("library.log contains an update that could not be loaded")?;
        }
        if valid_len < total_len {
            let damaged = dir.join(format!("library.log.damaged-{}", now_ms()));
            fs::copy(&log_path, &damaged).context("preserving damaged log")?;
            tracing::warn!(kept = %damaged.display(), valid_len, total_len, "dropping torn log tail");
        }
        let mut log = OpenOptions::new().read(true).write(true).create(true).truncate(false).open(&log_path)?;
        if total_len == 0 {
            log.write_all(LOG_MAGIC)?;
            log.sync_all()?;
            sync_dir(dir)?;
        } else if valid_len < total_len {
            log.set_len(valid_len)?;
            log.sync_all()?;
        }
        let log_len = valid_len.max(LOG_MAGIC.len() as u64);
        let mut store = Self {
            dir: dir.to_owned(),
            log: Some(log),
            log_len,
            persisted: doc.oplog_vv(),
            #[cfg(test)]
            fail_writes: false,
        };
        if store.log_len > COMPACT_AT {
            if let Err(e) = store.compact(doc) {
                tracing::warn!("compaction failed; continuing with the existing log: {e:#}");
            }
        }
        Ok(store)
    }

    /// Version known to be durable on disk.
    pub fn persisted(&self) -> &VersionVector {
        &self.persisted
    }

    /// Durably appends every change in `doc` that is not yet on disk.
    /// Returns `false` when there was nothing to write.
    pub fn persist(&mut self, doc: &LoroDoc) -> Result<bool> {
        let current = doc.oplog_vv();
        if self.persisted.includes_vv(&current) {
            return Ok(false);
        }
        let update = doc.export(ExportMode::updates(&self.persisted))?;
        self.append(&update)?;
        self.persisted = current;
        // The update is durable now; a failed compaction must not report the save as failed.
        if self.log_len > COMPACT_AT {
            if let Err(e) = self.compact(doc) {
                tracing::warn!("compaction failed; continuing with the existing log: {e:#}");
            }
        }
        Ok(true)
    }

    fn append(&mut self, payload: &[u8]) -> Result<()> {
        if payload.len() > MAX_RECORD {
            bail!("update of {} bytes exceeds the record limit", payload.len());
        }
        let mut record = Vec::with_capacity(payload.len() + 8);
        record.extend_from_slice(&(payload.len() as u32).to_le_bytes());
        record.extend_from_slice(&crc32fast::hash(payload).to_le_bytes());
        record.extend_from_slice(payload);
        #[cfg(test)]
        if self.fail_writes {
            bail!("simulated write failure");
        }
        if self.log.is_none() {
            self.reopen_log()?;
        }
        let log = self.log.as_mut().expect("reopened");
        // The file is opened without O_APPEND so we can truncate; position explicitly.
        use std::io::{Seek, SeekFrom};
        let result = log.seek(SeekFrom::Start(self.log_len)).and_then(|_| log.write_all(&record)).and_then(|_| log.sync_data());
        if let Err(e) = result {
            // Drop any partial write so the next append starts on a record boundary.
            if log.set_len(self.log_len).is_err() {
                self.log = None;
            }
            return Err(e).context("writing library.log");
        }
        self.log_len += record.len() as u64;
        Ok(())
    }

    /// Writes a fresh snapshot and starts an empty log. Whatever happens, appends then
    /// continue on the log that is actually on disk (the old one if replacement failed).
    pub fn compact(&mut self, doc: &LoroDoc) -> Result<()> {
        let result = self.replace_with_snapshot(doc);
        self.reopen_log()?;
        result?;
        self.persisted = doc.oplog_vv();
        Ok(())
    }

    /// Reopens `library.log` for appending and resumes at its current end.
    fn reopen_log(&mut self) -> Result<()> {
        self.log = None;
        let path = self.dir.join("library.log");
        let (_, valid_len, _) = read_log(&path)?;
        let mut log = OpenOptions::new().read(true).write(true).create(true).truncate(false).open(&path)?;
        if valid_len == 0 {
            log.set_len(0)?;
            log.write_all(LOG_MAGIC)?;
            log.sync_all()?;
        } else {
            log.set_len(valid_len)?;
        }
        self.log_len = valid_len.max(LOG_MAGIC.len() as u64);
        self.log = Some(log);
        Ok(())
    }

    fn replace_with_snapshot(&mut self, doc: &LoroDoc) -> Result<()> {
        let mut bytes = SNAPSHOT_MAGIC.to_vec();
        let body = doc.export(ExportMode::Snapshot)?;
        bytes.extend_from_slice(&crc32fast::hash(&body).to_le_bytes());
        bytes.extend_from_slice(&body);
        let snapshot = self.dir.join("library.snapshot");
        write_atomic(&snapshot, &bytes)?;
        // The snapshot now contains everything in the log; replaying a stale log
        // after a crash at this point is harmless because imports are idempotent.
        // Release the old log handle first: Windows cannot replace an open file.
        self.log = None;
        write_atomic(&self.dir.join("library.log"), LOG_MAGIC)
    }
}

fn unwrap_snapshot(bytes: &[u8]) -> Result<&[u8]> {
    if bytes.len() < 12 || &bytes[..8] != SNAPSHOT_MAGIC {
        bail!("not a music-stream snapshot");
    }
    let crc = u32::from_le_bytes(bytes[8..12].try_into().unwrap());
    let body = &bytes[12..];
    if crc32fast::hash(body) != crc {
        bail!("snapshot checksum mismatch");
    }
    Ok(body)
}

/// Returns the intact records, the byte length they occupy (including the header),
/// and the file's total length.
fn read_log(path: &Path) -> Result<(Vec<Vec<u8>>, u64, u64)> {
    let mut bytes = Vec::new();
    match File::open(path) {
        Ok(mut f) => {
            f.read_to_end(&mut bytes)?;
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok((vec![], 0, 0)),
        Err(e) => return Err(e).context("reading library.log"),
    }
    let total = bytes.len() as u64;
    if bytes.is_empty() {
        return Ok((vec![], 0, 0));
    }
    if bytes.len() < 8 || &bytes[..8] != LOG_MAGIC {
        bail!("library.log is not a music-stream log; it has been left untouched");
    }
    let mut records = Vec::new();
    let mut pos = 8usize;
    while pos + 8 <= bytes.len() {
        let len = u32::from_le_bytes(bytes[pos..pos + 4].try_into().unwrap()) as usize;
        let crc = u32::from_le_bytes(bytes[pos + 4..pos + 8].try_into().unwrap());
        let end = pos + 8 + len;
        if len > MAX_RECORD || end > bytes.len() || crc32fast::hash(&bytes[pos + 8..end]) != crc {
            break;
        }
        records.push(bytes[pos + 8..end].to_vec());
        pos = end;
    }
    Ok((records, pos as u64, total))
}

pub(crate) fn write_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    let dir = path.parent().context("path has no parent")?;
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(".tmp");
    let tmp = PathBuf::from(tmp);
    {
        let mut f = OpenOptions::new().write(true).create(true).truncate(true).open(&tmp)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            f.set_permissions(fs::Permissions::from_mode(0o600))?;
        }
        f.write_all(bytes)?;
        f.sync_all()?;
    }
    fs::rename(&tmp, path).with_context(|| format!("replacing {}", path.display()))?;
    sync_dir(dir)
}

fn sync_dir(dir: &Path) -> Result<()> {
    #[cfg(unix)]
    File::open(dir)?.sync_all()?;
    #[cfg(not(unix))]
    let _ = dir;
    Ok(())
}

pub(crate) fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn note(doc: &LoroDoc, key: &str, value: &str) {
        doc.get_map("notes").insert(key, value).unwrap();
        doc.commit();
    }

    #[test]
    fn reopen_restores_everything() {
        let dir = tempfile::tempdir().unwrap();
        let doc = LoroDoc::new();
        let mut store = DocStore::open(dir.path(), &doc).unwrap();
        note(&doc, "a", "1");
        assert!(store.persist(&doc).unwrap());
        note(&doc, "b", "2");
        store.persist(&doc).unwrap();
        assert!(!store.persist(&doc).unwrap());
        drop(store);

        let again = LoroDoc::new();
        DocStore::open(dir.path(), &again).unwrap();
        assert_eq!(again.get_deep_value(), doc.get_deep_value());
    }

    #[test]
    fn torn_tail_is_dropped_and_preserved() {
        let dir = tempfile::tempdir().unwrap();
        let doc = LoroDoc::new();
        let mut store = DocStore::open(dir.path(), &doc).unwrap();
        note(&doc, "a", "1");
        store.persist(&doc).unwrap();
        drop(store);
        // Simulate a crash halfway through the next append.
        let mut f = OpenOptions::new().append(true).open(dir.path().join("library.log")).unwrap();
        f.write_all(&[200, 0, 0, 0, 1, 2, 3, 4, 9, 9]).unwrap();
        drop(f);

        let again = LoroDoc::new();
        let mut store = DocStore::open(dir.path(), &again).unwrap();
        assert_eq!(again.get_deep_value(), doc.get_deep_value());
        assert!(fs::read_dir(dir.path()).unwrap().any(|e| e.unwrap().file_name().to_string_lossy().starts_with("library.log.damaged-")));
        // Appends continue cleanly after the truncation.
        note(&again, "b", "2");
        store.persist(&again).unwrap();
        drop(store);
        let third = LoroDoc::new();
        DocStore::open(dir.path(), &third).unwrap();
        assert_eq!(third.get_deep_value(), again.get_deep_value());
    }

    #[test]
    fn compaction_keeps_content() {
        let dir = tempfile::tempdir().unwrap();
        let doc = LoroDoc::new();
        let mut store = DocStore::open(dir.path(), &doc).unwrap();
        note(&doc, "a", "1");
        store.persist(&doc).unwrap();
        store.compact(&doc).unwrap();
        note(&doc, "b", "2");
        store.persist(&doc).unwrap();
        drop(store);
        let again = LoroDoc::new();
        DocStore::open(dir.path(), &again).unwrap();
        assert_eq!(again.get_deep_value(), doc.get_deep_value());
    }

    #[test]
    fn failed_compaction_keeps_saving() {
        let dir = tempfile::tempdir().unwrap();
        let doc = LoroDoc::new();
        let mut store = DocStore::open(dir.path(), &doc).unwrap();
        note(&doc, "a", "1");
        store.persist(&doc).unwrap();
        // Obstruct replacing the log after the snapshot is written.
        fs::create_dir(dir.path().join("library.log.tmp")).unwrap();
        assert!(store.compact(&doc).is_err());
        note(&doc, "b", "2");
        store.persist(&doc).expect("saves continue after a failed compaction");
        fs::remove_dir(dir.path().join("library.log.tmp")).unwrap();
        store.compact(&doc).unwrap();
        note(&doc, "c", "3");
        store.persist(&doc).unwrap();
        drop(store);
        let again = LoroDoc::new();
        DocStore::open(dir.path(), &again).unwrap();
        assert_eq!(again.get_deep_value(), doc.get_deep_value());
    }

    #[test]
    fn foreign_log_is_left_untouched() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("library.log"), b"something else").unwrap();
        assert!(DocStore::open(dir.path(), &LoroDoc::new()).is_err());
        assert_eq!(fs::read(dir.path().join("library.log")).unwrap(), b"something else");
    }
}
