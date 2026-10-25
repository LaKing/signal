// The schematic's inspector card (M2.9-1, ADR 0026 §2): what a selected
// instance, port or connection shows; since 2026-10-17 the base of the
// sidebar's model (ui/lib/schematic-sidebar.js), which adds the rest. An instance's parameters follow the
// Parameters tab's rules (`ui/app/lib/parameters.js`): a top-level
// instance's (a composite's exported ones included) are controls, set with a
// 5 ms ramp while dragged and committed once on release, with the same
// unsaved marks; a parameter inside a composite is shown, never written
// (it is set in the composite's file). Base and effective values are the
// `parameters` stream's; `describe`'s are only shown until the stream has
// reported an address (another client may have changed it live since). No
// DOM.

import { rows } from '../app/lib/parameters.js';
import { unsavedOf } from './unsaved.js';
import { planPort } from './schematic-scene.js';

export { dragMessage, releaseMessage, resetMessage } from '../app/lib/parameters.js';

/** The effective-value stream's rate (frames per second), as the Parameters tab's. */
export const PARAMETER_RATE = 15;

/**
 * What the schematic says when the project stops loading (`graph.error`,
 * or a refused `graph`: `{code, message}`), over its last drawing if it
 * has one (`hasDrawing`), which it marks stale: `{title, text}`, the text
 * the diagnostics as given, one per line.
 */
export function staleNotice(error, hasDrawing) {
  const what = error?.code === 'no-project' ? 'No project is open' : 'The project does not load';
  return { title: hasDrawing ? `${what}; the drawing is stale` : what, text: String(error?.message ?? '') };
}

function shapeText(shape) {
  if (shape && typeof shape === 'object' && 'vector' in shape) return `vector of ${shape.vector}`;
  return shape ?? '—';
}

/** The `describe` base of plan parameter port `address`, or NaN. */
function planBase(description, address) {
  const dot = address.lastIndexOf('.');
  const instance = (description?.instances ?? []).find((i) => i.address === address.slice(0, dot));
  const parameter = (instance?.parameters ?? []).find((p) => p.name === address.slice(dot + 1));
  return parameter?.base ?? NaN;
}

/**
 * The card of `selection` (a hit: `{kind, path, node, port, wire, address,
 * from, to}`), from `graph` and `description`: `{kind, title, subtitle,
 * fields: [[label, text]], parameters: [row]}`. A row is `{name, address
 * (what set/ramp/commit take), stream (what the parameters stream takes),
 * unit, min, max, default, taper and kind (a control's, for a typed value),
 * editable, fallback}`.
 */
export function card(selection, { graph, description } = {}) {
  if (!selection || selection.kind === 'background') return null;
  if (selection.kind === 'port') {
    const port = selection.port;
    return {
      kind: 'port',
      title: selection.address,
      subtitle: port.parameter ? 'parameter' : `${port.direction === 'out' ? 'output' : 'input'} port`,
      fields: [
        ['address', selection.address],
        ['shape', shapeText(port.shape)],
        ['kind', port.kind ?? '—'],
        ['rate', port.rate == null ? '—' : `divisor ${port.rate}`],
      ],
      parameters: [],
    };
  }
  if (selection.kind === 'wire') {
    const wire = selection.wire;
    const converters = wire.converters ?? [];
    return {
      kind: 'wire',
      title: `${selection.from} → ${selection.to}`,
      subtitle: wire.kind === 'export' ? 'export' : 'connection',
      fields: [
        ['from', selection.from],
        ['to', selection.to],
        ['scale', wire.scale == null ? '1' : String(wire.scale)],
        ['converter', converters.length ? converters.map(converterText).join('; ') : 'none'],
      ],
      parameters: [],
    };
  }
  const node = selection.node;
  const address = selection.address;
  const top = selection.path === '';
  let parameters;
  if (top) {
    const row = rows(description).find((r) => r.name === node.name);
    parameters = (row?.parameters ?? []).map((p) => ({
      name: p.name,
      address: p.address,
      stream: p.address,
      unit: p.unit,
      min: p.min,
      max: p.max,
      default: p.default,
      taper: p.taper,
      kind: p.kind,
      values: p.values ?? null,
      editable: true,
      control: true,
      fallback: p.base,
    }));
  } else {
    parameters = [];
    for (const p of node.parameters ?? node.ins.filter((i) => i.parameter)) {
      const shown = `${address}.${p.name}`;
      const stream = node.composite ? planPort(graph, shown, { parameter: true }) : shown;
      if (!stream) continue;
      const range = p.range ?? {};
      parameters.push({
        name: p.name,
        address: shown,
        stream,
        unit: p.unit ?? null,
        min: range.min ?? 0,
        max: range.max ?? 1,
        default: p.default ?? null,
        values: p.values ?? null,
        editable: false,
        control: false,
        fallback: planBase(description, stream),
      });
    }
  }
  return {
    kind: 'instance',
    title: address,
    subtitle: `${node.module}${node.composite ? ' (composite)' : ''}`,
    fields: [],
    parameters,
    editable: top,
  };
}

/** A converter as the card and the wire's hover label name it. */
export function converterText(c) {
  return `${c.module ?? c.name} (${c.conversion}, divisor ${c.fromRate} → ${c.toRate})`;
}

/** The parameters stream a card needs: its rows' stream addresses, or null. */
export function cardStream(model) {
  const addresses = [...new Set((model?.parameters ?? []).map((p) => p.stream))];
  return addresses.length ? { stream: 'parameters', addresses, rate: PARAMETER_RATE } : null;
}

/** A row's base in `description` (the latest `describe`): its control's, else its plan port's; NaN if none. */
export function describedBase(row, description) {
  if (row.control) {
    const control = (description?.controls ?? []).find((c) => c.address === row.address);
    return control?.base ?? NaN;
  }
  return planBase(description, row.stream);
}

/**
 * A row's values now: `base` and `effective` from the latest stream frame
 * for its stream address (`values`: address → `{base, effective}`, NaN
 * until reported); until the stream gave a finite base, the base of the
 * latest `describe` (`description`), else the one the card was built with
 * (`fallback`); the effective value is unknown (NaN) without a frame.
 * `unsaved` is the host's entry for the row's address, as the Parameters
 * tab marks it.
 */
export function rowValues(row, values, unsavedState, description = null) {
  const reported = values?.get(row.stream);
  const described = description ? describedBase(row, description) : NaN;
  const fallback = Number.isFinite(described) ? described : row.fallback;
  const base = Number.isFinite(reported?.base) ? reported.base : fallback;
  const effective = Number.isFinite(reported?.effective) ? reported.effective : NaN;
  return { base, effective, unsaved: unsavedOf(unsavedState, row.address) };
}
