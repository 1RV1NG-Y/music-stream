# music-stream — plan

Play the music on my phone from my PC (Linux and Windows, dual-boot), streamed over the local
network, replacing BubbleUPnP + VLC. The library stays on the phone; the PC browses it
instantly and plays it without copying it over first. No accounts, servers or cloud.

Why replace BubbleUPnP + VLC: it is unreliable and I often have to wait for the phone to
"show up". UPnP finds the phone again on every use, by broadcasting a search (SSDP) and waiting
for answers. Android drops those packets when the screen is off, and VLC's UPnP browser is slow
to list what answers. This plan removes that step: once paired, the PC knows the phone's
identity and last addresses, and dials it directly.

Status: draft plan, 2026-10-29. Nothing is built yet. Working name `music-stream`.
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
| Durable store: checksummed fsynced log, compaction, torn-tail recovery, exclusive lock | `store.rs` | Yes |
| Android `connectedDevice` foreground service, CPU/Wi-Fi/multicast locks, network callbacks, node owned by the Android runtime | `app/src-tauri/src/android.rs`, `node_slot.rs`, `gen/android/.../LanSync*.kt` | Yes. This is the part that makes the phone a reliable server |
| Message framing, `FileMsg::Want { hash, offset }` streaming from an offset | `proto.rs`, `node/files.rs` | Basis for the stream protocol (§5); range reads need an end, so it is rewritten |
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
`READ_EXTERNAL_STORAGE` before that). Each row gives the file's path, size, modification time
and MediaStore id. Settings can limit the library to chosen folders (default: all music
MediaStore knows).

**Reading the bytes from Rust.** On Android 11+, apps with media permission can open media
files by their real path, through the system's FUSE layer, so the Rust node can open
`/storage/emulated/0/Music/...` directly. If that fails or is slow on the S22, Kotlin opens
the file with `ContentResolver.openFileDescriptor(uri, "r")` and hands the raw descriptor to
Rust (`detachFd()` → `File::from_raw_fd`). Checked in M0 (§8) before anything is built on it.

### Tags and art

MediaStore's tag columns are incomplete for FLAC (album artist, disc numbers, ReplayGain,
multiple artists). The phone reads tags itself with `lofty`, from the Vorbis comments, only for
files whose (size, mtime) changed since the last scan. Embedded or folder cover art is scaled
to a thumbnail (e.g. 512 px JPEG), stored content-addressed by BLAKE3, and fetched by the PC
on demand.

Rescans run when the app starts, when the service starts, and on MediaStore change
notifications (`ContentObserver`), debounced.

### Track identity

A track id must survive rescans, because playlists refer to it.

- **id = BLAKE3 of the file's path relative to its storage volume**, truncated to 16 bytes.
  It is cheap, and stable while the file stays where it is.
- **Moves and renames:** a playlist entry stores the id plus a fallback key (album artist,
  album, disc, track number, title). An id that disappears is matched again by the fallback
  key, and the entry is updated.
- **Content changes** (retagging, replacing a rip) keep the id and change `(size, mtime)`.
  Every read names the `(size, mtime)` it expects (§5), and the PC cache is keyed by it, so
  stale bytes are never mixed with new ones.

Not a hash of the file contents: hashing a large FLAC library on the phone takes minutes,
and retagging would change every id.

### The index is a snapshot, not a Loro document

Only the phone writes the index, so it needs no merging. Loro also keeps full history, so a
10,000-track index that changes with every rescan would make the document grow forever. So:

- The index is one serialized snapshot (postcard + zstd) with a `generation` and a content
  hash. Per track: id, relative path, size, mtime, duration, sample rate, bit depth,
  channels, tags, ReplayGain values, art hash.
- The PC asks `Index { have: <hash> }`. The phone answers `Same` or sends the full snapshot.
  At a few MB compressed this is fine on a LAN. Measure it with the real library before
  adding deltas.
- The PC keeps the last snapshot on disk and shows it immediately at startup.

## 4. User data: playlists, favorites, play history (Loro)

Playlists and favorites are written on both PCs, and those two are never on at the same time.
That is the reader-sync case exactly: a Loro document, with the phone forwarding changes
between them. The phone may also edit them later (§9).

```
playlists/<playlistId>     { name, created, entries: movable list of { trackId, fallbackKey } }
favorites/<trackId>        { since }                  (map; remove = unfavorite)
```

Play counts and history are left for later (§9): every play would add history to the
document, so they need a different design (per-device counters, compaction).

## 5. Streaming protocol

Runs as extra QUIC streams on the established `music/sync/1` connection, one stream per
request, so a seek simply drops the old stream.

```
Read  { track: id, size, mtime, start, end }   PC → phone
  ← Have { total }  then exactly end - start raw bytes
  ← Changed { size, mtime }   (file differs from what the PC expected)
  ← Missing                   (no such track anymore)
  ← Refused { reason }        (unpaired or removed device, range out of bounds)
Art   { hash }  ← Have { total } + bytes, or Missing
Index { have: hash }  ← Same, or Snapshot { generation } + bytes
```

- Only paired devices are served, as in reader-sync. The connection is authenticated by key.
- The phone keeps a small cache of open file handles and reads with plain `seek` + `read`.
  No decoding on the phone.
- Bandwidth: 16/44.1 FLAC is about 1 Mbit/s; 24/192 can reach 9 Mbit/s. Both are easy on
  home Wi-Fi.

## 6. The PC player

### Native playback, not the webview

Most of the library is FLAC, which pushes playback out of the webview: `<audio>` can't do
gapless, its FLAC support on Linux depends on the installed GStreamer plugins, and it stops
when the window's webview does. So:

- **Decode with `symphonia`** (pure-Rust FLAC, with seeking via SEEKTABLE or search).
- **Output with `cpal`** (WASAPI on Windows; ALSA on Linux, served by PipeWire).
- **Gapless:** the next track is opened and decoded before the current one ends, and the
  samples run on without a break. Sample-rate changes between tracks reopen the output
  stream at the boundary.
- **ReplayGain** (track/album/off) from the tags in the index.
- **Volume** in software, in floating point.
- Playback runs in the Rust side of the Tauri app and continues with the window closed (tray
  icon). The UI only sends commands and receives state.
- **OS media controls:** Windows SMTC and Linux MPRIS through `souvlaki`, so media keys and the
  system's now-playing panel work.

### Track cache (random-access, sparse)

The decoder reads from a source that looks like a local file and is backed by a sparse cache:

- Per track, a cache file plus a map of which 256 KiB blocks are present.
- A read that hits a missing block requests it right away, at highest priority, and waits.
- In the background, the current track is filled from the playhead to the end, then the next
  track in the queue. A Wi-Fi blip or a short Doze stall during playback costs nothing,
  because the track is already local.
- Seeking to a part not fetched yet costs one round trip, which is fine on a LAN.
- The cache is LRU with a size limit (default 4 GB, adjustable). Entries are keyed by
  `(track id, size, mtime)`. Tracks in the current queue are never evicted.
- **Offline:** tracks that are complete in the cache play without the phone, and the library
  view marks them.

### "Open in another player"

A localhost HTTP server (`127.0.0.1`, random port, random per-run token in the path) serves
tracks with Range support from the same cache. It is for using VLC/mpv during the switchover,
and for debugging. It is not the main playback path.

## 7. Apps

Tauri v2 on all three platforms, like reader-sync, with React + Vite for the UI.

- **PC (Linux, Windows):** library browsing (albums, artists, tracks, folders), search, queue,
  now playing, playlists, favorites, devices and pairing, cache settings.
- **Phone (Android):** pairing, the media permission, folder selection, the "Serve music"
  background toggle with its notification and Pause, and the scan status. It is not a player
  in v1.

Android background: reuse reader-sync's `connectedDevice` service. The phone serves while the
toggle is on. The service notification shows "Streaming to <PC>" while a read is in flight,
and "Ready" otherwise.

## 8. Work plan

All code is the real app; no throwaway prototypes.

### M0 — measurements first (a day or two)

1. **Baseline:** from opening VLC to music playing with BubbleUPnP + VLC, phone locked in a
   pocket, ten tries on Linux. This is the number to beat.
2. **Phone file access:** in a small debug build of the Android shell, the Rust side opens and
   reads 100 FLACs by MediaStore path on the S22 and measures read throughput. If that fails,
   use the `openFileDescriptor` route (§3).
3. **Throughput in Doze:** over the forked core, stream a 150 MB file from the locked S22 in
   deep Doze, with the service on. It must stay above 20 Mbit/s, with no stall over 1 s.

### Track A — phone library

Core fork and renames (§2), MediaStore scan, tag and art extraction, track identity, index
snapshot, rescans on change. **Checks (automated):**
- Rescanning an unchanged library produces the same snapshot hash.
- A retagged file keeps its id, with a new `(size, mtime)`.
- A moved file gets a new id, and a playlist entry finds it again by fallback key.
- Scan time for the real library on the S22 is recorded.

### Track B — streaming and cache

Stream protocol (§5), sparse block cache, prefetch scheduler, LRU eviction, the localhost
HTTP server. **Checks (automated, two nodes in-process, as in reader-sync's
`core/tests/sync.rs`):**
- Random reads match the source bytes.
- A seek drops the old stream, and the new range arrives first.
- `Changed` when the file is replaced mid-play; the stale cache entry is discarded.
- Unpaired and removed devices are refused.
- Eviction never touches queued tracks.
- A dropped link mid-track resumes from the missing blocks only.

### Track C — player

symphonia + cpal playback from the cache source, gapless, ReplayGain, sample-rate changes,
seek, media controls. **Checks:**
- **Gapless, sample-exact:** a test album split from one continuous sine sweep plays back with
  no discontinuity, checked by rendering to a buffer instead of the device.
- Seek lands within 50 ms of the target.
- Sample-rate changes between tracks are handled.
- Listening on Linux: 16/44.1, 24/96 and 24/192 files play correctly. Windows: the same
  tests in CI (rendering to a buffer), plus one listening pass at I1.

### Integration — I1: the vertical slice

Pairing, devices, the cached library view, queue and playback, wired together.
**Checks on real devices:**
- Phone locked in a pocket, PC app cold start: the library shows at once from the cache.
  Music plays within 1 s of pressing play (compare with the M0 baseline).
- Turn the phone's Wi-Fi off for 10 s mid-track: playback doesn't stop. The next track
  starts if it was cached; otherwise the UI says the phone is unreachable.
- Phone's IP changes (rejoin Wi-Fi): reconnects without the user doing anything.
- An album plays gapless on Linux and Windows.
- Dual-boot: playlist made on Linux, shut down, boot Windows: the playlist is there, carried
  by the phone.

### D — daily driver

Search across tags, album and artist views with art, playlists and favorites UI, folder view,
tray, cache settings, "Open in VLC". **Check:** a week without BubbleUPnP + VLC.

## 9. Later, as needed

- Sync away from home: opt-in iroh relay (self-hosted `iroh-relay` or n0's), with the relay
  URL kept in the replicated address book. With a slow link, the phone transcodes to Opus.
- The phone as a remote for the PC player (play/pause/skip, queue).
- Play counts and history (per-device counters that merge cleanly; compaction).
- Exclusive/bit-perfect output (WASAPI exclusive mode, ALSA `hw:`).
- Index deltas, if the full snapshot proves slow.
- More sources than one phone (e.g. a second phone or a NAS as another library).

## 10. Decisions and what's still open

Decided:

- Phone is the only library and server; the PC never holds the only copy of anything.
- The reader-sync core is copied, not shared; files are not synced.
- The index is a snapshot written only by the phone; Loro holds only user data.
- Native playback (symphonia + cpal), not the webview.
- Tauri on all three platforms.
- Linux is the main development platform. Windows builds and runs `cargo test` in CI from the
  start (as in reader-sync's `.github/workflows/windows.yml`), and gets hands-on checks at I1
  and D.

Open:

- **App identifier** (Android package id and Tauri identifier), fixed from the first
  install: proposed `app.musicstream`. The display name can change any time.
- **Which folders count as the library:** all of MediaStore, or chosen folders only.
  Proposed: all, with an exclude list.
- **Cache size default:** 4 GB is a guess. Set it once the real library's track sizes are
  known.

## 11. Risks

| Risk | Mitigation |
|---|---|
| Android blocks or slows direct path reads from Rust | M0 check; file-descriptor route via Kotlin (§3) |
| Phone stalls in Doze mid-track | reader-sync's verified service; whole-track prefetch makes stalls invisible during a track; M0 measures it |
| FLAC seeking without a SEEKTABLE is slow over the network | Block cache makes repeated reads local; symphonia's search needs few blocks; measured in Track C |
| Gapless breaks at sample-rate changes | Only gapless within one format; reopen the output stream at a change (albums rarely mix) |
| Track ids break when files move | Fallback key re-matching in playlists (§3) |
| Index too big for one snapshot | Measure with the real library; deltas in §9 |
| Copied core drifts from reader-sync fixes | Source commit recorded; port fixes by hand |
| Battery drain on the phone | Serving is reads only, no decoding; prefetching whole tracks lets the radio idle between tracks; Pause in the notification |
