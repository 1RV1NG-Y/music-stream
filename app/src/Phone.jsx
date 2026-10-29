import React, { useCallback, useEffect, useState } from 'react';
import { Loader2, Music2, RefreshCw, Smartphone, Wifi } from 'lucide-react';
import { devices, errorText, music, on } from './api.mjs';
import { androidSyncControls, onBackButton } from './platform.mjs';
import Devices from './Devices.jsx';
import { ago } from './ui.jsx';

/** The phone app: it serves its music to the paired computers; it isn't a player in v1. */
export default function Phone() {
  const [status, setStatus] = useState(null);
  const [node, setNode] = useState(null);
  const [devicesOpen, setDevicesOpen] = useState(false);
  const [error, setError] = useState('');

  const load = useCallback(() => {
    music('phoneStatus').then(setStatus, e => setError(errorText(e)));
    devices.status().then(setNode, () => {});
  }, []);
  useEffect(() => {
    load();
    const off = on('core-event', load);
    const timer = setInterval(load, 2000);
    return () => { off(); clearInterval(timer); };
  }, [load]);
  // Look for new music whenever the app comes to the front (also right after the audio
  // permission is granted). A scan of unchanged files only checks their metadata.
  useEffect(() => {
    let last = 0;
    const scan = () => {
      if (Date.now() - last < 10_000) return;
      last = Date.now();
      music('rescan').then(load, () => {});
    };
    scan();
    window.addEventListener('focus', scan);
    const visible = () => { if (document.visibilityState === 'visible') scan(); };
    document.addEventListener('visibilitychange', visible);
    return () => { window.removeEventListener('focus', scan); document.removeEventListener('visibilitychange', visible); };
  }, [load]);
  useEffect(() => onBackButton(() => { if (devicesOpen) { setDevicesOpen(false); return true; } return false; }), [devicesOpen]);

  const rescan = async () => {
    setError('');
    try { await music('rescan'); } catch (e) { setError(errorText(e)); }
    load();
  };

  const computers = (node?.devices ?? []).filter(d => !d.isSelf && !d.removed);
  const connected = computers.filter(d => d.connected);
  const scan = status?.lastScan;

  return (
    <div className="phone">
      <header className="phone-head">
        <div className="brand"><Music2 size={22} strokeWidth={2} /> music-stream</div>
      </header>

      <section className="card hero">
        <div className={`pulse ${connected.length ? 'on' : ''}`}><Wifi size={26} /></div>
        <div>
          <h2>{connected.length ? `Serving to ${connected.map(d => d.name).join(', ')}` : computers.length ? 'Ready' : 'Not paired yet'}</h2>
          <p className="hint">{computers.length
            ? 'Your computers can play this phone’s music while it stays on the same Wi-Fi.'
            : 'Open music-stream on your computer, then here: Devices → Scan code.'}</p>
        </div>
      </section>

      <section className="card">
        <h3>Library</h3>
        {!status ? <p className="hint">Loading…</p> : <>
          <p className="big">{status.tracks.toLocaleString()} <span>tracks</span> · {(status.bytes / 1e9).toFixed(1)} <span>GB</span></p>
          <p className="hint">
            {status.scanning ? <><Loader2 size={14} className="spin" /> Scanning…</>
              : status.lastScanAt ? `Checked ${ago(status.lastScanAt)}${scan ? ` · ${scan.added} new, ${scan.changed} changed, ${scan.removed} removed` : ''}` : 'Not scanned yet'}
          </p>
          {status.error && <p className="error">{status.error}</p>}
          {!status.error && scan?.unreadable > 0 && <p className="hint">{scan.unreadable} files couldn’t be read.</p>}
        </>}
        <button type="button" onClick={rescan} disabled={status?.scanning}><RefreshCw size={16} /> Scan again</button>
      </section>

      <section className="card actions">
        <button type="button" className="primary" onClick={() => setDevicesOpen(true)}><Smartphone size={16} /> Devices</button>
        {androidSyncControls.available && <button type="button" onClick={() => androidSyncControls().catch(e => setError(errorText(e)))}>Background serving</button>}
      </section>

      {error && <p className="error" role="alert">{error}</p>}
      {devicesOpen && <Devices onClose={() => setDevicesOpen(false)} />}
    </div>
  );
}
