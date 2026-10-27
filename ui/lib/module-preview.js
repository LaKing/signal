// The Add module browser's previews (docs/ui-controls.md "Editing the
// patch"): what an instance of a type would look like before it is added,
// built in the page from its `modules` entry, with no instance, no audio and
// nothing sent. The panel preview is the generated panel's card of the
// instance (`defaultPanel` of a `describe` that has only it, ui/lib/panel.js),
// drawn by the real controls over a connection that sends nothing; the
// schematic preview is the instance's box as the schematic lays it out
// (ui/lib/schematic-layout.js) and, for a composite, its inside, drawn as the
// schematic draws a level (the same classes and wires). No DOM.

import { defaultPanel } from './panel.js';
import { layout } from './schematic-layout.js';
import { curveData } from './schematic-wire.js';

/** Space around a preview's drawing, in layout units. */
export const PREVIEW_MARGIN = 12;

/** The address an instance named `name` has at the top level. */
function at(name, part) {
  return `/${name}.${part}`;
}

/** A level holding one instance `name` of `entry`, as `graph` gives a level: its box alone. */
export function nodeLevel(entry, name) {
  return {
    instances: [
      {
        name,
        address: name,
        module: entry.module,
        composite: Boolean(entry.composite),
        ports: (entry.ports ?? []).map((p) => ({ ...p, address: `${name}.${p.name}` })),
        parameters: (entry.parameters ?? []).map((p) => ({ ...p, address: `${name}.${p.name}` })),
      },
    ],
    connections: [],
  };
}

/** A composite's inside as `graph` gives a composite's level (`modules`' `inside`), else null. */
export function insideLevel(entry) {
  const inside = entry?.inside;
  return inside && Array.isArray(inside.instances) ? inside : null;
}

/**
 * The `describe` a project holding only one top-level instance `name` of
 * `entry` would give, for the controls of the panel preview: the instance
 * with its ports and parameters at their addresses, and a control per
 * parameter (its range, default, value set, at its default), no sink.
 */
export function previewDescribe(entry, name) {
  const parameters = (entry.parameters ?? []).map((p) => ({ ...p, address: at(name, p.name), base: p.default ?? null }));
  const control = (p) => ({
    address: p.address,
    instance: name,
    parameter: p.name,
    module: entry.module,
    ports: [p.address],
    unit: p.unit ?? null,
    min: p.range?.min ?? null,
    max: p.range?.max ?? null,
    default: p.default ?? null,
    base: p.default ?? null,
    inFile: false,
    modulated: false,
    combine: p.combine ?? null,
    values: Array.isArray(p.values) ? p.values : null,
    snap: p.snap ?? null,
  });
  return {
    type: 'describe',
    planVersion: null,
    sampleRate: null,
    project: null,
    playing: false,
    instances: [
      {
        address: `/${name}`,
        module: entry.module,
        sink: null,
        ports: (entry.ports ?? []).map((p) => ({ ...p, address: at(name, p.name) })),
        parameters,
      },
    ],
    controls: parameters.map(control),
    sinks: [],
    meters: { output: [], input: [] },
    unsaved: [],
  };
}

/** The generated panel's structure for the preview: `defaultPanel` of `previewDescribe` (one card, no output card). */
export function previewPanel(entry, name) {
  return defaultPanel(previewDescribe(entry, name));
}

/**
 * The shapes a level's layout is drawn with, in layout units: `{viewBox,
 * shapes}`, `viewBox` `{x, y, w, h}` the layout's bounds with
 * `PREVIEW_MARGIN` around, each shape `{tag, attrs, text?, children?}` with
 * the schematic's classes (`wire`; a box's group `node`, holding `box`,
 * `frame`, `name`, `module`, `pin` and `pin-label`): the wires first, then
 * each box with its name and module, its pins (a parameter's pin as the
 * schematic marks it) and their names. `level` is as `graph` gives one; `positions` the stored ones;
 * `factsOf(module)` a composite's export facts, for its pins' groups and
 * order (`exportFacts`, as the schematic passes `layout`).
 */
export function previewShapes(level, positions = {}, factsOf = () => null) {
  const drawn = layout(level, positions, { factsOf });
  const shapes = [];
  for (const wire of drawn.wires) {
    shapes.push({ tag: 'path', attrs: { class: ['wire', wire.shape, wire.dashed ? 'dashed' : ''].filter(Boolean).join(' '), d: curveData(wire.curve) } });
  }
  for (const node of drawn.nodes) {
    const children = [];
    children.push({ tag: 'rect', attrs: { class: 'box', x: node.x, y: node.y, width: node.w, height: node.h, rx: 4 } });
    if (node.composite) children.push({ tag: 'rect', attrs: { class: 'frame', x: node.x + 3, y: node.y + 3, width: node.w - 6, height: node.h - 6, rx: 3 } });
    children.push({ tag: 'text', attrs: { class: 'name', x: node.x + 8, y: node.y + 14, 'font-size': 12 }, text: node.name });
    if (node.module) children.push({ tag: 'text', attrs: { class: 'module', x: node.x + 8, y: node.y + 27, 'font-size': 10 }, text: node.module });
    for (const pin of [...node.ins, ...node.outs]) {
      const out = pin.direction === 'out';
      const kind = ['pin', pin.shape, pin.parameter ? 'parameter' : '', pin.connected ? 'connected' : ''].filter(Boolean).join(' ');
      children.push({ tag: 'circle', attrs: { class: kind, cx: pin.x, cy: pin.y, r: 3 } });
      children.push({ tag: 'text', attrs: { class: 'pin-label', x: pin.x + (out ? -6 : 6), y: pin.y + 3, 'font-size': 9, 'text-anchor': out ? 'end' : 'start' }, text: pin.name });
    }
    // A group as the schematic draws a box: `.node`, `.pins` for a frame's pins, `.composite`.
    const group = ['node', node.kind === 'instance' ? '' : 'pins', node.composite ? 'composite' : ''].filter(Boolean).join(' ');
    shapes.push({ tag: 'g', attrs: { class: group }, children });
  }
  const b = drawn.bounds;
  return { viewBox: { x: b.x - PREVIEW_MARGIN, y: b.y - PREVIEW_MARGIN, w: b.w + 2 * PREVIEW_MARGIN, h: b.h + 2 * PREVIEW_MARGIN }, shapes };
}

/** The positions stored for a project composite's inside (`graph.layout.positions[module]`), else none. */
export function storedPositions(graph, entry) {
  if (!entry || entry.source !== 'project') return {};
  return graph?.layout?.positions?.[entry.module] ?? {};
}

/** The refusal of everything a preview control would send. */
function refused() {
  const error = new Error('a preview: nothing is sent until the module is added');
  error.code = 'preview';
  return Promise.reject(error);
}

/**
 * The connection of the panel preview's controls (the interface of
 * `Connection` in ui/signal-ui.js that controls use): it holds the
 * preview's `describe`, streams nothing, and refuses every edit without
 * sending it anywhere. `sent` counts what a control tried to send (none
 * should: the preview is inert).
 */
export class PreviewConnection {
  constructor(description) {
    this.description = description;
    this.status = 'open';
    this.unsaved = null;
    this.sent = 0;
  }

  on() {
    return () => {};
  }

  subscribe() {
    return () => {};
  }

  request() {
    this.sent++;
    return refused();
  }

  set() {
    return this.request();
  }

  ramp() {
    return this.request();
  }

  commit() {
    return this.request();
  }

  retry() {
    return this.request();
  }

  revert() {
    return this.request();
  }

  trigger() {
    return this.request();
  }

  note() {
    return this.request();
  }
}
