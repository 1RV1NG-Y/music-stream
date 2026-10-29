// Platform glue: Tauri (desktop / Android) vs plain browser preview.
export const isTauri = () => !!globalThis.__TAURI_INTERNALS__;
export const isAndroid = () => /Android/i.test(navigator.userAgent);

export async function androidSyncControls() {
  const { invoke } = await import('@tauri-apps/api/core');
  await invoke('android_sync_controls');
}
androidSyncControls.available = isTauri() && isAndroid();

const desktopTauri = () => isTauri() && !isAndroid();
const invokeDesktop = async (cmd, args) => (await import('@tauri-apps/api/core')).invoke(cmd, args);

/** Desktop: closing the window leaves the app playing in the tray (false where there's no tray). */
let keepsRunning;
export const keepsRunningInBackground = () =>
  keepsRunning ??= desktopTauri() ? invokeDesktop('keeps_running_in_background').catch(() => false) : Promise.resolve(false);

/** Desktop: starting in the tray when the user logs in. */
export const startAtLogin = {
  available: desktopTauri(),
  get: () => invokeDesktop('start_at_login'),
  set: on => invokeDesktop('set_start_at_login', { on }),
};

/** Android Back button. handler() returns true when it handled the press; otherwise the app exits. */
export function onBackButton(handler) {
  if (!isTauri() || !isAndroid()) return () => {};
  const listener = import('@tauri-apps/api/app').then(({ onBackButtonPress }) =>
    onBackButtonPress(async () => {
      if (await handler()) return;
      const { exit } = await import('@tauri-apps/api/app');
      await exit(0);
    }));
  return () => { listener.then(l => l.unregister()); };
}

export async function copyText(text) {
  try {
    await navigator.clipboard.writeText(text);
    return true;
  } catch {
    return false;
  }
}

export async function readClipboard() {
  try { return await navigator.clipboard.readText(); } catch { return ''; }
}

/** Linux desktop: the window has no title bar, so the app draws its own controls. */
export const ownWindowControls = () => isTauri() && !isAndroid() && /Linux/.test(navigator.userAgent);

/** Scans a QR code with the camera (Android). `scanQrCode.available` says whether it can. */
export async function scanQrCode() {
  const { scan, Format, checkPermissions, requestPermissions } = await import('@tauri-apps/plugin-barcode-scanner');
  let permission = await checkPermissions();
  if (permission !== 'granted') permission = await requestPermissions();
  if (permission !== 'granted') throw new Error('Camera access is needed to scan a code. You can paste the code instead.');
  return (await scan({ formats: [Format.QRCode], windowed: false })).content;
}
scanQrCode.available = isTauri() && isAndroid();
