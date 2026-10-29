import { readFileSync } from 'node:fs';
import { defineConfig } from 'vite';
import react from '@vitejs/plugin-react';

// The app's Content-Security-Policy lives in src-tauri/tauri.conf.json (Tauri sends it as a
// header). The same policy goes into index.html as a <meta> so the plain browser preview
// (`npm run dev`) runs under it too.
const csp = JSON.parse(readFileSync(new URL('./src-tauri/tauri.conf.json', import.meta.url))).app.security.csp;

const cspMeta = () => ({
  name: 'music-stream-csp',
  transformIndexHtml: (_html, ctx) => [{
    tag: 'meta',
    injectTo: 'head-prepend',
    attrs: {
      'http-equiv': 'Content-Security-Policy',
      // Dev only: React Fast Refresh injects an inline module script, and Vite's HMR uses a
      // websocket. Production builds get the policy unchanged.
      content: ctx.server
        ? csp.replace("script-src 'self'", "script-src 'self' 'unsafe-inline'")
          .replace("connect-src 'self'", "connect-src 'self' ws://127.0.0.1:5190")
        : csp,
    },
  }],
});

export default defineConfig({
  plugins: [react(), cspMeta()],
  base: './',
  server: { port: 5190, strictPort: true },
  build: { target: 'es2022' },
});
