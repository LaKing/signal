// A slot's face in the schematic (2026-10-30; the owner, after ADR 0046
// phase 1: "grouping works but it's clumsy, and the many knobs laid out
// vertically make huge columns. Could a slot have a panel header with the
// same layout as on the panel itself?"). A slot box whose slot has a part
// (`ui/parts/<key>.html`) shows that part inside it, under its title row,
// rendered by the panel's own code with the panel's CSS and skins
// (ui/controls/schematic-faces.js): the same cards, rows, groups and
// controls, live and operable. Its pins stay on the box's edges, so a wire
// into a parameter a face control binds still ends at the box. A slot's
// inside opened in place shows the face at the frame's top, the inner
// structure below it. The geometry here, the address and URL rules, and
// what a hover or a selection lights in a face. No DOM.

// The layout's units (a cycle with it: used only when called).
import { CHAR, GAP_Y, HEADER, PAD, ROW } from './schematic-layout.js';
import { rootedAttribute } from './parts.js';

/** A face's width in layout units, and in CSS px before the view scales it: the panel's cards are laid out at this width. */
export const FACE_W = 400;
/** A pin label beside a face is cut to this many characters (its column's width). */
export const FACE_LABEL_CHARS = 14;
/** Under this width on screen (px) a face is not shown: its area alone is drawn. */
export const FACE_MIN_PX = 60;
/** Measured heights are rounded up to this step (layout units), so a pixel's change lays nothing out again. */
export const FACE_STEP = 8;

/** A face's size before it is measured: its controls in rows of knobs (`count`, the part's controls). */
export function estimateFace(count) {
  const perRow = Math.max(1, Math.floor((FACE_W - 32) / 80));
  const rows = Math.max(1, Math.ceil((count ?? 0) / perRow));
  return { w: FACE_W, h: 48 + rows * 96 };
}

/** A measured face height (CSS px at FACE_W) as the layout takes it; null for nothing measured (0, hidden). */
export function measuredHeight(px) {
  if (!Number.isFinite(px) || px <= 0) return null;
  return Math.ceil(px / FACE_STEP) * FACE_STEP;
}

/**
 * The width a box's pin labels take on one side, beside its face: the
 * longest name, cut at FACE_LABEL_CHARS, and its padding.
 */
export function labelColumn(pins) {
  let longest = 0;
  for (const p of pins ?? []) longest = Math.max(longest, Math.min(FACE_LABEL_CHARS, String(p.name ?? '').length));
  return PAD + (longest ? Math.ceil(CHAR * longest) + PAD : 0);
}

/**
 * Node `node` (a slot box, before placement) given its face `face`
 * (`levelFaces`' `{key, file, error, count}`) of size `size` (`{w, h}`):
 * `node.face` its area from the box's corner (`dx`, `dy`, `w`, `h`), under
 * the title row between the pins' label columns; the box grows to hold it.
 * Its pins stay where they were, on its edges.
 */
export function placeFace(node, face, size) {
  const left = labelColumn(node.ins);
  const right = labelColumn(node.outs);
  const w = size?.w ?? FACE_W;
  const h = size?.h ?? estimateFace(face.count).h;
  node.face = { ...face, dx: left, dy: HEADER, w, h };
  node.w = Math.max(node.w, left + w + right);
  node.h = Math.max(node.h, HEADER + h + PAD + (node.slot ? ROW : 0));
  // Centred when the pins' names make the box wider than the face needs.
  node.face.dx = Math.max(left, Math.round((node.w - w) / 2));
  return node;
}

/**
 * A slot's inside's face (`face`, of `size`): its area above the inside's
 * nodes (their extent `extent`, `{x0, y0, x1, y1}`), centred on them, a
 * row's gap between: `{...face, x, y, w, h}` in the inside's units.
 */
export function frameFace(face, size, extent) {
  const w = size?.w ?? FACE_W;
  const h = size?.h ?? estimateFace(face.count).h;
  const has = Number.isFinite(extent?.x0) && Number.isFinite(extent?.x1);
  const x0 = has ? extent.x0 : 0;
  const x1 = has ? extent.x1 : w;
  const y0 = Number.isFinite(extent?.y0) ? extent.y0 : 0;
  return { ...face, x: Math.round((x0 + x1) / 2 - w / 2), y: y0 - GAP_Y - h, w, h };
}

/** Whether URL text `url` is relative to its page (not `/…`, not `scheme:`, not `//…`). */
function relative(url) {
  return !/^(?:[a-z][a-z0-9+.-]*:|\/)/i.test(url);
}

/**
 * A `skin` attribute (`svg:<url>`, `strip:<url>;…;2x=<url>`) of a part's
 * control with its relative URLs resolved against `base` (the project's
 * panel page, `/project/main.html`): in the panel a skin's URL is the
 * page's (ADR 0046 §2), and the schematic is another page. Text that is
 * no skin is kept as it is.
 */
export function faceSkin(value, base) {
  const text = String(value ?? '');
  const colon = text.indexOf(':');
  if (colon <= 0 || !base) return value;
  const kind = text.slice(0, colon).trim();
  if (kind !== 'svg' && kind !== 'strip') return value;
  const at = (url) => (url && relative(url) ? new URL(url, base).href : url);
  const [url, ...options] = text.slice(colon + 1).split(';');
  const rest = options.map((o) => {
    const eq = o.indexOf('=');
    return eq > 0 && o.slice(0, eq).trim() === '2x' ? `${o.slice(0, eq)}=${at(o.slice(eq + 1).trim())}` : o;
  });
  return [`${kind}:${at(url.trim())}`, ...rest].join(';');
}

/** Attribute `name`'s `value` of a face's control: its addresses rooted at `root` (the slot drawn), a skin's URLs from `base`. */
export function faceAttribute(name, value, root, base) {
  if (value === undefined || value === null) return value;
  if (name === 'skin') return faceSkin(value, base);
  return rootedAttribute(name, value, root);
}

/**
 * The style sheets of the project's panel page (`text`, `ui/main.html`'s
 * whole text) resolved against `base`, in order: what styles its cards
 * (the studio's `studio.css`), so a face looks as the part does on the
 * panel. Comments are skipped; only `<link rel="stylesheet" href>`.
 */
export function stylesheets(text, base) {
  const found = [];
  const plain = String(text ?? '').replace(/<!--[\s\S]*?(?:-->|$)/g, '');
  for (const m of plain.matchAll(/<link\b([^>]*)>/gi)) {
    const attributes = m[1];
    const rel = /\brel\s*=\s*(?:"([^"]*)"|'([^']*)'|([^\s>]+))/i.exec(attributes);
    if (!rel || !(rel[1] ?? rel[2] ?? rel[3]).toLowerCase().split(/\s+/).includes('stylesheet')) continue;
    const href = /\bhref\s*=\s*(?:"([^"]*)"|'([^']*)'|([^\s>]+))/i.exec(attributes);
    const url = href && (href[1] ?? href[2] ?? href[3]);
    if (!url) continue;
    try {
      const resolved = new URL(url, base).href;
      if (!found.includes(resolved)) found.push(resolved);
    } catch {
      // Not a URL: left out.
    }
  }
  return found;
}

/** `level`'s path and a written end (`synth.cutoff`, `/synth.cutoff`) as the address it names. */
function endAddress(path, end) {
  return `${path}/${String(end ?? '').replace(/^\//, '')}`;
}

/**
 * The addresses a face lights for `hits` (the wire or pin under the
 * pointer, the selection, a menu's target): a wire's ends (a connection's
 * both, an export's at the frame as the slot's own port), a pin's address
 * (a frame's pin the slot's port), a box's or control's bound addresses. A
 * face control binding any of them is lit, so a wire into a parameter
 * ending at the box shows which knob it moves.
 */
export function hotAddresses(hits) {
  const found = new Set();
  for (const hit of hits ?? []) {
    if (!hit) continue;
    if (hit.kind === 'wire') {
      const w = hit.wire?.written;
      if (!w) continue;
      if (hit.wire.kind === 'export') {
        // A frame's export: `from` (an input's, a parameter's) or `to` (an output's) is the slot's own port.
        const outputs = hit.wire.to?.node === '@outputs';
        found.add(outputs ? `${hit.path}.${w.to}` : `${hit.path}.${w.from}`);
        found.add(endAddress(hit.path, outputs ? w.from : w.to));
      } else {
        found.add(endAddress(hit.path, w.from));
        found.add(endAddress(hit.path, w.to));
      }
    } else if (hit.kind === 'port' && hit.address) found.add(hit.address);
    else if (hit.kind === 'control') for (const a of hit.addresses ?? []) found.add(a);
  }
  return found;
}

/** Whether a face control whose `bind` (rooted) is `bind` is lit by `hot` (`hotAddresses`'). */
export function lit(bind, hot) {
  if (!hot?.size) return false;
  return String(bind ?? '')
    .split(/[\s,]+/)
    .some((a) => a && hot.has(a));
}

/**
 * Where a face goes on the stage: its area's screen rectangle `rect`
 * (`{x, y, w, h}`, from the drawn area) as a CSS transform of the face
 * (laid out FACE_W px wide), or null when it is too small to show.
 */
export function facePlacement(rect, w = FACE_W) {
  if (!rect || !(rect.w > 0) || rect.w < FACE_MIN_PX) return null;
  const scale = rect.w / w;
  return { x: rect.x, y: rect.y, scale, transform: `translate(${rect.x}px, ${rect.y}px) scale(${scale})` };
}

/** Whether screen rectangles `a` and `b` overlap. */
export function overlapping(a, b) {
  return Boolean(a && b) && a.x < b.x + b.w && b.x < a.x + a.w && a.y < b.y + b.h && b.y < a.y + a.h;
}
