// The `mesh` renderer of <signal-canvas>: a membrane's displacements (a
// `z` vector of grid × grid nodes) as a slanted wire grid. Canvas 2D, or
// WebGL lines. Presentation only.

import { gridSide, meshLines, meshProjection, toScreen } from '../lib/canvas.js';

export const inputs = ['z'];

function points([z], { grid, tilt, height }) {
  const side = grid ?? gridSide(z?.length ?? 0);
  return { side, world: meshProjection(z, side, { tilt, height }), lines: meshLines(side) };
}

export function draw2d(g, data, options) {
  const { fit, ratio, colour } = options;
  const { world, lines } = points(data, options);
  g.strokeStyle = colour;
  g.lineWidth = Math.max(1, ratio);
  g.beginPath();
  for (let k = 0; k < lines.length; k += 2) {
    const a = lines[k];
    const b = lines[k + 1];
    const [x0, y0] = toScreen(fit, world[2 * a], world[2 * a + 1]);
    const [x1, y1] = toScreen(fit, world[2 * b], world[2 * b + 1]);
    g.moveTo(x0, y0);
    g.lineTo(x1, y1);
  }
  g.stroke();
}

/** The WebGL vertices: each edge's two ends, world x, y and size 1. */
export function vertices(data, options) {
  const { world, lines } = points(data, options);
  const out = new Float32Array(lines.length * 3);
  for (let k = 0; k < lines.length; k++) {
    out[3 * k] = world[2 * lines[k]];
    out[3 * k + 1] = world[2 * lines[k] + 1];
    out[3 * k + 2] = 1;
  }
  return { mode: 'lines', data: out };
}
