// The `bodies` renderer of <signal-canvas>: rigid circles and boxes from a
// `bodies` vector (x, y, size, kind per body), with the walls of the box.
// Canvas 2D: outlines, or a sprite or SVG image stretched over each body.
// Presentation only.

import { bodiesList, spriteSource, toScreen } from '../lib/canvas.js';

export const inputs = ['bodies'];

export function draw2d(g, [bodies], { fit, ratio, colour, sprite }) {
  g.strokeStyle = colour;
  g.lineWidth = Math.max(1, ratio);
  const [wx, wy] = toScreen(fit, -1, 1);
  g.strokeRect(wx, wy, 2 * fit.scale, 2 * fit.scale);
  const image = sprite?.image.complete && sprite.image.naturalWidth > 0 ? sprite.image : null;
  for (const body of bodiesList(bodies)) {
    const [x, y] = toScreen(fit, body.x, body.y);
    const r = body.size * fit.scale;
    if (image) {
      // Frame 0 of a sheet for circles, the last for boxes.
      if (sprite.spec.kind === 'strip') {
        const index = body.box ? sprite.spec.frames - 1 : 0;
        const s = spriteSource({ index, frames: sprite.spec.frames, orientation: sprite.spec.orientation, columns: sprite.spec.columns, imageWidth: image.naturalWidth, imageHeight: image.naturalHeight });
        g.drawImage(image, s.sx, s.sy, s.sw, s.sh, x - r, y - r, 2 * r, 2 * r);
      } else {
        g.drawImage(image, x - r, y - r, 2 * r, 2 * r);
      }
      continue;
    }
    g.beginPath();
    if (body.box) g.rect(x - r, y - r, 2 * r, 2 * r);
    else g.arc(x, y, r, 0, 2 * Math.PI);
    g.stroke();
  }
}
