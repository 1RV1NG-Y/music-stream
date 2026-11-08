// The app's calls into Rust: `rpc` (devices, settings) and `music` (library and player on a
// PC, serving status on the phone), plus the events they raise.
import { convertFileSrc, invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { useEffect, useState } from 'react';

export const rpc = (method, params) => invoke('rpc', { method, params });
export const music = (method, params) => invoke('music', { method, params });

/** Listens to a Rust event; returns an unsubscribe function. */
export function on(name, handler) {
  const off = listen(name, e => handler(e.payload));
  return () => { off.then(f => f()); };
}

/** The devices interface Devices.jsx uses. */
export const devices = {
  status: () => rpc('status'),
  createInvite: () => rpc('createInvite'),
  join: ticket => rpc('join', { ticket }),
  removeDevice: id => rpc('removeDevice', { id }),
  subscribe: listener => on('core-event', e => listener({ type: e === 'statusChanged' ? 'status' : 'library' })),
};

/** URL of a cover thumbnail, served by the app (`art:` scheme). */
export const artUrl = hash => hash ? convertFileSrc(hash, 'art') : null;

/**
 * A cover's URL for an <img>, or null to show the placeholder. A failed load (the phone busy
 * or away for a moment) is retried once a few seconds later, and is forgotten when the cover
 * changes: the player bar's image stays mounted from song to song.
 */
export function useArt(hash) {
  const [state, setState] = useState({ hash, tries: 0, failed: false });
  const current = state.hash === hash ? state : { hash, tries: 0, failed: false };
  useEffect(() => {
    if (!current.failed || current.tries > 1) return;
    const t = setTimeout(() => setState(s => (s.hash === hash ? { ...s, failed: false } : s)), 4000);
    return () => clearTimeout(t);
  }, [hash, current.failed, current.tries]);
  const base = artUrl(hash);
  return {
    url: base && !current.failed ? (current.tries ? `${base}?retry=${current.tries}` : base) : null,
    onError: () => setState(s => {
      const c = s.hash === hash ? s : { hash, tries: 0, failed: false };
      return { hash, tries: c.tries + 1, failed: true };
    }),
  };
}

export const errorText = e => e?.message ?? String(e);
