#!/usr/bin/env node
// Draws the gallery's pixel skins into ui/assets/ (docs/ui-controls.md
// "Skins"): a knob filmstrip (vertical, 64 frames, 1x and @2x), a toggle
// strip (horizontal, 2 frames, 1x and @2x) and a knob sprite sheet (8 × 8
// frames, @2x only); with `pulse`, the pulse panel's knob filmstrip into
// projects/pulse/ui/assets/ instead (vertical, 64 frames, 1x and @2x); with
// `sparks`, the <signal-canvas> spark sprite sheet (horizontal, 8 frames of
// 32 × 32, a bright point fading and cooling with age, docs/ui-controls.md
// "Canvas") as ui/assets/spark-strip.png and
// projects/physical-av/ui/assets/spark.png. The PNGs are committed; this
// script only makes them again (`node scripts/ui-strip.mjs [pulse|sparks]`).
// Node built-ins only.

import { writeFileSync } from 'node:fs';
import { deflateSync } from 'node:zlib';
import { fileURLToPath } from 'node:url';

const PULSE = process.argv[2] === 'pulse';
const SPARKS = process.argv[2] === 'sparks';
const OUT = fileURLToPath(new URL(PULSE ? '../projects/pulse/ui/assets/' : '../ui/assets/', import.meta.url));

const CRC = Array.from({ length: 256 }, (_, n) => {
  let c = n;
  for (let k = 0; k < 8; k++) c = c & 1 ? 0xedb88320 ^ (c >>> 1) : c >>> 1;
  return c >>> 0;
});

function crc32(bytes) {
  let c = 0xffffffff;
  for (const b of bytes) c = CRC[(c ^ b) & 0xff] ^ (c >>> 8);
  return (c ^ 0xffffffff) >>> 0;
}

function chunk(type, data) {
  const head = Buffer.alloc(8);
  head.writeUInt32BE(data.length, 0);
  head.write(type, 4, 'ascii');
  const crc = Buffer.alloc(4);
  crc.writeUInt32BE(crc32(Buffer.concat([head.subarray(4), data])), 0);
  return Buffer.concat([head, data, crc]);
}

/** An RGBA image (`pixels`: width × height × 4 bytes) as PNG. */
function png(width, height, pixels) {
  const header = Buffer.alloc(13);
  header.writeUInt32BE(width, 0);
  header.writeUInt32BE(height, 4);
  header.set([8, 6, 0, 0, 0], 8); // 8 bits, RGBA
  const rows = Buffer.alloc((width * 4 + 1) * height);
  for (let y = 0; y < height; y++) pixels.copy(rows, y * (width * 4 + 1) + 1, y * width * 4, (y + 1) * width * 4);
  return Buffer.concat([Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]), chunk('IHDR', header), chunk('IDAT', deflateSync(rows, { level: 9 })), chunk('IEND', Buffer.alloc(0))]);
}

/** Paints `shape(x, y) → [r, g, b, a] | null` (unit coordinates) over a cell, 4 × 4 supersampled. */
function paint(pixels, stride, left, top, w, h, shape) {
  for (let py = 0; py < h; py++) {
    for (let px = 0; px < w; px++) {
      let r = 0, g = 0, b = 0, a = 0;
      for (let sy = 0; sy < 4; sy++) {
        for (let sx = 0; sx < 4; sx++) {
          const colour = shape((px + (sx + 0.5) / 4) / w, (py + (sy + 0.5) / 4) / h);
          if (!colour) continue;
          const alpha = colour[3] / 255;
          r += colour[0] * alpha; g += colour[1] * alpha; b += colour[2] * alpha; a += alpha;
        }
      }
      if (a === 0) continue;
      const at = ((top + py) * stride + left + px) * 4;
      pixels[at] = Math.round(r / a); pixels[at + 1] = Math.round(g / a); pixels[at + 2] = Math.round(b / a); pixels[at + 3] = Math.round((a / 16) * 255);
    }
  }
}

const SWEEP = 270;

/** A knob at `position`: an amber value arc on a ring, a brushed body, a white pointer. */
function knob(position) {
  const angle = (-SWEEP / 2 + position * SWEEP) * (Math.PI / 180);
  return (x, y) => {
    const dx = x - 0.5, dy = y - 0.5;
    const r = Math.hypot(dx, dy);
    const a = Math.atan2(dx, -dy);
    // Pointer: a line from near the centre to the body's edge.
    const along = dx * Math.sin(angle) - dy * Math.cos(angle);
    const across = Math.abs(dx * Math.cos(angle) + dy * Math.sin(angle));
    if (along > 0.08 && along < 0.31 && across < 0.025) return [250, 250, 250, 255];
    if (r < 0.34) {
      const shade = 70 - Math.round(60 * r) + Math.round(10 * Math.cos(6 * a));
      return [shade, shade + 4, shade + 10, 255];
    }
    if (r < 0.36) return [20, 22, 26, 255];
    if (r > 0.4 && r < 0.47 && Math.abs(a) <= (SWEEP / 2) * (Math.PI / 180)) {
      return a <= angle ? [255, 167, 38, 255] : [52, 58, 68, 255];
    }
    return null;
  };
}

/** The pulse knob at `position`: a flat disc with a dot, inside a thin teal value arc. */
function dotKnob(position) {
  const angle = (-SWEEP / 2 + position * SWEEP) * (Math.PI / 180);
  const half = (SWEEP / 2) * (Math.PI / 180);
  return (x, y) => {
    const dx = x - 0.5, dy = y - 0.5;
    const r = Math.hypot(dx, dy);
    const a = Math.atan2(dx, -dy);
    // The dot, two thirds out along the pointer's angle.
    const px = 0.5 + 0.21 * Math.sin(angle), py = 0.5 - 0.21 * Math.cos(angle);
    if (Math.hypot(x - px, y - py) < 0.045) return [236, 240, 244, 255];
    if (r < 0.32) {
      const shade = 40 + Math.round(14 * (0.5 - y));
      return [shade, shade + 3, shade + 7, 255];
    }
    if (r < 0.335) return [14, 16, 19, 255];
    if (r > 0.38 && r < 0.43 && Math.abs(a) <= half) {
      return a <= angle ? [77, 208, 196, 255] : [44, 50, 58, 255];
    }
    return null;
  };
}

/** A toggle: a rounded cap with a lamp, green when on. */
function toggle(on) {
  return (x, y) => {
    const inside = (x0, y0, x1, y1, radius) => {
      const cx = Math.min(Math.max(x, x0 + radius), x1 - radius);
      const cy = Math.min(Math.max(y, y0 + radius * 1.75), y1 - radius * 1.75);
      return Math.hypot(x - cx, (y - cy) / 1.75) <= radius;
    };
    if (inside(0.33, 0.38, 0.67, 0.62, 0.04)) return on ? [102, 220, 120, 255] : [44, 48, 56, 255];
    if (inside(0.02, 0.04, 0.98, 0.96, 0.1)) return on ? [46, 58, 70, 255] : [36, 40, 48, 255];
    return null;
  };
}

/** A spark at age `t` (0 new, 1 old): a round glow, white-gold to dim red, shrinking. */
function spark(t) {
  const radius = 0.45 - 0.25 * t;
  const hot = [255, 244 - Math.round(150 * t), 200 - Math.round(190 * t)];
  return (x, y) => {
    const r = Math.hypot(x - 0.5, y - 0.5) / radius;
    if (r >= 1) return null;
    const alpha = Math.round(255 * (1 - r * r) * (1 - 0.6 * t));
    return [...hot, alpha];
  };
}

function strip(name, frames, columns, cellW, cellH, draw, out = OUT) {
  const rows = Math.ceil(frames / columns);
  const width = cellW * columns;
  const height = cellH * rows;
  const pixels = Buffer.alloc(width * height * 4);
  for (let f = 0; f < frames; f++) {
    paint(pixels, width, (f % columns) * cellW, Math.floor(f / columns) * cellH, cellW, cellH, draw(f / (frames - 1)));
  }
  writeFileSync(`${out}${name}`, png(width, height, pixels));
  console.log(`${name}: ${width} × ${height}`);
}

if (SPARKS) {
  strip('spark-strip.png', 8, 8, 32, 32, spark);
  strip('spark.png', 8, 8, 32, 32, spark, fileURLToPath(new URL('../projects/physical-av/ui/assets/', import.meta.url)));
} else if (PULSE) {
  for (const scale of [1, 2]) strip(`knob-strip${scale === 2 ? '@2x' : ''}.png`, 64, 1, 56 * scale, 56 * scale, dotKnob);
} else {
  for (const scale of [1, 2]) {
    const suffix = scale === 2 ? '@2x' : '';
    strip(`knob-strip${suffix}.png`, 64, 1, 48 * scale, 48 * scale, knob);
    strip(`toggle-strip${suffix}.png`, 2, 2, 56 * scale, 32 * scale, (p) => toggle(p >= 0.5));
  }
  strip('knob-sheet@2x.png', 64, 8, 96, 96, knob);
}
