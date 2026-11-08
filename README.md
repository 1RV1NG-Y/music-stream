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
  It's also a full player of its own: lock screen and notification controls, headset and
  Bluetooth buttons, Now Playing with the queue, and the same library views as the PC.
- **PC (Linux, Windows):** songs, playlists, albums, artists and search; a queue panel
  (play next, add, drag to reorder, remove), shuffle and repeat, seek and volume. The queue
  and position come back after a restart, paused. Media keys and the system's now-playing
  controls (MPRIS on Linux). Keyboard shortcuts (`?` lists them).
- **Your library, synced:** playlists, liked songs and play history, kept in step between
  the phone and every PC. Automatic playlists: On Repeat, Recently played, Recently added,
  Most played. Hide songs or whole folders you don't want in the library.
- **Lyrics:** a song's own lyrics (its tags or a matching `.lrc` file). Timed lyrics follow
  the song line by line, and a tap on a line jumps there; the background takes its colours
  from the album cover.
- **Download to the PC:** save songs, albums or playlists from the phone into your Music
  folder (`music-stream/<artist>/<album>/`), in the background.
- **Native playback:** FLAC, MP3, AAC/ALAC, Vorbis, Opus and WAV, decoded with
  [symphonia](https://github.com/pdeljanov/Symphonia) (Opus with libopus). Output via
  [cpal](https://github.com/RustAudio/cpal), with resampling to the device's rate
  ([rubato](https://github.com/HEnquist/rubato)), channel mapping, gapless playback and
  ReplayGain (album or track; Opus R128 gains too). Playback keeps going with the window
  closed (tray icon).
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

## Layout

| Path | What it is |
|---|---|
| `core/` | `musicstream-core`: identity, pairing, discovery, the replicated document and durable storage (forked from [reader-sync](https://github.com/1RV1NG-Y/reader-sync)); the phone's library scan, index and lyrics; the stream protocol; the PC's index, art and track cache; playlists, likes and history. |
| `player/` | Native playback: decode thread, resampler and lock-free ring feeding the audio callback; queue, shuffle, repeat, seek, gapless; the Opus decoder. |
| `app/` | The app: React + Vite UI, Tauri v2 shell for Linux, Windows and Android (`app/src-tauri`). |

## Get it

Windows: every push to `main` builds `music-stream.exe`, published under
[Releases](https://github.com/1RV1NG-Y/music-stream/releases). Linux and Android: build from
source as below.

## Build and test

```sh
cargo test -p musicstream-core -p player   # library, streaming over real QUIC, playback rendered to a buffer
cd app && npm ci && npm test               # UI logic
cd app && npx tauri build --no-bundle      # desktop app → target/release/music-stream
cd app && npm run install:linux            # release build, installed for this user with a menu entry
cd app && npm run android:build            # debug-signed arm64 APK → app/android-dist/ (Android SDK + NDK)
```

The desktop app also needs **CMake**, which builds libopus for Opus playback (Windows:
`winget install Kitware.CMake`). On Linux, playback needs ALSA's development files
(`alsa-lib`; PipeWire serves ALSA).

The devices talk over UDP port **47120** and mDNS (UDP 5353) on your local network; allow them
in your firewall (e.g. `sudo ufw allow from 192.168.1.0/24 to any port 47120 proto udp`).

## License

MIT. See [LICENSE](LICENSE).
