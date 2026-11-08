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

test('timed lyrics parse in order, with offsets, repeats and word stamps', async () => {
  const { parseLyrics, lineAt } = await import('../src/library.mjs');
  const l = parseLyrics('[ti:Song]\n[by:SpotiFLAC (source: LRCLIB)]\n\n[00:33.57]Load up on guns\n[01:02.5][00:41.82]Chorus  line\n[00:45.00]\n[00:50.123]<00:50.12>Word <00:51.00>stamps\n');
  assert.equal(l.timed, true);
  assert.deepEqual(l.lines, [
    { ms: 33570, text: 'Load up on guns' }, { ms: 41820, text: 'Chorus line' }, { ms: 45000, text: '' },
    { ms: 50123, text: 'Word stamps' }, { ms: 62500, text: 'Chorus line' },
  ]);
  assert.deepEqual([lineAt(l.lines, 0), lineAt(l.lines, 33570), lineAt(l.lines, 44000), lineAt(l.lines, 99999)], [-1, 0, 1, 4]);
  assert.equal(parseLyrics('[offset:+500]\n[00:10.00]Early')?.lines[0].ms, 9500);
  // Enhanced LRC marks the singer; the mark isn't sung.
  const v = parseLyrics('[00:08.299]v1:<00:08.299>This <00:08.658><00:08.658>world <00:09.039>\n[00:27.25]v2:Te la toco\n[00:30.00]bg:(ooh)');
  assert.deepEqual(v.lines.map(l => l.text), ['This world', 'Te la toco', '(ooh)']);
});

test('plain lyrics keep their lines; empty lyrics are none', async () => {
  const { parseLyrics } = await import('../src/library.mjs');
  assert.deepEqual(parseLyrics('\nVerse one\n\nVerse two\n\n'), { timed: false, lines: [{ ms: null, text: 'Verse one' }, { ms: null, text: '' }, { ms: null, text: 'Verse two' }] });
  assert.equal(parseLyrics('  \n'), null);
  assert.equal(parseLyrics(null), null);
});

test('cover palettes: main colour first, a different hue next; greys stay grey', async () => {
  const { pickPalette, defaultPalette } = await import('../src/palette.mjs');
  const pixels = (...runs) => new Uint8ClampedArray(runs.flatMap(([n, rgb]) => Array.from({ length: n }, () => [...rgb, 255]).flat()));
  const rgb = s => s.match(/\d+/g).map(Number);
  // Mostly red with some blue, on black.
  const [main, second] = pickPalette(pixels([300, [200, 30, 20]], [120, [20, 40, 210]], [400, [0, 0, 0]])).map(rgb);
  assert.ok(main[0] > main[1] * 2 && main[0] > main[2] * 2, `reddish: ${main}`);
  assert.ok(second[2] > second[0] * 2, `bluish: ${second}`);
  // Behind white text: none of them light.
  for (const c of pickPalette(pixels([500, [255, 240, 120]])).map(rgb)) assert.ok(Math.max(...c) < 200, `${c}`);
  // A black-and-white cover gives greys.
  for (const [r, g, b] of pickPalette(pixels([300, [250, 250, 250]], [300, [20, 20, 20]], [100, [128, 128, 128]])).map(rgb)) {
    assert.ok(Math.max(r, g, b) - Math.min(r, g, b) < 20, `grey: ${[r, g, b]}`);
  }
  assert.deepEqual(pickPalette(new Uint8ClampedArray(0)), defaultPalette);
});
