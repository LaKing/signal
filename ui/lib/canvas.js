// The pure parts of <signal-canvas> (docs/ui-controls.md "Canvas"):
// projection of the models' world (the box [−1, 1]², y up) onto a canvas,
// scaling of values to sizes and opacities, sprite frame choice, and the
// draw lists the renderers (ui/canvas/) take. No DOM, so node --test runs
// them. The canvas only draws what arrives: nothing here simulates.

import { clamp } from './taper.js';
import { stripFrame } from './skin.js';

/**
 * The transform that fits `box` ([x0, y0, x1, y1] in world units) into a
 * canvas of `width` × `height` CSS pixels at `ratio` device pixels per CSS
 * pixel, keeping its aspect, centred, with `margin` CSS pixels around it.
 * World y points up, canvas y down. Returns `{ scale, x0, y0 }`: device
 * pixel `(x0 + scale·x, y0 − scale·y)` shows world `(x, y)`.
 */
export function fitBox({ width, height, ratio = 1, box = [-1, -1, 1, 1], margin = 0 }) {
  const [bx0, by0, bx1, by1] = box;
  const w = Math.max(1e-9, bx1 - bx0);
  const h = Math.max(1e-9, by1 - by0);
  const pw = Math.max(0, width - 2 * margin) * ratio;
  const ph = Math.max(0, height - 2 * margin) * ratio;
  const scale = Math.min(pw / w, ph / h);
  // Centre the box: its middle at the canvas's middle.
  const x0 = (width * ratio) / 2 - scale * ((bx0 + bx1) / 2);
  const y0 = (height * ratio) / 2 + scale * ((by0 + by1) / 2);
  return { scale, x0, y0 };
}

/** World `(x, y)` in device pixels under `fit`. */
export function toScreen(fit, x, y) {
  return [fit.x0 + fit.scale * x, fit.y0 - fit.scale * y];
}

/** `value` from `[lo, hi]` to `[0, 1]`, clamped (0 for an empty range or a non-number). */
export function unit(value, lo, hi) {
  if (!Number.isFinite(value) || !(hi > lo)) return 0;
  return clamp((value - lo) / (hi - lo), 0, 1);
}

/** A size between `min` and `max` for `t` in [0, 1]. */
export function scaleSize(t, { min = 2, max = 8 } = {}) {
  return min + (max - min) * clamp(Number.isFinite(t) ? t : 0, 0, 1);
}

/**
 * The frame of a sprite sheet of `frames` frames for an age `t` in [0, 1]
 * (0 newborn, 1 at the end of its life): the frames are spread evenly over
 * the life, the last one shown only at its end. Clamped; 0 for a
 * non-number.
 */
export function spriteFrame(t, frames) {
  const n = Math.max(1, Math.floor(frames));
  const x = clamp(Number.isFinite(t) ? t : 0, 0, 1);
  return Math.min(n - 1, Math.floor(x * n));
}

/**
 * Where frame `index` is in a sprite sheet image, in image pixels, for
 * `drawImage`: the layout of a filmstrip skin (`strip:…;frames=N`, ui/lib/
 * skin.js), at its scale.
 */
export function spriteSource({ index, frames, orientation = null, columns = null, imageWidth, imageHeight }) {
  const position = frames > 1 ? index / (frames - 1) : 0;
  const f = stripFrame({ position, frames, orientation, columns, imageWidth, imageHeight, scale: 1 });
  const [x, y] = f.backgroundPosition.split(' ').map((v) => parseFloat(v));
  // `+ 0` makes −0 (the first column or row) plain 0.
  return { sx: -x + 0, sy: -y + 0, sw: f.width, sh: f.height };
}

/**
 * The live particles of a `pos` vector (x, y pairs) and an optional `life`
 * vector (remaining fraction, 0 dead): `[{ x, y, life, age }]`. Without
 * `life` every particle is drawn at full life.
 */
export function pointsList(pos, life = null) {
  const points = [];
  const n = Math.floor((pos?.length ?? 0) / 2);
  for (let k = 0; k < n; k++) {
    const l = life ? life[k] : 1;
    if (!(l > 0)) continue;
    points.push({ x: pos[2 * k], y: pos[2 * k + 1], life: l, age: 1 - l });
  }
  return points;
}

/** The side of a square grid of `n` nodes (16 for 256), at least 1. */
export function gridSide(n) {
  return Math.max(1, Math.round(Math.sqrt(Math.max(0, n))));
}

/**
 * A membrane `z` (node (i, j) at element j·grid + i) projected to world
 * points, a slanted view: node (i, j) sits at x = −1 + 2i/(grid − 1), its
 * depth y = −1 + 2j/(grid − 1) foreshortened by `tilt` and its height
 * `z × height` added. Returns a Float32Array of x, y pairs.
 */
export function meshProjection(z, grid, { tilt = 0.5, height = 0.5 } = {}) {
  const g = Math.max(2, grid);
  const out = new Float32Array(g * g * 2);
  for (let j = 0; j < g; j++) {
    for (let i = 0; i < g; i++) {
      const k = j * g + i;
      const x = -1 + (2 * i) / (g - 1);
      const depth = -1 + (2 * j) / (g - 1);
      out[2 * k] = x;
      out[2 * k + 1] = depth * tilt + (Number.isFinite(z?.[k]) ? z[k] : 0) * height;
    }
  }
  return out;
}

/** The grid's edges as node index pairs: rows, then columns. */
export function meshLines(grid) {
  const g = Math.max(2, grid);
  const lines = [];
  for (let j = 0; j < g; j++) for (let i = 0; i + 1 < g; i++) lines.push(j * g + i, j * g + i + 1);
  for (let i = 0; i < g; i++) for (let j = 0; j + 1 < g; j++) lines.push(j * g + i, (j + 1) * g + i);
  return Uint16Array.from(lines);
}

/** The bodies of a `bodies` vector (x, y, size, kind per body): `[{ x, y, size, box }]`. */
export function bodiesList(bodies) {
  const list = [];
  const n = Math.floor((bodies?.length ?? 0) / 4);
  for (let b = 0; b < n; b++) {
    list.push({ x: bodies[4 * b], y: bodies[4 * b + 1], size: Math.abs(bodies[4 * b + 2]), box: bodies[4 * b + 3] >= 0.5 });
  }
  return list;
}

/**
 * A CSS colour as WebGL's `[r, g, b, a]` in 0..1: `#rgb`, `#rrggbb`,
 * `rgb(r, g, b)` or `rgba(r, g, b, a)`; `fallback` for anything else.
 */
export function rgba(text, fallback = [0.31, 0.76, 0.97, 1]) {
  const t = String(text ?? '').trim();
  let m = /^#([0-9a-f]{3})$/i.exec(t);
  if (m) return [...m[1]].map((c) => parseInt(c + c, 16) / 255).concat(1);
  m = /^#([0-9a-f]{6})$/i.exec(t);
  if (m) return [0, 2, 4].map((i) => parseInt(m[1].slice(i, i + 2), 16) / 255).concat(1);
  m = /^rgba?\(\s*([\d.]+)\s*,\s*([\d.]+)\s*,\s*([\d.]+)\s*(?:,\s*([\d.]+)\s*)?\)$/i.exec(t);
  if (m) return [m[1] / 255, m[2] / 255, m[3] / 255, m[4] === undefined ? 1 : Number(m[4])].map((v) => clamp(v, 0, 1));
  return fallback;
}

/**
 * Which context to draw with: `webgl` when asked (`backend="webgl"`) or by
 * default (`auto`) for an unskinned points or mesh renderer that WebGL can
 * draw, if the browser gives one; else `2d`. Skins draw through Canvas 2D.
 */
export function chooseBackend({ backend = 'auto', renderer, skinned, webgl }) {
  if (backend === '2d') return '2d';
  const drawable = (renderer === 'points' || renderer === 'mesh') && !skinned;
  if ((backend === 'webgl' || backend === 'auto') && drawable && webgl) return 'webgl';
  return '2d';
}
