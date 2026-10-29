import React, { useEffect, useState } from 'react';
import { Copy, Minus, Square, X } from 'lucide-react';
import { ownWindowControls } from './platform.mjs';

/** Minimize, maximize/restore and close, drawn like the rest of the toolbar (Linux only). */
export default function WindowControls() {
  const [win, setWin] = useState(null);
  const [maximized, setMaximized] = useState(false);

  useEffect(() => {
    if (!ownWindowControls()) return;
    let off = null;
    let gone = false;
    import('@tauri-apps/api/window').then(async ({ getCurrentWindow }) => {
      const w = getCurrentWindow();
      const update = () => w.isMaximized().then(m => { if (!gone) setMaximized(m); });
      setWin(w);
      update();
      off = await w.onResized(update);
      if (gone) off();
    });
    return () => { gone = true; off?.(); };
  }, []);

  if (!win) return null;
  return (
    <div className="window-controls">
      <button type="button" className="window-control" aria-label="Minimize" title="Minimize" onClick={() => win.minimize()}>
        <Minus size={16} strokeWidth={1.75} aria-hidden="true" />
      </button>
      <button type="button" className="window-control" aria-label={maximized ? 'Restore' : 'Maximize'} title={maximized ? 'Restore' : 'Maximize'}
        onClick={() => win.toggleMaximize()}>
        {maximized
          ? <Copy size={14} strokeWidth={1.75} aria-hidden="true" style={{ transform: 'scaleX(-1)' }} />
          : <Square size={14} strokeWidth={1.75} aria-hidden="true" />}
      </button>
      <button type="button" className="window-control close" aria-label="Close" title="Close" onClick={() => win.close()}>
        <X size={17} strokeWidth={1.75} aria-hidden="true" />
      </button>
    </div>
  );
}
