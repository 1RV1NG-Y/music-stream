import React, { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import {
  ArrowDownWideNarrow, ArrowUpNarrowWide, ChevronDown, ChevronLeft, Disc3, Ellipsis, Eye, EyeOff, Flame, Folder, FolderX, Heart,
  History, ListEnd, ListMusic, ListPlus, Loader2, MoreHorizontal, Music2, Pause, Play, Plus, RefreshCw, Repeat, Repeat1, Search,
  Shuffle, SkipBack, SkipForward, Smartphone, Sparkles, Trash2, TrendingUp, Users, Wifi, X,
} from 'lucide-react';
import { artUrl, devices, errorText, music, on } from './api.mjs';
import { SONG_SORTS, albumKey, formatLength, formatTime, groupAlbums, groupArtists, matcher, sortSongs, sortTracks, tracksFor } from './library.mjs';
import { androidSyncControls, onBackButton } from './platform.mjs';
import Devices from './Devices.jsx';
import { ago } from './ui.jsx';

const SMART_ICONS = { 'on-repeat': Flame, 'recently-played': History, 'recently-added': Sparkles, 'most-played': TrendingUp };
const TABS = [
  { id: 'songs', label: 'Songs', icon: Music2 },
  { id: 'playlists', label: 'Playlists', icon: ListMusic },
  { id: 'albums', label: 'Albums', icon: Disc3 },
  { id: 'artists', label: 'Artists', icon: Users },
  { id: 'more', label: 'More', icon: MoreHorizontal },
];
const stored = (key, fallback) => { try { return JSON.parse(localStorage.getItem(key)) ?? fallback; } catch { return fallback; } };
const store = (key, value) => { try { localStorage.setItem(key, JSON.stringify(value)); } catch { /* convenience only */ } };
const folderName = f => f.split('/').pop() || f;
const folderLabel = f => {
  const [volume, ...rest] = f.split('/');
  const path = rest.join('/');
  return volume === 'external_primary' ? (path || 'Internal storage') : `${volume}${path ? `/${path}` : ''}`;
};

/** The phone app: a player for its own music, and the server for the paired computers. */
export default function Phone() {
  const [tracks, setTracks] = useState(null);
  const [lists, setLists] = useState({ liked: [], smart: [], user: [] });
  const [player, setPlayer] = useState(null);
  const [tab, setTab] = useState('songs');
  const [stack, setStack] = useState([]);
  const [query, setQuery] = useState(null);
  const [sheet, setSheet] = useState(null);
  const [nowOpen, setNowOpen] = useState(false);
  const [queueOpen, setQueueOpen] = useState(false);
  const [devicesOpen, setDevicesOpen] = useState(false);
  const [naming, setNaming] = useState(null);
  const [error, setError] = useState('');

  const loadLibrary = useCallback(() => music('library').then(r => setTracks(r.tracks), e => setError(errorText(e))), []);
  const loadLists = useCallback(() => music('playlists').then(setLists, () => {}), []);
  const loadState = useCallback(() => music('state').then(setPlayer, () => {}), []);
  useEffect(() => {
    loadLibrary();
    loadLists();
    const off = on('core-event', e => { if (e === 'libraryChanged') { loadLibrary(); loadLists(); } });
    return off;
  }, [loadLibrary, loadLists]);
  // The player lives in a service; follow it while the app is on screen.
  useEffect(() => {
    let timer = null;
    const tick = () => { if (document.visibilityState === 'visible') loadState(); };
    tick();
    timer = setInterval(tick, 500);
    return () => clearInterval(timer);
  }, [loadState]);
  // New music appears when the app comes to the front (and right after the permission prompt).
  useEffect(() => {
    let last = 0;
    const scan = () => {
      if (Date.now() - last < 10_000) return;
      last = Date.now();
      music('rescan').then(() => { loadLibrary(); loadLists(); }, () => {});
    };
    scan();
    window.addEventListener('focus', scan);
    const visible = () => { if (document.visibilityState === 'visible') scan(); };
    document.addEventListener('visibilitychange', visible);
    return () => { window.removeEventListener('focus', scan); document.removeEventListener('visibilitychange', visible); };
  }, [loadLibrary, loadLists]);

  const run = useCallback(async (method, params) => {
    try { setError(''); const r = await music(method, params); loadState(); return r; } catch (e) { setError(errorText(e)); }
  }, [loadState]);

  const visible = useMemo(() => (tracks ?? []).filter(t => !t.hidden), [tracks]);
  const hiddenTracks = useMemo(() => (tracks ?? []).filter(t => t.hidden), [tracks]);
  const albums = useMemo(() => groupAlbums(visible), [visible]);
  const artists = useMemo(() => groupArtists(albums), [albums]);
  const byId = useMemo(() => new Map((tracks ?? []).map(t => [t.id, t])), [tracks]);
  const current = player?.index != null ? byId.get(player.queue?.[player.index]) : null;

  const push = view => { setStack(s => [...s, view]); setQuery(null); window.scrollTo(0, 0); };
  const ids = list => list.map(t => t.id);
  const refresh = () => { loadLibrary(); loadLists(); };
  const actions = {
    play: (list, start = 0) => run('play', { ids: ids(list), start }),
    shuffle: list => run('play', { ids: ids(list), shuffle: true }),
    playNext: list => run('enqueue', { ids: ids(list), next: true }),
    addToQueue: list => run('enqueue', { ids: ids(list), next: false }),
    toggleLike: async list => { await run(list.every(t => t.liked) ? 'unlike' : 'like', { ids: ids(list) }); refresh(); },
    hide: async list => { await run('hide', { ids: ids(list) }); refresh(); },
    unhide: async list => { await run('unhide', { ids: ids(list) }); refresh(); },
    setFolder: async (folder, hidden) => { await run('setFolderHidden', { folder, hidden }); refresh(); },
    openAlbum: key => push({ name: 'album', key }),
    openArtist: name => push({ name: 'artist', key: name.toLowerCase() }),
    openList: view => push(view),
    menu: (list, extra) => setSheet({ tracks: list, extra }),
    currentId: current?.id,
    playing: player?.playing,
  };

  // Back: close what's open, then step back through pages, then leave.
  const backRef = useRef(null);
  backRef.current = () => {
    if (sheet) { setSheet(null); return true; }
    if (naming) { setNaming(null); return true; }
    if (devicesOpen) { setDevicesOpen(false); return true; }
    if (queueOpen) { setQueueOpen(false); return true; }
    if (nowOpen) { setNowOpen(false); return true; }
    if (query !== null) { setQuery(null); return true; }
    if (stack.length) { setStack(s => s.slice(0, -1)); return true; }
    if (tab !== 'songs') { setTab('songs'); return true; }
    return false;
  };
  useEffect(() => onBackButton(() => backRef.current()), []);

  const top = stack[stack.length - 1];
  let title = TABS.find(t => t.id === tab)?.label;
  let body;
  if (tracks === null) body = <p className="empty">Loading…</p>;
  else if (query !== null) body = <SearchResults albums={albums} query={query} actions={actions} />;
  else if (top?.name === 'album') {
    const album = albums.find(a => a.key === top.key);
    title = album?.title ?? 'Album';
    body = album ? <AlbumDetail album={album} actions={actions} /> : <p className="empty">This album is no longer in the library.</p>;
  } else if (top?.name === 'artist') {
    const artist = artists.find(a => a.key === top.key);
    title = artist?.name ?? 'Artist';
    body = artist ? <ArtistDetail artist={artist} actions={actions} /> : <p className="empty">Not found.</p>;
  } else if (top && ['liked', 'smart', 'playlist'].includes(top.name)) {
    const l = listFor(top, lists, byId);
    title = l?.title ?? 'Playlist';
    body = l ? <ListDetail list={l} actions={actions} run={run} refresh={refresh} onRename={() => setNaming({ playlist: l.playlist })}
      onDeleted={() => { refresh(); setStack(s => s.slice(0, -1)); }} /> : <p className="empty">This playlist no longer exists.</p>;
  } else if (top?.name === 'folders') { title = 'Folders'; body = <FoldersPage tracks={tracks} actions={actions} />; }
  else if (top?.name === 'hidden') { title = 'Hidden'; body = <TrackList tracks={hiddenTracks} actions={actions} showFolder />; }
  else if (tab === 'songs') body = <SongsTab albums={albums} actions={actions} />;
  else if (tab === 'playlists') body = <PlaylistsTab lists={lists} byId={byId} actions={actions} onNew={() => setNaming({})} />;
  else if (tab === 'albums') body = <AlbumGrid albums={albums} actions={actions} />;
  else if (tab === 'artists') body = <ArtistsTab artists={artists} actions={actions} />;
  else body = <MoreTab hiddenCount={hiddenTracks.length} onDevices={() => setDevicesOpen(true)} onOpen={push} onError={setError} />;

  return (
    <div className="m-app">
      <header className="m-top">
        {stack.length > 0 && query === null
          ? <button type="button" className="m-icon" aria-label="Back" onClick={() => setStack(s => s.slice(0, -1))}><ChevronLeft size={24} /></button>
          : null}
        {query !== null
          ? <input className="m-search" autoFocus type="search" placeholder="Search songs, albums, artists" value={query} onChange={e => setQuery(e.target.value)} />
          : <h1>{title}</h1>}
        <button type="button" className="m-icon" aria-label={query !== null ? 'Close search' : 'Search'} onClick={() => setQuery(q => (q === null ? '' : null))}>
          {query !== null ? <X size={22} /> : <Search size={22} />}
        </button>
      </header>

      {error && <p className="m-error" role="alert" onClick={() => setError('')}>{error}</p>}
      <main className="m-body">{body}</main>

      {current && <MiniPlayer track={current} player={player} run={run} onOpen={() => setNowOpen(true)} />}
      <nav className="m-tabs">
        {TABS.map(t => (
          <button key={t.id} type="button" className={tab === t.id && !stack.length && query === null ? 'on' : ''}
            onClick={() => { setTab(t.id); setStack([]); setQuery(null); window.scrollTo(0, 0); }}>
            <t.icon size={22} strokeWidth={tab === t.id ? 2.2 : 1.75} /><span>{t.label}</span>
          </button>
        ))}
      </nav>

      {nowOpen && current && <NowPlaying track={current} player={player} run={run} actions={actions} onClose={() => setNowOpen(false)} onQueue={() => setQueueOpen(true)} />}
      {queueOpen && <QueueSheet player={player} byId={byId} run={run} onClose={() => setQueueOpen(false)} />}
      {sheet && <ActionSheet sheet={sheet} lists={lists} actions={actions} run={run} refresh={refresh} onNew={list => setNaming({ tracks: list })}
        onClose={() => setSheet(null)} close={() => { setSheet(null); setNowOpen(false); }} />}
      {naming && <NameSheet naming={naming} run={run} onClose={() => setNaming(null)} onDone={id => { setNaming(null); refresh(); if (id) push({ name: 'playlist', id }); }} />}
      {devicesOpen && <Devices onClose={() => setDevicesOpen(false)} />}
    </div>
  );
}

function listFor(v, lists, byId) {
  if (v.name === 'liked') return { title: 'Liked songs', tracks: tracksFor(lists.liked, byId), kind: 'liked' };
  if (v.name === 'smart') {
    const s = lists.smart.find(x => x.id === v.id);
    return s && { title: s.name, hint: s.hint, tracks: tracksFor(s.tracks, byId), kind: 'smart' };
  }
  const pl = lists.user.find(x => x.id === v.id);
  return pl && { title: pl.name, playlist: pl, tracks: tracksFor(pl.entries.map(e => e.id).filter(Boolean), byId), kind: 'user' };
}

function Cover({ hash, size = 'md' }) {
  const [failed, setFailed] = useState(false);
  const url = artUrl(hash);
  return url && !failed
    ? <img className={`m-cover m-cover-${size}`} src={url} alt="" loading="lazy" decoding="async" onError={() => setFailed(true)} />
    : <div className={`m-cover m-cover-${size} m-cover-empty`}><Disc3 strokeWidth={1.25} /></div>;
}

function TrackList({ tracks, actions, showFolder, onRemove }) {
  const press = useRef(null);
  return (
    <div className="m-list">
      {tracks.map((t, i) => (
        <div key={`${t.id}-${i}`} className={`m-row ${actions.currentId === t.id ? 'current' : ''}`}
          onClick={() => actions.play(tracks, i)}
          onContextMenu={e => { e.preventDefault(); actions.menu([t], { remove: onRemove && (() => onRemove(i)) }); }}
          onTouchStart={() => { press.current = setTimeout(() => actions.menu([t], { remove: onRemove && (() => onRemove(i)) }), 550); }}
          onTouchEnd={() => clearTimeout(press.current)} onTouchMove={() => clearTimeout(press.current)}>
          <Cover hash={t.art} size="sm" />
          <div className="m-row-text">
            <strong>{t.title}</strong>
            <small>{t.liked && <Heart size={11} fill="currentColor" className="m-liked" />}{showFolder ? folderLabel(t.folder) : `${t.artist}${t.album ? ` · ${t.album}` : ''}`}</small>
          </div>
          <button type="button" className="m-icon" aria-label="More" onClick={e => { e.stopPropagation(); actions.menu([t], { remove: onRemove && (() => onRemove(i)) }); }}>
            <Ellipsis size={20} />
          </button>
        </div>
      ))}
    </div>
  );
}

function PlayRow({ list, actions }) {
  return (
    <div className="m-play-row">
      <button type="button" className="primary" disabled={!list.length} onClick={() => actions.play(list)}><Play size={16} fill="currentColor" /> Play</button>
      <button type="button" disabled={!list.length} onClick={() => actions.shuffle(list)}><Shuffle size={16} /> Shuffle</button>
    </div>
  );
}

function SongsTab({ albums, actions }) {
  const [sort, setSort] = useState(() => stored('songs.sort', { by: 'default', desc: false }));
  const change = next => { setSort(next); store('songs.sort', next); };
  const songs = useMemo(() => sortSongs(albums, sort.by, sort.desc), [albums, sort]);
  return (
    <>
      <PlayRow list={songs} actions={actions} />
      <div className="m-chips">
        {SONG_SORTS.map(s => (
          <button key={s.id} type="button" className={sort.by === s.id ? 'on' : ''}
            onClick={() => change({ by: s.id, desc: sort.by === s.id ? !sort.desc : s.id === 'added' })}>{s.label}</button>
        ))}
        <button type="button" className="m-icon" aria-label={sort.desc ? 'Descending' : 'Ascending'} onClick={() => change({ ...sort, desc: !sort.desc })}>
          {sort.desc ? <ArrowDownWideNarrow size={18} /> : <ArrowUpNarrowWide size={18} />}
        </button>
        <span className="m-count">{songs.length}</span>
      </div>
      <TrackList tracks={songs} actions={actions} />
    </>
  );
}

function PlaylistsTab({ lists, byId, actions, onNew }) {
  const row = (Icon, title, sub, onClick, liked) => (
    <button type="button" className="m-pl-row" onClick={onClick}>
      <span className={`m-pl-icon ${liked ? 'liked' : ''}`}><Icon size={22} fill={liked ? 'currentColor' : 'none'} /></span>
      <span className="m-row-text"><strong>{title}</strong><small>{sub}</small></span>
    </button>
  );
  return (
    <div className="m-list">
      {row(Plus, 'New playlist', 'Make one, then add songs from their ⋯ menu', onNew)}
      {row(Heart, 'Liked songs', `${tracksFor(lists.liked, byId).length} songs`, () => actions.openList({ name: 'liked' }), true)}
      {lists.smart.map(s => <React.Fragment key={s.id}>{row(SMART_ICONS[s.id] ?? Sparkles, s.name, `${tracksFor(s.tracks, byId).length} songs · made for you`, () => actions.openList({ name: 'smart', id: s.id }))}</React.Fragment>)}
      {lists.user.map(pl => <React.Fragment key={pl.id}>{row(ListMusic, pl.name, `${pl.entries.filter(e => e.id).length} songs`, () => actions.openList({ name: 'playlist', id: pl.id }))}</React.Fragment>)}
    </div>
  );
}

function AlbumGrid({ albums, actions }) {
  return (
    <ul className="m-grid">
      {albums.map(a => (
        <li key={a.key}>
          <button type="button" onClick={() => actions.openAlbum(a.key)}>
            <Cover hash={a.art} size="md" />
            <strong>{a.title}</strong>
            <small>{a.artist}</small>
          </button>
        </li>
      ))}
    </ul>
  );
}

function ArtistsTab({ artists, actions }) {
  return (
    <div className="m-list">
      {artists.map(a => (
        <button key={a.key} type="button" className="m-pl-row" onClick={() => actions.openArtist(a.name)}>
          <Cover hash={a.art} size="sm-round" />
          <span className="m-row-text"><strong>{a.name}</strong><small>{a.albums.length} {a.albums.length === 1 ? 'album' : 'albums'} · {a.tracks} songs</small></span>
        </button>
      ))}
    </div>
  );
}

function AlbumDetail({ album, actions }) {
  return (
    <>
      <div className="m-hero">
        <Cover hash={album.art} size="lg" />
        <h2>{album.title}</h2>
        <button type="button" className="m-link" onClick={() => actions.openArtist(album.artist)}>{album.artist}</button>
        <small>{album.year ? `${album.year} · ` : ''}{album.tracks.length} songs · {formatLength(album.durationMs)}</small>
      </div>
      <PlayRow list={album.tracks} actions={actions} />
      <TrackList tracks={album.tracks} actions={actions} />
    </>
  );
}

function ArtistDetail({ artist, actions }) {
  const all = artist.albums.flatMap(a => a.tracks);
  return (
    <>
      <PlayRow list={all} actions={actions} />
      <AlbumGrid albums={artist.albums} actions={actions} />
    </>
  );
}

function ListDetail({ list, actions, run, refresh, onRename, onDeleted }) {
  const { tracks, kind, playlist, hint } = list;
  const positions = playlist ? playlist.entries.map((e, i) => (e.id ? i : -1)).filter(i => i >= 0) : [];
  return (
    <>
      <p className="m-sub">{tracks.length} songs · {formatLength(tracks.reduce((n, t) => n + t.durationMs, 0))}{hint ? ` · ${hint}` : ''}</p>
      <PlayRow list={tracks} actions={actions} />
      {kind === 'user' && (
        <div className="m-chips">
          <button type="button" onClick={onRename}>Rename</button>
          <button type="button" onClick={async () => { if (confirm(`Delete “${list.title}”? This removes it on your computers too.`)) { await run('deletePlaylist', { id: playlist.id }); onDeleted(); } }}>Delete</button>
        </div>
      )}
      {!tracks.length && <p className="empty">{kind === 'smart' ? 'Nothing here yet: it fills up as you listen.' : kind === 'liked' ? 'Like songs with the heart to find them here.' : 'Add songs from their ⋯ menu.'}</p>}
      <TrackList tracks={tracks} actions={actions}
        onRemove={kind === 'user' ? i => run('playlistRemove', { id: playlist.id, positions: [positions[i]] }).then(refresh) : undefined} />
    </>
  );
}

function SearchResults({ albums, query, actions }) {
  const match = matcher(query);
  if (!match) return <p className="empty">Type to search your music.</p>;
  const songs = sortTracks(albums).filter(match).slice(0, 200);
  const found = albums.filter(a => match({ title: '', artist: a.artist, albumArtist: '', album: a.title })).slice(0, 12);
  return (
    <>
      {!songs.length && !found.length && <p className="empty">Nothing found.</p>}
      {songs.length > 0 && <TrackList tracks={songs} actions={actions} />}
      {found.length > 0 && <><h3 className="m-section">Albums</h3><AlbumGrid albums={found} actions={actions} /></>}
    </>
  );
}

function MoreTab({ hiddenCount, onDevices, onOpen, onError }) {
  const [status, setStatus] = useState(null);
  const [node, setNode] = useState(null);
  const load = useCallback(() => {
    music('phoneStatus').then(setStatus, e => onError(errorText(e)));
    devices.status().then(setNode, () => {});
  }, [onError]);
  useEffect(() => {
    load();
    const off = on('core-event', load);
    const timer = setInterval(load, 3000);
    return () => { off(); clearInterval(timer); };
  }, [load]);
  const computers = (node?.devices ?? []).filter(d => !d.isSelf && !d.removed);
  const connected = computers.filter(d => d.connected);
  const scan = status?.lastScan;
  return (
    <div className="m-more">
      <section className="card hero">
        <div className={`pulse ${connected.length ? 'on' : ''}`}><Wifi size={26} /></div>
        <div>
          <h2>{connected.length ? `Serving to ${connected.map(d => d.name).join(', ')}` : computers.length ? 'Ready to serve' : 'Not paired yet'}</h2>
          <p className="hint">{computers.length ? 'Your computers can play this phone’s music on the same Wi-Fi.' : 'Open music-stream on your computer, then here: Devices → Scan code.'}</p>
        </div>
      </section>
      <section className="card">
        <h3>Library</h3>
        {status && <>
          <p className="big">{status.tracks.toLocaleString()} <span>tracks</span> · {(status.bytes / 1e9).toFixed(1)} <span>GB</span></p>
          <p className="hint">{status.scanning ? <><Loader2 size={14} className="spin" /> Scanning…</> : status.lastScanAt ? `Checked ${ago(status.lastScanAt)}${scan ? ` · ${scan.added} new, ${scan.changed} changed, ${scan.removed} removed` : ''}` : 'Not scanned yet'}</p>
          {status.error && <p className="error">{status.error}</p>}
        </>}
        <button type="button" disabled={status?.scanning} onClick={() => music('rescan').then(load, e => onError(errorText(e)))}><RefreshCw size={16} /> Scan again</button>
      </section>
      <div className="m-list">
        <button type="button" className="m-pl-row" onClick={() => onOpen({ name: 'folders' })}>
          <span className="m-pl-icon"><Folder size={22} /></span><span className="m-row-text"><strong>Folders</strong><small>Choose which folders count as music</small></span>
        </button>
        <button type="button" className="m-pl-row" onClick={() => onOpen({ name: 'hidden' })}>
          <span className="m-pl-icon"><EyeOff size={22} /></span><span className="m-row-text"><strong>Hidden</strong><small>{hiddenCount} songs left out</small></span>
        </button>
        <button type="button" className="m-pl-row" onClick={onDevices}>
          <span className="m-pl-icon"><Smartphone size={22} /></span><span className="m-row-text"><strong>Devices</strong><small>Pair a computer, see who’s connected</small></span>
        </button>
        {androidSyncControls.available && (
          <button type="button" className="m-pl-row" onClick={() => androidSyncControls().catch(e => onError(errorText(e)))}>
            <span className="m-pl-icon"><Wifi size={22} /></span><span className="m-row-text"><strong>Background serving</strong><small>Keep serving with the screen off</small></span>
          </button>
        )}
      </div>
    </div>
  );
}

function FoldersPage({ tracks, actions }) {
  const [data, setData] = useState(null);
  useEffect(() => { music('folders').then(setData, () => {}); }, [tracks]);
  if (!data) return <p className="empty">Loading…</p>;
  return (
    <>
      <p className="m-sub">Switch a folder off to leave its songs, and its subfolders’, out of the library on every device. Nothing is deleted.</p>
      <div className="m-list">
        {data.folders.filter(f => f.folder.includes('/')).map(f => {
          const depth = f.folder.split('/').length - 2;
          const inherited = f.off && !f.switchedOff;
          return (
            <label key={f.folder} className={`m-folder ${f.off ? 'off' : ''}`} style={{ paddingLeft: 16 + depth * 18 }}>
              <span className="m-row-text"><strong>{folderName(folderLabel(f.folder))}</strong><small>{f.total} {f.total === 1 ? 'song' : 'songs'}</small></span>
              <input type="checkbox" role="switch" checked={!f.off} disabled={inherited} onChange={e => actions.setFolder(f.folder, !e.target.checked)} />
            </label>
          );
        })}
      </div>
    </>
  );
}

function MiniPlayer({ track, player, run, onOpen }) {
  const progress = player?.durationMs ? Math.min(1, player.positionMs / player.durationMs) : 0;
  return (
    <div className="m-mini" onClick={onOpen}>
      <div className="m-mini-progress" style={{ width: `${progress * 100}%` }} />
      <Cover hash={track.art} size="sm" />
      <div className="m-row-text"><strong>{track.title}</strong><small>{track.artist}</small></div>
      <button type="button" className="m-icon" aria-label={player?.playing ? 'Pause' : 'Play'} onClick={e => { e.stopPropagation(); run('toggle'); }}>
        {player?.playing ? <Pause size={24} fill="currentColor" /> : <Play size={24} fill="currentColor" />}
      </button>
      <button type="button" className="m-icon" aria-label="Next" onClick={e => { e.stopPropagation(); run('next'); }}><SkipForward size={22} fill="currentColor" /></button>
    </div>
  );
}

function NowPlaying({ track, player, run, actions, onClose, onQueue }) {
  const [drag, setDrag] = useState(null);
  const duration = player?.durationMs || track.durationMs || 0;
  const position = drag ?? player?.positionMs ?? 0;
  const repeat = player?.repeat ?? 'off';
  const commit = ms => { run('seek', { ms }); setTimeout(() => setDrag(null), 600); };
  return (
    <div className="m-now">
      <header>
        <button type="button" className="m-icon" aria-label="Close" onClick={onClose}><ChevronDown size={28} /></button>
        <span>Now playing</span>
        <button type="button" className="m-icon" aria-label="More" onClick={() => actions.menu([track])}><Ellipsis size={22} /></button>
      </header>
      <div className="m-now-art"><Cover hash={track.art} size="xl" /></div>
      <div className="m-now-title">
        <div className="m-row-text">
          <strong>{track.title}</strong>
          <small>{track.artist}{track.album ? ` · ${track.album}` : ''}</small>
        </div>
        <button type="button" className={`m-icon like ${track.liked ? 'on' : ''}`} aria-label={track.liked ? 'Unlike' : 'Like'} onClick={() => actions.toggleLike([track])}>
          <Heart size={26} fill={track.liked ? 'currentColor' : 'none'} />
        </button>
      </div>
      <input type="range" className="m-seek" min={0} max={Math.max(1, duration)} step={500} value={Math.min(position, duration)}
        style={{ '--fill': `${duration ? (position / duration) * 100 : 0}%` }}
        onChange={e => setDrag(Number(e.target.value))} onPointerUp={e => commit(Number(e.target.value))} onTouchEnd={e => commit(Number(e.target.value))} />
      <div className="m-times"><span>{formatTime(position)}</span><span>{formatTime(duration)}</span></div>
      <div className="m-transport">
        <button type="button" className={`m-icon mode ${player?.shuffle ? 'on' : ''}`} aria-label="Shuffle" onClick={() => run('setShuffle', { on: !player?.shuffle })}><Shuffle size={22} /></button>
        <button type="button" className="m-icon" aria-label="Previous" onClick={() => run('prev')}><SkipBack size={30} fill="currentColor" /></button>
        <button type="button" className="m-play" aria-label={player?.playing ? 'Pause' : 'Play'} onClick={() => run('toggle')}>
          {player?.buffering ? <Loader2 size={30} className="spin" /> : player?.playing ? <Pause size={32} fill="currentColor" /> : <Play size={32} fill="currentColor" />}
        </button>
        <button type="button" className="m-icon" aria-label="Next" onClick={() => run('next')}><SkipForward size={30} fill="currentColor" /></button>
        <button type="button" className={`m-icon mode ${repeat !== 'off' ? 'on' : ''}`} aria-label={`Repeat ${repeat}`}
          onClick={() => run('setRepeat', { mode: { off: 'all', all: 'one', one: 'off' }[repeat] })}>{repeat === 'one' ? <Repeat1 size={22} /> : <Repeat size={22} />}</button>
      </div>
      <button type="button" className="m-queue-button" onClick={onQueue}><ListMusic size={18} /> Queue</button>
      {player?.error && <p className="error m-now-error">{player.error}</p>}
    </div>
  );
}

function QueueSheet({ player, byId, run, onClose }) {
  const queue = (player?.queue ?? []).map(id => byId.get(id));
  const index = player?.index ?? -1;
  return (
    <div className="m-sheet-layer" onClick={onClose}>
      <div className="m-sheet tall" onClick={e => e.stopPropagation()}>
        <header><h2>Queue</h2>{queue.length > 0 && <button type="button" className="small" onClick={() => run('clear').then(onClose)}>Clear</button>}</header>
        <div className="m-list">
          {queue.map((t, i) => t && (
            <div key={`${t.id}-${i}`} className={`m-row ${i === index ? 'current' : ''} ${i < index ? 'played' : ''}`} onClick={() => run('jump', { index: i })}>
              <Cover hash={t.art} size="sm" />
              <div className="m-row-text"><strong>{t.title}</strong><small>{t.artist}</small></div>
              {i !== index && <button type="button" className="m-icon" aria-label="Remove" onClick={e => { e.stopPropagation(); run('remove', { index: i }); }}><X size={18} /></button>}
            </div>
          ))}
        </div>
      </div>
    </div>
  );
}

function ActionSheet({ sheet, lists, actions, run, refresh, onNew, onClose, close }) {
  const [picking, setPicking] = useState(false);
  const { tracks, extra } = sheet;
  const one = tracks.length === 1 ? tracks[0] : null;
  const item = (Icon, label, fn, cls = '') => (
    <button type="button" className={cls} onClick={() => { close(); fn(); }}><Icon size={20} strokeWidth={1.75} /> {label}</button>
  );
  return (
    <div className="m-sheet-layer" onClick={onClose}>
      <div className="m-sheet" onClick={e => e.stopPropagation()}>
        {one && <div className="m-sheet-head"><Cover hash={one.art} size="sm" /><div className="m-row-text"><strong>{one.title}</strong><small>{one.artist}</small></div></div>}
        {picking ? <>
          {item(Plus, 'New playlist…', () => onNew(tracks))}
          {lists.user.map(pl => <React.Fragment key={pl.id}>{item(ListMusic, pl.name, () => run('playlistAdd', { id: pl.id, ids: tracks.map(t => t.id) }).then(refresh))}</React.Fragment>)}
        </> : <>
          {item(ListPlus, 'Play next', () => actions.playNext(tracks))}
          {item(ListEnd, 'Add to queue', () => actions.addToQueue(tracks))}
          <button type="button" onClick={() => setPicking(true)}><ListMusic size={20} strokeWidth={1.75} /> Add to playlist…</button>
          {item(Heart, tracks.every(t => t.liked) ? 'Unlike' : 'Like', () => actions.toggleLike(tracks))}
          {one && item(Disc3, 'Go to album', () => actions.openAlbum(albumKey(one)))}
          {one && item(Users, 'Go to artist', () => actions.openArtist(one.albumArtist || one.artist))}
          {extra?.remove && item(Trash2, 'Remove from this playlist', extra.remove, 'danger')}
          {tracks.every(t => t.hidden)
            ? item(Eye, 'Show again', () => actions.unhide(tracks))
            : item(EyeOff, 'Hide song', () => actions.hide(tracks))}
          {one && !one.hidden && item(FolderX, `Hide folder “${folderName(one.folder)}”`, () => {
            if (confirm(`Hide every song in “${folderLabel(one.folder)}” and its subfolders? Nothing is deleted; switch it back on under More → Folders.`)) actions.setFolder(one.folder, true);
          })}
        </>}
      </div>
    </div>
  );
}

function NameSheet({ naming, run, onClose, onDone }) {
  const renaming = !!naming.playlist;
  const [name, setName] = useState(naming.playlist?.name ?? '');
  const submit = async e => {
    e.preventDefault();
    if (!name.trim()) return;
    if (renaming) { await run('renamePlaylist', { id: naming.playlist.id, name }); onDone(); }
    else { const r = await run('createPlaylist', { name, ids: (naming.tracks ?? []).map(t => t.id) }); if (r) onDone(r.id); }
  };
  return (
    <div className="m-sheet-layer" onClick={onClose}>
      <form className="m-sheet m-name" onClick={e => e.stopPropagation()} onSubmit={submit}>
        <h2>{renaming ? 'Rename playlist' : 'New playlist'}</h2>
        <input autoFocus type="text" value={name} maxLength={200} placeholder="Playlist name" onChange={e => setName(e.target.value)} />
        <div className="m-play-row">
          <button type="button" onClick={onClose}>Cancel</button>
          <button type="submit" className="primary" disabled={!name.trim()}>{renaming ? 'Rename' : 'Create'}</button>
        </div>
      </form>
    </div>
  );
}
