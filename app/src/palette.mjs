// Colours from an album cover, for backgrounds that follow the song (the lyrics view first).
// `pickPalette` is pure (RGBA pixels in, colours out) so it can be tested; `coverPalette`
// loads the cover into a small canvas and caches the answer per cover.

const BUCKETS = 12;          // hue slices of 30°
const FALLBACK = ['rgb(122 52 10)', 'rgb(92 30 8)', 'rgb(150 70 16)'];   // the app's orange, dark

function rgbToHsl(r, g, b) {
  r /= 255; g /= 255; b /= 255;
  const max = Math.max(r, g, b), min = Math.min(r, g, b), l = (max + min) / 2, d = max - min;
  if (!d) return [0, 0, l];
  const s = d / (1 - Math.abs(2 * l - 1));
  const h = max === r ? ((g - b) / d + 6) % 6 : max === g ? (b - r) / d + 2 : (r - g) / d + 4;
  return [h * 60, s, l];
}

function hslToRgb(h, s, l) {
  const c = (1 - Math.abs(2 * l - 1)) * s, x = c * (1 - Math.abs(((h / 60) % 2) - 1)), m = l - c / 2;
  const [r, g, b] = h < 60 ? [c, x, 0] : h < 120 ? [x, c, 0] : h < 180 ? [0, c, x] : h < 240 ? [0, x, c] : h < 300 ? [x, 0, c] : [c, 0, x];
  return [r, g, b].map(v => Math.round((v + m) * 255));
}

const linear = v => (v /= 255) <= 0.04045 ? v / 12.92 : ((v + 0.055) / 1.055) ** 2.4;
const luminance = ([r, g, b]) => 0.2126 * linear(r) + 0.7152 * linear(g) + 0.0722 * linear(b);

/** A colour toned for a background behind white text: vivid enough to read as the cover's,
 *  dark enough that white stays legible (luminance at most 0.16: contrast with white ≥ 5). */
function tone([h, s, l], lightness, shade = 1) {
  let rgb = hslToRgb(h, Math.min(0.85, Math.max(0.4, s)), Math.min(lightness, Math.max(0.18, l)));
  for (let k = 0.95; luminance(rgb) > 0.16 && k > 0.3; k -= 0.05) rgb = hslToRgb(h, Math.min(0.85, Math.max(0.4, s)), Math.min(lightness, l) * k);
  return `rgb(${rgb.map(v => Math.round(v * shade)).join(' ')})`;
}

/**
 * Three background colours from a cover's RGBA pixels: the cover's main colour first, then
 * two others that differ from it in hue when the cover has them. A grey cover gives greys
 * (tinted by its faint colour, if any); an empty one gives the app's orange.
 */
export function pickPalette(data) {
  const buckets = Array.from({ length: BUCKETS }, () => ({ w: 0, x: 0, y: 0, s: 0, l: 0 }));
  let all = 0;
  for (let i = 0; i + 3 < data.length; i += 4) {
    if (data[i + 3] < 128) continue;
    const [h, s, l] = rgbToHsl(data[i], data[i + 1], data[i + 2]);
    all++;
    // Near-black, near-white and washed-out pixels say little about the cover's colour.
    const chroma = s * (1 - Math.abs(2 * l - 1));
    if (chroma < 0.12 || l < 0.08 || l > 0.92) continue;
    const b = buckets[Math.floor(h / (360 / BUCKETS)) % BUCKETS];
    const w = chroma;
    // Hue averages as a vector sum, so 355° and 5° make 0°, not 180°.
    b.w += w; b.s += w * s; b.l += w * l;
    b.x += w * Math.cos(h * Math.PI / 180); b.y += w * Math.sin(h * Math.PI / 180);
  }
  if (!all) return FALLBACK;
  const colours = buckets
    .map((b, i) => ({ i, w: b.w, hsl: b.w ? [((Math.atan2(b.y, b.x) * 180 / Math.PI) + 360) % 360, b.s / b.w, b.l / b.w] : null }))
    .filter(b => b.w > 0)
    .sort((a, b) => b.w - a.w);
  const chromatic = colours.reduce((n, c) => n + c.w, 0);
  if (chromatic < all * 0.04) {
    // A grey cover: greys, keeping only a hint of whatever tint it has.
    const [hue, sat] = colours.length ? [colours[0].hsl[0], 0.1] : [0, 0];
    return [0.30, 0.20, 0.38].map(l => `rgb(${hslToRgb(hue, sat, l).join(' ')})`);
  }
  const main = colours[0];
  const far = (c, from) => Math.min(Math.abs(c.i - from.i), BUCKETS - Math.abs(c.i - from.i)) >= 2;
  const second = colours.find(c => c !== main && far(c, main) && c.w > main.w * 0.08) ?? main;
  const third = colours.find(c => c !== main && c !== second && far(c, main) && far(c, second) && c.w > main.w * 0.05)
    ?? colours.find(c => c !== main && c !== second) ?? main;
  // The same hue again: darker, so the blobs still read as separate.
  return [tone(main.hsl, 0.42), tone(second.hsl, 0.36, second === main ? 0.55 : 1), tone(third.hsl, 0.34, third === main ? 0.78 : 1)];
}

const palettes = new Map();   // cover hash → Promise<colours>

/** The palette for a cover (by its `art:` URL), cached. Falls back to orange on any error. */
export function coverPalette(hash, url) {
  if (!hash || !url) return Promise.resolve(FALLBACK);
  if (!palettes.has(hash)) {
    palettes.set(hash, new Promise(resolve => {
      const img = new Image();
      img.crossOrigin = 'anonymous';
      img.onload = () => {
        try {
          const canvas = document.createElement('canvas');
          canvas.width = canvas.height = 40;
          const ctx = canvas.getContext('2d', { willReadFrequently: true });
          ctx.drawImage(img, 0, 0, 40, 40);
          resolve(pickPalette(ctx.getImageData(0, 0, 40, 40).data));
        } catch {
          resolve(FALLBACK);   // a tainted canvas (no CORS header) lands here
        }
      };
      // Not kept: the phone may only have been away for a moment.
      img.onerror = () => { palettes.delete(hash); resolve(FALLBACK); };
      // Its own URL: a copy cached for an <img> without the CORS header would taint the canvas.
      img.src = `${url}?palette`;
    }));
  }
  return palettes.get(hash);
}

export { FALLBACK as defaultPalette };
