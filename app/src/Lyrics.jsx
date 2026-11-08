// Lyrics, Spotify-style: timed lyrics follow the song a line at a time (tap a line to jump
// there); plain lyrics just scroll. Shared by the desktop and the phone.
import React, { useEffect, useRef, useState } from 'react';
import { artUrl, errorText, music } from './api.mjs';
import { lineAt, parseLyrics } from './library.mjs';
import { coverPalette, defaultPalette } from './palette.mjs';

const found = new Map();   // track id → parsed lyrics (null: none)

function useLyrics(id) {
  const [state, setState] = useState({ status: 'loading' });
  useEffect(() => {
    if (!id) return;
    if (found.has(id)) { setState({ status: 'ready', lyrics: found.get(id) }); return; }
    let live = true;
    setState({ status: 'loading' });
    music('lyrics', { id }).then(
      r => { const lyrics = parseLyrics(r?.text); found.set(id, lyrics); if (live) setState({ status: 'ready', lyrics }); },
      e => { if (live) setState({ status: 'error', error: errorText(e) }); },
    );
    return () => { live = false; };
  }, [id]);
  return state;
}

/** The line being sung, following the player between its position updates. */
function useCurrentLine(lines, positionMs, playing) {
  // Re-anchor only on a new position: other re-renders mustn't pull the clock back.
  const anchor = useRef({ ms: positionMs, at: performance.now() });
  const seen = useRef(positionMs);
  if (seen.current !== positionMs) { seen.current = positionMs; anchor.current = { ms: positionMs, at: performance.now() }; }
  const [index, setIndex] = useState(-1);
  useEffect(() => {
    if (!lines) return;
    let frame;
    const tick = () => {
      const a = anchor.current;
      const ms = playing ? a.ms + (performance.now() - a.at) : a.ms;
      const i = lineAt(lines, ms + 150);   // a touch early, like reading ahead
      setIndex(prev => (prev === i ? prev : i));
      frame = requestAnimationFrame(tick);
    };
    tick();
    return () => cancelAnimationFrame(frame);
  }, [lines, playing]);
  return index;
}

export default function Lyrics({ track, player, onSeek, className = '' }) {
  const state = useLyrics(track?.id);
  const lyrics = state.status === 'ready' ? state.lyrics : null;
  const timed = lyrics?.timed ? lyrics.lines : null;
  const index = useCurrentLine(timed, player?.positionMs ?? 0, !!player?.playing);
  const box = useRef(null);
  const userUntil = useRef(0);

  // Keep the current line a third of the way down, unless the user is scrolling.
  useEffect(() => {
    const el = box.current;
    if (!el || !timed || performance.now() < userUntil.current) return;
    const line = el.querySelector(`[data-i="${Math.max(0, index)}"]`);
    if (line) el.scrollTo({ top: line.offsetTop - el.clientHeight * 0.33, behavior: index < 0 ? 'auto' : 'smooth' });
  }, [index, timed]);
  useEffect(() => { box.current?.scrollTo({ top: 0 }); }, [track?.id]);
  const userScrolled = () => { userUntil.current = performance.now() + 3000; };

  let body;
  if (!track) body = <p className="lyrics-note">Nothing playing.</p>;
  else if (state.status === 'loading') body = <p className="lyrics-note">Looking for lyrics…</p>;
  else if (state.status === 'error') body = <p className="lyrics-note">Couldn’t get the lyrics: {state.error}</p>;
  else if (!lyrics) body = <p className="lyrics-note">This song has no lyrics in its file.</p>;
  else if (timed) body = timed.map((l, i) => (
    <p key={i} data-i={i} className={`lyric ${i < index ? 'past' : i === index ? 'now' : ''} ${l.text ? '' : 'gap'}`}
      onClick={() => onSeek?.(l.ms)}>{l.text || '♪'}</p>
  ));
  else body = <>
    <p className="lyrics-note small">These lyrics aren’t timed, so they don’t follow the song.</p>
    {lyrics.lines.map((l, i) => <p key={i} className={`lyric plain ${l.text ? '' : 'gap'}`}>{l.text || ' '}</p>)}
  </>;

  return (
    <div ref={box} className={`lyrics ${className}`} onWheel={userScrolled} onTouchMove={userScrolled}>
      <div className="lyrics-lines">{body}</div>
    </div>
  );
}

/** A slowly drifting background in the cover's colours; fades to the next song's. Fills its
 *  nearest positioned parent. */
export function CoverGlow({ hash }) {
  const [colours, setColours] = useState(defaultPalette);
  useEffect(() => {
    let live = true;
    coverPalette(hash, artUrl(hash)).then(c => { if (live) setColours(c); });
    return () => { live = false; };
  }, [hash]);
  return (
    <div className="glow" aria-hidden="true">
      {colours.map((c, i) => <span key={i} style={{ color: c }} />)}
    </div>
  );
}
