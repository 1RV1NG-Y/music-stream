//! The stream protocol (PLAN.md §5): the device that owns the library serves its index,
//! cover art and verified byte ranges of tracks; other devices request them.

use std::{sync::Arc, time::Duration};

use anyhow::{Context, Result, anyhow, bail};
use iroh::endpoint::{Connection, RecvStream, SendStream};

use super::{Event, Node};
use crate::{
    library::{Library, LibraryStatus, ScanReport, ServeError},
    model,
    proto::{self, BLOCK, StreamReq, StreamResp},
};

const MAX_INDEX: u64 = 256 * 1024 * 1024;
const MAX_ART: u64 = 16 * 1024 * 1024;
/// How often the library is rescanned without a reason (change notifications come later).
const RESCAN_INTERVAL: Duration = Duration::from_secs(5 * 60);

/// What an index request got.
pub enum IndexReply {
    Same,
    Snapshot { hash: [u8; 32], bytes: Vec<u8> },
}

/// One event of a `Read` (PLAN.md §5).
#[derive(Debug)]
pub enum ReadEvent {
    Block { offset: u64, bytes: Vec<u8> },
    End,
    Changed { rev: u64, size: u64 },
    Missing,
}

/// The responses to one `Read`, as they arrive.
pub struct ReadStream {
    recv: RecvStream,
    _send: SendStream,
}

impl ReadStream {
    pub async fn next(&mut self) -> Result<ReadEvent> {
        match proto::read_msg::<_, StreamResp>(&mut self.recv).await? {
            Some(StreamResp::Block { offset, len }) => {
                if len as u64 > BLOCK {
                    bail!("block too large");
                }
                let mut bytes = vec![0; len as usize];
                self.recv.read_exact(&mut bytes).await?;
                Ok(ReadEvent::Block { offset, bytes })
            }
            Some(StreamResp::End) | None => Ok(ReadEvent::End),
            Some(StreamResp::Changed { rev, size }) => Ok(ReadEvent::Changed { rev, size }),
            Some(StreamResp::Missing) => Ok(ReadEvent::Missing),
            Some(StreamResp::Refused { reason }) => bail!(reason),
            Some(other) => bail!("unexpected answer {other:?}"),
        }
    }
}

impl Node {
    /// Makes this device serve `library` to its paired devices, and keeps it rescanned.
    pub fn serve_library(&self, library: Arc<Library>) {
        if self.0.library.set(library).is_ok() {
            tokio::spawn(self.clone().library_loop());
        }
    }

    /// The library this device serves (the phone's own), if any.
    pub fn served_library(&self) -> Option<Arc<Library>> {
        self.0.library.get().cloned()
    }

    pub fn library_status(&self) -> Option<LibraryStatus> {
        self.0.library.get().map(|l| l.status())
    }

    /// Rescans the served library now (blocking work runs off the async threads).
    pub async fn rescan(&self) -> Result<ScanReport> {
        let library = self.0.library.get().cloned().context("This device doesn't serve a library.")?;
        let _ = self.0.events.send(Event::StatusChanged);
        let lib = library.clone();
        let report = tokio::task::spawn_blocking(move || lib.scan()).await?;
        // Tell the other devices, through the replicated document, that there's a new index.
        let head = crate::library::hex(&library.snapshot().0);
        if model::read_library_head(&self.0.state.lock().unwrap().doc).as_deref() != Some(head.as_str()) {
            if let Err(e) = self.mutate(move |doc| model::write_library_head(doc, &head).map(|_| ())).await {
                tracing::warn!("could not publish the library head: {e:#}");
            }
        }
        let _ = self.0.events.send(Event::StatusChanged);
        let _ = self.0.events.send(Event::LibraryChanged);
        report
    }

    async fn library_loop(self) {
        let mut closing = self.0.closing.subscribe();
        let mut last_scan: Option<tokio::time::Instant> = None;
        loop {
            if last_scan.is_none_or(|t| t.elapsed() >= RESCAN_INTERVAL) {
                last_scan = Some(tokio::time::Instant::now());
                if let Err(e) = self.rescan().await {
                    tracing::warn!("library scan failed: {e:#}");
                }
            }
            if let Some(library) = self.0.library.get() {
                library.close_idle();
            }
            tokio::select! {
                _ = tokio::time::sleep(Duration::from_secs(30)) => {}
                _ = self.0.rescan_now.notified() => last_scan = None,
                _ = closing.wait_for(|c| *c) => return,
            }
        }
    }

    /// Answers one request stream from a paired device.
    pub(super) async fn serve_stream(&self, mut send: SendStream, mut recv: RecvStream) -> Result<()> {
        let req = proto::read_msg::<_, StreamReq>(&mut recv).await?.context("empty request")?;
        let Some(library) = self.0.library.get().cloned() else {
            proto::write_msg(&mut send, &StreamResp::Refused { reason: "This device doesn't serve a library.".into() }).await?;
            send.finish()?;
            return Ok(());
        };
        match req {
            StreamReq::Index { have } => {
                let (hash, bytes) = library.snapshot();
                if have == Some(hash) {
                    proto::write_msg(&mut send, &StreamResp::Same).await?;
                } else {
                    proto::write_msg(&mut send, &StreamResp::Snapshot { hash, len: bytes.len() as u64 }).await?;
                    send.write_all(&bytes).await?;
                }
            }
            StreamReq::Art { hash } => match tokio::fs::read(library.art_path(&hash)).await {
                Ok(bytes) => {
                    proto::write_msg(&mut send, &StreamResp::Have { len: bytes.len() as u64 }).await?;
                    send.write_all(&bytes).await?;
                }
                Err(_) => proto::write_msg(&mut send, &StreamResp::Missing).await?,
            },
            StreamReq::Read { track, rev, start, end } => self.serve_read(&library, &mut send, track, rev, start, end).await?,
        }
        send.finish()?;
        let _ = tokio::time::timeout(Duration::from_secs(5), send.stopped()).await;
        Ok(())
    }

    async fn serve_read(&self, library: &Arc<Library>, send: &mut SendStream, track: [u8; 16], rev: u64, start: u64, end: u64) -> Result<()> {
        let lib = library.clone();
        let opened = tokio::task::spawn_blocking(move || lib.open_for_serving(&track, rev)).await?;
        let served = match opened {
            Ok(served) => Arc::new(served),
            Err(e) => return self.refuse_read(send, e).await,
        };
        let size = served.fp.size;
        if start % BLOCK != 0 || start > end || end > size {
            proto::write_msg(send, &StreamResp::Refused { reason: "range out of bounds".into() }).await?;
            return Ok(());
        }
        let mut offset = start;
        while offset < end {
            let len = (end - offset).min(BLOCK) as usize;
            let s = served.clone();
            let block = tokio::task::spawn_blocking(move || {
                let mut buf = vec![0; len];
                s.read_block(offset, &mut buf).map(|()| buf)
            })
            .await?;
            match block {
                Ok(bytes) => {
                    proto::write_msg(send, &StreamResp::Block { offset, len: len as u32 }).await?;
                    send.write_all(&bytes).await?;
                }
                Err(e) => return self.refuse_read(send, e).await,
            }
            offset += len as u64;
        }
        proto::write_msg(send, &StreamResp::End).await
    }

    async fn refuse_read(&self, send: &mut SendStream, e: ServeError) -> Result<()> {
        let resp = match e {
            ServeError::Changed { rev, size } => {
                // The index is out of date: rescan so the requester can catch up.
                self.0.rescan_now.notify_one();
                StreamResp::Changed { rev, size }
            }
            ServeError::Missing => {
                self.0.rescan_now.notify_one();
                StreamResp::Missing
            }
        };
        proto::write_msg(send, &resp).await
    }

    // ---- Requesting ------------------------------------------------------------------------

    /// The connected device that serves the library: the phone, else any connected device.
    pub fn library_peer(&self) -> Option<(String, Connection)> {
        let st = self.0.state.lock().unwrap();
        let devices = model::read_devices(&st.doc);
        let sessions = self.0.sessions.lock().unwrap();
        let mut best: Option<(bool, String, Connection)> = None;
        for (id, s) in sessions.iter() {
            let Some((_, record)) = devices.iter().find(|(d, _)| *d == id.to_string()) else { continue };
            let phone = record.platform == "android";
            if best.as_ref().is_none_or(|(p, _, _)| phone && !p) {
                best = Some((phone, record.name.clone(), s.conn.clone()));
            }
        }
        best.map(|(_, name, conn)| (name, conn))
    }

    async fn request(&self, req: &StreamReq) -> Result<(SendStream, RecvStream)> {
        let (_, conn) = self.library_peer().ok_or_else(|| anyhow!("The phone isn't connected."))?;
        let (mut send, recv) = conn.open_bi().await?;
        proto::write_msg(&mut send, req).await?;
        Ok((send, recv))
    }

    pub async fn fetch_index(&self, have: Option<[u8; 32]>) -> Result<IndexReply> {
        let (_send, mut recv) = self.request(&StreamReq::Index { have }).await?;
        match proto::read_msg::<_, StreamResp>(&mut recv).await?.context("no answer")? {
            StreamResp::Same => Ok(IndexReply::Same),
            StreamResp::Snapshot { hash, len } => {
                if len > MAX_INDEX {
                    bail!("index too large");
                }
                let mut bytes = vec![0; len as usize];
                recv.read_exact(&mut bytes).await?;
                Ok(IndexReply::Snapshot { hash, bytes })
            }
            StreamResp::Refused { reason } => bail!(reason),
            other => bail!("unexpected answer {other:?}"),
        }
    }

    pub async fn fetch_art(&self, hash: [u8; 32]) -> Result<Option<Vec<u8>>> {
        let (_send, mut recv) = self.request(&StreamReq::Art { hash }).await?;
        match proto::read_msg::<_, StreamResp>(&mut recv).await?.context("no answer")? {
            StreamResp::Have { len } if len <= MAX_ART => {
                let mut bytes = vec![0; len as usize];
                recv.read_exact(&mut bytes).await?;
                Ok(Some(bytes))
            }
            StreamResp::Missing => Ok(None),
            StreamResp::Refused { reason } => bail!(reason),
            other => bail!("unexpected answer {other:?}"),
        }
    }

    /// Starts reading `start..end` of a track at `rev`. Dropping the stream cancels it.
    pub async fn read_range(&self, track: [u8; 16], rev: u64, start: u64, end: u64) -> Result<ReadStream> {
        let (send, recv) = self.request(&StreamReq::Read { track, rev, start, end }).await?;
        Ok(ReadStream { recv, _send: send })
    }
}
