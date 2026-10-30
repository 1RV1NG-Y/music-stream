# music-stream — plan

Play the music on my phone from my PC (Linux and Windows, dual-boot), streamed over the local
network, replacing BubbleUPnP + VLC. The library stays on the phone; the PC browses it
instantly and plays it without copying it over first. No accounts, servers or cloud.

Why replace BubbleUPnP + VLC: it is unreliable and I often have to wait for the phone to
"show up". UPnP finds the phone again on every use, by broadcasting a search (SSDP) and waiting
for answers. Android drops those packets when the screen is off, and VLC's UPnP browser is slow
to list what answers. This plan removes that step: once paired, the PC knows the phone's
identity and last addresses, and dials it directly.

Status: draft plan, 2026-10-29, revised the same day after Astra's first review (§12).
Nothing is built yet. Working name `music-stream`.
Development happens mainly on Linux; Windows is built and tested in CI, with hands-on checks
at milestones.

---

## 1. Shape: the phone is the server, the PC is a client

This is the opposite of bits and reader-sync, where every device holds a full copy of the
data. Here one device owns the files and the other plays them, which is the t3code model: one
environment owns the state, and clients reach into it.

- **Phone**: scans its music, publishes a library index, serves byte ranges of tracks, and
  stays reachable in the background.
- **PC**: keeps a cached copy of the index (so browsing never waits for the phone), plays
  tracks by fetching their bytes on demand, and caches recently played and upcoming tracks.

Files are never synced. A PC cache is a cache: evictable, never the only copy of anything.

Lessons taken from t3code (`docs/internals/remote.md`, `connection-runtime.md`):

- **The device's identity stays the same whatever route reaches it.** Here that is the phone's
  iroh key. Addresses are hints, and the key makes a stranger at an old address harmless.
- **Connection state and data freshness are separate.** "Phone offline" never hides the
  library: the cached index and cached tracks stay usable. The UI says which tracks can play
  now.
- **One owner for retries, and check that the link is alive.** Backoff with random spread.
  On window focus, network change or a play request, probe the link with a short deadline;
  replace a silent connection instead of waiting for the idle timeout.

## 2. What we start from: the reader-sync core

`1RV1NG-Y/reader-sync` at commit `72541e1` (2026-10-28). Its `core/` was forked from the
bits sync core, and its Android service is already verified on the S22.

| Piece | Where | Reused as is? |
|---|---|---|
| Device identity (iroh key per install), QUIC/TLS-authenticated peers | `core/src/node/mod.rs` | Yes |
| Pairing by QR / text code, single-use 10-min secret, join vs add codes | `node/mod.rs`, `proto.rs` | Yes, with new identifiers |
| mDNS discovery, replicated address book, reconnect with backoff | `node/mod.rs` | Yes, plus the liveness probe of §1 |
| Replicated Loro document, version-vector sync, forwarding through a middle device | `node/mod.rs` | Yes, for user data only (§4) |
| Durable store: checksummed fsynced log, compaction, torn-tail recovery, exclusive lock | `store.rs` | Yes, also for the PC cache's block maps (§6) |
| Android `connectedDevice` foreground service, CPU/Wi-Fi/multicast locks, network callbacks, node owned by the Android runtime | `app/src-tauri/src/android.rs`, `node_slot.rs`, `gen/android/.../LanSync*.kt` | Yes. This is the part that makes the phone a reliable server. Its battery cost is unmeasured; M0 measures it |
| Message framing, `FileMsg::Want { hash, offset }` streaming from an offset | `proto.rs`, `node/files.rs` | Basis for the stream protocol (§5); range reads need an end and revisions, so it is rewritten |
| Books, positions, holdings, file store, file sync, handoff status | `model.rs`, `positions.rs`, `files.rs`, `node/files.rs` | **No**, removed |
| RPC surface, events | `rpc.rs` | Rewritten |

**A focused copy, not a shared framework**, as reader-sync decided for bits: copy the core,
record the source commit, port fixes by hand.

**Separate identity from bits and reader-sync**, so the apps can never pair with each other
or fight over a port:

- ALPNs `music/sync/1` (index, user data and streaming share one connection) and `music/pair/1`
- Pairing-code prefix `MUSIC1` (bits `BITS2`, reader-sync `READ1`)
- UDP port `47120` (bits `47100`, reader-sync `47110`)
- Own data directory, `device.key` and app identifier (§10)

## 3. The library on the phone

### Finding the music

Android's MediaStore already indexes audio. Query it with `READ_MEDIA_AUDIO` (Android 13+;
`READ_EXTERNAL_STORAGE` before that). Each row gives the file's volume, path and MediaStore
id. Settings can limit the library to chosen folders (default: all music MediaStore knows).
MediaStore is used only to find files and to hear about changes; sizes and times come from
`fstat` on the file itself, because MediaStore's `DATE_MODIFIED` has one-second precision.

**Reading the bytes from Rust.** On Android 11+, apps with media permission can open media
files by their real path, through the system's FUSE layer, so the Rust node can open
`/storage/emulated/0/Music/...` directly. If that fails or is slow on the S22, Kotlin opens
the file with `ContentResolver.openFileDescriptor(uri, "r")` and hands the raw descriptor to
Rust (`detachFd()` → `File::from_raw_fd`). Checked in M0 (§8) before anything is built on it,
together with what `fstat` reports through FUSE (below).

### Tags and art

MediaStore's tag columns are incomplete for FLAC (album artist, disc numbers, ReplayGain,
multiple artists). The phone reads tags itself with `lofty`, from the Vorbis comments, only for
files whose revision changed since the last scan. Embedded or folder cover art is scaled to a
thumbnail (e.g. 512 px JPEG), stored content-addressed by BLAKE3, and fetched by the PC on
demand.

Rescans run when the app starts, when the service starts, and on MediaStore change
notifications (`ContentObserver`), debounced.

### Track identity

A track id must survive rescans, because playlists and favorites refer to it.

- **id = BLAKE3 of (volume name, path relative to that volume)**, truncated to 16 bytes. The
  volume name is MediaStore's `VOLUME_NAME` (`external_primary`, or an SD card's id), so the
  same relative path on two volumes gives two ids. Cheap, and stable while the file stays
  where it is.
- Not a hash of the file contents: hashing a large FLAC library on the phone takes minutes,
  and retagging would change every id.

### File revisions

The PC caches bytes, and must never mix bytes from two versions of a file. Size and mtime
alone can't promise that: a replacement can keep both, and a tagger can restore the old mtime
after writing into FLAC padding without changing the size. So every version of a file gets a
**revision**:

- **Fingerprint** (from `fstat`, not MediaStore): size; mtime and ctime in nanoseconds; inode;
  and a **sample hash**, BLAKE3 of the first and last 64 KiB. ctime can't be set from user
  space, so an in-place edit changes it even when the tool restores mtime; a replacement by
  rename changes the inode; the head sample covers the tags and, for FLAC, STREAMINFO's MD5
  of the decoded audio.
- **rev = BLAKE3 of the fingerprint**, truncated to 8 bytes. Any fingerprint change gives a
  new rev, including a bare `touch` (one re-download, which is fine). Deterministic, so
  reinstalling the app and rescanning an unchanged library gives the same revs, and the PC
  cache stays valid.
- Scans `fstat` every file and read the sample hash only for new files or files whose stat
  fields changed.
- **Checked again when serving** (§5): opening a file to serve it re-reads its fingerprint,
  and before each block is sent the open file's metadata is checked again. A scan that is out
  of date can't lead to stale bytes, *provided the metadata changes when the file does*.
- Accepted residual risk: two writes inside one kernel timestamp tick (a few ms) on either
  side of our open, with size, head and tail unchanged.

**The guarantee rests on the metadata, and M0 decides it** (M0 step 3). The sample hash only
covers the ends of the file, and the per-block check compares metadata, not content; neither
catches an edit in the middle that leaves the metadata alone. So:

- **Gate passes** (ctime changes on every write, the inode changes on rename-over, mtime has
  sub-second precision): the guarantee stands as above. A mid-file edit is caught by ctime.
  *First result, 2026-10-29, S22 on Android 16, through FUSE from `adb shell`: passes.*
  ctime moved on an in-place edit and on edit + `touch -r`; rename-over changed the inode;
  times are kept to a 4 ms tick (the residual-risk window above). Still to confirm from the
  app's own uid (M0 step 2).
- **Gate fails:** the guarantee is narrowed in writing before Track A starts, to: *changes
  are detected when they alter the size, the mtime as reported, or the first or last 64 KiB*.
  For FLAC that still covers retagging (tags are in the head) and changed audio (STREAMINFO's
  MD5); what escapes is a same-size, mtime-preserving edit confined to the middle, which no
  tagger or ripper we know of makes. Closing it fully would need a content hash of every
  block recorded on first serve and checked on every later one; that is the fallback if the
  narrowed guarantee turns out not to be enough, not the default.

### Moves, renames and references

Playlists and favorites store a **track reference**, `TrackRef { id, fallback }`, where
`fallback` is (album artist, album, disc, track number, title, duration rounded to the
second). A reference is resolved at read time, by a pure function of the reference and the
current index, so both PCs always agree and nothing is rewritten behind the user's back:

1. The id is a current track → that track. (If a new file appears at an old path, the path
   wins: the id names the path.)
2. Otherwise follow the index's **aliases**. When a scan sees ids disappear and new ids
   appear, and exactly one new track has the same size and sample hash as a vanished one, the
   phone records `old id → new id`. This covers moves, folder renames, and copy-then-delete,
   which keep the bytes. Two or more candidates (duplicate rips) → no alias. Aliases are kept
   forever (32 bytes each), normalized so they can't form chains or cycles:
   - **No alias key is ever a current id.** When an id becomes current again (a file moved
     back to its old path), its alias is deleted.
   - **Flattened.** Recording `old → new` rewrites every alias that pointed at `old` to point
     at `new`, so every alias goes one hop to an id that was current when it was written.
   - Resolution follows **at most one hop**. If the target is gone too, it falls through to
     step 3. The phone checks both invariants when it writes the index; the PC's resolver
     keeps the one-hop limit regardless, so a malformed index can't make it loop.
   - Example: `A` moved to `B` → `{A→B}`. Moved back to `A` → `A` is current, so `A→B` is
     deleted and `{B→A}` recorded. `A` deleted → a reference to `A` or `B` falls through to
     the fallback key, then to missing.
3. Otherwise the fallback key, **only if exactly one** current track matches it (a re-rip
   that changed bytes and path). Several matches (editions, duplicates) → no automatic match.
4. Otherwise the reference shows as missing, and the UI offers the fallback candidates. The
   user's pick writes the chosen id into the reference.

### The index is a snapshot, not a Loro document

Only the phone writes the index, so it needs no merging. Loro also keeps full history, so a
10,000-track index that changes with every rescan would make the document grow forever. So:

- The index is one serialized snapshot (postcard + zstd) with a `generation` and a content
  hash. Per track: id, volume, relative path, rev, size, duration, sample rate, bit depth,
  channels, tags, ReplayGain values, art hash. Plus the alias table.
- The PC asks `Index { have: <hash> }`. The phone answers `Same` or sends the full snapshot.
  At a few MB compressed this is fine on a LAN. Measure it with the real library before
  adding deltas.
- The PC keeps the last snapshot on disk and shows it immediately at startup.

## 4. User data: playlists, favorites, play history (Loro)

Playlists and favorites are written on both PCs, and those two are never on at the same time.
That is the reader-sync case exactly: a Loro document, with the phone forwarding changes
between them. The phone may also edit them later (§9).

```
playlists/<playlistId>     { name, created, entries: movable list of TrackRef }
favorites/<trackId>        { since, fallback }        (map; remove = unfavorite)
```

A favorite's key and `fallback` together are a `TrackRef`, resolved like a playlist entry
(§3). Unfavoriting removes every key that resolves to that track.

**Play history** (for the automatic playlists, §7) uses per-device counters, so it merges
without conflicts and stays bounded:

```
plays/<deviceId>/<trackId>   { count, last, recent: [day numbers of the last ≤ 32 plays], fallback }
```

Each PC writes only its own `deviceId`, one entry per track it has played; a play overwrites
that entry. A play counts once at least half the track, or 4 minutes, has been heard. Loro
still keeps each overwrite in its history (about 100 bytes a play, so roughly 1 MB for
10,000 plays); compacting old history is in §9.

## 5. Streaming protocol

Runs as extra QUIC streams on the established `music/sync/1` connection, one stream per
request, so a seek simply drops the old stream.

```
Read  { track: id, rev, start, end }   PC → phone   (start on a 256 KiB block boundary)
  ← Block { offset, len } + bytes     one per block, in order (the file's last block may be short)
  ← End
  ← Changed { rev, size }             the file is not at that rev; may follow some Blocks
  ← Missing                           no such track anymore
  ← Refused { reason }                unpaired or removed device, range out of bounds
Art   { hash }  ← Have { total } + bytes, or Missing
Index { have: hash }  ← Same, or Snapshot { generation } + bytes
```

- Only paired devices are served, as in reader-sync. The connection is authenticated by key.
  No decoding on the phone.
- **Revision checks.** Opening a file to serve a `Read` recomputes its fingerprint (one
  `fstat` and 128 KiB for the sample hash). A different rev → `Changed`, and the phone rescans
  that file. During the transfer, after reading each block the phone `fstat`s the handle and
  sends the block only if size, mtime and ctime are what they were when the handle was
  checked; otherwise it sends `Changed`. So every `Block` the PC receives is verified bytes of
  the rev it asked for.
- **File handles on the phone.** A small LRU of open handles (8, closed after 60 s idle),
  keyed by (id, rev) and shared between requests as `Arc<File>`. Reads are positional only
  (`pread`, Rust's `FileExt::read_at`), never `seek` + `read`, so playback and prefetch reading
  the same track at once never share a cursor (`File::try_clone` would still share one).
  Before a handle is reused, the phone `stat`s the path: an open descriptor keeps reading the
  old file after a rename-over, so a different inode, size or time drops the handle and the
  file is checked again. File reads run on blocking threads, not the async executor.
- **On the PC**, a block is written to the cache only once its whole frame has arrived, so a
  stream cut mid-frame leaves nothing behind. `Changed` drops every cached block and every
  in-flight request for the old rev; frames for an old rev that arrive late are discarded.
  The PC takes the new rev and size from the reply, refreshes the index, and the player
  reopens the track at the same time position (§6).
- Bandwidth: 16/44.1 FLAC is about 1 Mbit/s; 24/192 can reach 9 Mbit/s. Both are easy on
  home Wi-Fi.

## 6. The PC player

### Native playback, not the webview

Most of the library is FLAC, which pushes playback out of the webview: `<audio>` can't do
gapless, its FLAC support on Linux depends on the installed GStreamer plugins, and it stops
when the window's webview does. So:

- **Decode with `symphonia`** (pure-Rust FLAC, with seeking via SEEKTABLE or search).
- **Output with `cpal`** (WASAPI on Windows; ALSA on Linux, served by PipeWire).
- **ReplayGain** (track/album/off) from the tags in the index.
- **Volume** in software, in floating point.
- Playback runs in the Rust side of the Tauri app and continues with the window closed (tray
  icon). The UI only sends commands and receives state.
- **OS media controls:** Windows SMTC and Linux MPRIS through `souvlaki`, so media keys and the
  system's now-playing panel work.

### Audio pipeline and threads

```
cache source ──► decode thread ───────────────────────────────────► PCM ring ──► output callback
(blocks while    symphonia → ReplayGain → channel map → resampler     (bounded,    (cpal: volume,
 a block is       → f32 frames at the output's rate and channels       ~1 s)        sample format,
 missing)                                                                           silence on underrun)
```

- **Output format.** The output stream opens with the device's default configuration (on
  Windows the WASAPI shared-mode mix format, usually 48 kHz; on Linux what PipeWire offers)
  and stays open across tracks. The pipeline works in f32; the callback converts to the
  device's sample format (f32, i32 or i16). Reopening the stream per track is not the plan:
  a device needn't accept a track's rate or layout at all. Bit-perfect output, matching the
  device to the track, is in §9.
- **Resampling** with `rubato` (sinc) when the track's rate differs from the output's. The
  resampler is kept across consecutive tracks of the same rate, so a 44.1 kHz album on a
  48 kHz output stays continuous at track boundaries. A rate change between tracks switches
  resamplers at the boundary, without reopening the stream.
- **Channels.** Same count → passed through. Mono → copied to left and right. More channels
  than the device → downmixed with the standard coefficients (5.1 → stereo per ITU-R BS.775).
  Stereo on a multichannel device → front left and right.
- **Threads.** The audio callback only reads the ring, applies volume (smoothed) and converts
  the sample format: it never locks, allocates, does I/O or waits. If the ring is empty it
  plays silence and counts an underrun. The decode thread owns the decoders, ReplayGain,
  channel mapping and resamplers, opens the next track early for gapless, and is the only
  thread that waits on the cache. Network, prefetch and cache writes run on the tokio
  runtime.
- **Cache waits are cancellable.** Every wait for a missing block also watches the player's
  command generation; seek, skip, stop and shutdown bump it and wake the waiter, the read
  returns `Interrupted`, and the decode thread drops that decoder and handles the command. So
  skipping away from a track stalled on an unreachable phone, or seeking to a cached part of
  it, takes effect at once. The early open of the next track (gapless) is cancelled the same
  way.
- **Ring:** about 1 s at the output rate. Seeks and skips flush it with an epoch marker, so
  the callback drops old samples at once. Volume is applied in the callback, so it responds
  immediately whatever is buffered.
- **Gapless:** the next track is opened and decoded before the current one ends, and its
  frames follow in the ring without a break.
- **Device changes** (headphones unplugged, new default device): reopen the stream with the
  new configuration, rebuild the resamplers, continue from the current position.
- **A file changed mid-play** (`Changed`, §5): the decode thread reopens the track at the new
  rev and seeks to the same time. Rare, and an audible jump is acceptable.

### Track cache (random-access, sparse)

The decoder reads from a source that looks like a local file and is backed by a sparse cache:

- **Per (track id, rev):** a directory of **segment files**, 8 MiB each (32 blocks of
  256 KiB), plus a block map of which blocks are present. Freeing space means deleting whole
  segment files, which reclaims disk on every filesystem; no reliance on hole punching
  (`fallocate(PUNCH_HOLE)` is filesystem-dependent on Linux, and NTFS needs sparse-file
  flags). Cache files are accessed only with positional reads and writes (`read_at` /
  `write_at` on Unix, `seek_read` / `seek_write` on Windows, which take the offset but also
  move the cursor, so nothing may rely on the cursor). One helper wraps both.
- **Accounting:** every segment file present counts as a full 8 MiB, whatever it holds, so
  the limit is checked against what the disk can actually be holding, not blocks marked
  present. Freeing a segment removes its map entries first (logged), then deletes the file;
  at startup, segment files with no map entries are deleted.
- A read that hits a missing block requests it right away, at highest priority, and waits.
- In the background, the current track is filled from the playhead to the end, then the next
  pinned tracks (below). A Wi-Fi blip or a short Doze stall then costs nothing, as long as
  the audio past the playhead is already local.
- Seeking to a part not fetched yet costs one round trip, which is fine on a LAN.
- **Crash safety.** A block is recorded in the map only after its bytes are written and
  `fsync`ed (fsyncs batched, at most 1 s apart). Each map entry carries the block's BLAKE3
  (16 bytes), checked when the block is read back. The map is a log in the reader-sync store
  format (checksums, torn-tail recovery). After a crash, an unrecorded block is missing and is
  fetched again; a recorded block whose bytes don't match is dropped and fetched again.
- **Limit and pinning.** LRU over whole tracks, with a size limit (default 4 GB, adjustable).
  Pinned, never evicted: the current track, and the upcoming queue tracks while their total
  stays within a quarter of the limit (at least the next one). Prefetch fills only pinned
  tracks; the rest of a long queue is fetched as it enters the window. A queue bigger than
  the cache therefore never pushes the cache past its limit.
- **Oversized track** (bigger than a quarter of the limit): played in window mode, keeping
  the segments from behind the playhead to ahead of it and deleting played segments. The
  window scales with the limit: ahead = 1/8 of the limit, at most 128 MiB; behind = 1/32, at
  most 32 MiB; never less than 2 segments ahead and 1 behind. (At 4 GB: 128 + 32 MiB; at
  512 MB: 64 + 16 MiB.) It is never complete in the cache, so it isn't offline-playable, and
  the UI says so.
- **Disk full.** The cache keeps a free-space floor (1 GB or 5 % of the disk, whichever is
  smaller). Reaching it, or any write error: stop prefetching, evict unpinned tracks, then
  put the current track in window mode; if even that can't be written, blocks go from the
  network to the decoder in memory only. A cache problem never stops playback; the UI shows
  it.
- **Stale revisions.** Cache entries whose rev is no longer the index's rev for that id are
  deleted when the index is refreshed.
- **Offline:** tracks that are complete in the cache play without the phone, and the library
  view marks them.

### "Open in another player"

A localhost HTTP server (`127.0.0.1`, random port, random per-run token in the path) serves
tracks with Range support from the same cache. It is for using VLC/mpv during the switchover,
and for debugging. It is not the main playback path.

## 7. Apps

Tauri v2 on all three platforms, like reader-sync, with React + Vite for the UI.

**Look:** OLED black, true `#000` backgrounds, with orange accents (playing state, selection,
progress, primary actions), the same on the PC and the phone. On Linux there's no system
title bar: the window controls and drag area sit in the app's own header, in this style.

- **PC (Linux, Windows):** **songs first** (the user mostly plays songs, not albums), so the
  sidebar reads Songs → Playlists → Albums → Artists. Search, a queue panel, now playing,
  playlists (made by hand, Liked songs, and automatic ones), devices and pairing, cache
  settings. A right-click menu on any track (play next, add to queue, add to playlist, like,
  go to album or artist). Keyboard shortcuts in the app; media keys and the system's
  now-playing panel through MPRIS (Linux) and SMTC (Windows).
- **Songs** sort three ways, each ascending or descending: the default (album artist, album,
  disc, track), by name, and by date added.
- **Automatic playlists**, computed from the play history (§4) and the index, never stored:
  *On Repeat* (most played in the last 30 days), *Recently played*, *Recently added* (by the
  file's modification time on the phone), *Most played*. More can follow (e.g. *Forgotten
  favorites*: played often, not in months).
- **Phone (Android):** pairing, the media permission, folder selection, the "Serve music"
  background toggle with its notification and Pause, and the scan status. Not a player in
  the first builds; **the user wants it to be one too** (the same library, playlists and
  likes, playing its own files locally). That comes after the desktop is polished.

### Player behavior (v1)

Defined before UI work, so the queue and playlist code has one spec:

- **Play now** (an album, a playlist, a selection, starting at a chosen track) replaces the
  queue. **Play next** inserts after the current track; **Add to queue** appends. Queued
  tracks can be reordered and removed, and the queue cleared.
- **Shuffle** on: the rest of the queue is played in a random order, the current track
  stays; off: the original order resumes after the current track. **Repeat:** off / all /
  one.
- **Playlists:** create, rename, delete; add and remove tracks (from the library, the queue,
  or now playing); reorder entries. The playlist list is sorted by name. A track may appear
  in a playlist more than once.
- **Restart:** the queue, the current track and the position are saved (on every track change
  and pause, and every 10 s while playing) and restored at startup, **paused**. The queue is
  per PC in v1; carrying it across the dual boot is in §9.
- **Next track unavailable:**
  - Gone from the library (missing reference, §3): skipped, and shown greyed in the queue.
  - Not cached and the phone unreachable: wait up to 5 s for the link, then skip ahead to the
    next cached track, marking the skipped ones. If no later track is cached, pause with
    "Waiting for phone" and resume by itself when the phone is back.
  - Decode error mid-track: skip to the next track and show a notice.
- With repeat off, the end of the queue stops playback and keeps the queue.

### Android

Android background: reuse reader-sync's `connectedDevice` service. The phone serves while the
toggle is on. The service notification shows "Streaming to <PC>" while a read is in flight,
and "Ready" otherwise.

## 8. Work plan

All code is the real app; no throwaway prototypes. Order: M0 (measure) → M1 (first sound, the
thinnest real path through every layer) → Tracks A, B, C widen it → I1 → D.

**Reordered 2026-10-29 (user):** something usable comes first. **U1, usable build:** the
phone scans and serves its library, and the PC browses it (albums, tracks, art), plays a
queue with seek, pause, next/previous and volume, and pairs. It is built on the real
protocol and pipeline, cut down where noted:
- The track cache starts as whole-track files plus in-memory blocks for the track being
  fetched; segment files, pinning and crash-safe block maps (§6) follow in Track B.
- The scan walks the shared storage folders directly (`/storage/emulated/0`, skipping
  `Android/`); MediaStore change notifications and SD cards follow in Track A.
- Playlists, favorites, shuffle and repeat follow in D.

M0's measurements (baseline, Doze throughput, long idle, battery) wait until after U1. The
FUSE gate (step 3) is done.

### M0 — measurements first (a few days, mostly waiting)

1. **Baseline:** from opening VLC to music playing with BubbleUPnP + VLC, phone locked in a
   pocket, ten tries on Linux. This is the number to beat.
2. **Phone file access:** in a small debug build of the Android shell, the Rust side opens and
   reads 100 FLACs by MediaStore path on the S22 and measures read throughput. If that fails,
   use the `openFileDescriptor` route (§3). `pread` works on whichever descriptor is used.
3. **What FUSE reports:** on the S22, through the same route, `fstat` before and after: an
   in-place same-size edit; the same edit followed by `touch -r` to restore mtime; a same-size
   replacement by rename-over; a plain `touch`. Record whether mtime has nanoseconds, whether
   ctime and the inode change as expected, and the `VOLUME_NAME`s of internal storage and an
   SD card if one is present. **Decision gate** (§3): pass → the revision guarantee stands;
   fail → the narrowed guarantee is written into §3 before Track A starts.
4. **Throughput in Doze:** over the forked core, stream a 150 MB file from the locked S22 in
   deep Doze, with the service on. It must stay above 20 Mbit/s, with no stall over 1 s.
5. **Long idle, then play:** phone unplugged, screen off, still, service on, no battery
   exemption, for 2 hours, entering Doze on its own (not forced; `dumpsys deviceidle` shows
   `IDLE` before the request). Then the PC requests an uncached 50 MB range: record the time to
   the first byte and the throughput. Repeat with the PC app closed for the whole 2 hours, so
   the request needs a fresh connection.
6. **Battery:** reset `batterystats`, then measure (a) 4 h idle with the service on, (b) 4 h
   idle with it off, as the baseline, (c) 1 h of streaming an album to the PC. Record the
   percentage drop and the app's estimated mAh. Target: idle with the service costs at most
   1 %/h more than the baseline. If it costs more, tune before Track A ends (keepalive
   interval; holding the multicast lock only while discovering) and measure again.

For every Android measurement, record the setup so it can be repeated: Android and One UI
versions, the app's battery setting (Unrestricted / Optimized / Restricted), whether it is on
the `deviceidle` whitelist, Adaptive Battery, Wi-Fi band and router, the keepalive interval,
and which locks the service holds.

### M1 — first sound

The thinnest real path through every layer, before the library, cache and UI are built out:
pair → the phone serves one FLAC → the PC streams it → decode → resample → hear it → seek.

- Core fork with the new identifiers (§2); pairing and Devices as in reader-sync.
- The phone serves one file picked in a debug setting (no scan yet), with its rev (§3) and
  the real `Read` protocol, revision checks and positional reads (§5).
- The PC reads through a source that fetches missing ranges on demand and keeps them in
  memory: the cache's miss path, without the disk cache yet.
- The real pipeline (§6): decode thread, channel map, resampler, ring, cpal callback.
- UI: Devices, pairing, and one "Play test track" control with a seek bar.

**Checks:**
- Linux: the track plays; seeking forward and back works; time from pressing play to audio
  is recorded.
- Windows: early listening check from the CI artifact, by the user: a 44.1 kHz track through
  the 48 kHz WASAPI mix format plays at the right pitch and seeks.
- A 24/192 file through a 48 kHz output, rendered to a buffer (automated): a 1 kHz sine comes
  out at 1 kHz, with the right length.
- Replacing the file on the phone during playback gives `Changed`; the player reopens it at
  the same position.

Tracks A, B and C then run in parallel, each widening M1.

### Track A — phone library

MediaStore scan, tag and art extraction, track identity, revisions and aliases, index
snapshot, rescans on change. **Checks (automated):**
- Rescanning an unchanged library produces the same snapshot hash.
- The same relative path on two volumes gives two ids.
- New rev, same id, for: a retag in place; a same-size in-place edit with mtime restored; a
  same-size rename-over with `touch -r`; a bare `touch`.
- A moved file (same bytes) gets an alias; with a duplicate copy also appearing, no alias.
- Moved `A → B → A`, then deleted: the alias table holds no current id as a key and no
  chains, and references to `A` and `B` both end as missing (or a unique fallback match).
- A moved and re-ripped file resolves by fallback key only when exactly one track matches;
  with two matches the reference is missing, with both as candidates.
- Playlist entries and favorites resolve identically after a move.
- Scan time for the real library on the S22 is recorded.

### Track B — streaming and cache

Sparse block cache on disk, prefetch scheduler, pinning and eviction, crash recovery, the
localhost HTTP server. **Checks (automated, two nodes in-process, as in reader-sync's
`core/tests/sync.rs`; the concurrency and file-access ones also in Windows CI):**
- Random reads match the source bytes.
- Concurrent overlapping reads of one track (playback plus prefetch, 16 tasks) all match the
  source, on both the phone side and the PC cache.
- A seek drops the old stream, and the new range arrives first.
- The file modified during a transfer (in-place write between blocks, truncation,
  rename-over) → `Changed`, and the cache never serves bytes that differ from the file at the
  rev it was asked for.
- Unpaired and removed devices are refused.
- A queue larger than the limit keeps disk use within the limit, and never evicts the current
  track or the pinned window.
- A track bigger than a quarter of the limit plays correctly in window mode, and the cache
  directory's **allocated** size (`st_blocks` on Unix, the allocation size on Windows),
  sampled throughout a long playback, stays within the window plus one segment.
- Injected `ENOSPC` and write errors: playback continues, prefetch stops, the UI state says so.
- Crash recovery: a kill between the data write and the map entry, a map entry over corrupt
  bytes, a truncated data file → after a restart those blocks are fetched again, and served
  bytes match the source.
- A dropped link mid-track resumes from the missing blocks only.

### Track C — player

Gapless, ReplayGain, all rates and channel layouts, seek, device changes, media controls.
**Checks (rendering to a buffer instead of the device, so they also run in Windows CI):**
- **Gapless, sample-exact:** a test album split from one continuous sine sweep plays back with
  no discontinuity at the output's own rate; resampled 44.1 → 48 kHz, no discontinuity above
  the resampler's noise floor at the boundaries.
- 192 kHz → 48 kHz, mono → stereo and 5.1 → stereo come out right.
- A sample-rate change between tracks without reopening the output.
- **The callback never waits:** with a source that stalls for 2 s, every callback returns in
  time, plays silence, counts the underrun, and playback resumes on its own.
- **Controls interrupt a stalled read:** with the phone offline and the current track stalled
  on a missing block, skip to a cached track plays it within 200 ms; seeking to a cached part
  of the stalled track plays from there; stop and shutdown return within 200 ms.
- Seek lands within 50 ms of the target.
- A device change mid-track continues playback.
- Listening on Linux: 16/44.1, 24/96 and 24/192 files play correctly. Windows: one listening
  pass at I1.

### Integration — I1: the vertical slice

Pairing, devices, the cached library view, queue and playback, wired together.
**Checks on real devices:**
- Phone locked in a pocket, PC app cold start: the library shows at once from the cache.
  Music plays within 1 s of pressing play (compare with the M0 baseline). Again after the
  phone has been idle and locked for 2 hours: record the time.
- Turn the phone's Wi-Fi off for 10 s mid-track, in three cases, recording the seconds of
  audio buffered ahead when the link drops: (a) 2 s after starting an uncached track,
  (b) right after seeking into an unfetched part, (c) after the current track's prefetch has
  finished. Expected: (c) never interrupts. (a) and (b) don't interrupt if the buffer covered
  the outage; otherwise playback pauses with "Waiting for phone" (not an error, not a skip)
  and resumes on its own within 2 s of the link returning. The next track starts if it was
  cached; otherwise the UI says the phone is unreachable.
- Phone's IP changes (rejoin Wi-Fi): reconnects without the user doing anything.
- An album plays gapless on Linux and Windows.
- Dual-boot: playlist made on Linux, shut down, boot Windows: the playlist is there, carried
  by the phone.

### D — daily driver

Search across tags, album and artist views with art, playlists and favorites UI (including
picking a candidate for a missing reference), folder view, tray, cache settings, "Open in
VLC". **Check:** a week without BubbleUPnP + VLC, with the phone's battery use recorded over
the week.

## 9. Later, as needed

- Sync away from home: opt-in iroh relay (self-hosted `iroh-relay` or n0's), with the relay
  URL kept in the replicated address book. With a slow link, the phone transcodes to Opus.
- The phone as a remote for the PC player (play/pause/skip, queue).
- Compacting the Loro document's history (play counts overwrite one entry per track).
- Exclusive/bit-perfect output (WASAPI exclusive mode, ALSA `hw:`), opening the device at the
  track's rate when it supports it.
- Index deltas, if the full snapshot proves slow.
- More sources than one phone (e.g. a second phone or a NAS as another library).
- The queue and position carried between the PCs through the phone (dual-boot handoff).

## 10. Decisions and what's still open

Decided:

- Phone is the only library and server; the PC never holds the only copy of anything.
- The reader-sync core is copied, not shared; files are not synced.
- The index is a snapshot written only by the phone; Loro holds only user data.
- Every read names a file revision; bytes are verified against it while being served.
- Playlists and favorites hold the same kind of track reference, resolved by one rule (§3).
- Native playback (symphonia + cpal), not the webview. v1 outputs at the device's default
  format and resamples and remixes in the app; the audio callback never waits.
- Tauri on all three platforms.
- Linux is the main development platform. Windows builds and runs `cargo test` in CI from the
  start (as in reader-sync's `.github/workflows/windows.yml`), and gets hands-on checks at M1,
  I1 and D.

Open:

- **App identifier** (Android package id and Tauri identifier), fixed from the first
  install: proposed `app.musicstream`. The display name can change any time.
- **Which folders count as the library:** all of MediaStore, or chosen folders only.
  Proposed: all, with an exclude list.
- **Cache size default:** 4 GB is a guess. Set it once the real library's track sizes are
  known, along with the pin window (a quarter of the limit).

## 11. Risks

| Risk | Mitigation |
|---|---|
| Android blocks or slows direct path reads from Rust | M0 check; file-descriptor route via Kotlin (§3) |
| FUSE hides ctime or inode changes | M0 decision gate (§3): if the metadata is unreliable, the guarantee is narrowed in writing (sampling does not catch mid-file edits), with per-block content hashes as the fallback |
| A stalled read blocks the player's controls | Cache waits cancelled by seek, skip, stop and shutdown (§6); tested with the phone offline |
| Evicted blocks still occupy disk | Segment files deleted whole; the limit counts files present, not blocks; measured on disk (§6) |
| Stale or mixed bytes after a file changes | Revisions in every read; fingerprint rechecked at open and after every block; cache keyed by rev (§3, §5, §6) |
| Phone stalls in Doze mid-track | reader-sync's verified service; prefetch ahead of the playhead; M0 measures throughput in Doze and after a long idle |
| Battery drain on the phone | Measured in M0 against a 1 %/h target before building on it; serving is reads only, no decoding; prefetching whole tracks lets the radio idle between tracks; Pause in the notification |
| Device rejects the track's rate or layout | Fixed output format, resampling and channel mapping in the app (§6) |
| Audio dropouts while the phone is slow | The callback never waits; a bounded ring fed by a decode thread; underruns counted and tested (§6) |
| FLAC seeking without a SEEKTABLE is slow over the network | Block cache makes repeated reads local; symphonia's search needs few blocks; measured in Track C |
| Cache outgrows its limit or the disk | Pin window instead of pinning the whole queue; window mode for big tracks; free-space floor (§6) |
| Cache corrupted by a crash | fsync before the map entry; per-block hashes checked on read (§6) |
| Track references break when files move | Aliases for same-bytes moves; tag fallback only when unambiguous; otherwise the user picks (§3) |
| Index too big for one snapshot | Measure with the real library; deltas in §9 |
| Copied core drifts from reader-sync fixes | Source commit recorded; port fixes by hand |
| Integration problems found late | M1 runs the whole path, on Linux and Windows, before the tracks build out |

## 12. Review log

**2026-10-29, Astra's first review** (architecture kept; contracts made precise):

1. File freshness → revisions from a stat + sample-hash fingerprint, rechecked when serving
   and after every block (§3 File revisions, §5); same-size, preserved-mtime and mid-transfer
   cases in M0 and Tracks A/B.
2. Output conversion and threads → §6 Audio pipeline: fixed output format, `rubato`, channel
   mapping, decode thread feeding a bounded ring, a callback that never waits; 192 → 48 kHz
   checks in M1 and Track C.
3. Cache limit vs. queue → pin window, window mode for oversized tracks, free-space floor,
   crash-safe block map with per-block hashes (§6); tests in Track B.
4. Track identity → volume in the id; aliases for moves; fallback only when unambiguous; one
   `TrackRef` rule for playlists and favorites (§3, §4).
5. Concurrent range reads → positional reads only, on the phone and in the PC cache (§5, §6);
   overlapping concurrent reads in Track B, including Windows CI.
6. M0 → FUSE metadata check, a 2-hour natural-Doze idle followed by an uncached read, battery
   measurements with a target, and the recorded setup (§8 M0).
7. Early end-to-end → M1 "first sound" right after M0, with a Windows listening check.
   The 10 s Wi-Fi-outage check is split into three cases, measured by the buffer ahead (§8 I1).

**2026-10-29, Astra's second review** (go-ahead for M0; four corrections, plus v1 behavior):

1. Fingerprint fallback overpromised → M0 step 3 is a decision gate; on failure the
   guarantee is narrowed in writing, and sampling is not claimed to catch mid-file edits (§3,
   §11).
2. Controls vs. a blocked read → cache waits cancelled by seek, skip, stop and shutdown (§6);
   tested with the phone offline (Track C).
3. Window eviction must free disk → 8 MiB segment files deleted whole, accounting by files
   present, window scaled to the limit (§6); allocated size measured in Track B.
4. Alias cycles → no current id as a key, flattened aliases, one-hop resolution (§3);
   `A → B → A` then delete tested in Track A.
5. v1 player behavior (queue, shuffle, repeat, playlists, restore, unavailable tracks) → §7.

**2026-10-29, Astra's third review** (of the U1 build): fetch streams read whole events in their
own task (a seek's wake could cut a frame); play/pause and status no longer wait on a blocked
decoder; a real-time gapless test added, and gapless on real albums stays to be heard. U1 is
verified only after the device checklist in STATUS.md.

**2026-10-29, user priorities after U1:** songs-first navigation; shuffle, repeat, keyboard
and media keys; playlists and likes; automatic playlists like Spotify's *On Repeat* (§4 play
history, §7).
