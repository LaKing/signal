// ADR 0047's note "Layered placement and channel routing" (2026-10-31): the
// board's measures per example project, every top level on its plate and
// every die at depth 1 and 2, held under bounds so that a later change
// cannot make them worse unseen. Bounds, not judgements (AGENTS.md rule 6):
// they fail a regression, they choose nothing; the picture is the owner's
// verdict. Each bound is the value measured on 2026-10-31 with headroom
// (crossings +10 % and 2, bends +5 % and 2, the most bends of one wire +2,
// the forward wires with more than two bends +1, length +5 %). Lowered with
// a recorded reason when a change improves them; raised only so.
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { table, markdown } from './board-metrics.js';
import { exampleProjects } from '../../scripts/board-metrics.mjs';

/** `level: [crossings, bends, maxBends, over2, length]`. */
const BOUNDS = {
  // 2026-11-05: its beats and ghost notes meet in a std.merge (ADR 0054 phase 2): a card more, 6 → 10 crossings.
  'kick (top)': [13, 42, 6, 8, 4400],
  'kick /drums (drums)': [2, 11, 4, 1, 300],
  'kick /drums/kick (kick)': [4, 15, 6, 1, 1900],
  'pulse (top)': [48, 172, 16, 17, 15900],
  'physical (top)': [17, 59, 6, 7, 9100],
  'physical-av (top)': [49, 126, 10, 12, 13900],
  'studio (top)': [66, 372, 8, 26, 52800],
  // 2026-11-05: its three sums made instances (ADR 0054): longer by their own wires.
  'studio /synth (moog_voice)': [52, 206, 12, 4, 56800],
  // 2026-11-05: its time and feed sums instances (ADR 0054).
  'studio /echo (tape_delay)': [16, 79, 6, 4, 16900],
  'studio /kick (kick909)': [15, 66, 6, 2, 10600],
  // 2026-11-05: its two pitch sums instances (ADR 0054).
  'studio /snare (snare909)': [8, 63, 6, 1, 8400],
  'studio /clap (clap909)': [4, 36, 6, 1, 5100],
  'studio /hats (hat909)': [11, 80, 6, 3, 13300],
  'studio /tom (tom909)': [7, 48, 6, 1, 5900],
  'studio /rim (rim909)': [2, 34, 6, 1, 4700],
  'studio /cymbal (cymbal909)': [2, 47, 6, 1, 7900],
  'studio-transport (top)': [197, 468, 16, 33, 85100],
  'studio-transport /synth (moog_voice)': [52, 202, 8, 4, 54700],
  'studio-transport /echo (tape_delay)': [16, 79, 6, 4, 16900],
  'studio-transport /kick (kick909)': [15, 66, 6, 2, 10600],
  'studio-transport /snare (snare909)': [8, 63, 6, 1, 8400],
  'studio-transport /clap (clap909)': [4, 36, 6, 1, 5100],
  'studio-transport /hats (hat909)': [11, 80, 6, 3, 13300],
  'studio-transport /tom (tom909)': [7, 48, 6, 1, 5900],
  'studio-transport /rim (rim909)': [2, 34, 6, 1, 4700],
  'studio-transport /cymbal (cymbal909)': [2, 47, 6, 1, 7900],
  'library-demo (top)': [13, 101, 6, 2, 17300],
  'two-decks (top)': [11, 82, 14, 9, 8900],
  'two-studios (top)': [2, 7, 4, 1, 2400],
};
const KEYS = ['crossings', 'bends', 'maxBends', 'over2', 'length'];

test('the board’s measures stay within their bounds: every example project’s top level and its dies at depth 1 and 2', () => {
  const rows = table(exampleProjects());
  console.log(markdown(rows));
  const seen = new Set();
  for (const r of rows) {
    const key = `${r.project} ${r.level}`;
    seen.add(key);
    const bound = BOUNDS[key];
    assert.ok(bound, `${key}: no bound recorded (add one with its measures)`);
    KEYS.forEach((k, i) => assert.ok(r[k] <= bound[i], `${key}: ${k} ${r[k]} over its bound ${bound[i]}`));
  }
  for (const key of Object.keys(BOUNDS)) assert.ok(seen.has(key), `${key}: measured no more`);
});

test('the synth’s die (the owner’s worst case): well under the 63 crossings and 368 bends of the drawing he judged', () => {
  const rows = table(exampleProjects().filter(([name]) => name === 'studio'));
  const synth = rows.find((r) => r.level === '/synth (moog_voice)');
  assert.ok(synth.crossings < 63 && synth.bends < 368, `${synth.crossings} crossings, ${synth.bends} bends`);
  const top = rows.find((r) => r.level === '(top)');
  assert.ok(top.crossings <= 63 && top.bends <= 368, `the top board: ${top.crossings} crossings, ${top.bends} bends`);
});
