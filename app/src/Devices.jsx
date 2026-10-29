import React, { useEffect, useState } from 'react';
import { ClipboardPaste, Copy, Laptop, Monitor, QrCode, ScanLine, Smartphone } from 'lucide-react';
import { Sheet, ago } from './ui.jsx';
import { devices as library } from './api.mjs';
import { androidSyncControls, copyText, keepsRunningInBackground, scanQrCode, startAtLogin } from './platform.mjs';

const PlatformIcon = ({ platform }) =>
  platform === 'android' ? <Smartphone size={18} aria-hidden="true" />
    : platform === 'windows' ? <Laptop size={18} aria-hidden="true" />
      : <Monitor size={18} aria-hidden="true" />;

/** Paired devices, and pairing a new one by scanning, showing or pasting a code (as in Bits). */
export default function Devices({ onClose }) {
  const [status, setStatus] = useState(null);
  const [invite, setInvite] = useState(null);
  const [pasting, setPasting] = useState(false);
  const [ticket, setTicket] = useState('');
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState('');
  const [note, setNote] = useState('');
  const [inTray, setInTray] = useState(false);
  const [atLogin, setAtLogin] = useState(null);

  useEffect(() => {
    keepsRunningInBackground().then(setInTray);
    if (startAtLogin.available) startAtLogin.get().then(setAtLogin, () => {});
  }, []);
  const changeAtLogin = on => run(async () => { await startAtLogin.set(on); setAtLogin(on); });

  useEffect(() => {
    const load = () => library.status().then(setStatus, e => setError(e.message ?? String(e)));
    load();
    return library.subscribe(e => { load(); });
  }, [library]);

  const run = async action => {
    setError('');
    try { return await action(); } catch (e) { setError(e?.message ?? String(e)); }
  };
  const pair = async code => {
    setBusy(true);
    const result = await run(() => library.join(code.trim()));
    setBusy(false);
    if (result) {
      setNote(result.joined ? `Paired with ${result.device}.` : `Added ${result.device}.`);
      setPasting(false);
      setTicket('');
    }
  };
  const scan = async () => {
    let text;
    try { text = await scanQrCode(); }
    catch (e) { if (!/cancel/i.test(String(e))) setError(e?.message ?? String(e)); return; }
    if (text) await pair(text);
  };

  if (!status) return <Sheet side="right" title="Devices" onClose={onClose}>{error ? <p className="error">{error}</p> : <p className="hint">Loading…</p>}</Sheet>;
  const others = status.devices.filter(d => !d.isSelf);
  const paired = others.some(d => !d.removed);
  const canScan = !!scanQrCode.available;

  return (
    <Sheet side="right" title="Devices" onClose={onClose} className="devices">
      <p className="hint">Your phone serves its music directly to your paired computers over your Wi-Fi network. No account or cloud service is involved.</p>
      {androidSyncControls.available && <button type="button" onClick={() => run(androidSyncControls)}>Background serving</button>}

      <section>
        <h3>This device</h3>
        <div className="device-row">
          <span className="device-icon"><PlatformIcon platform={status.platform} /></span>
          <div className="device-text">
            <strong>{status.deviceName}</strong>
            <small>{status.addresses.filter(a => !a.startsWith('[')).join(' · ') || 'No network address yet'}</small>
          </div>
        </div>
        {inTray && <p className="hint">Closing the window keeps music-stream playing in the background. Open or quit it from its icon in the system tray.</p>}
        {atLogin !== null && (
          <label className="field toggle">
            <span>Start when you log in{inTray ? ' (in the tray)' : ''}</span>
            <input type="checkbox" role="switch" checked={atLogin} onChange={e => changeAtLogin(e.target.checked)} />
          </label>
        )}
      </section>

      <section>
        <h3>Your other devices</h3>
        {!others.length && <p className="hint">None yet.</p>}
        {others.map(d => (
          <div key={d.id} className={`device-row ${d.removed ? 'removed' : ''}`}>
            <span className="device-icon"><PlatformIcon platform={d.platform} /></span>
            <div className="device-text">
              <strong>{d.name}</strong>
              <small>
                {d.removed ? 'Removed' : <>
                  <span className={`status-dot ${d.connected ? 'on' : ''}`} />
                  {d.connected ? 'Connected' : d.lastSeen ? `Last seen ${ago(d.lastSeen)}` : 'Not seen yet'}
                  
                </>}
              </small>
              {!d.connected && !d.removed && d.lastError && <small className="error">{d.lastError}</small>}
            </div>
            {!d.removed && (
              <button type="button" className="small" onClick={() => {
                if (confirm(`Remove ${d.name}? It stops connecting to this device.`)) run(() => library.removeDevice(d.id));
              }}>Remove</button>
            )}
          </div>
        ))}
      </section>

      <section>
        <h3>{paired ? 'Add a device' : 'Connect a device'}</h3>
        {invite ? (
          <div className="invite">
            <div className="qr" dangerouslySetInnerHTML={{ __html: invite.qrSvg }} />
            <p className="hint">
              {invite.kind === 'Add'
                ? <>On your phone, open <strong>Devices → Scan code</strong> and scan this.</>
                : <>On the new device, open <strong>Devices → Scan code</strong> and scan this, or paste the text below there.</>}
              {' '}It works once and expires at {new Date(invite.expiresAt).toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' })}. Both devices need to be on the same Wi-Fi.
            </p>
            <textarea readOnly className="ticket" value={invite.ticket} onFocus={e => e.target.select()} />
            <div className="row">
              <button type="button" onClick={() => copyText(invite.ticket).then(() => setNote('Copied.'), () => {})}><Copy size={15} aria-hidden="true" /> Copy text</button>
              <button type="button" onClick={() => setInvite(null)}>Done</button>
            </div>
          </div>
        ) : pasting ? (
          <>
            <textarea className="ticket" autoFocus placeholder="Paste the code (starts with MUSIC1)" value={ticket} onChange={e => setTicket(e.target.value)} />
            <div className="row">
              <button type="button" onClick={() => setPasting(false)}>Cancel</button>
              <button type="button" className="primary" disabled={busy || !ticket.trim()} onClick={() => pair(ticket)}>{busy ? 'Connecting…' : 'Connect'}</button>
            </div>
          </>
        ) : (
          <>
            <p className="hint">{canScan
              ? (paired ? 'Scan the code a new computer shows, or show this phone’s code on it.' : 'On your computer, open Devices → Show this device’s code, then scan it here.')
              : (paired ? 'Show this code on the new device, or paste the code it shows.' : 'Show this computer’s code and scan it from music-stream on your phone.')}</p>
            <div className="device-actions">
              {canScan && <button type="button" className="primary" disabled={busy} onClick={scan}><ScanLine size={16} aria-hidden="true" /> {busy ? 'Connecting…' : 'Scan code'}</button>}
              <button type="button" className={canScan ? '' : 'primary'} onClick={() => run(() => library.createInvite()).then(i => i && setInvite(i))}><QrCode size={16} aria-hidden="true" /> Show this device’s code</button>
              <button type="button" onClick={() => setPasting(true)}><ClipboardPaste size={15} aria-hidden="true" /> Paste a code</button>
            </div>
          </>
        )}
      </section>

      {note && <p className="hint" role="status">{note}</p>}
      {error && <p role="alert" className="error">{error}</p>}
    </Sheet>
  );
}
