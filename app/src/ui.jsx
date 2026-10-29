// Small shared UI pieces.
import React, { useEffect, useRef } from 'react';
import { X } from 'lucide-react';

export function IconButton({ icon: Icon, label, onClick, active, disabled, className = '', ...rest }) {
  return (
    <button type="button" className={`icon-button ${active ? 'active' : ''} ${className}`} onClick={onClick}
      aria-label={label} title={label} aria-pressed={active ?? undefined} disabled={disabled} {...rest}>
      <Icon size={20} strokeWidth={1.75} aria-hidden="true" />
    </button>
  );
}

/** Side sheet (left or right) with a scrim; full width on phones. */
export function Sheet({ side = 'left', title, onClose, children, className = '' }) {
  const ref = useRef(null);
  useEffect(() => { ref.current?.querySelector('[autofocus], input, button')?.focus(); }, []);
  return (
    <div className="sheet-layer" onKeyDown={e => { if (e.key === 'Escape') { e.stopPropagation(); onClose(); } }}>
      <div className="scrim" onClick={onClose} />
      <aside ref={ref} className={`sheet sheet-${side} ${className}`} role="dialog" aria-label={title}>
        <header className="sheet-header">
          <h2>{title}</h2>
          <IconButton icon={X} label="Close" onClick={onClose} />
        </header>
        <div className="sheet-body">{children}</div>
      </aside>
    </div>
  );
}

/** Centered modal dialog. `onClose` null makes it non-dismissable (a choice is required). */
export function Dialog({ title, onClose, children, className = '' }) {
  const ref = useRef(null);
  useEffect(() => { ref.current?.querySelector('[autofocus], input, button.primary, button')?.focus(); }, []);
  return (
    <div className="dialog-layer" onKeyDown={e => { if (e.key === 'Escape' && onClose) { e.stopPropagation(); onClose(); } }}>
      <div className="scrim" onClick={onClose ?? undefined} />
      <div ref={ref} className={`dialog ${className}`} role="dialog" aria-modal="true" aria-label={title}>
        <header className="dialog-header">
          <h2>{title}</h2>
          {onClose && <IconButton icon={X} label="Close" onClick={onClose} />}
        </header>
        {children}
      </div>
    </div>
  );
}

export const percent = f => `${Math.floor((f ?? 0) * 100)}%`;

const rtf = new Intl.RelativeTimeFormat(undefined, { numeric: 'auto' });
export function ago(iso) {
  if (!iso) return '';
  const s = (Date.parse(iso) - Date.now()) / 1000;
  const abs = Math.abs(s);
  if (abs < 60) return 'just now';
  if (abs < 3600) return rtf.format(Math.round(s / 60), 'minute');
  if (abs < 86400) return rtf.format(Math.round(s / 3600), 'hour');
  return rtf.format(Math.round(s / 86400), 'day');
}

export const formatSize = bytes => bytes >= 1e6 ? `${(bytes / 1e6).toFixed(1)} MB` : `${Math.max(1, Math.round(bytes / 1e3))} kB`;
