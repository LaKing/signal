// A project panel's plate as the page would lay it out at 1280 px, by its
// structure (ADR 0047, the board's tests): its cards stacked as `wide`
// cards are, each a heading, its rows of controls wrapping at the card's
// width (groups a gap apart), a piano roll or a step grid the card's whole
// width; every control's cell its default panel size (`CELLS`, the knob
// the studio's 46 px knob with its label). An approximation of the
// measured plate, as dense as it is (it has no browser): every rectangle
// in whole plate units. Pure.

import { panelControls } from '../../schematic-controls.js';
import { PU } from '../../plate.js';

const WIDTH = 1280;
const PAD = 16;
const GAP = 8;
const GROUP_GAP = 16;
/** Cell sizes (CSS px) of each element, with its label. */
const CELLS = {
  knob: [64, 88],
  fader: [64, 200],
  meter: [40, 200],
  scope: [320, 160],
  canvas: [320, 160],
  toggle: [64, 48],
  button: [64, 48],
  value: [96, 40],
  xy: [184, 184],
  keyboard: [480, 96],
  entry: [96, 40],
  slot: [WIDTH - 2 * PAD, 240],
};
const BLOCKS = { 'piano-roll': 320, 'step-grid': 200 };

const snap = (r) => ({ x: Math.round(r.x / PU) * PU, y: Math.round(r.y / PU) * PU, w: Math.max(PU, Math.round(r.w / PU) * PU), h: Math.max(PU, Math.round(r.h / PU) * PU) });

/** `{plate, vias, obstacles, cards}` of panel text `html`: `vias` a Map of `panelControls`' index → rectangle. */
export function approximatePlate(html) {
  const text = String(html).replace(/<!--[\s\S]*?-->/g, '');
  const controls = panelControls(text);
  const vias = new Map();
  const obstacles = [];
  let y = PAD;
  let x = PAD;
  let rowH = 0;
  let inCard = false;
  let bound = 0;
  const cards = [];
  let cardTop = 0;
  const newline = () => {
    if (rowH) y += rowH + GAP;
    x = PAD + PAD;
    rowH = 0;
  };
  for (const m of text.matchAll(/<(\/?)(section|h1|h2|h3|p|div|signal-[a-z-]+)\b([^>]*)>/g)) {
    const [, close, tag, attrs] = m;
    if (tag === 'section') {
      if (!close) {
        inCard = true;
        y += GAP;
        cardTop = y;
        x = 2 * PAD;
        rowH = 0;
      } else {
        newline();
        y += PAD;
        cards.push({ x: 0, y: cardTop, w: WIDTH, h: y - cardTop });
        inCard = false;
      }
      continue;
    }
    if (!inCard || close) {
      if (close && tag === 'div' && rowH) {
        x += GROUP_GAP;
      }
      continue;
    }
    if (tag === 'h1' || tag === 'h2' || tag === 'h3') {
      newline();
      y += 32;
      continue;
    }
    if (tag === 'p') {
      newline();
      y += 40;
      continue;
    }
    if (tag === 'div') {
      if (/class="[^"]*row/.test(attrs)) newline();
      continue;
    }
    const type = tag.slice(7);
    if (BLOCKS[type]) {
      newline();
      obstacles.push(snap({ x: 2 * PAD, y, w: WIDTH - 4 * PAD, h: BLOCKS[type] }));
      y += BLOCKS[type] + GAP;
      continue;
    }
    const cell = CELLS[type];
    if (!cell) continue;
    if (x + cell[0] > WIDTH - 2 * PAD) newline();
    const r = snap({ x, y, w: cell[0] - 8, h: cell[1] - 8 });
    obstacles.push(r);
    if (/\bbind\s*=\s*"[^"]+"/.test(attrs) && type !== 'slot') {
      const c = controls[bound++];
      if (c) vias.set(c.index, r);
    }
    x += cell[0];
    rowH = Math.max(rowH, cell[1]);
  }
  return { vias, obstacles, cards, plate: { w: WIDTH, h: Math.ceil(y / PU) * PU }, controls };
}
