// The `points` renderer of <signal-canvas>: particles from a `pos` vector
// (x, y pairs) and an optional `life` vector. Canvas 2D (dots, or a sprite
// sheet frame per particle chosen by its age, or an SVG image), or WebGL
// (point sprites, unskinned). Presentation only.

import { pointsList, scaleSize, spriteFrame, spriteSource, toScreen } from '../lib/canvas.js';

/** The vectors this renderer reads, in `bind` order. */
export const inputs = ['pos', 'life'];

/** Draws with Canvas 2D. */
export function draw2d(g, [pos, life], { fit, ratio, colour, sprite }) {
  const points = pointsList(pos, life);
  for (const p of points) {
    const [x, y] = toScreen(fit, p.x, p.y);
    const size = scaleSize(p.life, { min: 2, max: 7 }) * ratio;
    if (sprite?.image.complete && sprite.image.naturalWidth > 0) {
      const { image, spec } = sprite;
      const side = 4 * size;
      g.globalAlpha = 0.25 + 0.75 * p.life;
      if (spec.kind === 'strip') {
        const index = spriteFrame(p.age, spec.frames);
        const s = spriteSource({ index, frames: spec.frames, orientation: spec.orientation, columns: spec.columns, imageWidth: image.naturalWidth, imageHeight: image.naturalHeight });
        g.drawImage(image, s.sx, s.sy, s.sw, s.sh, x - side / 2, y - side / 2, side, side);
      } else {
        g.drawImage(image, x - side / 2, y - side / 2, side, side);
      }
    } else {
      g.globalAlpha = 0.3 + 0.7 * p.life;
      g.fillStyle = colour;
      g.beginPath();
      g.arc(x, y, size / 2, 0, 2 * Math.PI);
      g.fill();
    }
  }
  g.globalAlpha = 1;
}

/** The WebGL vertices: world x, y and a point size in device pixels. */
export function vertices([pos, life], { ratio }) {
  const points = pointsList(pos, life);
  const out = new Float32Array(points.length * 3);
  points.forEach((p, k) => {
    out[3 * k] = p.x;
    out[3 * k + 1] = p.y;
    out[3 * k + 2] = scaleSize(p.life, { min: 2, max: 7 }) * ratio;
  });
  return { mode: 'points', data: out };
}
