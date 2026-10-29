import React, { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import {
  Disc3, ListMusic, ListPlus, ListEnd, Music2, Pause, Play, Search, SkipBack, SkipForward, Smartphone,
  Trash2, Users, Volume1, Volume2, VolumeX, ChevronLeft, ChevronUp, ChevronDown, X, Loader2, HardDriveDownload,
} from 'lucide-react';
import { artUrl, errorText, music, on } from './api.mjs';
import { formatLength, formatQuality, formatTime, groupAlbums, groupArtists, matcher, sortTracks } from './library.mjs';
import { ownWindowControls } from './platform.mjs';
import WindowControls from './WindowControls.jsx';
import Devices from './Devices.jsx';
import { IconButton } from './ui.jsx';

/** The PC app: the phone's library, the queue, and the player. */
export default function Desktop() {
  const [tracks, setTracks] = useState(null);
  const [remote, setRemote] = useState(null);
  const [player, setPlayer] = useState(null);
  const [view, setView] = useState({ name: 'albums' });
  const [query, setQuery] = useState('');
  const [devicesOpen, setDevicesOpen] = useState(false);
  const [error, setError] = useState('');
  const mainRef = useRef(null);

  const loadLibrary = useCallback(() => music('library').then(r => setTracks(r.tracks), e => setError(errorText(e))), []);
  const loadRemote = useCallback(() => music('remoteStatus').then(setRemote, () => {}), []);
  useEffect(() => {
    loadLibrary();
    loadRemote();
    music('state').then(setPlayer, e => setError(errorText(e)));
    const offs = [
      on('library-event', () => { loadLibrary(); loadRemote(); }),
      on('core-event', loadRemote),
      on('player', setPlayer),
    ];
    const timer = setInterval(loadRemote, 5000);
    return () => { offs.forEach(f => f()); clearInterval(timer); };
  }, [loadLibrary, loadRemote]);

  const run = useCallback(async (method, params) => {
    try { setError(''); return await music(method, params); } catch (e) { setError(errorText(e)); }
  }, []);

  // Space toggles playback anywhere but in a text field.
  useEffect(() => {
    const key = e => {
      if (e.code === 'Space' && !e.target.closest('input, textarea, button, [contenteditable]')) {
        e.preventDefault();
        run('toggle');
      }
    };
    window.addEventListener('keydown', key);
    return () => window.removeEventListener('keydown', key);
  }, [run]);

  const albums = useMemo(() => groupAlbums(tracks ?? []), [tracks]);
  const artists = useMemo(() => groupArtists(albums), [albums]);
  const byId = useMemo(() => new Map((tracks ?? []).map(t => [t.id, t])), [tracks]);
  const queue = useMemo(() => (player?.queue ?? []).map(id => byId.get(id)).filter(Boolean), [player, byId]);
  const current = player?.index != null ? byId.get(player.queue[player.index]) : null;

  const go = next => { setView(next); mainRef.current?.scrollTo(0, 0); };
  const openAlbum = key => go({ name: 'album', key, back: view });
  const play = (list, start = 0) => run('play', { ids: list.map(t => t.id), start });
  const actions = {
    play,
    playNext: list => run('enqueue', { ids: list.map(t => t.id), next: true }),
    addToQueue: list => run('enqueue', { ids: list.map(t => t.id), next: false }),
    openAlbum,
    currentId: current?.id,
    playing: player?.playing,
  };

  const match = matcher(query);
  let content;
  if (tracks === null) content = <p className="empty">Loading…</p>;
  else if (!tracks.length) content = <EmptyLibrary remote={remote} onDevices={() => setDevicesOpen(true)} />;
  else if (match) content = <SearchResults albums={albums} match={match} actions={actions} />;
  else if (view.name === 'album') {
    const album = albums.find(a => a.key === view.key);
    content = album ? <AlbumPage album={album} actions={actions} onBack={() => go(view.back ?? { name: 'albums' })} /> : <p className="empty">This album is no longer in the library.</p>;
  } else if (view.name === 'artist') {
    const artist = artists.find(a => a.key === view.key);
    content = artist
      ? <section className="page"><button type="button" className="back" onClick={() => go({ name: 'artists' })}><ChevronLeft size={18} /> Artists</button>
          <h1 className="page-title">{artist.name}</h1>
          <p className="page-sub">{artist.albums.length} {artist.albums.length === 1 ? 'album' : 'albums'} · {artist.tracks} tracks</p>
          <AlbumGrid albums={artist.albums} actions={{ ...actions, openAlbum: key => go({ name: 'album', key, back: view }) }} /></section>
      : <p className="empty">Not found.</p>;
  } else if (view.name === 'artists') content = <ArtistList artists={artists} onOpen={key => go({ name: 'artist', key })} />;
  else if (view.name === 'songs') content = <section className="page"><h1 className="page-title">Songs</h1><TrackTable tracks={sortTracks(albums)} actions={actions} showAlbum showArtist /></section>;
  else if (view.name === 'queue') content = <QueuePage queue={queue} player={player} run={run} />;
  else content = <section className="page"><h1 className="page-title">Albums</h1><AlbumGrid albums={albums} actions={actions} /></section>;

  const nav = (name, Icon, label, extra) => (
    <button type="button" className={`nav-item ${view.name === name || (name === 'albums' && view.name === 'album') || (name === 'artists' && view.name === 'artist') ? 'active' : ''}`}
      onClick={() => { setQuery(''); go({ name }); }}>
      <Icon size={18} strokeWidth={1.75} aria-hidden="true" /> <span>{label}</span>{extra}
    </button>
  );

  return (
    <div className="desktop">
      <header className="topbar" data-tauri-drag-region>
        <div className="brand" data-tauri-drag-region><Music2 size={20} strokeWidth={2} aria-hidden="true" /> music-stream</div>
        <label className="search">
          <Search size={16} aria-hidden="true" />
          <input type="search" placeholder="Search songs, albums, artists" value={query} onChange={e => setQuery(e.target.value)}
            onKeyDown={e => { if (e.key === 'Escape') setQuery(''); }} />
        </label>
        <div className="topbar-spacer" data-tauri-drag-region />
        <button type="button" className={`source ${remote?.source ? 'on' : ''}`} onClick={() => setDevicesOpen(true)} title="Devices">
          <Smartphone size={16} aria-hidden="true" />
          <span className="status-dot" />
          {remote?.source ?? 'Phone not connected'}
        </button>
        {ownWindowControls() && <WindowControls />}
      </header>

      <nav className="sidebar">
        {nav('albums', Disc3, 'Albums')}
        {nav('artists', Users, 'Artists')}
        {nav('songs', Music2, 'Songs')}
        {nav('queue', ListMusic, 'Queue', queue.length ? <span className="count">{queue.length}</span> : null)}
        <div className="sidebar-foot">
          {tracks && <span>{tracks.length} tracks</span>}
          {remote && <span>{(remote.cacheBytes / 1e9).toFixed(1)} of {(remote.cacheLimit / 1e9).toFixed(0)} GB cached</span>}
        </div>
      </nav>

      <main className="content" ref={mainRef}>
        {error && <p className="banner error" role="alert">{error} <IconButton icon={X} label="Dismiss" onClick={() => setError('')} /></p>}
        {content}
      </main>

      <PlayerBar player={player} track={current} run={run} onOpenAlbum={() => current && openAlbum(albums.find(a => a.tracks.some(t => t.id === current.id))?.key)}
        onQueue={() => go({ name: 'queue' })} />

      {devicesOpen && <Devices onClose={() => setDevicesOpen(false)} />}
    </div>
  );
}

function Cover({ hash, size = 'md', title = '' }) {
  const [failed, setFailed] = useState(false);
  const url = artUrl(hash);
  return url && !failed
    ? <img className={`cover cover-${size}`} src={url} alt="" loading="lazy" decoding="async" onError={() => setFailed(true)} />
    : <div className={`cover cover-${size} cover-empty`} aria-hidden="true"><Disc3 strokeWidth={1.25} />{size !== 'sm' && <span>{title.slice(0, 40)}</span>}</div>;
}

function EmptyLibrary({ remote, onDevices }) {
  return (
    <div className="empty-state">
      <Smartphone size={40} strokeWidth={1.25} />
      <h2>{remote?.source ? 'Waiting for the phone’s library…' : 'Connect your phone'}</h2>
      <p className="hint">
        {remote?.source
          ? 'The phone is connected and is listing its music. This takes a moment the first time.'
          : 'Install music-stream on your phone, then pair it with this computer: open Devices here, show this computer’s code, and scan it from the phone.'}
      </p>
      {!remote?.source && <button type="button" className="primary" onClick={onDevices}>Devices</button>}
    </div>
  );
}

function AlbumGrid({ albums, actions }) {
  return (
    <ul className="album-grid">
      {albums.map(a => (
        <li key={a.key}>
          <div className="album-card">
            <button type="button" className="album-open" onClick={() => actions.openAlbum(a.key)} title={`${a.title} — ${a.artist}`}>
              <Cover hash={a.art} title={a.title} />
              <strong>{a.title}</strong>
              <small>{a.artist}{a.year ? ` · ${a.year}` : ''}</small>
            </button>
            <button type="button" className="album-play" aria-label={`Play ${a.title}`} title="Play" onClick={() => actions.play(a.tracks)}>
              <Play size={20} fill="currentColor" />
            </button>
          </div>
        </li>
      ))}
    </ul>
  );
}

function AlbumPage({ album, actions, onBack }) {
  const quality = formatQuality(album.tracks[0]);
  return (
    <section className="page">
      <button type="button" className="back" onClick={onBack}><ChevronLeft size={18} /> Back</button>
      <div className="album-head">
        <Cover hash={album.art} size="lg" title={album.title} />
        <div className="album-meta">
          <small className="kicker">Album</small>
          <h1>{album.title}</h1>
          <p>{album.artist}{album.year ? ` · ${album.year}` : ''} · {album.tracks.length} tracks · {formatLength(album.durationMs)}</p>
          <p className="hint">{quality}{album.cached === album.tracks.length ? ' · available offline' : ''}</p>
          <div className="row">
            <button type="button" className="primary" onClick={() => actions.play(album.tracks)}><Play size={16} fill="currentColor" /> Play</button>
            <button type="button" onClick={() => actions.playNext(album.tracks)}><ListPlus size={16} /> Play next</button>
            <button type="button" onClick={() => actions.addToQueue(album.tracks)}><ListEnd size={16} /> Add to queue</button>
          </div>
        </div>
      </div>
      <TrackTable tracks={album.tracks} actions={actions} numbered showArtist={album.tracks.some(t => t.artist && t.artist !== album.artist)}
        discs={new Set(album.tracks.map(t => t.disc || 1)).size > 1} />
    </section>
  );
}

function TrackTable({ tracks, actions, numbered, showAlbum, showArtist, discs }) {
  let lastDisc = null;
  return (
    <div className="tracks" role="table">
      {tracks.map((t, i) => {
        const discRow = discs && t.disc !== lastDisc ? <div className="disc-row" key={`disc-${t.disc}`}>Disc {t.disc || 1}</div> : null;
        lastDisc = t.disc;
        const playingHere = actions.currentId === t.id;
        return (
          <React.Fragment key={t.id}>
            {discRow}
            <div className={`track-row ${playingHere ? 'current' : ''}`} role="row" tabIndex={0}
              onDoubleClick={() => actions.play(tracks, i)} onKeyDown={e => { if (e.key === 'Enter') actions.play(tracks, i); }}>
              <span className="col-num">
                <span className="num">{playingHere ? <span className={`eq ${actions.playing ? 'on' : ''}`}><i /><i /><i /></span> : numbered ? (t.track || '–') : i + 1}</span>
                <button type="button" className="row-play" aria-label={`Play ${t.title}`} onClick={() => actions.play(tracks, i)}><Play size={14} fill="currentColor" /></button>
              </span>
              <span className="col-title">
                <strong>{t.title}</strong>
                {showArtist && <small>{t.artist}</small>}
              </span>
              {showAlbum && <button type="button" className="col-album link" onClick={() => actions.openAlbum(albumKeyOf(t))}>{t.album}</button>}
              <span className="col-actions">
                {t.cached && <HardDriveDownload size={14} className="cached-mark" aria-label="Available offline" />}
                <IconButton icon={ListPlus} label="Play next" onClick={() => actions.playNext([t])} />
                <IconButton icon={ListEnd} label="Add to queue" onClick={() => actions.addToQueue([t])} />
              </span>
              <span className="col-time">{formatTime(t.durationMs)}</span>
            </div>
          </React.Fragment>
        );
      })}
    </div>
  );
}

const albumKeyOf = t => `${(t.albumArtist || t.artist || 'Unknown artist').toLowerCase()}\u0000${(t.album || 'Unknown album').toLowerCase()}`;

function ArtistList({ artists, onOpen }) {
  return (
    <section className="page">
      <h1 className="page-title">Artists</h1>
      <ul className="artist-grid">
        {artists.map(a => (
          <li key={a.key}>
            <button type="button" className="artist-card" onClick={() => onOpen(a.key)}>
              <Cover hash={a.art} size="round" title={a.name} />
              <strong>{a.name}</strong>
              <small>{a.albums.length} {a.albums.length === 1 ? 'album' : 'albums'}</small>
            </button>
          </li>
        ))}
      </ul>
    </section>
  );
}

function SearchResults({ albums, match, actions }) {
  const songs = sortTracks(albums).filter(match);
  const found = albums.filter(a => a.tracks.some(match) && match({ title: '', artist: a.artist, albumArtist: '', album: a.title }));
  return (
    <section className="page">
      {!songs.length && !found.length && <p className="empty">Nothing found.</p>}
      {found.length > 0 && <><h2 className="section-title">Albums</h2><AlbumGrid albums={found.slice(0, 24)} actions={actions} /></>}
      {songs.length > 0 && <><h2 className="section-title">Songs</h2><TrackTable tracks={songs.slice(0, 300)} actions={actions} showAlbum showArtist /></>}
    </section>
  );
}

function QueuePage({ queue, player, run }) {
  if (!queue.length) return <section className="page"><h1 className="page-title">Queue</h1><p className="empty">Nothing queued. Play an album or a song to start.</p></section>;
  return (
    <section className="page">
      <div className="row page-title-row">
        <h1 className="page-title">Queue</h1>
        <button type="button" onClick={() => run('clear')}><Trash2 size={16} /> Clear</button>
      </div>
      <div className="tracks">
        {queue.map((t, i) => (
          <div key={`${t.id}-${i}`} className={`track-row queue-row ${i === player?.index ? 'current' : ''} ${player?.index != null && i < player.index ? 'played' : ''}`}
            onDoubleClick={() => run('jump', { index: i })}>
            <span className="col-num">
              <span className="num">{i === player?.index ? <span className={`eq ${player.playing ? 'on' : ''}`}><i /><i /><i /></span> : i + 1}</span>
              <button type="button" className="row-play" aria-label={`Play ${t.title}`} onClick={() => run('jump', { index: i })}><Play size={14} fill="currentColor" /></button>
            </span>
            <Cover hash={t.art} size="sm" />
            <span className="col-title"><strong>{t.title}</strong><small>{t.artist} · {t.album}</small></span>
            <span className="col-actions">
              <IconButton icon={ChevronUp} label="Move up" disabled={i === 0} onClick={() => run('move', { from: i, to: i - 1 })} />
              <IconButton icon={ChevronDown} label="Move down" disabled={i === queue.length - 1} onClick={() => run('move', { from: i, to: i + 1 })} />
              <IconButton icon={X} label="Remove from queue" onClick={() => run('remove', { index: i })} />
            </span>
            <span className="col-time">{formatTime(t.durationMs)}</span>
          </div>
        ))}
      </div>
    </section>
  );
}

function PlayerBar({ player, track, run, onOpenAlbum, onQueue }) {
  const [drag, setDrag] = useState(null);
  const [volume, setVolume] = useState(null);
  const duration = player?.durationMs || track?.durationMs || 0;
  const position = drag ?? player?.positionMs ?? 0;
  const vol = volume ?? player?.volume ?? 0.8;
  const VolumeIcon = vol === 0 ? VolumeX : vol < 0.5 ? Volume1 : Volume2;
  const setVol = v => { setVolume(v); run('setVolume', { volume: v }).then(() => setVolume(null)); };
  const lastVol = useRef(0.8);

  return (
    <footer className="playerbar">
      <div className="now">
        {track ? <>
          <button type="button" className="now-art" onClick={onOpenAlbum} aria-label="Open the album"><Cover hash={track.art} size="sm" /></button>
          <div className="now-text">
            <strong title={track.title}>{track.title}</strong>
            <small>{track.artist}{track.album ? ` — ${track.album}` : ''}</small>
          </div>
        </> : <div className="now-text"><small>Nothing playing</small></div>}
      </div>

      <div className="transport">
        <div className="buttons">
          <IconButton icon={SkipBack} label="Previous" onClick={() => run('prev')} disabled={!track} />
          <button type="button" className="play-button" aria-label={player?.playing ? 'Pause' : 'Play'} onClick={() => run('toggle')} disabled={!player?.queue?.length}>
            {player?.playing && player?.buffering ? <Loader2 size={22} className="spin" />
              : player?.playing ? <Pause size={22} fill="currentColor" /> : <Play size={22} fill="currentColor" />}
          </button>
          <IconButton icon={SkipForward} label="Next" onClick={() => run('next')} disabled={!track} />
        </div>
        <div className="seek">
          <span className="time">{formatTime(position)}</span>
          <input type="range" min={0} max={Math.max(1, duration)} step={250} value={Math.min(position, duration)} disabled={!track}
            style={{ '--fill': `${duration ? (position / duration) * 100 : 0}%` }}
            onChange={e => setDrag(Number(e.target.value))}
            onPointerUp={e => { run('seek', { ms: Number(e.target.value) }); setTimeout(() => setDrag(null), 400); }}
            onKeyUp={e => { if (drag != null) { run('seek', { ms: drag }); setTimeout(() => setDrag(null), 400); } }} />
          <span className="time">{formatTime(duration)}</span>
        </div>
        {player?.error && <small className="error player-error" title={player.error}>{player.error}</small>}
      </div>

      <div className="side-controls">
        {track && <small className="quality" title={player?.output}>{formatQuality(track)}</small>}
        <IconButton icon={ListMusic} label="Queue" onClick={onQueue} />
        <IconButton icon={VolumeIcon} label={vol === 0 ? 'Unmute' : 'Mute'}
          onClick={() => { if (vol > 0) { lastVol.current = vol; setVol(0); } else setVol(lastVol.current || 0.8); }} />
        <input type="range" className="volume" min={0} max={1} step={0.01} value={vol} aria-label="Volume"
          style={{ '--fill': `${vol * 100}%` }} onChange={e => setVol(Number(e.target.value))} />
      </div>
    </footer>
  );
}
