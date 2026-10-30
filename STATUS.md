# Status: where the work stands

Read `PLAN.md` first (the agreed design; §8 is the work plan, §12 the review log). This file
says what is done, what is verified, and exactly what is left, so work can resume from here.

Last updated: 2026-10-29. The plan has passed two reviews by Astra (go-ahead for M0). Then the
user reprioritized: **a usable build first (U1, PLAN §8)**, measurements after. U1 is built,
installed on the user's Linux PC and S22, and plays end to end. Astra's third review found two
bugs (fixed below). The user checked the main device paths and waived the edge cases; next
come app features (see Next).

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
| PC side: cached index, art cache, on-demand fetch (from the wanted block, restarts on far seeks), whole-track cache with LRU, reads interruptible by the player | `core/src/remote.rs` | same: random and concurrent reads match; cached track plays with the phone gone; changed file → error + index catches up; a stalled read is released by the interrupt within 500 ms; constant seeking while blocks arrive breaks no read (`readErrors` = 0; the old loop broke 9) |
| Player: symphonia decode, ReplayGain, channel mapping, rubato (FFT) resampling, 1 s lock-free ring, cpal callback that never waits, gapless, accurate seek, queue ops | `player/` | 8 tests rendering to a buffer: gapless sample-exact at 44.1; continuous through 44.1→48; 192 kHz mono → 48 kHz stereo at the right pitch and length; downmix; seek; skip away from a stalled track plays the next within 200 ms; **real-time gapless** (10 ms device-style pulls, silence counted, next tracks 300 ms slow to open: no gap, sample-exact); **pause and status during a stall** (pause silences the output and shows at once; position moves; an empty ring while playing reports buffering) |
| App shell from reader-sync (Tauri, Android service, tray), `music` command, `art:` scheme, queue/volume saved and restored paused | `app/src-tauri/` | user (first build): pairs and plays on Linux + S22, phone locked. Full checklist below still open |
| UI: OLED black / orange; albums, album page, artists, songs, search, queue, player bar; phone screen (status, rescan, devices, background serving) | `app/src/` | `npm test` (grouping, search, formats); used by the user on Linux and the S22 |

**Astra's third review (2026-10-29), fixed:**
1. *A wake for a seek could cancel a block read halfway*, leaving the stream mid-frame and
   the next read misparsing audio bytes as a header (retries, stalls). Now a task of its own
   reads each stream's events whole and hands them over a channel; a wake never touches the
   stream. Test: `constant_seeking_never_breaks_a_read`.
2. *Pause didn't respond while the decoder waited for the phone*, and status froze. Now
   play/pause act on the output's flag directly, and playing/position/buffering are computed
   from the output's counters and a shared timeline, not published by the worker.
3. *Gapless claim qualified:* the earlier tests waited for samples and ignored underrun
   silence. The real-time test above closes that for the player; **listening across real FLAC
   album boundaries on the user's devices is still to do.**

**U1 device checklist (Astra), results from the user on the second build (2026-10-29):**
- Pairing: works. Playback with the phone locked: works.
- Uncached playback: most songs start almost instantly; the slowest found down the queue
  was still quick.
- Seeking forward and back within a song: good.
- Album transitions (gapless by ear), phone Wi-Fi off mid-track, cached playback offline:
  **not checked, waived by the user as edge cases** for now; they'll report issues if they
  hit them. The real-time gapless test covers the player side only.
- Outside U1, still open: Windows, the long-idle play, battery.

## Desktop features, round 1 (2026-10-30; installed on Linux, not yet tried by the user)

| Piece | Where | Verified by |
|---|---|---|
| Shuffle (current song first, the rest random; off restores the order, keeping songs queued meanwhile after the current one) and repeat off/all/one | `player/` | 4 new tests, incl. a real bug found and fixed: near a track's end the decoder is already into the next song, so shuffle and queue edits now follow the song being heard |
| Playlists (movable lists of `TrackRef`), Liked songs, per-device play history; mergeable Loro containers | `core/src/model.rs`, `node/userdata.rs` | merge tests (edits made apart on two docs; plays from two PCs); `resolve_ref` by id, alias, or a unique fallback |
| Automatic playlists: On Repeat (≥2 plays in 30 days), Recently played, Recently added (file mtime), Most played; a play counts after half the song or 4 min | `app/src-tauri/src/desktop.rs` | builds; to be tried |
| Media keys and the desktop's now-playing panel (MPRIS; SMTC code path for Windows untested) | `app/src-tauri/src/media.rs` | registered on the session bus; `playerctl -p music_stream status` answers |
| UI: songs-first sidebar (Songs, Playlists, Albums, Artists); Songs sort Default / Name / Date added, each up or down; right-click / ⋯ menu; drag to reorder playlists and the queue, drag songs onto a sidebar playlist; queue side panel; shuffle, repeat and like in the player bar; keyboard shortcuts with a `?` sheet | `app/src/` | `npm test` (sorting); the UI itself can't be screenshotted here (GNOME refuses), so the user's look is the check |

## Phone player, hiding and folders (2026-10-30; installed on Linux and the S22)

| Piece | Where | Verified by |
|---|---|---|
| Hidden songs and folders, synced (`hidden`, `hiddenFolders`); shared views (library, playlists, automatic playlists, folders) built the same from the PC's cached index or the phone's own | `core/src/model.rs`, `node/userdata.rs`, `core/src/views.rs` | folder test (counts, subfolders, prefix not a parent); 36 Rust tests pass |
| Shared `music` commands for both platforms (library, playlists, likes, hide, folders) | `app/src-tauri/src/userdata.rs` | desktop and Android builds |
| Desktop: Hide song / Hide folder in the menu; **Folders** page (switch per folder, subfolders follow) and **Hidden** page | `app/src/Desktop.jsx` | builds; not yet tried by the user |
| **Phone player:** Media3 ExoPlayer in a `MediaSessionService` (lock screen, notification, headset/Bluetooth, audio focus, unplugged); `PlayerPlugin` commands in play order; plays counted in the service and recorded via JNI | `gen/android/.../PlayerService.kt`, `PlayerPlugin.kt`, `src-tauri/src/android.rs` | on the S22 over adb: a tapped song plays (`dumpsys media_session`: PLAYING with title/artist), a system media-key pause pauses it; the phone kept serving the PC meanwhile. **Play counting on the phone not yet seen working** (needs half a song) |
| Phone UI: tabs Songs / Playlists / Albums / Artists / More; sort chips; mini player; Now Playing (cover, like, seek, shuffle, repeat, queue); ⋯ / long-press action sheet (incl. hide); More: serving status, scan, Folders, Hidden, Devices, Background serving | `app/src/Phone.jsx` | screenshots on the S22: Songs with art, mini player, Now Playing, More, Folders (Alarms 1, Download 12 / Mornye 11, Movies 9, Music 351) |

Known gaps: with shuffle on, Media3 puts "Play next" songs at a random place in the shuffled
order (needs a custom shuffle order in the service); queue reordering on the phone only with
shuffle off; two notifications while playing and serving (could be merged).

Still to come: Opus on the PC (4 songs; the phone plays Opus natively but the index skips
`.opus`), naming the 2 unreadable files, Windows checks.

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
- **Second install (2026-10-29, 15:26):** both devices run the build with Astra's two fixes,
  the phone header, and rescan-on-focus. The phone reconnected to the PC by itself after the
  APK was replaced. (The PC app must be restarted to run the new build.)
- Build settings changed by the user's disk-space session: `[profile.dev]` line-tables-only
  debug info (none for dependencies), and the Tauri lib no longer builds the iOS-only
  `staticlib`. Both builds work with them.

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
