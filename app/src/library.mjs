// The library as the PC shows it: tracks from the phone's index grouped into albums and
// artists, and searched. Pure functions, tested in tests/library.test.mjs.

const UNKNOWN_ARTIST = 'Unknown artist';
const UNKNOWN_ALBUM = 'Unknown album';
const collator = new Intl.Collator(undefined, { sensitivity: 'base', numeric: true });

export const albumArtistOf = t => t.albumArtist || t.artist || UNKNOWN_ARTIST;
export const albumKey = t => `${albumArtistOf(t).toLowerCase()}\u0000${(t.album || UNKNOWN_ALBUM).toLowerCase()}`;

const byDiscTrack = (a, b) => (a.disc || 1) - (b.disc || 1) || (a.track || 0) - (b.track || 0) || collator.compare(a.title, b.title);

/** Albums, sorted by artist then title; each with its tracks in disc/track order. */
export function groupAlbums(tracks) {
  const map = new Map();
  for (const t of tracks) {
    const key = albumKey(t);
    let a = map.get(key);
    if (!a) {
      a = { key, title: t.album || UNKNOWN_ALBUM, artist: albumArtistOf(t), year: 0, art: null, tracks: [], durationMs: 0, cached: 0 };
      map.set(key, a);
    }
    a.tracks.push(t);
    a.year = Math.max(a.year, t.year || 0);
    a.art ??= t.art;
    a.durationMs += t.durationMs;
    if (t.cached) a.cached++;
  }
  const albums = [...map.values()];
  for (const a of albums) a.tracks.sort(byDiscTrack);
  albums.sort((a, b) => collator.compare(a.artist, b.artist) || (a.year || 9999) - (b.year || 9999) || collator.compare(a.title, b.title));
  return albums;
}

/** Artists (by album artist) with their albums, sorted by name. */
export function groupArtists(albums) {
  const map = new Map();
  for (const a of albums) {
    const key = a.artist.toLowerCase();
    if (!map.has(key)) map.set(key, { key, name: a.artist, albums: [], art: null, tracks: 0 });
    const ar = map.get(key);
    ar.albums.push(a);
    ar.art ??= a.art;
    ar.tracks += a.tracks.length;
  }
  return [...map.values()].sort((a, b) => collator.compare(a.name, b.name));
}

/** All tracks: by album artist, album (year), disc and track. */
export function sortTracks(albums) {
  return albums.flatMap(a => a.tracks);
}

const fold = s => (s || '').normalize('NFD').replace(/\p{Diacritic}/gu, '').toLowerCase();

/** Whether every word of the query appears in the track's title, artist or album. */
export function matcher(query) {
  const words = fold(query).split(/\s+/).filter(Boolean);
  if (!words.length) return null;
  return t => {
    const hay = fold(`${t.title} ${t.artist} ${t.albumArtist} ${t.album}`);
    return words.every(w => hay.includes(w));
  };
}

export function formatTime(ms) {
  if (!Number.isFinite(ms) || ms < 0) ms = 0;
  const s = Math.floor(ms / 1000);
  const h = Math.floor(s / 3600);
  const m = Math.floor((s % 3600) / 60);
  const sec = String(s % 60).padStart(2, '0');
  return h ? `${h}:${String(m).padStart(2, '0')}:${sec}` : `${m}:${sec}`;
}

export function formatLength(ms) {
  const min = Math.round(ms / 60000);
  return min >= 60 ? `${Math.floor(min / 60)} h ${min % 60} min` : `${min} min`;
}

/** "FLAC · 24-bit · 96 kHz" */
export function formatQuality(t) {
  const parts = [t.format?.toUpperCase()];
  if (t.bitDepth) parts.push(`${t.bitDepth}-bit`);
  if (t.sampleRate) parts.push(`${(t.sampleRate / 1000).toString().replace(/\.0$/, '')} kHz`);
  return parts.filter(Boolean).join(' · ');
}

/** Songs list orders (PLAN.md §7): the album order, by name, by date added; each either way. */
export const SONG_SORTS = [
  { id: 'default', label: 'Default' },
  { id: 'name', label: 'Name' },
  { id: 'added', label: 'Date added' },
];

export function sortSongs(albums, sort = 'default', descending = false) {
  let list = sortTracks(albums);
  if (sort === 'name') list = [...list].sort((a, b) => collator.compare(a.title, b.title) || collator.compare(a.artist, b.artist));
  // Newest first reads as "descending"; the album order is the tie-break.
  else if (sort === 'added') list = list.map((t, i) => [t, i]).sort((a, b) => (a[0].added - b[0].added) || (a[1] - b[1])).map(([t]) => t);
  return descending ? [...list].reverse() : list;
}

/** Tracks for a list of ids, skipping any no longer in the library. */
export const tracksFor = (ids, byId) => ids.map(id => byId.get(id)).filter(Boolean);
