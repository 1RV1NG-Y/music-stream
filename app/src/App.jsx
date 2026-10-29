import React from 'react';
import { isAndroid, isTauri } from './platform.mjs';
import Desktop from './Desktop.jsx';
import Phone from './Phone.jsx';

export default function App() {
  if (!isTauri()) {
    return <div className="empty-state"><h2>music-stream</h2><p className="hint">Open the app: this page only runs inside it.</p></div>;
  }
  return isAndroid() ? <Phone /> : <Desktop />;
}
