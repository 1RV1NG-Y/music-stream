// The app's calls into Rust: `rpc` (devices, settings) and `music` (library and player on a
// PC, serving status on the phone), plus the events they raise.
import { convertFileSrc, invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';

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

export const errorText = e => e?.message ?? String(e);
