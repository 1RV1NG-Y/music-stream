import React, { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import {
  ArrowDownWideNarrow, ArrowUpNarrowWide, ChevronLeft, Disc3, Ellipsis, Flame, Heart, History, Keyboard, ListEnd, ListMusic,
  ListPlus, Loader2, Music2, Pause, Pencil, Play, Plus, Repeat, Repeat1, Search, Shuffle, SkipBack, SkipForward, Smartphone,
  Sparkles, Trash2, TrendingUp, Users, Volume1, Volume2, VolumeX, X, HardDriveDownload, GripVertical,
} from 'lucide-react';
import { artUrl, errorText, music, on } from './api.mjs';
import {
  SONG_SORTS, albumKey, formatLength, formatQuality, formatTime, groupAlbums, groupArtists, matcher, sortSongs, sortTracks, tracksFor,
} from './library.mjs';
import { ownWindowControls } from './platform.mjs';
import WindowControls from './WindowControls.jsx';
import Devices from './Devices.jsx';
import { Dialog, IconButton } from './ui.jsx';

const SMART_ICONS = { 'on-repeat': Flame, 'recently-played': History, 'recently-added': Sparkles, 'most-played': TrendingUp };
const stored = (key, fallback) => { try { return JSON.parse(localStorage.getItem(key)) ?? fallback; } catch { return fallback; } };
const store = (key, value) => { try { localStorage.setItem(key, JSON.stringify(value)); } catch { /* per-viewer convenience only */ } };

/** The PC app: the phone's library, playlists, the queue, and the player. */
export default function Desktop() {
  const [tracks, setTracks] = useState(null);
  const [lists, setLists] = useState({ liked: [], smart: [], user: [] });
  const [remote, setRemote] = useState(null);
  const [player, setPlayer] = useState(null);
  const [view, setView] = useState({ name: 'songs' });
  const [query, setQuery] = useState('');
  const [devicesOpen, setDevicesOpen] = useState(false);
  const [queueOpen, setQueueOpen] = useState(false);
  const [helpOpen, setHelpOpen] = useState(false);
  const [menu, setMenu] = useState(null);
  const [naming, setNaming] = useState(null);
  const [error, setError] = useState('');
  const mainRef = useRef(null);
  const searchRef = useRef(null);

  const loadLibrary = useCallback(() => music('library').then(r => setTracks(r.tracks), e => setError(errorText(e))), []);
  const loadLists = useCallback(() => music('playlists').then(setLists, () => {}), []);
  const loadRemote = useCallback(() => music('remoteStatus').then(setRemote, () => {}), []);
  useEffect(() => {
    loadLibrary();
    loadLists();
    loadRemote();
    music('state').then(setPlayer, e => setError(errorText(e)));
    const offs = [
      on('library-event', () => { loadLibrary(); loadLists(); loadRemote(); }),
      on('core-event', e => { loadRemote(); if (e === 'libraryChanged') { loadLibrary(); loadLists(); } }),
      on('player', setPlayer),
    ];
    const timer = setInterval(loadRemote, 5000);
    return () => { offs.forEach(f => f()); clearInterval(timer); };
  }, [loadLibrary, loadLists, loadRemote]);

  const run = useCallback(async (method, params) => {
    try { setError(''); return await music(method, params); } catch (e) { setError(errorText(e)); }
  }, []);

  const albums = useMemo(() => groupAlbums(tracks ?? []), [tracks]);
  const artists = useMemo(() => groupArtists(albums), [albums]);
  const byId = useMemo(() => new Map((tracks ?? []).map(t => [t.id, t])), [tracks]);
  const queue = useMemo(() => tracksFor(player?.queue ?? [], byId), [player, byId]);
  const current = player?.index != null ? byId.get(player.queue[player.index]) : null;

  const go = next => { setView(next); setQuery(''); mainRef.current?.scrollTo(0, 0); };
  const ids = list => list.map(t => t.id);
  const toggleLike = async list => {
    const allLiked = list.every(t => t.liked);
    await run(allLiked ? 'unlike' : 'like', { ids: ids(list) });
    loadLibrary();
    loadLists();
  };
  const actions = {
    play: (list, start = 0) => run('play', { ids: ids(list), start }),
    shuffle: list => run('play', { ids: ids(list), shuffle: true }),
    playNext: list => run('enqueue', { ids: ids(list), next: true }),
    addToQueue: list => run('enqueue', { ids: ids(list), next: false }),
    toggleLike,
    openAlbum: key => go({ name: 'album', key, back: view }),
    openArtist: name => go({ name: 'artist', key: name.toLowerCase() }),
    menu: (e, list, extra) => { e.preventDefault(); e.stopPropagation(); setMenu({ at: { x: e.clientX, y: e.clientY }, tracks: list, extra }); },
    currentId: current?.id,
    playing: player?.playing,
  };

  // Keyboard shortcuts (the ? sheet lists them).
  const playerRef = useRef(player);
  playerRef.current = player;
  useEffect(() => {
    const key = e => {
      if (e.key === 'Escape') { setMenu(null); if (document.activeElement === searchRef.current) { setQuery(''); searchRef.current.blur(); } return; }
      if (e.target.closest('input, textarea, [contenteditable]') && !(e.target.type === 'range')) return;
      if (e.target.closest('dialog, [role="dialog"]')) return;
      const p = playerRef.current;
      const ctrl = e.ctrlKey || e.metaKey;
      const handled = () => e.preventDefault();
      if (e.code === 'Space' && !e.target.closest('button')) { handled(); run('toggle'); }
      else if (ctrl && e.key === 'ArrowRight') { handled(); run('next'); }
      else if (ctrl && e.key === 'ArrowLeft') { handled(); run('prev'); }
      else if (ctrl && e.key === 'ArrowUp') { handled(); run('setVolume', { volume: Math.min(1, (p?.volume ?? 0.8) + 0.05) }); }
      else if (ctrl && e.key === 'ArrowDown') { handled(); run('setVolume', { volume: Math.max(0, (p?.volume ?? 0.8) - 0.05) }); }
      else if (ctrl && e.key.toLowerCase() === 'f') { handled(); searchRef.current?.focus(); }
      else if (ctrl) return;
      else if (e.key === 'ArrowRight' && e.target.type !== 'range') { handled(); run('seek', { ms: (p?.positionMs ?? 0) + 5000 }); }
      else if (e.key === 'ArrowLeft' && e.target.type !== 'range') { handled(); run('seek', { ms: Math.max(0, (p?.positionMs ?? 0) - 5000) }); }
      else if (e.key === '/') { handled(); searchRef.current?.focus(); }
      else if (e.key.toLowerCase() === 's') { handled(); run('setShuffle', { on: !p?.shuffle }); }
      else if (e.key.toLowerCase() === 'r') { handled(); run('setRepeat', { mode: { off: 'all', all: 'one', one: 'off' }[p?.repeat ?? 'off'] }); }
      else if (e.key.toLowerCase() === 'm') { handled(); run('setVolume', { volume: p?.volume > 0 ? 0 : 0.8 }); }
      else if (e.key.toLowerCase() === 'l') { const t = currentRef.current; if (t) { handled(); toggleLike([t]); } }
      else if (e.key.toLowerCase() === 'q') { handled(); setQueueOpen(o => !o); }
      else if (e.key === '?') { handled(); setHelpOpen(true); }
    };
    window.addEventListener('keydown', key);
    return () => window.removeEventListener('keydown', key);
  }, [run]); // eslint-disable-line react-hooks/exhaustive-deps
  const currentRef = useRef(current);
  currentRef.current = current;

  const listFor = v => {
    if (v.name === 'liked') return { title: 'Liked songs', hint: 'Songs you liked. They sync to your other computers through the phone.', tracks: tracksFor(lists.liked, byId), icon: Heart, kind: 'liked' };
    if (v.name === 'smart') {
      const s = lists.smart.find(x => x.id === v.id);
      return s && { title: s.name, hint: s.hint, tracks: tracksFor(s.tracks, byId), icon: SMART_ICONS[s.id] ?? Sparkles, kind: 'smart' };
    }
    if (v.name === 'playlist') {
      const pl = lists.user.find(x => x.id === v.id);
      return pl && { title: pl.name, playlist: pl, tracks: tracksFor(pl.entries.map(e => e.id).filter(Boolean), byId), icon: ListMusic, kind: 'user' };
    }
    return null;
  };

  const match = matcher(query);
  let content;
  if (tracks === null) content = <p className="empty">Loading…</p>;
  else if (!tracks.length) content = <EmptyLibrary remote={remote} onDevices={() => setDevicesOpen(true)} />;
  else if (match) content = <SearchResults albums={albums} match={match} actions={actions} />;
  else if (view.name === 'songs') content = <SongsPage albums={albums} actions={actions} />;
  else if (['liked', 'smart', 'playlist'].includes(view.name)) {
    const l = listFor(view);
    content = l ? <ListPage list={l} actions={actions} run={run} onRename={() => setNaming({ playlist: l.playlist })}
      onDeleted={() => { loadLists(); go({ name: 'songs' }); }} reload={loadLists} /> : <p className="empty">This playlist no longer exists.</p>;
  } else if (view.name === 'album') {
    const album = albums.find(a => a.key === view.key);
    content = album ? <AlbumPage album={album} actions={actions} onBack={() => go(view.back ?? { name: 'albums' })} /> : <p className="empty">This album is no longer in the library.</p>;
  } else if (view.name === 'artist') {
    const artist = artists.find(a => a.key === view.key);
    content = artist
      ? <section className="page"><button type="button" className="back" onClick={() => go({ name: 'artists' })}><ChevronLeft size={18} /> Artists</button>
          <h1 className="page-title">{artist.name}</h1>
          <p className="page-sub">{artist.albums.length} {artist.albums.length === 1 ? 'album' : 'albums'} · {artist.tracks} songs</p>
          <div className="row page-actions">
            <button type="button" className="primary" onClick={() => actions.play(artist.albums.flatMap(a => a.tracks))}><Play size={16} fill="currentColor" /> Play</button>
            <button type="button" onClick={() => actions.shuffle(artist.albums.flatMap(a => a.tracks))}><Shuffle size={16} /> Shuffle</button>
          </div>
          <AlbumGrid albums={artist.albums} actions={{ ...actions, openAlbum: key => go({ name: 'album', key, back: view }) }} /></section>
      : <p className="empty">Not found.</p>;
  } else if (view.name === 'artists') content = <ArtistList artists={artists} onOpen={key => go({ name: 'artist', key })} />;
  else content = <section className="page"><h1 className="page-title">Albums</h1><AlbumGrid albums={albums} actions={actions} /></section>;

  const isView = (name, id) => view.name === name && (id === undefined || view.id === id);
  const navItem = (active, Icon, label, onClick, extra) => (
    <button type="button" className={`nav-item ${active ? 'active' : ''}`} onClick={onClick}>
      <Icon size={18} strokeWidth={1.75} aria-hidden="true" /> <span>{label}</span>{extra}
    </button>
  );

  return (
    <div className={`desktop ${queueOpen ? 'with-queue' : ''}`} onClick={() => menu && setMenu(null)}>
      <header className="topbar" data-tauri-drag-region>
        <div className="brand" data-tauri-drag-region><Music2 size={20} strokeWidth={2} aria-hidden="true" /> music-stream</div>
        <label className="search">
          <Search size={16} aria-hidden="true" />
          <input ref={searchRef} type="search" placeholder="Search songs, albums, artists" value={query} onChange={e => setQuery(e.target.value)}
            onKeyDown={e => { if (e.key === 'Escape') { setQuery(''); e.target.blur(); } }} />
        </label>
        <div className="topbar-spacer" data-tauri-drag-region />
        <IconButton icon={Keyboard} label="Keyboard shortcuts (?)" onClick={() => setHelpOpen(true)} />
        <button type="button" className={`source ${remote?.source ? 'on' : ''}`} onClick={() => setDevicesOpen(true)} title="Devices">
          <Smartphone size={16} aria-hidden="true" />
          <span className="status-dot" />
          {remote?.source ?? 'Phone not connected'}
        </button>
        {ownWindowControls() && <WindowControls />}
      </header>

      <nav className="sidebar">
        {navItem(isView('songs'), Music2, 'Songs', () => go({ name: 'songs' }), tracks ? <span className="count">{tracks.length}</span> : null)}
        <div className="nav-section">
          <span>Playlists</span>
          <IconButton icon={Plus} label="New playlist" onClick={() => setNaming({})} />
        </div>
        {navItem(isView('liked'), Heart, 'Liked songs', () => go({ name: 'liked' }), lists.liked.length ? <span className="count">{lists.liked.length}</span> : null)}
        {lists.smart.map(s => navItem(isView('smart', s.id), SMART_ICONS[s.id] ?? Sparkles, s.name, () => go({ name: 'smart', id: s.id })))}
        {lists.user.map(pl => (
          <div key={pl.id} onContextMenu={e => { e.preventDefault(); setMenu({ at: { x: e.clientX, y: e.clientY }, playlist: pl }); }}
            onDragOver={e => e.preventDefault()}
            onDrop={e => { const dragged = e.dataTransfer.getData('text/x-track-ids'); if (dragged) { e.preventDefault(); run('playlistAdd', { id: pl.id, ids: JSON.parse(dragged) }).then(loadLists); } }}>
            {navItem(isView('playlist', pl.id), ListMusic, pl.name, () => go({ name: 'playlist', id: pl.id }), <span className="count">{pl.entries.length}</span>)}
          </div>
        ))}
        <div className="nav-section"><span>Library</span></div>
        {navItem(isView('albums') || isView('album'), Disc3, 'Albums', () => go({ name: 'albums' }))}
        {navItem(isView('artists') || isView('artist'), Users, 'Artists', () => go({ name: 'artists' }))}
        <div className="sidebar-foot">
          {remote && <span title="Songs you’ve played stay here, so they start at once and play without the phone. When it’s full, the songs played longest ago make room.">{(remote.cacheBytes / 1e9).toFixed(1)} of {(remote.cacheLimit / 1e9).toFixed(1)} GB cached</span>}
        </div>
      </nav>

      <main className="content" ref={mainRef}>
        {error && <p className="banner error" role="alert">{error} <IconButton icon={X} label="Dismiss" onClick={() => setError('')} /></p>}
        {content}
      </main>

      {queueOpen && <QueuePanel queue={queue} player={player} run={run} actions={actions} onClose={() => setQueueOpen(false)} />}

      <PlayerBar player={player} track={current} run={run} actions={actions} queueOpen={queueOpen}
        onOpenAlbum={() => current && actions.openAlbum(albumKey(current))} onQueue={() => setQueueOpen(o => !o)} />

      {menu && <TrackMenu menu={menu} lists={lists} actions={actions} run={run} reload={loadLists} onNew={tracks => setNaming({ tracks })}
        onRenamePlaylist={pl => setNaming({ playlist: pl })} onOpenPlaylist={id => go({ name: 'playlist', id })} onClose={() => setMenu(null)} />}
      {naming && <NameDialog naming={naming} run={run} onClose={() => setNaming(null)}
        onDone={id => { setNaming(null); loadLists(); if (id) go({ name: 'playlist', id }); }} />}
      {helpOpen && <ShortcutsHelp onClose={() => setHelpOpen(false)} />}
      {devicesOpen && <Devices onClose={() => setDevicesOpen(false)} />}
    </div>
  );
}

function Cover({ hash, size = 'md', title = '' }) {
  const [failed, setFailed] = useState(false);
  const url = artUrl(hash);
  return url && !failed
    ? <img className={`cover cover-${size}`} src={url} alt="" loading="lazy" decoding="async" draggable={false} onError={() => setFailed(true)} />
    : <div className={`cover cover-${size} cover-empty`} aria-hidden="true"><Disc3 strokeWidth={1.25} />{(size === 'md' || size === 'lg') && <span>{title.slice(0, 40)}</span>}</div>;
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

function SongsPage({ albums, actions }) {
  const [sort, setSort] = useState(() => stored('songs.sort', { by: 'default', desc: false }));
  const change = next => { setSort(next); store('songs.sort', next); };
  const songs = useMemo(() => sortSongs(albums, sort.by, sort.desc), [albums, sort]);
  const total = songs.reduce((n, t) => n + t.durationMs, 0);
  return (
    <section className="page">
      <div className="list-head">
        <div className="list-icon"><Music2 size={40} /></div>
        <div>
          <small className="kicker">Library</small>
          <h1>Songs</h1>
          <p className="page-sub">{songs.length} songs · {formatLength(total)}</p>
        </div>
      </div>
      <div className="toolbar-row">
        <button type="button" className="primary" onClick={() => actions.play(songs)}><Play size={16} fill="currentColor" /> Play</button>
        <button type="button" onClick={() => actions.shuffle(songs)}><Shuffle size={16} /> Shuffle</button>
        <div className="spacer" />
        <div className="segmented" role="radiogroup" aria-label="Sort by">
          {SONG_SORTS.map(s => (
            <button key={s.id} type="button" role="radio" aria-checked={sort.by === s.id} className={sort.by === s.id ? 'on' : ''}
              onClick={() => change({ by: s.id, desc: sort.by === s.id ? !sort.desc : s.id === 'added' })}>{s.label}</button>
          ))}
        </div>
        <IconButton icon={sort.desc ? ArrowDownWideNarrow : ArrowUpNarrowWide} label={sort.desc ? 'Descending' : 'Ascending'}
          onClick={() => change({ ...sort, desc: !sort.desc })} />
      </div>
      <TrackTable tracks={songs} actions={actions} showAlbum showArtist showAdded={sort.by === 'added'} />
    </section>
  );
}

function ListPage({ list, actions, run, onRename, onDeleted, reload }) {
  const { title, hint, tracks, icon: Icon, kind, playlist } = list;
  const total = tracks.reduce((n, t) => n + t.durationMs, 0);
  const missing = playlist ? playlist.entries.filter(e => !e.id) : [];
  // Positions in the stored playlist of the tracks shown (missing entries are skipped).
  const positions = playlist ? playlist.entries.map((e, i) => (e.id ? i : -1)).filter(i => i >= 0) : [];
  return (
    <section className="page">
      <div className="list-head">
        <div className={`list-icon ${kind}`}><Icon size={40} fill={kind === 'liked' ? 'currentColor' : 'none'} /></div>
        <div>
          <small className="kicker">{kind === 'smart' ? 'Made for you' : 'Playlist'}</small>
          <h1>{title}</h1>
          <p className="page-sub">{tracks.length} songs · {formatLength(total)}{hint ? ` · ${hint}` : ''}</p>
        </div>
      </div>
      <div className="toolbar-row">
        <button type="button" className="primary" disabled={!tracks.length} onClick={() => actions.play(tracks)}><Play size={16} fill="currentColor" /> Play</button>
        <button type="button" disabled={!tracks.length} onClick={() => actions.shuffle(tracks)}><Shuffle size={16} /> Shuffle</button>
        {kind === 'user' && <>
          <div className="spacer" />
          <IconButton icon={Pencil} label="Rename" onClick={onRename} />
          <IconButton icon={Trash2} label="Delete playlist" onClick={async () => {
            if (confirm(`Delete “${title}”? This removes it on your other computers too.`)) { await run('deletePlaylist', { id: playlist.id }); onDeleted(); }
          }} />
        </>}
      </div>
      {!tracks.length && <p className="empty">{kind === 'smart' ? 'Nothing here yet: it fills up as you listen.' : kind === 'liked' ? 'Like songs with the heart to find them here.' : 'Add songs from their ⋯ menu, or drag them onto this playlist in the sidebar.'}</p>}
      <TrackTable tracks={tracks} actions={actions} showAlbum showArtist numbered={false}
        onRemove={kind === 'user' ? i => run('playlistRemove', { id: playlist.id, positions: [positions[i]] }).then(reload) : undefined}
        onMove={kind === 'user' ? (from, to) => run('playlistMove', { id: playlist.id, from: positions[from], to: positions[to] }).then(reload) : undefined} />
      {missing.length > 0 && <p className="hint missing-note">{missing.length} {missing.length === 1 ? 'song is' : 'songs are'} no longer on the phone: {missing.map(m => m.title).join(', ')}.</p>}
    </section>
  );
}

function AlbumGrid({ albums, actions }) {
  return (
    <ul className="album-grid">
      {albums.map(a => (
        <li key={a.key}>
          <div className="album-card" onContextMenu={e => actions.menu(e, a.tracks)}>
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
  return (
    <section className="page">
      <button type="button" className="back" onClick={onBack}><ChevronLeft size={18} /> Back</button>
      <div className="album-head">
        <Cover hash={album.art} size="lg" title={album.title} />
        <div className="album-meta">
          <small className="kicker">Album</small>
          <h1>{album.title}</h1>
          <p><button type="button" className="link artist-link" onClick={() => actions.openArtist(album.artist)}>{album.artist}</button>
            {album.year ? ` · ${album.year}` : ''} · {album.tracks.length} songs · {formatLength(album.durationMs)}</p>
          <p className="hint">{formatQuality(album.tracks[0])}{album.cached === album.tracks.length ? ' · available offline' : ''}</p>
          <div className="row">
            <button type="button" className="primary" onClick={() => actions.play(album.tracks)}><Play size={16} fill="currentColor" /> Play</button>
            <button type="button" onClick={() => actions.shuffle(album.tracks)}><Shuffle size={16} /> Shuffle</button>
            <IconButton icon={Ellipsis} label="More" onClick={e => actions.menu(e, album.tracks)} />
          </div>
        </div>
      </div>
      <TrackTable tracks={album.tracks} actions={actions} numbered showArtist={album.tracks.some(t => t.artist && t.artist !== album.artist)}
        discs={new Set(album.tracks.map(t => t.disc || 1)).size > 1} />
    </section>
  );
}

const dateFormat = new Intl.DateTimeFormat(undefined, { year: 'numeric', month: 'short', day: 'numeric' });

function TrackTable({ tracks, actions, numbered, showAlbum, showArtist, showAdded, discs, onRemove, onMove }) {
  const [dragOver, setDragOver] = useState(null);
  const dragFrom = useRef(null);
  let lastDisc = null;
  const columns = ['col-num', 'col-title', showAlbum && 'col-album', showAdded && 'col-added', 'col-actions', 'col-time'].filter(Boolean);
  return (
    <div className={`tracks cols-${columns.length} ${showAlbum ? 'has-album' : ''} ${showAdded ? 'has-added' : ''}`} role="table">
      {tracks.map((t, i) => {
        const discRow = discs && t.disc !== lastDisc ? <div className="disc-row" key={`disc-${t.disc}`}>Disc {t.disc || 1}</div> : null;
        lastDisc = t.disc;
        const playingHere = actions.currentId === t.id;
        return (
          <React.Fragment key={`${t.id}-${i}`}>
            {discRow}
            <div className={`track-row ${playingHere ? 'current' : ''} ${dragOver === i ? 'drag-over' : ''}`} role="row" tabIndex={0}
              draggable onDragStart={e => { dragFrom.current = i; e.dataTransfer.setData('text/x-track-ids', JSON.stringify([t.id])); e.dataTransfer.effectAllowed = 'copyMove'; }}
              onDragOver={onMove ? e => { e.preventDefault(); setDragOver(i); } : undefined}
              onDragLeave={onMove ? () => setDragOver(null) : undefined}
              onDrop={onMove ? e => { e.preventDefault(); setDragOver(null); if (dragFrom.current != null && dragFrom.current !== i) onMove(dragFrom.current, i); dragFrom.current = null; } : undefined}
              onDragEnd={() => { dragFrom.current = null; setDragOver(null); }}
              onDoubleClick={() => actions.play(tracks, i)} onKeyDown={e => { if (e.key === 'Enter') actions.play(tracks, i); }}
              onContextMenu={e => actions.menu(e, [t], { remove: onRemove && (() => onRemove(i)) })}>
              <span className="col-num">
                <span className="num">{playingHere ? <span className={`eq ${actions.playing ? 'on' : ''}`}><i /><i /><i /></span> : numbered ? (t.track || '–') : i + 1}</span>
                <button type="button" className="row-play" aria-label={`Play ${t.title}`} onClick={() => actions.play(tracks, i)}><Play size={14} fill="currentColor" /></button>
              </span>
              <span className="col-title">
                {!numbered && <Cover hash={t.art} size="sm" />}
                <span className="title-text">
                  <strong>{t.title}</strong>
                  {showArtist && <button type="button" className="link" onClick={() => actions.openArtist(t.albumArtist || t.artist)}>{t.artist}</button>}
                </span>
              </span>
              {showAlbum && <button type="button" className="col-album link" onClick={() => actions.openAlbum(albumKey(t))}>{t.album}</button>}
              {showAdded && <span className="col-added">{t.added ? dateFormat.format(new Date(t.added * 1000)) : ''}</span>}
              <span className="col-actions">
                {t.cached && <HardDriveDownload size={14} className="cached-mark" aria-label="Available offline" />}
                <button type="button" className={`like ${t.liked ? 'on' : ''}`} aria-label={t.liked ? 'Unlike' : 'Like'} title={t.liked ? 'Unlike' : 'Like'}
                  onClick={() => actions.toggleLike([t])}><Heart size={16} fill={t.liked ? 'currentColor' : 'none'} /></button>
                <IconButton icon={Ellipsis} label="More" onClick={e => actions.menu(e, [t], { remove: onRemove && (() => onRemove(i)) })} />
              </span>
              <span className="col-time">{formatTime(t.durationMs)}</span>
            </div>
          </React.Fragment>
        );
      })}
    </div>
  );
}

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
  const found = albums.filter(a => match({ title: '', artist: a.artist, albumArtist: '', album: a.title }));
  return (
    <section className="page">
      {!songs.length && !found.length && <p className="empty">Nothing found.</p>}
      {songs.length > 0 && <><h2 className="section-title">Songs</h2><TrackTable tracks={songs.slice(0, 300)} actions={actions} showAlbum showArtist /></>}
      {found.length > 0 && <><h2 className="section-title">Albums</h2><AlbumGrid albums={found.slice(0, 24)} actions={actions} /></>}
    </section>
  );
}

/** The right-click / ⋯ menu for tracks, and for playlists in the sidebar. */
function TrackMenu({ menu, lists, actions, run, reload, onNew, onRenamePlaylist, onOpenPlaylist, onClose }) {
  const ref = useRef(null);
  const [sub, setSub] = useState(false);
  useEffect(() => { ref.current?.querySelector('button')?.focus(); }, []);
  const { at, tracks = [], extra, playlist } = menu;
  const style = { left: Math.min(at.x, innerWidth - 250), top: Math.min(at.y, innerHeight - 330) };
  const item = (Icon, label, onSelect, cls = '') => (
    <button type="button" role="menuitem" className={cls} onClick={e => { e.stopPropagation(); onClose(); onSelect(); }}>
      {Icon && <Icon size={16} strokeWidth={1.75} />} {label}
    </button>
  );
  const one = tracks.length === 1 ? tracks[0] : null;
  return (
    <div ref={ref} className="menu" role="menu" style={style} onClick={e => e.stopPropagation()}
      onKeyDown={e => { if (e.key === 'Escape') onClose(); }}>
      {playlist ? <>
        {item(Play, 'Open', () => onOpenPlaylist(playlist.id))}
        {item(Pencil, 'Rename', () => onRenamePlaylist(playlist))}
        {item(Trash2, 'Delete', async () => {
          if (confirm(`Delete “${playlist.name}”? This removes it on your other computers too.`)) { await run('deletePlaylist', { id: playlist.id }); reload(); }
        }, 'danger')}
      </> : <>
        {item(Play, 'Play', () => actions.play(tracks))}
        {item(ListPlus, 'Play next', () => actions.playNext(tracks))}
        {item(ListEnd, 'Add to queue', () => actions.addToQueue(tracks))}
        <div className="menu-sub" onMouseEnter={() => setSub(true)} onMouseLeave={() => setSub(false)}>
          <button type="button" role="menuitem" aria-haspopup="menu" aria-expanded={sub} onClick={() => setSub(s => !s)}>
            <ListMusic size={16} strokeWidth={1.75} /> Add to playlist <span className="menu-arrow">›</span>
          </button>
          {sub && (
            <div className="menu submenu" role="menu" style={{ left: at.x + 480 > innerWidth ? -224 : 232 }}>
              {item(Plus, 'New playlist…', () => onNew(tracks))}
              {lists.user.map(pl => item(null, pl.name, () => run('playlistAdd', { id: pl.id, ids: tracks.map(t => t.id) }).then(reload)))}
            </div>
          )}
        </div>
        {item(Heart, tracks.every(t => t.liked) ? 'Unlike' : 'Like', () => actions.toggleLike(tracks))}
        {one && item(Disc3, 'Go to album', () => actions.openAlbum(albumKey(one)))}
        {one && item(Users, 'Go to artist', () => actions.openArtist(one.albumArtist || one.artist))}
        {extra?.remove && item(Trash2, 'Remove from this playlist', extra.remove, 'danger')}
      </>}
    </div>
  );
}

function NameDialog({ naming, run, onClose, onDone }) {
  const renaming = !!naming.playlist;
  const [name, setName] = useState(naming.playlist?.name ?? '');
  const submit = async e => {
    e.preventDefault();
    if (!name.trim()) return;
    if (renaming) { await run('renamePlaylist', { id: naming.playlist.id, name }); onDone(); }
    else {
      const r = await run('createPlaylist', { name, ids: (naming.tracks ?? []).map(t => t.id) });
      if (r) onDone(r.id);
    }
  };
  return (
    <Dialog title={renaming ? 'Rename playlist' : 'New playlist'} onClose={onClose}>
      <form className="name-form" onSubmit={submit}>
        <input autoFocus type="text" value={name} maxLength={200} placeholder="Playlist name" onChange={e => setName(e.target.value)} />
        {naming.tracks?.length > 0 && <p className="hint">With {naming.tracks.length === 1 ? `“${naming.tracks[0].title}”` : `${naming.tracks.length} songs`}.</p>}
        <div className="row end">
          <button type="button" onClick={onClose}>Cancel</button>
          <button type="submit" className="primary" disabled={!name.trim()}>{renaming ? 'Rename' : 'Create'}</button>
        </div>
      </form>
    </Dialog>
  );
}

const SHORTCUTS = [
  ['Space', 'Play / pause'],
  ['Ctrl →  /  Ctrl ←', 'Next / previous song'],
  ['→  /  ←', 'Forward / back 5 seconds'],
  ['Ctrl ↑  /  Ctrl ↓', 'Volume up / down'],
  ['M', 'Mute'],
  ['S', 'Shuffle on / off'],
  ['R', 'Repeat: off → all → one'],
  ['L', 'Like the playing song'],
  ['Q', 'Show / hide the queue'],
  ['/  or  Ctrl F', 'Search'],
  ['?', 'This list'],
];

function ShortcutsHelp({ onClose }) {
  return (
    <Dialog title="Keyboard shortcuts" onClose={onClose}>
      <dl className="shortcuts">
        {SHORTCUTS.map(([keys, what]) => <React.Fragment key={keys}><dt>{keys.split('  ').map(k => <kbd key={k}>{k}</kbd>)}</dt><dd>{what}</dd></React.Fragment>)}
      </dl>
      <p className="hint shortcuts-note">Media keys and your desktop’s media controls work too, even with the window closed.</p>
    </Dialog>
  );
}

function QueuePanel({ queue, player, run, actions, onClose }) {
  const [dragOver, setDragOver] = useState(null);
  const from = useRef(null);
  const index = player?.index ?? -1;
  const upcoming = queue.map((t, i) => [t, i]).filter(([, i]) => i > index);
  const now = index >= 0 ? queue[index] : null;
  const row = ([t, i]) => (
    <div key={`${t.id}-${i}`} className={`queue-item ${dragOver === i ? 'drag-over' : ''}`} draggable
      onDragStart={() => { from.current = i; }} onDragOver={e => { e.preventDefault(); setDragOver(i); }} onDragLeave={() => setDragOver(null)}
      onDrop={e => { e.preventDefault(); setDragOver(null); if (from.current != null && from.current !== i) run('move', { from: from.current, to: i }); from.current = null; }}
      onDoubleClick={() => run('jump', { index: i })} onContextMenu={e => actions.menu(e, [t], { remove: () => run('remove', { index: i }) })}>
      <GripVertical size={14} className="grip" />
      <Cover hash={t.art} size="sm" />
      <span className="title-text"><strong>{t.title}</strong><small>{t.artist}</small></span>
      <IconButton icon={X} label="Remove from queue" onClick={() => run('remove', { index: i })} />
    </div>
  );
  return (
    <aside className="queue-panel" onClick={e => e.stopPropagation()}>
      <header>
        <h2>Queue</h2>
        <div className="row">
          {queue.length > 0 && <button type="button" className="small" onClick={() => run('clear')}>Clear</button>}
          <IconButton icon={X} label="Close (Q)" onClick={onClose} />
        </div>
      </header>
      <div className="queue-body">
        {now && <>
          <h3>Now playing</h3>
          <div className="queue-item now"><Cover hash={now.art} size="sm" /><span className="title-text"><strong>{now.title}</strong><small>{now.artist}</small></span></div>
        </>}
        <h3>{player?.shuffle ? 'Next up · shuffled' : 'Next up'}{player?.repeat !== 'off' ? ` · repeat ${player.repeat}` : ''}</h3>
        {upcoming.length ? upcoming.map(row) : <p className="hint">Nothing queued after this.</p>}
      </div>
    </aside>
  );
}

function PlayerBar({ player, track, run, actions, queueOpen, onOpenAlbum, onQueue }) {
  const [drag, setDrag] = useState(null);
  const [volume, setVolume] = useState(null);
  const duration = player?.durationMs || track?.durationMs || 0;
  const position = drag ?? player?.positionMs ?? 0;
  const vol = volume ?? player?.volume ?? 0.8;
  const VolumeIcon = vol === 0 ? VolumeX : vol < 0.5 ? Volume1 : Volume2;
  const setVol = v => { setVolume(v); run('setVolume', { volume: v }).then(() => setVolume(null)); };
  const lastVol = useRef(0.8);
  const repeat = player?.repeat ?? 'off';
  const commitSeek = ms => { run('seek', { ms }); setTimeout(() => setDrag(null), 400); };

  return (
    <footer className="playerbar">
      <div className="now">
        {track ? <>
          <button type="button" className="now-art" onClick={onOpenAlbum} aria-label="Open the album"><Cover hash={track.art} size="sm" /></button>
          <div className="now-text">
            <strong title={track.title}>{track.title}</strong>
            <button type="button" className="link" onClick={() => actions.openArtist(track.albumArtist || track.artist)}>{track.artist}</button>
          </div>
          <button type="button" className={`like ${track.liked ? 'on' : ''}`} aria-label={track.liked ? 'Unlike' : 'Like'} title={track.liked ? 'Unlike (L)' : 'Like (L)'}
            onClick={() => actions.toggleLike([track])}><Heart size={18} fill={track.liked ? 'currentColor' : 'none'} /></button>
        </> : <div className="now-text"><small>Nothing playing</small></div>}
      </div>

      <div className="transport">
        <div className="buttons">
          <IconButton icon={Shuffle} label={player?.shuffle ? 'Shuffle on (S)' : 'Shuffle off (S)'} active={player?.shuffle} className="mode"
            onClick={() => run('setShuffle', { on: !player?.shuffle })} />
          <IconButton icon={SkipBack} label="Previous (Ctrl ←)" onClick={() => run('prev')} disabled={!track} />
          <button type="button" className="play-button" aria-label={player?.playing ? 'Pause' : 'Play'} onClick={() => run('toggle')} disabled={!player?.queue?.length}>
            {player?.playing && player?.buffering ? <Loader2 size={22} className="spin" />
              : player?.playing ? <Pause size={22} fill="currentColor" /> : <Play size={22} fill="currentColor" />}
          </button>
          <IconButton icon={SkipForward} label="Next (Ctrl →)" onClick={() => run('next')} disabled={!track} />
          <IconButton icon={repeat === 'one' ? Repeat1 : Repeat} label={`Repeat: ${repeat} (R)`} active={repeat !== 'off'} className="mode"
            onClick={() => run('setRepeat', { mode: { off: 'all', all: 'one', one: 'off' }[repeat] })} />
        </div>
        <div className="seek">
          <span className="time">{formatTime(position)}</span>
          <input type="range" min={0} max={Math.max(1, duration)} step={250} value={Math.min(position, duration)} disabled={!track}
            aria-label="Position" style={{ '--fill': `${duration ? (position / duration) * 100 : 0}%` }}
            onChange={e => setDrag(Number(e.target.value))}
            onPointerUp={e => commitSeek(Number(e.target.value))}
            onKeyUp={() => { if (drag != null) commitSeek(drag); }} />
          <span className="time">{formatTime(duration)}</span>
        </div>
        {player?.error && <small className="error player-error" title={player.error}>{player.error}</small>}
      </div>

      <div className="side-controls">
        {track && <small className="quality" title={player?.output}>{formatQuality(track)}</small>}
        <IconButton icon={ListMusic} label="Queue (Q)" active={queueOpen} onClick={onQueue} />
        <IconButton icon={VolumeIcon} label={vol === 0 ? 'Unmute (M)' : 'Mute (M)'}
          onClick={() => { if (vol > 0) { lastVol.current = vol; setVol(0); } else setVol(lastVol.current || 0.8); }} />
        <input type="range" className="volume" min={0} max={1} step={0.01} value={vol} aria-label="Volume"
          style={{ '--fill': `${vol * 100}%` }} onChange={e => setVol(Number(e.target.value))} />
      </div>
    </footer>
  );
}

