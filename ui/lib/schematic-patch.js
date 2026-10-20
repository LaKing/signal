// Patching from the schematic (M2.9-3, ADR 0026 §6, docs/ui-controls.md
// "Editing the patch"): the `patch` ops as the page builds them, the
// default name of an added instance, the targets a dragged output may be
// connected to, how many instances a composite level's edit reaches, and
// the add palette's entries from `modules`. The server writes the text and
// is the judge (docs/ui-protocol.md "patch"); nothing here edits a file.
// No DOM.

import { shapeName } from './schematic-layout.js';
import { levelRect } from './schematic-scene.js';
import { groupOf } from './schematic-palette.js';

/** An instance name: `[a-z][a-z0-9_]*`. */
export const NAME = /^[a-z][a-z0-9_]*$/;

/** An endpoint as a patch writes it: `instance.port`, without the top level's `/`. */
export function written(address) {
  return String(address ?? '').replace(/^\//, '');
}

export function addInstance(name, module, { x, y } = {}, params = null) {
  const op = { op: 'instance.add', name, module };
  if (params && Object.keys(params).length) op.params = params;
  if (Number.isFinite(x) && Number.isFinite(y)) Object.assign(op, { x: Math.round(x), y: Math.round(y) });
  return op;
}

export function removeInstance(name) {
  return { op: 'instance.remove', name };
}

export function addConnection(from, to) {
  return { op: 'connection.add', from: written(from), to: written(to) };
}

export function removeConnection(from, to) {
  return { op: 'connection.remove', from: written(from), to: written(to) };
}

export function scaleConnection(from, to, scale) {
  return { op: 'connection.scale', from: written(from), to: written(to), scale };
}

/** The `patch` message of `ops` at `level` (`""` the top level), against `base` (the file's SHA-256). */
export function patchMessage(level, base, ops) {
  return { type: 'patch', level, base, ops };
}

/** The instance names of a level as `graph` gives it. */
export function namesOf(levelGraph) {
  return new Set((levelGraph?.instances ?? []).map((i) => i.name));
}

/**
 * An added instance's default name: the type's last segment (`std.svf` →
 * `svf`, a composite's own name), made a valid name, with the lowest free
 * `_n` suffix (`svf`, `svf_2`, `svf_3`).
 */
export function defaultName(module, taken) {
  let base = String(module ?? '').split('.').pop().toLowerCase().replace(/[^a-z0-9_]/g, '_');
  if (!/^[a-z]/.test(base)) base = `m_${base}`;
  if (!taken.has(base)) return base;
  for (let n = 2; ; n++) if (!taken.has(`${base}_${n}`)) return `${base}_${n}`;
}

/** Why `name` cannot be added at a level with names `taken`, or null. */
export function nameError(name, taken) {
  if (!NAME.test(name)) return 'A name is a lower-case letter, then lower-case letters, digits and _';
  if (taken.has(name)) return `The level already has an instance ${name}`;
  return null;
}

/**
 * Where output `source` (`{instance, port, shape}`) of a level may be
 * connected: the inputs of the same shape (signal, events, vector) and, for
 * a signal, the parameters, by direction and shape as `graph` gives them;
 * never a connection the level already has. `[{instance, port, parameter}]`.
 * The server's compile is the judge of the rest (rates, a cycle).
 */
export function connectTargets(levelGraph, source) {
  // `graph`'s shape (`{vector: n}`), or the layout's pin shape (already a name).
  const shape = ['signal', 'events', 'vector'].includes(source.shape) ? source.shape : shapeName(source.shape);
  const from = `${source.instance}.${source.port}`;
  const existing = new Set((levelGraph?.connections ?? []).filter((c) => written(c.from) === from).map((c) => written(c.to)));
  const targets = [];
  for (const instance of levelGraph?.instances ?? []) {
    for (const port of instance.ports ?? []) {
      if (port.direction === 'in' && shapeName(port.shape) === shape && !existing.has(`${instance.name}.${port.name}`)) {
        targets.push({ instance: instance.name, port: port.name, parameter: false });
      }
    }
    if (shape !== 'signal') continue;
    for (const parameter of instance.parameters ?? []) {
      if (!existing.has(`${instance.name}.${parameter.name}`)) targets.push({ instance: instance.name, port: parameter.name, parameter: true });
    }
  }
  return targets;
}

/** How many instances of composite `level` the project has, through the nesting (1 for the top level). */
export function instanceCount(graph, level) {
  if (!level) return 1;
  const levels = [['', graph?.top], ...Object.entries(graph?.composites ?? {})];
  const memo = new Map();
  const count = (name, depth) => {
    if (memo.has(name)) return memo.get(name);
    if (depth > levels.length) return 0;
    let n = 0;
    for (const [parent, body] of levels) {
      const uses = (body?.instances ?? []).filter((i) => i.composite && i.module === name).length;
      if (uses) n += uses * (parent === '' ? 1 : count(parent, depth + 1));
    }
    memo.set(name, n);
    return n;
  };
  return count(level, 0);
}

/**
 * The level a double-click at screen `point` on `hit` (in `drawn`, a scene)
 * adds to in edit mode, as a scene level (`{path, level, transform, …}`),
 * or null: on the background, the innermost level there; inside an open
 * composite (which hit-tests as its box), that composite's level. Only the
 * innermost open level is added to (M2.13-3): elsewhere, null.
 */
export function paletteLevel(hit, drawn, point) {
  let level = null;
  if (hit?.kind === 'background') level = levelAt(drawn, point);
  else if (hit?.kind === 'instance' && hit.node?.composite && drawn?.open?.has(hit.address)) {
    level = drawn.levels.find((l) => l.path === hit.address) ?? null;
  }
  return level && level === drawn.levels.at(-1) ? level : null;
}

/** The innermost level drawn under screen `point` (an open composite's inside, else the top level). */
export function levelAt(drawn, point) {
  for (let i = drawn.levels.length - 1; i >= 0; i--) {
    const level = drawn.levels[i];
    const r = level.path === '' ? null : levelRect(drawn, level.path);
    if (level.path === '' || (r && point.x >= r.x && point.x <= r.x + r.w && point.y >= r.y && point.y <= r.y + r.h)) return level;
  }
  return drawn.levels[0];
}

/**
 * The add palette's entries from a `modules` reply, as `searchPalette` and
 * `paletteRows` (ui/lib/schematic-palette.js) search them, each with its
 * palette `group` (`groupOf`) and `description`.
 */
export function paletteEntries(modules) {
  const entry = (m) => ({
    address: m.module,
    label: m.description ?? (m.composite ? 'composite' : ''),
    module: m.module,
    composite: Boolean(m.composite),
    group: groupOf(m),
    description: m.description ?? null,
  });
  return [...(modules?.primitives ?? []).map(entry), ...(modules?.composites ?? []).map(entry)];
}
