// Installs the desktop app for the current user (no sudo): the release binary, its icon, and
// an applications-menu entry. Build first with `npx tauri build --no-bundle` (or run
// `npm run install:linux`, which does both). Same approach as Bits' scripts/install-linux.mjs.
//
// The menu entry is named after the binary (`music-stream.desktop`): Tauri sets no GTK app id,
// so on Wayland the window's app id is the binary name, and GNOME matches the two by name.
import { chmod, copyFile, mkdir, rename, rm, writeFile } from 'node:fs/promises';
import { randomUUID } from 'node:crypto';
import { homedir } from 'node:os';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { spawnSync } from 'node:child_process';

if (process.platform !== 'linux') throw new Error('This installer is for Linux desktops.');
if (process.getuid?.() === 0) throw new Error('Run this as your normal user, without sudo.');

const app = fileURLToPath(new URL('../', import.meta.url));
const source = process.argv[2] ? path.resolve(process.argv[2]) : path.join(app, '../target/release/music-stream');
const xdg = process.env.XDG_DATA_HOME;
const dataHome = xdg && path.isAbsolute(xdg) ? xdg : path.join(homedir(), '.local/share');
const executable = path.join(dataHome, 'music-stream', 'music-stream');
const applications = path.join(dataHome, 'applications');
const desktopFile = path.join(applications, 'music-stream.desktop');
const iconTheme = path.join(dataHome, 'icons/hicolor');

// Desktop files have their own string escaping and command-line quoting rules.
const desktopString = value => value.replaceAll('\\', '\\\\').replaceAll('\n', '\\n').replaceAll('\r', '\\r').replaceAll('\t', '\\t');
const execArgument = value => desktopString(`"${value.replace(/[\\"`$]/g, '\\$&').replaceAll('%', '%%')}"`);

async function install(destination, mode, { from, text }) {
  await mkdir(path.dirname(destination), { recursive: true });
  const temporary = `${destination}.${randomUUID()}.tmp`;
  try {
    if (from) await copyFile(from, temporary);
    else await writeFile(temporary, text, 'utf8');
    await chmod(temporary, mode);
    await rename(temporary, destination);
  } finally {
    await rm(temporary, { force: true });
  }
}

// Replaced atomically, so updating doesn't alter a running copy.
await install(executable, 0o755, { from: source });
await install(path.join(iconTheme, '512x512/apps/music-stream.png'), 0o644, { from: path.join(app, 'src-tauri/icons/icon.png') });
await install(path.join(iconTheme, 'scalable/apps/music-stream.svg'), 0o644, { from: path.join(app, 'icon.svg') });
await install(desktopFile, 0o644, { text: `[Desktop Entry]
Version=1.0
Type=Application
Name=music-stream
GenericName=Music Player
Comment=Play the music on your phone from this computer, over your local network.
Exec=${execArgument(executable)}
TryExec=${desktopString(executable)}
Icon=music-stream
Terminal=false
Categories=AudioVideo;Audio;Player;
Keywords=music;player;flac;audio;phone;stream;
StartupNotify=true
StartupWMClass=music-stream
` });

for (const [command, args] of [
  ['desktop-file-validate', [desktopFile]],
  ['update-desktop-database', [applications]],
  ['gtk-update-icon-cache', ['--force', '--ignore-theme-index', iconTheme]],
]) {
  const result = spawnSync(command, args, { encoding: 'utf8' });
  if (result.error?.code === 'ENOENT') continue;
  if (result.error || result.status !== 0) throw new Error(`${command}: ${result.error?.message || result.stderr}`);
}

console.log(`Installed music-stream: ${executable}`);
console.log(`Applications menu entry: ${desktopFile}`);
