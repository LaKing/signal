#!/usr/bin/env node
// The board's measures of every example project (a guard, never a judge:
// AGENTS.md rule 6; ADR 0047's note "Layered placement and channel routing",
// 2026-10-31): each project's top level as a board on its plate (its panel
// laid out as ui/lib/fixtures/board/plate-approx.js approximates it; the
// studio's as the page measured it) and each composite's die at depth 1 and
// 2: crossings, bends, the most bends of one wire, the forward wires with
// more than two bends, the total length. The structures are `graph`'s, kept
// as fixtures (ui/lib/fixtures/). gpu-partials needs a `lab` build to load
// and has none.
//
//   node scripts/board-metrics.mjs            the table (Markdown)
//   node scripts/board-metrics.mjs --json     the rows

import { existsSync, readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { table, markdown } from '../ui/lib/board-metrics.js';
import { approximatePlate } from '../ui/lib/fixtures/board/plate-approx.js';

const root = new URL('../', import.meta.url);
const read = (path) => readFileSync(new URL(path, root), 'utf8');

/** Each example project with a fixture: its structure's path. */
export const FIXTURES = {
  kick: 'ui/lib/fixtures/board/kick.json',
  pulse: 'ui/lib/fixtures/board/pulse.json',
  physical: 'ui/lib/fixtures/board/physical.json',
  'physical-av': 'ui/lib/fixtures/board/physical-av.json',
  studio: 'ui/lib/fixtures/studio-graph.json',
  'studio-transport': 'ui/lib/fixtures/board/studio-transport.json',
  'library-demo': 'ui/lib/fixtures/board/library-demo.json',
  'two-decks': 'ui/lib/fixtures/board/two-decks.json',
  'two-studios': 'ui/lib/fixtures/board/two-studios.json',
};

/** `[[name, graph, plate]]` of every example project with a fixture. */
export function exampleProjects() {
  return Object.entries(FIXTURES).map(([name, path]) => {
    const graph = JSON.parse(read(path));
    // The studio's panel as its plate was measured (frozen when ADR 0051 phase 1 moved its tempo knob).
    const panel = new URL(name === 'studio' ? 'ui/lib/fixtures/board/studio-main.txt' : `projects/${name}/ui/main.html`, root);
    let plate = existsSync(panel) ? approximatePlate(readFileSync(panel, 'utf8')) : undefined;
    if (name === 'studio') {
      const fx = JSON.parse(read('ui/lib/fixtures/board/studio-plate.json'));
      plate = { controls: plate.controls, vias: new Map(fx.vias), obstacles: fx.obstacles, cards: fx.cards, plate: fx.size };
    }
    return [name, graph, plate];
  });
}

if (process.argv[1] === fileURLToPath(import.meta.url)) {
  const rows = table(exampleProjects());
  console.log(process.argv.includes('--json') ? JSON.stringify(rows, null, 1) : markdown(rows));
}
