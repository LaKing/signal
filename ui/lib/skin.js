// Skins (docs/ui-controls.md "Skins"): the `skin` attribute, filmstrip and
// sprite-sheet frame selection, and the string rules the SVG sanitizer
// (controls/skin-dom.js) applies. Pure functions, no DOM.

import { clamp } from './taper.js';

/**
 * Parses a `skin` attribute: `svg:<url>` or
 * `strip:<url>;frames=N[;orientation=vertical|horizontal][;columns=C][;scale=S][;2x=<url>]`.
 * URLs are kept exactly as written (never percent-encoded: the server
 * refuses `%`, and `knob@2x.png` must stay `knob@2x.png`). A URL whose name
 * ends in `@2x` is a double-resolution image (scale 2) unless `scale` says
 * otherwise; `2x=` names a double-resolution companion of a 1x image. Null
 * for an empty attribute; throws on a malformed one.
 */
export function parseSkin(attribute) {
  const text = String(attribute ?? '').trim();
  if (text === '') return null;
  const colon = text.indexOf(':');
  const kind = colon > 0 ? text.slice(0, colon) : '';
  const [url, ...options] = text.slice(colon + 1).split(';').map((part) => part.trim());
  if (!url) throw new Error(`skin "${text}" names no URL`);
  if (kind === 'svg') {
    if (options.length > 0) throw new Error(`an svg skin takes no options: "${text}"`);
    return { kind: 'svg', url };
  }
  if (kind !== 'strip') throw new Error(`unknown skin kind in "${text}" (svg: or strip:)`);
  const skin = { kind: 'strip', url, frames: 0, orientation: null, columns: null, scale: /@2x\.[a-z0-9]+$/i.test(url) ? 2 : 1, hidpi: null };
  for (const option of options) {
    const eq = option.indexOf('=');
    const key = eq > 0 ? option.slice(0, eq).trim() : option;
    const value = eq > 0 ? option.slice(eq + 1).trim() : '';
    if (key === 'frames' || key === 'columns') {
      const n = Number(value);
      if (!Number.isInteger(n) || n < 1) throw new Error(`${key} must be a whole number of at least 1, not "${value}"`);
      skin[key] = n;
    } else if (key === 'orientation') {
      if (value !== 'vertical' && value !== 'horizontal') throw new Error(`orientation is vertical or horizontal, not "${value}"`);
      skin.orientation = value;
    } else if (key === 'scale') {
      const n = Number(value);
      if (!(n > 0)) throw new Error(`scale must be positive, not "${value}"`);
      skin.scale = n;
    } else if (key === '2x') {
      if (!value) throw new Error('2x names no URL');
      skin.hidpi = value;
    } else {
      throw new Error(`unknown strip option "${key}"`);
    }
  }
  if (skin.frames < 1) throw new Error(`a strip skin needs frames=N: "${text}"`);
  return skin;
}

/** A skin URL against the page's base, without encoding the name. */
export function resolveSkinUrl(url, base) {
  return new URL(url, base).href;
}

/**
 * The frame of a filmstrip or sprite sheet for `position` (0..1), and how
 * to show it as a CSS background. `imageWidth`/`imageHeight` are the loaded
 * image's pixels, `scale` its resolution (2 for an @2x image), so the CSS
 * geometry is the same for a 1x and a 2x image. Frames run top to bottom
 * (vertical), left to right (horizontal), or row by row with `columns`; an
 * unknown orientation is vertical when the image is at least as tall as it
 * is wide.
 */
export function stripFrame({ position, frames, orientation = null, columns = null, imageWidth, imageHeight, scale = 1 }) {
  const width = imageWidth / scale;
  const height = imageHeight / scale;
  const index = Math.round(clamp(Number.isFinite(position) ? position : 0, 0, 1) * (frames - 1));
  let cols;
  if (columns) cols = columns;
  else if ((orientation ?? (height >= width ? 'vertical' : 'horizontal')) === 'vertical') cols = 1;
  else cols = frames;
  const rows = Math.ceil(frames / cols);
  const frameWidth = width / cols;
  const frameHeight = height / rows;
  const x = -(index % cols) * frameWidth;
  const y = -Math.floor(index / cols) * frameHeight;
  return {
    index,
    width: frameWidth,
    height: frameHeight,
    backgroundSize: `${width}px ${height}px`,
    backgroundPosition: `${x}px ${y}px`,
  };
}

/**
 * Whether a reference inside an SVG skin may stay: a fragment (`#part`) or
 * a relative or same-host path. Conservative: a URL parser drops tabs and
 * newlines anywhere and control characters at the ends, so `java\tscript:`
 * is `javascript:`; any reference with a control character, a backslash, a
 * scheme (`http:`, `data:`, `javascript:` …, or any `:` before the path) or
 * a host (`//host`) is refused.
 */
export function isSameOriginRef(reference) {
  const ref = String(reference ?? '').trim();
  if (ref === '') return false;
  if (/[\u0000-\u001f\u007f-\u009f\\]/.test(ref)) return false;
  if (ref.startsWith('#')) return true;
  if (ref.startsWith('//')) return false;
  const beforePath = ref.split(/[/?#]/)[0];
  return !beforePath.includes(':');
}

/**
 * CSS from a skin without anything that loads from another host. Comments
 * go first; CSS with a backslash or a control character is refused whole
 * (an escape can spell `url(` or a scheme, as in `u\72l(`), and so is CSS
 * with `image-set(` or `src(`, which take bare strings as URLs. Then
 * `@import` rules go, and `url()`s other than same-host ones become `none`.
 */
export function cleanCss(css) {
  const text = String(css ?? '').replace(/\/\*[\s\S]*?(\*\/|$)/g, '');
  if (/[\\\u0000-\u0008\u000b\u000e-\u001f\u007f]/.test(text)) return '';
  if (/(?:image-set|src)\s*\(/i.test(text)) return '';
  return text
    .replace(/@import[^;]*;?/gi, '')
    .replace(/url\(\s*(['"]?)(.*?)\1\s*\)/gi, (whole, _quote, ref) => (isSameOriginRef(ref) ? whole : 'none'));
}

/** What controls/skin-dom.js removes from an SVG skin before using it. */
export const SVG_SANITIZE = Object.freeze({
  /** Elements removed with everything inside them. */
  elements: Object.freeze(['script', 'foreignObject', 'iframe', 'object', 'embed', 'audio', 'video', 'animate', 'animateMotion', 'animateTransform', 'set']),
  /** Attributes that keep a reference only when `isSameOriginRef` allows it. */
  references: Object.freeze(['href', 'xlink:href', 'src']),
  /** Attributes removed outright: they change how references resolve. */
  removed: Object.freeze(['xml:base', 'base']),
  /** Attributes whose names start with this are removed (event handlers). */
  handlerPrefix: 'on',
});

/**
 * An SVG skin attribute's value once sanitized, or null to remove it:
 * event handlers and base-changing attributes go, references stay only on
 * the host, and any other value that can load something (`style`, or a
 * presentation attribute such as `fill`, `filter`, `mask` or `clip-path`
 * holding `url(`, or an escape) goes through `cleanCss`.
 */
export function sanitizeAttribute(name, value) {
  const key = String(name).toLowerCase();
  if (key.startsWith(SVG_SANITIZE.handlerPrefix) || SVG_SANITIZE.removed.includes(key)) return null;
  if (SVG_SANITIZE.references.includes(key)) return isSameOriginRef(value) ? value : null;
  if (key === 'style' || /url\s*\(|\\|image-set|src\s*\(/i.test(value)) {
    const clean = cleanCss(value);
    return clean.trim() === '' ? null : clean;
  }
  return value;
}
