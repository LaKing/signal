// The schematic's layout (M2.9-1, ADR 0026 §3): a layered drawing of one
// graph level (the top level, or a composite's inside, as `graph` gives
// it), left to right by signal flow. Feedback connections (back edges of a
// depth-first search in source order) are reversed for layering and drawn
// back around below; layers come from the longest path, and each layer is
// ordered by a few barycentre sweeps. A node with a stored position keeps it,
// and the others move down until they overlap nothing. Deterministic: the
// result depends on the arrays' order (source order) and never on an
// object's key order. No DOM.

/** Geometry, in the layout's own units. */
export const HEADER = 34;
export const ROW = 16;
export const PAD = 8;
export const CHAR = 6.5;
export const MIN_WIDTH = 96;
export const GAP_X = 72;
export const GAP_Y = 24;
const SWEEPS = 4;

/** Where an export pseudo-node's pins go: a composite's frame. */
export const INPUTS = '@inputs';
export const OUTPUTS = '@outputs';

/** `signal`, `events` or `vector`, from a port's `shape`. */
export function shapeName(shape) {
  if (shape && typeof shape === 'object' && 'vector' in shape) return 'vector';
  return shape === 'events' ? 'events' : 'signal';
}

/** An endpoint as written, without the top level's leading `/`. */
function endpoint(text) {
  const plain = String(text ?? '').replace(/^\//, '');
  const dot = plain.lastIndexOf('.');
  return dot < 0 ? { node: plain, port: '' } : { node: plain.slice(0, dot), port: plain.slice(dot + 1) };
}

function pin(port, direction) {
  return { name: port.name, shape: shapeName(port.shape), kind: port.kind ?? null, rate: port.rate ?? null, direction };
}

/** The nodes of a level, before placement. */
function nodesOf(level) {
  const nodes = [];
  const inputs = [...(level.inputs ?? []), ...(level.parameters ?? []).map((p) => ({ ...p, shape: 'signal' }))];
  if (inputs.length) {
    nodes.push({ id: INPUTS, name: 'inputs', module: '', kind: 'input', composite: false, ins: [], outs: inputs.map((p) => pin(p, 'out')) });
  }
  for (const instance of level.instances ?? []) {
    const ports = instance.ports ?? [];
    nodes.push({
      id: instance.name,
      name: instance.name,
      module: instance.module,
      kind: 'instance',
      composite: Boolean(instance.composite),
      // As `graph` gives them, for the inspector.
      parameters: instance.parameters ?? [],
      ins: [
        ...ports.filter((p) => p.direction === 'in').map((p) => pin(p, 'in')),
        ...(instance.parameters ?? []).map((p) => ({ ...pin({ ...p, shape: 'signal' }, 'in'), parameter: true })),
      ],
      outs: ports.filter((p) => p.direction === 'out').map((p) => pin(p, 'out')),
    });
  }
  if ((level.outputs ?? []).length) {
    nodes.push({ id: OUTPUTS, name: 'outputs', module: '', kind: 'output', composite: false, ins: level.outputs.map((p) => pin(p, 'in')), outs: [] });
  }
  for (const node of nodes) {
    const longestIn = Math.max(0, ...node.ins.map((p) => p.name.length));
    const longestOut = Math.max(0, ...node.outs.map((p) => p.name.length));
    node.w = Math.ceil(Math.max(MIN_WIDTH, CHAR * Math.max(node.name.length, node.module.length) + 2 * PAD, CHAR * (longestIn + longestOut) + 4 * PAD));
    node.h = HEADER + ROW * Math.max(1, node.ins.length, node.outs.length) + PAD;
  }
  return nodes;
}

/** The level's edges: its connections, and a composite's exports to and from its frame. */
function edgesOf(level, byId) {
  const edges = [];
  // Only between pins that are drawn.
  const add = (from, to, wire) => {
    if (!byId.get(from.node)?.outs.some((p) => p.name === from.port)) return;
    if (!byId.get(to.node)?.ins.some((p) => p.name === to.port)) return;
    edges.push({ from, to, ...wire });
  };
  for (const c of level.connections ?? []) {
    const from = endpoint(c.from);
    const source = byId.get(from.node)?.outs.find((p) => p.name === from.port);
    add(from, endpoint(c.to), {
      kind: 'connection',
      shape: source?.shape ?? 'signal',
      converters: c.converters ?? [],
      scale: c.scale ?? null,
      label: `${c.from} → ${c.to}`,
      written: { from: c.from, to: c.to },
    });
  }
  for (const e of [...(level.inputs ?? []), ...(level.parameters ?? [])]) {
    for (const target of e.to ?? []) {
      add({ node: INPUTS, port: e.name }, endpoint(target), { kind: 'export', shape: shapeName(e.shape ?? 'signal'), converters: [], scale: null, label: `${e.name} → ${target}`, written: { from: e.name, to: target } });
    }
  }
  for (const e of level.outputs ?? []) {
    add(endpoint(e.from), { node: OUTPUTS, port: e.name }, { kind: 'export', shape: shapeName(e.shape), converters: [], scale: null, label: `${e.from} → ${e.name}`, written: { from: e.from, to: e.name } });
  }
  return edges;
}

/** Back edges of a depth-first search in node order, by edge index. */
function backEdges(nodes, edges) {
  const out = new Map(nodes.map((n) => [n.id, []]));
  edges.forEach((e, i) => out.get(e.from.node).push(i));
  const state = new Map();
  const back = new Set();
  for (const root of nodes) {
    if (state.has(root.id)) continue;
    const stack = [[root.id, 0]];
    state.set(root.id, 'open');
    while (stack.length) {
      const top = stack[stack.length - 1];
      const list = out.get(top[0]);
      if (top[1] >= list.length) {
        state.set(top[0], 'done');
        stack.pop();
        continue;
      }
      const i = list[top[1]++];
      const next = edges[i].to.node;
      if (state.get(next) === 'open') back.add(i);
      else if (!state.has(next)) {
        state.set(next, 'open');
        stack.push([next, 0]);
      }
    }
  }
  return back;
}

/** Longest-path layers over the edges with back edges reversed. */
function layers(nodes, edges, back) {
  const preds = new Map(nodes.map((n) => [n.id, new Set()]));
  edges.forEach((e, i) => {
    const [a, b] = back.has(i) ? [e.to.node, e.from.node] : [e.from.node, e.to.node];
    if (a !== b) preds.get(b).add(a);
  });
  const layer = new Map();
  const visit = (id, seen) => {
    if (layer.has(id)) return layer.get(id);
    if (seen.has(id)) return 0;
    seen.add(id);
    let l = 0;
    for (const p of preds.get(id)) l = Math.max(l, visit(p, seen) + 1);
    layer.set(id, l);
    return l;
  };
  for (const n of nodes) visit(n.id, new Set());
  // The frame's pins: inputs first, outputs last.
  const last = most(nodes.filter((n) => n.kind === 'instance').map((n) => layer.get(n.id)), 0) + 1;
  for (const n of nodes) {
    if (n.kind === 'input') layer.set(n.id, 0);
    if (n.kind === 'output') layer.set(n.id, last);
  }
  if (nodes.some((n) => n.kind === 'input')) {
    for (const n of nodes) if (n.kind === 'instance') layer.set(n.id, Math.max(1, layer.get(n.id)));
  }
  return layer;
}

/** The largest of `values` and `from` (Math.max without spreading them). */
function most(values, from) {
  let m = from;
  for (const v of values) if (v > m) m = v;
  return m;
}
/** The smallest of `values` and `from`. */
function least(values, from) {
  let m = from;
  for (const v of values) if (v < m) m = v;
  return m;
}

/** Each layer's nodes in barycentre order: a few sweeps down and up. */
function order(nodes, edges, layer) {
  const count = most(layer.values(), 0) + 1;
  const rows = Array.from({ length: count }, () => []);
  for (const n of nodes) rows[layer.get(n.id)].push(n.id);
  const index = new Map();
  const number = () => rows.forEach((row) => row.forEach((id, i) => index.set(id, i)));
  number();
  // Each node's neighbours in the layer above and below, in edge order,
  // gathered once (not per sweep: a large project has thousands of edges).
  const above = new Map(nodes.map((n) => [n.id, []]));
  const beneath = new Map(nodes.map((n) => [n.id, []]));
  for (const e of edges) {
    if (layer.get(e.from.node) === layer.get(e.to.node) - 1) {
      above.get(e.to.node)?.push(e.from.node);
      beneath.get(e.from.node)?.push(e.to.node);
    }
  }
  const neighbours = (id, below) => (below ? above : beneath).get(id) ?? [];
  const sweep = (r, below) => {
    const keyed = rows[r].map((id, i) => {
      const near = neighbours(id, below);
      const key = near.length ? near.reduce((s, o) => s + index.get(o), 0) / near.length : i;
      return { id, key, i };
    });
    keyed.sort((a, b) => a.key - b.key || a.i - b.i);
    rows[r] = keyed.map((k) => k.id);
    rows[r].forEach((id, i) => index.set(id, i));
  };
  for (let s = 0; s < SWEEPS; s++) {
    for (let r = 1; r < count; r++) sweep(r, true);
    for (let r = count - 2; r >= 0; r--) sweep(r, false);
  }
  return rows;
}

function overlaps(a, b) {
  return a.x < b.x + b.w + PAD && b.x < a.x + a.w + PAD && a.y < b.y + b.h + PAD && b.y < a.y + a.h + PAD;
}

function anchor(node, name, out) {
  const list = out ? node.outs : node.ins;
  const i = Math.max(0, list.findIndex((p) => p.name === name));
  return { x: out ? node.x + node.w : node.x, y: node.y + HEADER + ROW * (i + 0.5) };
}

/**
 * The layout of `level` with the stored `positions` (`{instance: {x, y}}`)
 * of its instances: `{nodes, wires, bounds}`. Nodes carry their box (`x`,
 * `y`, `w`, `h`), `layer`, `pinned` and their pins (`ins` left, `outs`
 * right, each with its `x`, `y`); wires their polyline `points`, `shape`,
 * `converters`, `dashed` (a converter changes the rate) and `feedback`.
 */
export function layout(level, positions = {}) {
  const nodes = nodesOf(level ?? {});
  const byId = new Map(nodes.map((n) => [n.id, n]));
  const edges = edgesOf(level ?? {}, byId);
  const back = backEdges(nodes, edges);
  const layer = layers(nodes, edges, back);
  const rows = order(nodes, edges, layer);
  let x = 0;
  for (const row of rows) {
    let y = 0;
    let width = 0;
    for (const id of row) {
      const node = byId.get(id);
      node.layer = layer.get(id);
      node.x = x;
      node.y = y;
      y += node.h + GAP_Y;
      width = Math.max(width, node.w);
    }
    x += width + GAP_X;
  }
  // Stored positions first, then every other node, in layer order, moved
  // down past whatever it overlaps.
  const placed = [];
  for (const node of nodes) {
    const stored = node.kind === 'instance' ? positions?.[node.name] : undefined;
    node.pinned = Boolean(stored && Number.isFinite(stored.x) && Number.isFinite(stored.y));
    if (node.pinned) {
      node.x = stored.x;
      node.y = stored.y;
      placed.push(node);
    }
  }
  for (const row of rows) {
    for (const id of row) {
      const node = byId.get(id);
      if (node.pinned) continue;
      for (let moved = true; moved; ) {
        moved = false;
        for (const other of placed) {
          if (overlaps(node, other)) {
            node.y = other.y + other.h + GAP_Y;
            moved = true;
          }
        }
      }
      placed.push(node);
    }
  }
  for (const node of nodes) {
    node.ins = node.ins.map((p) => ({ ...p, ...anchor(node, p.name, false) }));
    node.outs = node.outs.map((p) => ({ ...p, ...anchor(node, p.name, true) }));
  }
  const bottom = most(nodes.map((n) => n.y + n.h), 0);
  let below = 0;
  const wires = edges.map((e, i) => {
    const source = byId.get(e.from.node);
    const target = byId.get(e.to.node);
    const a = anchor(source, e.from.port, true);
    const b = anchor(target, e.to.port, false);
    const feedback = back.has(i) || b.x <= a.x;
    let points;
    if (feedback) {
      below += 1;
      const under = Math.max(bottom, source.y + source.h, target.y + target.h) + GAP_Y / 2 + below * 6;
      points = [a, { x: a.x + 12, y: a.y }, { x: a.x + 12, y: under }, { x: b.x - 12, y: under }, { x: b.x - 12, y: b.y }, b];
    } else {
      const mid = (a.x + b.x) / 2;
      points = [a, { x: mid, y: a.y }, { x: mid, y: b.y }, b];
    }
    return {
      id: i,
      from: e.from,
      to: e.to,
      written: e.written,
      kind: e.kind,
      shape: e.shape,
      converters: e.converters,
      scale: e.scale,
      dashed: e.converters.length > 0,
      feedback,
      reversed: back.has(i),
      label: e.label,
      points,
    };
  });
  const xs = [...nodes.flatMap((n) => [n.x, n.x + n.w]), ...wires.flatMap((w) => w.points.map((p) => p.x))];
  const ys = [...nodes.flatMap((n) => [n.y, n.y + n.h]), ...wires.flatMap((w) => w.points.map((p) => p.y))];
  // Loops, not Math.min(...xs): a large project's points exceed an
  // engine's argument limit.
  const [x0, x1, y0, y1] = [least(xs, Infinity), most(xs, -Infinity), least(ys, Infinity), most(ys, -Infinity)];
  const bounds = xs.length ? { x: x0, y: y0, w: x1 - x0, h: y1 - y0 } : { x: 0, y: 0, w: MIN_WIDTH, h: HEADER };
  return { nodes, wires, bounds };
}
