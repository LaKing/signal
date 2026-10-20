// The schematic's path tracing (ADR 0030 §1): from a pin, a wire or an
// instance, the connections followed upstream, downstream or both, across
// composite boundaries (an exported port leads to the inner ports it
// forwards, and back out), through fan-in, fan-out and the converters on a
// connection, to the real endpoints: primitive ports, unconnected ends, and
// library composites whose inside `graph` does not carry (`lib.*`, ADR 0027
// §6: those are opaque endpoints). Computed from the `graph` message's
// structure, every level as written, by concrete instance path (a composite
// used twice is traced once per instance, as the canvas draws it). Cycles
// end where a pin is reached again. No DOM.
//
// Depth (M2.13-2): `next` (to the next module, as above) or `through`
// (through modules): a primitive is crossed as well, from any input or
// parameter to all its outputs downstream and from an output to all its
// inputs and parameters upstream, until sinks (no outputs) and sources (no
// input ports; a constant's parameters are set by their values). A structural path, not a signal-dependent one: a
// gain at 0 or a switch turned off still passes, since the trace reads the
// project's structure and never its values.
//
// Addresses are the canvas's: `/synth.hz` is the composite instance's port,
// which is also the boundary pin `hz` of the level drawn at `/synth`;
// `/synth/glide.in` a pin inside it. A wire is named by its level's path and
// its ends as written (`written` on a layout's wire): a connection's
// `from`, `to`; an exported input's or parameter's `name`, inner target; an
// exported output's inner source, `name`.

/** The directions of a trace. */
export const DIRECTIONS = ['up', 'down', 'both'];
/** Its depths: to the next module, or through modules. */
export const DEPTHS = ['next', 'through'];

/** A level's edges by end (relative: `inst.port`, or `@.name` for its frame), computed once per level object. */
const edgeCache = new WeakMap();

const strip = (text) => String(text ?? '').replace(/^\//, '');

function levelEdges(data) {
  let edges = edgeCache.get(data);
  if (edges) return edges;
  const down = new Map();
  const up = new Map();
  const add = (from, to, edge) => {
    const e = { from, to, ...edge };
    if (!down.has(from)) down.set(from, []);
    down.get(from).push(e);
    if (!up.has(to)) up.set(to, []);
    up.get(to).push(e);
  };
  for (const c of data.connections ?? []) {
    add(strip(c.from), strip(c.to), { kind: 'connection', written: { from: c.from, to: c.to }, converters: c.converters ?? [], scale: c.scale ?? null });
  }
  for (const e of [...(data.inputs ?? []), ...(data.parameters ?? [])]) {
    for (const target of e.to ?? []) add(`@.${e.name}`, strip(target), { kind: 'export', written: { from: e.name, to: target }, converters: [], scale: null });
  }
  for (const e of data.outputs ?? []) {
    if (e.from) add(strip(e.from), `@.${e.name}`, { kind: 'export', written: { from: e.from, to: e.name }, converters: [], scale: null });
  }
  edges = { down, up };
  edgeCache.set(data, edges);
  return edges;
}

/** `address` (`/a/b.port`) as `{instance: '/a/b', port}`, or null. */
export function splitAddress(address) {
  const text = String(address ?? '');
  const dot = text.lastIndexOf('.');
  if (dot <= 0) return null;
  return { instance: text.slice(0, dot), port: text.slice(dot + 1) };
}

/** The level path an instance path is drawn in (`/a/b` → `/a`, `/a` → ``). */
export function parentPath(instance) {
  return instance.slice(0, instance.lastIndexOf('/'));
}

/** The address of end `rel` (`inst.port`, `@.name`) of the level at `path`. */
function addressAt(path, rel) {
  return rel.startsWith('@.') ? `${path}.${rel.slice(2)}` : `${path}/${rel}`;
}

/**
 * The resolver of `graph`'s levels by instance path, cached for one trace:
 * `level(path)` the level's data (`graph.top`, a composite's), null when
 * the path is not a composite whose inside `graph` carries; `instance(path)`
 * the instance's entry in its parent level (`{name, module, composite,
 * ports, parameters}`), null when there is none.
 */
export function resolver(graph) {
  const levels = new Map([['', graph?.top ?? null]]);
  const instances = new Map();
  const instance = (path) => {
    if (instances.has(path)) return instances.get(path);
    const parent = level(parentPath(path));
    const name = path.slice(path.lastIndexOf('/') + 1);
    const found = parent?.instances?.find((n) => n.name === name) ?? null;
    instances.set(path, found);
    return found;
  };
  const level = (path) => {
    if (levels.has(path)) return levels.get(path);
    const node = path ? instance(path) : null;
    const data = node?.composite ? (graph?.composites?.[node.module] ?? null) : null;
    levels.set(path, data);
    return data;
  };
  return { level, instance };
}

/** What pin `address` is: `{address, instance, port, direction, parameter, shape, module, composite, opaque}`, or null. */
export function pinInfo(graph, address, r = resolver(graph)) {
  const at = splitAddress(address);
  if (!at) return null;
  const node = r.instance(at.instance);
  if (!node) return null;
  const port = (node.ports ?? []).find((p) => p.name === at.port);
  const parameter = port ? null : (node.parameters ?? []).find((p) => p.name === at.port);
  if (!port && !parameter) return null;
  const composite = Boolean(node.composite);
  return {
    address,
    instance: at.instance,
    port: at.port,
    direction: port ? port.direction : 'in',
    parameter: Boolean(parameter),
    shape: port?.shape ?? 'signal',
    module: node.module,
    composite,
    // A composite whose inside `graph` does not carry (a library composite).
    opaque: composite && !r.level(at.instance),
  };
}

/**
 * The edges leaving `address` in direction `dir` (`up`, `down`): those of
 * the level it is drawn in (its instance's parent) and, for a composite's
 * port, those of its inside (as the frame's pin). Each is `{path, level
 * data, edge, next}` (`next` the address at its other end).
 */
function edgesFrom(address, dir, r) {
  const at = splitAddress(address);
  if (!at) return [];
  const found = [];
  const view = (path, rel) => {
    const data = r.level(path);
    if (!data) return;
    for (const edge of levelEdges(data)[dir].get(rel) ?? []) {
      found.push({ path, edge, next: addressAt(path, dir === 'down' ? edge.to : edge.from) });
    }
  };
  view(parentPath(at.instance), `${at.instance.slice(at.instance.lastIndexOf('/') + 1)}.${at.port}`);
  view(at.instance, `@.${at.port}`);
  return found;
}

/**
 * Through modules: the pins of primitive pin `address`'s own instance it
 * leads to in direction `dir` (downstream from an input or a parameter, all
 * its outputs; upstream from an output, all its inputs and parameters), as
 * `{path, edge: null, next, through}`; none for a composite's pin.
 */
function crossing(address, dir, r) {
  const at = splitAddress(address);
  const node = at && r.instance(at.instance);
  if (!node || node.composite) return [];
  const port = (node.ports ?? []).find((p) => p.name === at.port);
  const output = port?.direction === 'out';
  const path = parentPath(at.instance);
  const via = (name) => ({ path, edge: null, next: `${at.instance}.${name}`, through: at.instance });
  if (dir === 'down' && !output) return (node.ports ?? []).filter((p) => p.direction === 'out').map((p) => via(p.name));
  if (dir === 'up' && output) return [...(node.ports ?? []).filter((p) => p.direction !== 'out'), ...(node.parameters ?? [])].map((p) => via(p.name));
  return [];
}

/** Whether level path `path` is `inside` (the composite instance's own level, or one deeper). */
export function within(path, inside) {
  return path === inside || path.startsWith(`${inside}/`);
}

/** The key of a wire in `wires`: its ends as written. */
export function wireKey(written) {
  return `${written.from}\n${written.to}`;
}

/**
 * A breadth-first walk from `starts` (addresses) in direction `dir`, the
 * edges limited to those whose level path passes `keep(path)`. Returns
 * `{pins: Map(address → {hops, via, from}), wires: [{path, edge}],
 * endpoints: [address], cycles}`: every pin reached (the starts too), the
 * edges taken, the pins with no edge onward (the starts only when they
 * are reached again), and how many edges led back to a pin already reached.
 */
function walk(starts, dir, r, keep = () => true, through = false) {
  const pins = new Map();
  const wires = [];
  const endpoints = [];
  let cycles = 0;
  const queue = [];
  for (const s of starts) {
    if (pins.has(s)) continue;
    pins.set(s, { hops: 0, via: [], from: null, start: true });
    queue.push(s);
  }
  for (let i = 0; i < queue.length; i++) {
    const address = queue[i];
    const here = pins.get(address);
    const onward = edgesFrom(address, dir, r).filter((e) => keep(e.path));
    if (through) onward.push(...crossing(address, dir, r));
    if (!onward.length && !here.start) endpoints.push(address);
    for (const { path, edge, next, through: crossed } of onward) {
      if (edge) wires.push({ path, edge });
      if (pins.has(next)) {
        // A module's own pin reached again through it is not a loop.
        if (edge) cycles += 1;
        continue;
      }
      pins.set(next, { hops: here.hops + 1, via: edge ? [...here.via, ...edge.converters.map((c) => c.name)] : here.via, from: address, start: false, through: crossed ?? null });
      queue.push(next);
    }
  }
  return { pins, wires, endpoints, cycles };
}

/**
 * What kind of end pin `address` is: `primitive`, `library` (opaque),
 * `unconnected` (an open end: a composite's port leading nowhere, or a
 * module's output reached downstream, its input upstream, with nothing
 * connected on), `unknown`; through
 * modules, a primitive with no outputs reached downstream is a `sink`, one
 * with no input ports (a constant: parameters only) reached upstream a
 * `source`.
 */
function endKind(info, dir, through, r) {
  if (!info) return 'unknown';
  if (info.composite) return info.opaque ? 'library' : 'unconnected';
  // A module's output reached downstream (or its input upstream) that leads nowhere: an open end, never a sink or a source (M2.13-2).
  if (!info.parameter && (dir === 'down' ? info.direction === 'out' : info.direction !== 'out')) return 'unconnected';
  if (through) {
    const node = r.instance(info.instance);
    const outs = (node?.ports ?? []).filter((p) => p.direction === 'out').length;
    // A source has no input ports: its parameters, if any, set by their values (a constant).
    const ins = (node?.ports ?? []).length - outs;
    if (dir === 'down' && outs === 0) return 'sink';
    if (dir === 'up' && ins === 0) return 'source';
  }
  return 'primitive';
}

function endpointOf(address, reached, r, graph, dir = 'down', through = false) {
  const info = pinInfo(graph, address, r);
  return {
    address,
    instance: info?.instance ?? splitAddress(address)?.instance ?? '',
    port: info?.port ?? splitAddress(address)?.port ?? '',
    level: parentPath(info?.instance ?? splitAddress(address)?.instance ?? ''),
    kind: endKind(info, dir, through, r),
    module: info?.module ?? null,
    direction: info?.direction ?? null,
    hops: reached.hops,
    via: reached.via,
  };
}

/**
 * Where a trace starts, from a hit (`hitTest`'s, `ui/lib/schematic-hit.js`):
 * `{kind: 'pin', address}` (an instance's pin, or a composite's frame pin,
 * which is the composite instance's port), `{kind: 'instance', address}`,
 * `{kind: 'wire', path, written: {from, to}}`; null for the background.
 */
export function startOf(hit) {
  if (!hit) return null;
  if (hit.kind === 'port') return { kind: 'pin', address: hit.address };
  if (hit.kind === 'instance') return { kind: 'instance', address: hit.address };
  if (hit.kind === 'wire') return { kind: 'wire', path: hit.path, written: { ...hit.wire.written } };
  // A collapsed port group (M2.13-2): its members together.
  if (hit.kind === 'group') return { kind: 'pins', addresses: hit.ports.map((p) => p.address), label: [hit.address, hit.label].filter(Boolean).join(' ') };
  return null;
}

/** The addresses of a wire's ends at level `path` (`written` as the layout's), as the trace names them. */
export function wireEnds(path, written, r) {
  const data = r.level(path);
  if (!data) return null;
  for (const edges of levelEdges(data).down.values()) {
    for (const e of edges) {
      if (e.written.from === written.from && e.written.to === written.to) return { from: addressAt(path, e.from), to: addressAt(path, e.to), edge: e };
    }
  }
  return null;
}

/**
 * The trace of `start` (`startOf`'s) in `direction` (`up`, `down`, `both`)
 * over `graph`. Returns null when the start is not in `graph`; else
 * `{start, direction, label, pins: Set(address), instances: Set(instance
 * path), wires: Map(level path → Set(wireKey)), levels: [path] (outermost
 * first, then by path), up, down, cycles, opaque: [address]}` with `up` and
 * `down` (null for a direction not traced) `{endpoints: [{address,
 * instance, port, level, kind, module, direction, hops, via}], pins: count,
 * wires: count}`. A pin start traces upstream from it, downstream from it
 * or both; an instance from its inputs and parameters upstream and its
 * outputs downstream (a composite instance also through its inside: its
 * inputs inward downstream, its outputs inward upstream); a wire from its source upstream and its target
 * downstream, the wire itself included. `depth` is `next` (stop at the next
 * module) or `through` (cross modules, see above); the result carries it,
 * and `hops`: each pin's fewest connections or crossings from the start.
 */
export function trace(graph, start, direction = 'both', { depth = 'next' } = {}) {
  if (!graph || !start) return null;
  const through = depth === 'through';
  const r = resolver(graph);
  const ups = [];
  const downs = [];
  const seed = { wires: [] };
  let label;
  if (start.kind === 'pin') {
    if (!pinInfo(graph, start.address, r)) return null;
    ups.push(start.address);
    downs.push(start.address);
    label = start.address;
  } else if (start.kind === 'pins') {
    const found = start.addresses.filter((a) => pinInfo(graph, a, r));
    if (!found.length) return null;
    ups.push(...found);
    downs.push(...found);
    label = start.label ?? found.join(', ');
  } else if (start.kind === 'instance') {
    const node = r.instance(start.address);
    if (!node) return null;
    for (const p of node.ports ?? []) (p.direction === 'out' ? downs : ups).push(`${start.address}.${p.name}`);
    for (const p of node.parameters ?? []) ups.push(`${start.address}.${p.name}`);
    label = start.address;
  } else if (start.kind === 'wire') {
    const ends = wireEnds(start.path, start.written, r);
    if (!ends) return null;
    ups.push(ends.from);
    downs.push(ends.to);
    seed.wires.push({ path: start.path, edge: ends.edge });
    seed.ends = ends;
    label = `${ends.from} → ${ends.to}`;
  } else return null;
  const wantUp = direction === 'up' || direction === 'both';
  const wantDown = direction === 'down' || direction === 'both';
  const pins = new Set();
  const wires = new Map();
  const levels = new Set();
  const opaque = new Set();
  let cycles = 0;
  const addWire = ({ path, edge }) => {
    if (!wires.has(path)) wires.set(path, new Set());
    wires.get(path).add(wireKey(edge.written));
    levels.add(path);
  };
  seed.wires.forEach(addWire);
  if (seed.ends) {
    pins.add(seed.ends.from);
    pins.add(seed.ends.to);
  }
  const hops = new Map();
  const side = (starts, dir) => {
    const w = walk(starts, dir, r, () => true, through);
    cycles += w.cycles;
    for (const [address, reached] of w.pins) {
      pins.add(address);
      if (!(hops.get(address) <= reached.hops)) hops.set(address, reached.hops);
    }
    w.wires.forEach(addWire);
    const endpoints = w.endpoints.map((a) => endpointOf(a, w.pins.get(a), r, graph, dir, through));
    for (const e of endpoints) if (e.kind === 'library') opaque.add(e.address);
    return { endpoints, pins: w.pins.size, wires: w.wires.length };
  };
  // A wire's own ends count even when the direction leaves one side out.
  const up = wantUp ? side(ups, 'up') : null;
  const down = wantDown ? side(downs, 'down') : null;
  // A composite instance's inside is part of it (2026-10-20, the owner's "tracing
  // must be able to go through the composite"): its inputs followed inward
  // downstream, its outputs inward upstream, within it only, so the trace
  // continues past its frame; its ends stay the outside ones.
  if (start.kind === 'instance' && r.instance(start.address)?.composite && r.level(start.address)) {
    const keep = (path) => within(path, start.address);
    const inner = (starts, dir) => {
      const w = walk(starts, dir, r, keep, through);
      cycles += w.cycles;
      for (const [address, reached] of w.pins) {
        pins.add(address);
        if (!(hops.get(address) <= reached.hops)) hops.set(address, reached.hops);
      }
      w.wires.forEach(addWire);
    };
    if (wantDown) inner(ups, 'down');
    if (wantUp) inner(downs, 'up');
  }
  if (start.kind !== 'wire') {
    // The start's pins are part of the trace whatever the direction.
    for (const a of [...ups, ...downs]) if (a) pins.add(a);
  }
  const instances = new Set();
  for (const address of pins) {
    const at = splitAddress(address);
    if (!at) continue;
    instances.add(at.instance);
    levels.add(parentPath(at.instance));
  }
  const nesting = (p) => (p ? p.split('/').length - 1 : 0);
  return {
    start,
    direction,
    depth: through ? 'through' : 'next',
    hops,
    label,
    pins,
    instances,
    wires,
    levels: [...levels].sort((a, b) => nesting(a) - nesting(b) || (a < b ? -1 : a > b ? 1 : 0)),
    up,
    down,
    cycles,
    opaque: [...opaque],
  };
}

/** Whether `result` (a trace) takes in wire `written` of the level at `path`. */
export function tracedWire(result, path, written) {
  return Boolean(result?.wires.get(path)?.has(wireKey(written)));
}

/**
 * What lies behind composite pin `address` on side `side` (`inside`: in
 * its composite instance; `outside`: around it): `{direction, endpoints,
 * opaque}` (`direction` `down` for an input reached from outside or an
 * output leaving it, else `up`), the trace walked on that side only; null
 * for a pin that is not a composite's, or a library composite's inside.
 */
export function behind(graph, address, side, r = resolver(graph)) {
  const info = pinInfo(graph, address, r);
  if (!info?.composite) return null;
  if (side === 'inside' && info.opaque) return { direction: info.direction === 'out' ? 'up' : 'down', endpoints: [], opaque: true };
  const incoming = info.direction !== 'out';
  // An input leads inward downstream and comes from outside upstream; an output the other way round.
  const direction = side === 'inside' ? (incoming ? 'down' : 'up') : incoming ? 'up' : 'down';
  const keep = side === 'inside' ? (path) => within(path, info.instance) : (path) => !within(path, info.instance);
  const w = walk([address], direction, r, keep);
  return { direction, endpoints: w.endpoints.map((a) => endpointOf(a, w.pins.get(a), r, graph, direction)), opaque: false };
}

/**
 * The pins one connection away from composite pin `address` on side `side`
 * (`inside`: the inner ports its export forwards to or from; `outside`: the
 * ends of the connections at its box's level, a parent frame's pin
 * included): `{direction, addresses, opaque}`, in the graph's order; null
 * for a pin that is not a composite's. A boundary's note (M2.13-2,
 * ui/lib/schematic-labels.js), where `behind` walks on to the endpoints.
 */
export function neighbours(graph, address, side, r = resolver(graph)) {
  const info = pinInfo(graph, address, r);
  if (!info?.composite) return null;
  const incoming = info.direction !== 'out';
  const direction = side === 'inside' ? (incoming ? 'down' : 'up') : incoming ? 'up' : 'down';
  const path = side === 'inside' ? info.instance : parentPath(info.instance);
  const addresses = edgesFrom(address, direction, r)
    .filter((e) => e.path === path)
    .map((e) => e.next);
  return { direction, addresses, opaque: side === 'inside' && info.opaque };
}

/** The most names a badge lists. */
export const BADGE_NAMES = 3;

/** An endpoint's name relative to `base` (an instance path): `osc1.freq` inside `/synth`, else the full address. */
function relative(address, base) {
  return base && address.startsWith(`${base}/`) ? address.slice(base.length + 1) : address;
}

/**
 * A badge's text for `behind`'s answer of pin `address` on `side`:
 * "→ 3 inside: osc1.freq, osc2.freq, osc3.freq", "→ inside: vca.b" (one),
 * "← from /melody.hz",
 * "→ to /echo.in", "← inside from volume.out"; null without an answer.
 */
export function badgeText(answer, address, side) {
  if (!answer) return null;
  const base = splitAddress(address)?.instance ?? '';
  const arrow = answer.direction === 'down' ? '→' : '←';
  if (answer.opaque) return `${arrow} inside a library composite (not in the graph)`;
  const names = answer.endpoints.map((e) => relative(e.address, side === 'inside' ? base : '') + (e.kind === 'library' ? ' (library)' : e.kind === 'unconnected' ? ' (open end)' : ''));
  const n = names.length;
  const list = names.slice(0, BADGE_NAMES).join(', ') + (n > BADGE_NAMES ? `, … ${n - BADGE_NAMES} more` : '');
  if (side === 'inside') {
    if (n === 0) return `${arrow} nothing inside`;
    if (answer.direction === 'up') return `← inside from ${list}`;
    return n === 1 ? `→ inside: ${list}` : `→ ${n} inside: ${list}`;
  }
  if (n === 0) return answer.direction === 'down' ? '→ nothing outside' : '← nothing outside';
  return answer.direction === 'down' ? `→ to ${list}` : `← from ${list}`;
}

/** A trace spanning more pins than this shows its levels collapsed, but the start's. */
export const COLLAPSE_PINS = 40;

/**
 * The trace as a tree by level for the sidebar: `[{path, label, wires,
 * pins, collapsed, children: [{address, name, module, composite, opaque,
 * children: [{address, port, direction, role}]}]}]`, the levels outermost
 * first, instances and pins in the graph's order; through modules, in the
 * chain's order instead (by their fewest hops from the start, then the
 * graph's). `role` is `start`, `endpoint` (of either direction), `open` (an
 * endpoint that is an open end: leads nowhere) or `through`. `label` names the level ("top level", "/synth (moog_voice)").
 * A trace of more than COLLAPSE_PINS pins has every level `collapsed` but
 * the start's.
 */
export function traceTree(graph, result) {
  if (!result) return [];
  const r = resolver(graph);
  const all = [...(result.up?.endpoints ?? []), ...(result.down?.endpoints ?? [])];
  const ends = new Set(all.map((e) => e.address));
  const open = new Set(all.filter((e) => e.kind === 'unconnected').map((e) => e.address));
  const starts = new Set();
  if (result.start.kind === 'pin') starts.add(result.start.address);
  if (result.start.kind === 'instance') for (const a of result.pins) if (splitAddress(a)?.instance === result.start.address) starts.add(a);
  if (result.start.kind === 'pins') for (const a of result.start.addresses ?? []) starts.add(a);
  const byLevel = new Map(result.levels.map((p) => [p, new Map()]));
  for (const address of result.pins) {
    const at = splitAddress(address);
    if (!at) continue;
    const path = parentPath(at.instance);
    if (!byLevel.has(path)) byLevel.set(path, new Map());
    const level = byLevel.get(path);
    if (!level.has(at.instance)) level.set(at.instance, []);
    level.get(at.instance).push(at.port);
  }
  const tree = [];
  const chain = result.depth === 'through';
  const hop = (address) => result.hops?.get(address) ?? Infinity;
  const startAddress = result.start.kind === 'pins' ? result.start.addresses?.[0] : result.start.address;
  const startPath = result.start.kind === 'wire' ? result.start.path : startAddress ? parentPath(splitAddress(startAddress)?.instance ?? startAddress) : '';
  const collapse = result.pins.size > COLLAPSE_PINS;
  for (const path of result.levels) {
    const data = r.level(path);
    const node = path ? r.instance(path) : null;
    const instances = byLevel.get(path) ?? new Map();
    const order = new Map((data?.instances ?? []).map((n, i) => [`${path}/${n.name}`, i]));
    const first = (ports, instance) => Math.min(...ports.map((p) => hop(`${instance}.${p}`)));
    const children = [...instances.entries()]
      .sort(([a, pa], [b, pb]) => (chain ? first(pa, a) - first(pb, b) : 0) || (order.get(a) ?? 1e9) - (order.get(b) ?? 1e9) || (a < b ? -1 : 1))
      .map(([instance, ports]) => {
        const entry = r.instance(instance);
        const portOrder = [...(entry?.ports ?? []).map((p) => p.name), ...(entry?.parameters ?? []).map((p) => p.name)];
        return {
          address: instance,
          name: instance.slice(instance.lastIndexOf('/') + 1),
          module: entry?.module ?? null,
          composite: Boolean(entry?.composite),
          opaque: Boolean(entry?.composite) && !r.level(instance),
          children: ports
            .sort((a, b) => (chain ? hop(`${instance}.${a}`) - hop(`${instance}.${b}`) : 0) || portOrder.indexOf(a) - portOrder.indexOf(b))
            .map((port) => {
              const address = `${instance}.${port}`;
              const info = pinInfo(graph, address, r);
              return { address, port, direction: info?.direction ?? null, parameter: Boolean(info?.parameter), role: starts.has(address) ? 'start' : open.has(address) ? 'open' : ends.has(address) ? 'endpoint' : 'through' };
            }),
        };
      });
    tree.push({
      path,
      label: path ? `${path} (${node?.module ?? '?'})` : 'top level',
      wires: result.wires.get(path)?.size ?? 0,
      pins: children.reduce((n, c) => n + c.children.length, 0),
      collapsed: collapse && path !== startPath,
      children,
    });
  }
  return tree;
}
