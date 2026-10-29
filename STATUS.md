# Status: where the work stands

Read `PLAN.md` first (the agreed design; §8 is the work plan, §12 the review log). This file
says what is done, what is verified, and exactly what is left, so work can resume from here.

Last updated: 2026-10-29. The plan has passed two reviews by Astra (go-ahead for M0). Then the
user reprioritized: **a usable build first (U1, PLAN §8)**, measurements after. U1 is built,
installed on the user's Linux PC and S22, and playing music end to end.

## Done

| Step | Verified by |
|---|---|
| Plan revised for Astra's two reviews (PLAN §12); look decided: OLED black, orange accents (§7) | Astra's second review: go-ahead for M0 |
| Core forked from reader-sync `4fab02e` into `core/` (`musicstream-core`): identity, pairing, mDNS, replicated doc, store, dialing/backoff. Books, positions, holdings and file transfer removed. New identifiers: ALPNs `music/sync/1`, `music/pair/1`; code prefix `MUSIC1`; UDP 47120; store magic `MUSTLOG1`/`MUSTSNP1` | `cargo test -p musicstream-core`: 11 pass (store recovery, framing, tickets, settings/devices, unsaved-change and failed-pairing scenarios over real QUIC) |

Fork notes: `4fab02e` is newer than PLAN §2's `72541e1`; the core diff between them is one
test helper and one RPC line. Extra streams on a sync connection carry the stream protocol
(PLAN §5). `model.rs` keeps settings, devices, addresses and the library head; playlists and
favorites arrive with `TrackRef` (PLAN §3–§4).

## U1: usable build (PLAN §8)

| Piece | Where | Verified by |
|---|---|---|
| Phone library: folder walk, fingerprints/revs, tags (lofty), 512 px JPEG art, aliases (normalized, one hop), index snapshot (postcard + zstd) | `core/src/library.rs` | 6 unit tests: unchanged rescan keeps the hash; two volumes → two ids; mid-file edit with mtime restored → new rev; move → alias, move back → reversed, delete → unresolved; move with a duplicate → no alias; serving refuses changed / renamed-over files |
| Stream protocol (Index, Read, Art) on extra QUIC streams; serving with per-block revision checks and positional reads; library head published through the replicated doc so PCs refresh at once | `core/src/node/stream.rs`, `proto.rs`, `model.rs` | `core/tests/stream.rs` (two nodes over real QUIC) |
| PC side: cached index, art cache, on-demand fetch (from the wanted block, restarts on far seeks), whole-track cache with LRU, reads interruptible by the player | `core/src/remote.rs` | same: random and concurrent reads match; cached track plays with the phone gone; changed file → error + index catches up; a stalled read is released by the interrupt within 500 ms |
| Player: symphonia decode, ReplayGain, channel mapping, rubato (FFT) resampling, 1 s lock-free ring, cpal callback that never waits, gapless, accurate seek, queue ops | `player/` | 6 tests rendering to a buffer: gapless sample-exact at 44.1; continuous through 44.1→48; 192 kHz mono → 48 kHz stereo at the right pitch and length; downmix; seek; skip away from a stalled track plays the next within 200 ms |
| App shell from reader-sync (Tauri, Android service, tray), `music` command, `art:` scheme, queue/volume saved and restored paused | `app/src-tauri/` | user: pairs and plays end to end on Linux + S22, phone locked |
| UI: OLED black / orange; albums, album page, artists, songs, search, queue, player bar; phone screen (status, rescan, devices, background serving) | `app/src/` | `npm test` (grouping, search, formats); used by the user on Linux and the S22 |

U1 cut-downs (as PLAN §8 says): whole-track cache instead of segment files; folder walk instead
of MediaStore notifications (rescan every 5 min, on start, and on "Scan again"); no playlists,
favorites, shuffle or repeat yet; no OS media keys yet.

## M0: what's left (PLAN §8; after U1)

1. **Baseline (user):** BubbleUPnP + VLC, phone locked in a pocket, ten tries on Linux, time
   from opening VLC to music playing.
2. **Phone file access:** needs the Android shell (next: copy reader-sync's `app/` Tauri
   shell and Android service with the new identifiers, and add a debug screen that reads 100
   FLACs by path).
3. **FUSE metadata, the decision gate: passed from `adb shell`** (2026-10-29), in a scratch
   folder under `/sdcard/Music/` (removed afterwards). `/storage/emulated` is a FUSE mount.

   | Change (same size throughout) | mtime | ctime | inode |
   |---|---|---|---|
   | in-place edit | new | new | same |
   | `touch -r` back to the old mtime | restored | **new** | same |
   | edit, then `touch -r` | restored | **new** | same |
   | rename-over by a same-size file with the old mtime | restored | new | **new** |
   | plain `touch` | new | new | same |

   Times have nanosecond fields on a 4 ms tick (250 Hz); 300 writes ≥ 20 ms apart all got
   distinct ctimes. Still to confirm: the same from the app's own uid, in step 2.
4. **Throughput in Doze**, 5. **long idle**, 6. **battery:** need the Android shell, the
   service, and a minimal `Read` handler.

**The real library** (MediaStore, 2026-10-29): 398 tracks, 10.1 GB, all on
`external_primary` (no SD card). 357 FLAC, 29 AAC (`audio/mp4`), 7 MP3, 5 Ogg. Track size:
median 24.6 MB, 95th percentile 44.6 MB, largest 141.6 MB. So with the 4 GB default no
track is "oversized" (threshold 1 GB), and the index will be small. **For Track C:** check
whether the 5 Ogg files are Vorbis or Opus. symphonia has no Opus decoder, so Opus would need
another decoder.

## Current state of the user's devices

- **music-stream on the phone** (2026-10-29): release APK installed with `adb install -r
  app/android-dist/music-stream.apk` (build: `npm run android:build` in `app/`). Audio
  permission granted; it serves **375 tracks, 9.9 GB** (2 files unreadable, not yet checked;
  MediaStore's 398 also counts audio under `Android/` and hidden folders, which the scan skips).
- **music-stream on Linux:** installed at `~/.local/share/music-stream/music-stream` with a menu
  entry (`npm run install:linux` in `app/`). Starts cleanly: node ready, output 48 kHz f32.
- **Verified by the user (2026-10-29):** paired, library browsed on Linux, music plays end to
  end, **including with the phone locked**. (Firewall rule for UDP 47120 as below.)
- The fixes after the first install (phone header hidden by a narrow-screen rule; rescan when
  the phone app comes to the front) are in the source, not in the installed builds yet.

**Library gaps found:**
- 5 `.opus` files (4 in `Music/SpotiFLAc`) aren't scanned: `.opus` isn't in the scanned
  extensions, and symphonia 0.5 has no Opus decoder. Needs an Opus decoder (libopus binding or
  a pure-Rust one) before adding the extension.
- 2 files fail to read (lofty) and are only logged at debug level. The scan report should
  carry their paths so the phone screen can name them.

- **Phone (Galaxy S22, SM-S901E, Android 16, `S901EXXSEGZH4`):** this PC is paired for
  wireless debugging (2026-10-29). The connect port changes when wireless debugging restarts:
  `adb mdns services` shows it, then `adb connect <ip:port>`. In fish:
  `set -x ANDROID_SERIAL 192.168.100.4:<port>`.
- **Firewall (when testing):** the user's Linux runs ufw; music-stream needs UDP 47120 from
  the LAN: `sudo ufw allow from 192.168.100.0/24 to any port 47120 proto udp`.

## Known gaps / notes for whoever continues

- `cargo clippy` isn't installed for the stable toolchain on this PC (`rustup component add
  clippy`).
- No Windows CI yet; copy reader-sync's `.github/workflows/windows.yml` together with the
  app shell.
- User preferences: no title bars on Linux apps (window controls in the app's own header);
  OLED black with orange accents; don't re-propose settled decisions; keep this file current
  after each step. Astra reviews progress against PLAN and this file.
