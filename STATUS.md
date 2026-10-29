# Status: where the work stands

Read `PLAN.md` first (the agreed design; §8 is the work plan, §12 the review log). This file
says what is done, what is verified, and exactly what is left, so work can resume from here.

Last updated: 2026-10-29. The plan has passed two reviews by Astra, who gave the go-ahead for
M0 on 2026-10-29. M0 has started.

## Done

| Step | Verified by |
|---|---|
| Plan revised for Astra's two reviews (PLAN §12); look decided: OLED black, orange accents (§7) | Astra's second review: go-ahead for M0 |
| Core forked from reader-sync `4fab02e` into `core/` (`musicstream-core`): identity, pairing, mDNS, replicated doc, store, dialing/backoff. Books, positions, holdings and file transfer removed. New identifiers: ALPNs `music/sync/1`, `music/pair/1`; code prefix `MUSIC1`; UDP 47120; store magic `MUSTLOG1`/`MUSTSNP1` | `cargo test -p musicstream-core`: 11 pass (store recovery, framing, tickets, settings/devices, unsaved-change and failed-pairing scenarios over real QUIC) |

Fork notes: `4fab02e` is newer than PLAN §2's `72541e1`; the core diff between them is one
test helper and one RPC line. Extra streams on a sync connection are accepted and dropped
until the stream protocol (PLAN §5) lands in M1. `model.rs` keeps only settings, devices and
addresses; playlists and favorites arrive with `TrackRef` (PLAN §3–§4).

## M0: what's left (PLAN §8)

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
