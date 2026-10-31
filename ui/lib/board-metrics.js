// The board's measures per level (ADR 0047 §6.6; the note "Layered
// placement and channel routing", 2026-10-31): a guard, never a judge
// (AGENTS.md rule 6). For a project's structure (`graph`'s), its top level
// as a board on its plate and each composite's die at depth 1 and 2, laid
// out as the page lays them: crossings, bends, the most bends of one wire,
// the forward wires with more than two bends (a chip's wires, not feedback,
// not a leg from the plate, not a die's parameter from its strip) and the
// total length. ui/lib/board-metrics.test.js holds the example projects
// under bounds; `node scripts/board-metrics.mjs` prints the table. Pure: the
// caller reads the files. No DOM.

import { PARAMS, layout } from './schematic-layout.js';
import { exportFacts } from './schematic-groups.js';

const NO_PLATE = Object.freeze({ vias: new Map(), obstacles: [], cards: [], plate: null, controls: [] });

/** Each composite's die laid out once, as the page's `dieFor` (its parameters in its package's top pin order). */
export function diesOf(graph) {
  const factsOf = (m) => exportFacts(graph.composites?.[m] ?? null);
  const cache = new Map();
  const of = (node) => {
    if (!cache.has(node.module)) cache.set(node.module, graph.composites?.[node.module] ? layout(graph.composites[node.module], {}, { factsOf, board: { die: true, dieOf: of, paramOrder: node.topOrder ?? null } }) : null);
    return cache.get(node.module);
  };
  return of;
}

/**
 * The levels of `graph` on `plate` (`{vias, obstacles, cards, plate,
 * controls}`, as ui/lib/fixtures/board/plate-approx.js gives it; none
 * without a panel): `{top, dies: [[path, layout]]}`, each composite once,
 * at depth 1 and 2.
 */
export function levelsOf(graph, plate = NO_PLATE) {
  const factsOf = (m) => exportFacts(graph.composites?.[m] ?? null);
  const of = diesOf(graph);
  const top = layout(graph.top, {}, { factsOf, controls: plate.controls, board: { vias: plate.vias, obstacles: plate.obstacles, plate: plate.plate, cards: plate.cards, dieOf: of } });
  const dies = [];
  const seen = new Set();
  const walk = (level, depth, path, around) => {
    if (depth > 2) return;
    for (const inst of level.instances ?? []) {
      const inside = graph.composites?.[inst.module];
      if (!inside || seen.has(inst.module)) continue;
      seen.add(inst.module);
      const node = around?.nodes.find((n) => n.name === inst.name);
      const die = of({ module: inst.module, topOrder: node?.topOrder ?? null });
      dies.push([`${path}/${inst.name} (${inst.module})`, die]);
      walk(inside, depth + 1, `${path}/${inst.name}`, die);
    }
  };
  walk(graph.top, 1, '', top);
  return { top, dies };
}

/** Whether `w` is a forward wire between chips (not feedback, not a plate's leg, not a die's parameter or frame). */
export function forward(w) {
  return !w.feedback && w.kind !== 'binding' && w.from.node !== PARAMS;
}

/** The measures of board `b` (a `layout`): `{crossings, bends, maxBends, over2, length, wires}`. */
export function measures(b) {
  const m = b.board.metrics;
  let most = 0;
  let over = 0;
  for (const w of b.wires) {
    const bends = Math.max(0, (w.corners?.length ?? 0) - 2);
    most = Math.max(most, bends);
    if (forward(w) && bends > 2) over += 1;
  }
  return { crossings: m.crossings, bends: m.bends, maxBends: most, over2: over, length: m.length, wires: b.wires.length };
}

/** The table's rows for `projects` (`[[name, graph, plate]]`): `[{project, level, ...measures}]`. */
export function table(projects) {
  const rows = [];
  for (const [name, graph, plate] of projects) {
    const { top, dies } = levelsOf(graph, plate ?? NO_PLATE);
    rows.push({ project: name, level: '(top)', ...measures(top) });
    for (const [path, die] of dies) if (die) rows.push({ project: name, level: path, ...measures(die) });
  }
  return rows;
}

/** `rows` as a Markdown table. */
export function markdown(rows) {
  const cols = ['project', 'level', 'crossings', 'bends', 'maxBends', 'over2', 'length'];
  return [`| ${cols.join(' | ')} |`, `|${cols.map(() => '---').join('|')}|`, ...rows.map((r) => `| ${cols.map((c) => r[c]).join(' | ')} |`)].join('\n');
}
