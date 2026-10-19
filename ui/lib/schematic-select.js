// The schematic's selection of several parts (the owner's, 2026-10-17: "it
// should be possible to select, besides wires, connection points and
// complete elements, and grouped groups too"; docs/ui-controls.md
// "Schematic"): a list of hits (`hitTest`'s instances, pins and wires),
// each at most once, the last the one added last. Shift- or ⌘-click
// toggles one (`toggled`), a Shift-drag on the background takes the boxes
// inside a rectangle and the wires between them (`inside`), ⌘A the level
// shown (`allOf`); a `graph` push finds them again (`reselectAll`); Delete
// removes them, one `patch` per level (`deletePlan`); a group's move is one
// `schematic.move` per part that fits a message (`moveMessages`). No DOM.

/** The largest message a client may send (docs/ui-protocol.md "Limits"): a larger one closes the connection. */
export const MESSAGE_LIMIT = 64 * 1024;
/** Room kept for what the client adds to a message (its `id`). */
const ROOM = 64;
/** A file's SHA-256 as `patch` carries it (`base`): its length is all that counts here. */
const BASE = '0'.repeat(64);

const encoder = new TextEncoder();

/** The bytes `message` takes on the socket, with room for its `id`. */
export function messageBytes(message) {
  return encoder.encode(JSON.stringify(message)).length + ROOM;
}

import { instanceHit, wireHit } from './schematic-hit.js';
import { patchMessage, removeConnection, removeInstance, written } from './schematic-patch.js';
import { screenRect } from './schematic-view.js';

/** What tells two hits of the same part apart from others: its kind and address (a wire: its level's path and written ends). */
export function hitKey(hit) {
  if (!hit || hit.kind === 'background') return '';
  if (hit.kind === 'wire') return `wire ${hit.path} ${hit.wire.written.from} ${hit.wire.written.to}`;
  if (hit.kind === 'port') return `port ${hit.address} ${hit.port.direction}`;
  return `instance ${hit.address}`;
}

/** Whether `hit` is in `selected`. */
export function has(selected, hit) {
  const key = hitKey(hit);
  return selected.some((s) => hitKey(s) === key);
}

/** `selected` without duplicates (the first of each kept) and without the background. */
export function unique(selected) {
  const seen = new Set();
  return selected.filter((s) => {
    const key = hitKey(s);
    if (!key || seen.has(key)) return false;
    seen.add(key);
    return true;
  });
}

/** `selected` with `hit` added (last), or taken out when it was there (a Shift- or ⌘-click). The background changes nothing. */
export function toggled(selected, hit) {
  const key = hitKey(hit);
  if (!key) return selected;
  return selected.some((s) => hitKey(s) === key) ? selected.filter((s) => hitKey(s) !== key) : [...selected, hit];
}

/** `a` and `b` together, `a`'s first, each part once. */
export function union(a, b) {
  return unique([...a, ...b]);
}

/** Whether instance address `address` is inside instance `outer` (its composite's inside, at any depth). */
function under(address, outer) {
  return address.startsWith(`${outer}/`);
}

/**
 * The marquee's take: in `drawn` (a scene), the boxes wholly inside screen
 * rectangle `rect` (`{x, y, w, h}`, any sign of `w`, `h`), every level
 * drawn, outer levels first; a box inside one taken (an open composite's
 * inside) is not taken again, so that a Delete never reaches into what it
 * deletes anyway. Then the connections of each level whose both ends are
 * boxes taken. Exports (a frame's wires) are not taken.
 */
export function inside(drawn, rect) {
  const x0 = Math.min(rect.x, rect.x + rect.w);
  const y0 = Math.min(rect.y, rect.y + rect.h);
  const x1 = Math.max(rect.x, rect.x + rect.w);
  const y1 = Math.max(rect.y, rect.y + rect.h);
  const boxes = [];
  const wires = [];
  for (const entry of drawn?.levels ?? []) {
    const taken = new Set();
    for (const node of entry.nodes) {
      if (node.kind !== 'instance') continue;
      const r = screenRect(entry.transform, node);
      if (r.x < x0 || r.y < y0 || r.x + r.w > x1 || r.y + r.h > y1) continue;
      const hit = instanceHit(entry, node);
      if (boxes.some((b) => under(hit.address, b.address))) continue;
      boxes.push(hit);
      taken.add(node.id);
    }
    for (const wire of entry.layout.wires) {
      if (wire.kind === 'connection' && taken.has(wire.from.node) && taken.has(wire.to.node)) wires.push(wireHit(entry, wire));
    }
  }
  return [...boxes, ...wires];
}

/** Every instance and connection of level `entry` (a scene level, `{path, level, layout}`): ⌘A on the level shown. */
export function allOf(entry) {
  if (!entry) return [];
  const boxes = entry.layout.nodes.filter((n) => n.kind === 'instance').map((n) => instanceHit(entry, n));
  const wires = entry.layout.wires.filter((w) => w.kind === 'connection').map((w) => wireHit(entry, w));
  return [...boxes, ...wires];
}

/**
 * `selected` found again in the levels as now laid out (`layoutOf(level)`,
 * after a new `graph`), as `reselect` finds one part (its whole path from
 * the top level, each ancestor still a composite leading to its level),
 * with every level indexed once, so thousands of parts cost one pass:
 * what is no longer there is let go.
 */
export function reselectAll(selected, layoutOf) {
  const indexes = new Map();
  const index = (level) => {
    if (!indexes.has(level)) {
      const drawn = layoutOf(level);
      indexes.set(
        level,
        drawn && {
          nodes: new Map(drawn.nodes.filter((n) => n.kind === 'instance').map((n) => [n.name, n])),
          frames: new Map(drawn.nodes.filter((n) => n.kind !== 'instance').map((n) => [n.id, n])),
          wires: new Map(drawn.wires.map((w) => [`${w.written.from}\n${w.written.to}`, w])),
        },
      );
    }
    return indexes.get(level);
  };
  // A path's level now (`""` the top level), or null when an ancestor went or is no longer a composite.
  const levels = new Map([['', '']]);
  const levelOfPath = (path) => {
    if (levels.has(path)) return levels.get(path);
    const cut = path.lastIndexOf('/');
    const parent = levelOfPath(path.slice(0, cut));
    const ancestor = parent === null ? null : index(parent)?.nodes.get(path.slice(cut + 1));
    const level = ancestor?.composite ? ancestor.module : null;
    levels.set(path, level);
    return level;
  };
  const found = [];
  for (const s of selected) {
    if (!s || s.kind === 'background') continue;
    const level = levelOfPath(String(s.path ?? ''));
    if (level === null || level !== s.level) continue;
    const drawn = index(level);
    if (!drawn) continue;
    if (s.kind === 'wire') {
      const wire = drawn.wires.get(`${s.wire.written.from}\n${s.wire.written.to}`);
      if (wire) found.push({ ...s, wire });
      continue;
    }
    const node = s.node.kind === 'instance' ? drawn.nodes.get(s.node.name) : drawn.frames.get(s.node.id);
    if (!node) continue;
    if (s.kind === 'instance') found.push({ ...s, node });
    else {
      const port = [...node.ins, ...node.outs].find((p) => p.name === s.port.name && p.direction === s.port.direction);
      if (port) found.push({ ...s, node, port });
    }
  }
  return unique(found);
}

/** The instances of `selected`, as the names of their level: level → Set of names. */
function instancesByLevel(selected) {
  const levels = new Map();
  for (const s of selected) {
    if (s.kind !== 'instance' || s.node?.kind !== 'instance') continue;
    if (!levels.has(s.level)) levels.set(s.level, new Set());
    levels.get(s.level).add(s.node.name);
  }
  return levels;
}

/** The instance a written endpoint (`/a.out`, `osc.out`) belongs to. */
function endInstance(end) {
  const plain = written(end);
  const dot = plain.lastIndexOf('.');
  return dot < 0 ? plain : plain.slice(0, dot);
}

/** Whether `hit` lies inside one of the instances at `addresses` (a Set): an open composite's inside. */
function insideOf(hit, addresses) {
  const path = String(hit.path ?? '');
  for (let at = path; at; at = at.slice(0, at.lastIndexOf('/'))) if (addresses.has(at)) return true;
  return false;
}

/**
 * What Delete does to `selected` (Edit patch): `{patches: [{level, ops,
 * bytes}], skipped, deleted, tooLarge}`. One patch per level (`""` the top
 * level, else a composite's file), in the order the levels first appear
 * in the selection, so one undo restores each level: an `instance.remove`
 * per instance (an instance of a composite selected in two places of the
 * drawing, the same file's, once) and a `connection.remove` per connection
 * that no removed instance takes with it. A part inside a removed
 * instance (its open inside) is left out: it goes with it, and its file is
 * every other instance's too. `skipped` counts what Delete leaves: pins (a
 * pin's Disconnect all removes its wires) and exports (not edited here);
 * `deleted` how many ops all patches hold. A patch is one message, and one
 * undo: one larger than `limit` (`bytes` with its `base` and `id`) is
 * listed in `tooLarge`, and nothing should be written.
 */
export function deletePlan(selected, limit = MESSAGE_LIMIT) {
  const gone = new Set(selected.filter((s) => s.kind === 'instance' && s.node?.kind === 'instance').map((s) => s.address));
  const kept = selected.filter((s) => !insideOf(s, gone));
  const removed = instancesByLevel(kept);
  const patches = new Map();
  const seen = new Set();
  let skipped = 0;
  const add = (level, key, op) => {
    if (seen.has(`${level}\n${key}`)) return;
    seen.add(`${level}\n${key}`);
    if (!patches.has(level)) patches.set(level, []);
    patches.get(level).push(op);
  };
  for (const s of kept) {
    if (s.kind === 'instance' && s.node?.kind === 'instance') add(s.level, `i ${s.node.name}`, removeInstance(s.node.name));
    else if (s.kind === 'wire' && s.wire.kind === 'connection') {
      const { from, to } = s.wire.written;
      const names = removed.get(s.level);
      if (names?.has(endInstance(from)) || names?.has(endInstance(to))) continue;
      add(s.level, `c ${written(from)} ${written(to)}`, removeConnection(from, to));
    } else skipped++;
  }
  const list = [...patches].map(([level, ops]) => ({ level, ops, bytes: messageBytes(patchMessage(level, BASE, ops)) }));
  return {
    patches: list,
    skipped,
    deleted: list.reduce((n, p) => n + p.ops.length, 0),
    tooLarge: list.filter((p) => p.bytes > limit),
  };
}

/**
 * The `schematic.move` messages that write `moves` (`[{instance, x, y}]`)
 * of `level`: one alone as the plain form; several in the `moves` form,
 * split in order into as few messages as each stays within `limit` bytes
 * (docs/ui-protocol.md "schematic.move"). Positions are presentation only:
 * each message is one write and one `graph` push of its own.
 */
export function moveMessages(level, moves, limit = MESSAGE_LIMIT) {
  if (moves.length === 0) return [];
  if (moves.length === 1) return [{ type: 'schematic.move', level, ...moves[0] }];
  const empty = messageBytes({ type: 'schematic.move', level, moves: [] });
  const messages = [];
  let batch = [];
  let bytes = empty;
  for (const move of moves) {
    const size = encoder.encode(JSON.stringify(move)).length + 1;
    if (batch.length && bytes + size > limit) {
      messages.push(batch);
      batch = [];
      bytes = empty;
    }
    batch.push(move);
    bytes += size;
  }
  messages.push(batch);
  return messages.map((part) => (part.length === 1 ? { type: 'schematic.move', level, ...part[0] } : { type: 'schematic.move', level, moves: part }));
}

/**
 * The boxes a drag of instance `hit` moves: with `hit` in `selected`, every
 * selected instance of the same drawn level (`path`), `hit`'s first; else
 * `hit`'s alone. Names, as `DragFollow` and `schematic.move` take them.
 */
export function dragGroup(selected, hit) {
  if (hit?.kind !== 'instance') return [];
  if (!has(selected, hit)) return [hit.node.name];
  const others = selected.filter((s) => s.kind === 'instance' && s.path === hit.path && s.node.name !== hit.node.name).map((s) => s.node.name);
  return [hit.node.name, ...new Set(others)];
}

/**
 * What T and the sidebar's Trace signal trace for `selected`: the part
 * selected last, but for several pins (a collapsed port group clicked, M2.13-2,
 * or pins Shift-clicked) all of them, as the context menu traces a group:
 * a `group` hit (`{kind: 'group', ports, address, label}`, `startOf`'s,
 * ui/lib/schematic-trace.js), its `address` the pins' box when they share
 * one and its `label` their group's (`osc1_*`) else their count. Null for
 * nothing selected.
 */
export function traceTarget(selected) {
  const list = (selected ?? []).filter((s) => s && s.kind !== 'background');
  if (list.length < 2 || !list.every((s) => s.kind === 'port')) return list.at(-1) ?? null;
  const box = (s) => (s.node.kind === 'instance' ? `${s.path}/${s.node.name}` : s.path);
  const address = list.every((s) => box(s) === box(list[0])) ? box(list[0]) : '';
  const group = list.every((s) => s.group && s.group === list[0].group) && address ? list[0].group : null;
  return { kind: 'group', path: list[0].path, level: list[0].level, ports: list, address, label: group ?? `${list.length} pins` };
}
