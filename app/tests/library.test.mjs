import { test } from 'node:test';
import assert from 'node:assert/strict';
import { groupAlbums, groupArtists, matcher, formatTime, formatQuality } from '../src/library.mjs';

const t = (id, o) => ({ id, title: id, artist: 'A', albumArtist: '', album: 'X', disc: 1, track: 1, year: 0, durationMs: 1000, art: null, cached: false, ...o });

test('albums group by album artist and sort tracks by disc and number', () => {
  const albums = groupAlbums([
    t('b2', { album: 'B', track: 2 }), t('b1', { album: 'B', track: 1 }),
    t('d2', { album: 'B', disc: 2, track: 1 }),
    t('feat', { album: 'C', artist: 'Guest', albumArtist: 'A', art: 'h' }),
    t('z', { album: 'B', albumArtist: 'Zed' }),
  ]);
  assert.deepEqual(albums.map(a => `${a.artist}/${a.title}`), ['A/B', 'A/C', 'Zed/B']);
  assert.deepEqual(albums[0].tracks.map(x => x.id), ['b1', 'b2', 'd2']);
  assert.equal(albums[1].art, 'h');
  assert.deepEqual(groupArtists(albums).map(a => [a.name, a.albums.length]), [['A', 2], ['Zed', 1]]);
});

test('search matches every word, ignoring case and accents', () => {
  const m = matcher('beyonce halo');
  assert.ok(m(t('x', { title: 'Halo', artist: 'Beyoncé' })));
  assert.ok(!m(t('y', { title: 'Halo', artist: 'Someone' })));
  assert.equal(matcher('  '), null);
});

test('formats', () => {
  assert.equal(formatTime(65_000), '1:05');
  assert.equal(formatTime(3_725_000), '1:02:05');
  assert.equal(formatQuality({ format: 'flac', bitDepth: 24, sampleRate: 96000 }), 'FLAC · 24-bit · 96 kHz');
  assert.equal(formatQuality({ format: 'flac', bitDepth: 16, sampleRate: 44100 }), 'FLAC · 16-bit · 44.1 kHz');
});

import { sortSongs } from '../src/library.mjs';

test('songs sort by album order, name or date added, either way', () => {
  const albums = groupAlbums([
    t('b', { title: 'Beta', album: 'Z', added: 300 }),
    t('a', { title: 'alpha', album: 'Z', track: 2, added: 100 }),
    t('c', { title: 'Gamma', album: 'A', added: 200 }),
  ]);
  const ids = list => list.map(x => x.id);
  assert.deepEqual(ids(sortSongs(albums)), ['c', 'b', 'a']);
  assert.deepEqual(ids(sortSongs(albums, 'default', true)), ['a', 'b', 'c']);
  assert.deepEqual(ids(sortSongs(albums, 'name')), ['a', 'b', 'c']);
  assert.deepEqual(ids(sortSongs(albums, 'name', true)), ['c', 'b', 'a']);
  assert.deepEqual(ids(sortSongs(albums, 'added')), ['a', 'c', 'b']);
  assert.deepEqual(ids(sortSongs(albums, 'added', true)), ['b', 'c', 'a']);
});
