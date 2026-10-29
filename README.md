# music-stream

Play the music on your phone from your PC (Linux and Windows), **streamed over your local
network**: no copying the library over first, and no account, server or cloud service. It
replaces a BubbleUPnP + VLC setup that kept waiting for the phone to "show up".

The phone owns the files and serves them; the PC keeps a copy of the library's index, so
browsing never waits for the phone, and fetches tracks as they play. Once paired, the PC
knows the phone by its key and dials it directly, including when the phone is locked.

Status: under active development. See [STATUS.md](STATUS.md) for what works today and
[PLAN.md](PLAN.md) for the design.

## Features

- **Phone (Android):** scans its music folders, reads tags and cover art, and serves the
  library in the background (a foreground service, so it keeps serving with the screen off).
- **PC (Linux, Windows):** albums with cover art, album pages, artists, songs, search, a
  queue (play next, add, reorder, remove), seek and volume. The queue and position come back
  after a restart, paused.
- **Native playback:** FLAC, MP3, AAC/ALAC, Vorbis and WAV, decoded with
  [symphonia](https://github.com/pdeljanov/Symphonia). Output via
  [cpal](https://github.com/RustAudio/cpal), with resampling to the device's rate
  ([rubato](https://github.com/HEnquist/rubato)), channel mapping, gapless playback and
  ReplayGain (album or track). Playback keeps going with the window closed (tray icon).
- **Streaming and cache:** tracks are fetched from the playhead on, and the next track is
  fetched ahead. Played tracks are cached (4 GB by default), so they also play without the
  phone. Every read names the file revision it expects, so a file changed on the phone is
  never mixed with the old copy.
- **Pairing** by QR code or a text code, discovery over mDNS, transport over QUIC
  ([iroh](https://github.com/n0-computer/iroh)). User data replicates with
  [Loro](https://loro.dev), carried by the phone between PCs that are never on at the same
  time (e.g. a dual-boot PC).
- OLED black with orange accents. On Linux the app draws its own window controls (no system
  title bar).

Not yet: shuffle and repeat, media keys, playlists and favorites, Opus files (see
[STATUS.md](STATUS.md)).

## Layout

| Path | What it is |
|---|---|
| `core/` | `musicstream-core`: identity, pairing, discovery, the replicated document and durable storage (forked from [reader-sync](https://github.com/1RV1NG-Y/reader-sync)); the phone's library scan and index; the stream protocol; the PC's index, art and track cache. |
| `player/` | Native playback: decode thread, resampler and lock-free ring feeding the audio callback; queue, seek, gapless. |
| `app/` | The app: React + Vite UI, Tauri v2 shell for Linux, Windows and Android (`app/src-tauri`). |

## Build and test

```sh
cargo test -p musicstream-core -p player   # library, streaming over real QUIC, playback rendered to a buffer
cd app && npm ci && npm test               # UI logic
cd app && npx tauri build --no-bundle      # desktop app → target/release/music-stream
cd app && npm run install:linux            # release build, installed for this user with a menu entry
cd app && npm run android:build            # debug-signed arm64 APK → app/android-dist/ (Android SDK + NDK)
```

On Linux, playback needs ALSA's development files (`alsa-lib`; PipeWire serves ALSA).

The devices talk over UDP port **47120** and mDNS (UDP 5353) on your local network; allow them
in your firewall (e.g. `sudo ufw allow from 192.168.1.0/24 to any port 47120 proto udp`).

## License

MIT. See [LICENSE](LICENSE).
