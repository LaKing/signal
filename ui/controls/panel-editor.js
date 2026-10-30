// The panel editor (docs/ui-controls.md "Editing a panel", ADR 0026 §4–5):
// <signal-panel>'s Edit layout mode. The panel's content is a tree
// (ui/lib/panel-tree.js) drawn into the page; every change is an operation
// on it (ui/lib/panel-edit.js), checked against the whitelist and written at
// once with `panel.write`, so undo and redo are the versions written. In
// edit mode the controls take no pointer or key (nothing is sent; their
// streams go on), and each card and control shows a frame, a handle and its
// address. ADR 0047 §2: on a plate (`plate` and each card's `place`) a card
// moves on the grid of plate units by dragging it and resizes by the
// corners of the selected card; a drop over another card or beyond the
// plate's sides is refused with a red outline, not pushed (a card moved or
// grown below the plate makes the plate taller). A card whose content grows
// is made taller, never shrunk under its content. A panel without places
// keeps the card grid (a drag changes the order) until Freeze layout writes
// the places it measures. Controls inside a card are ordered and grouped,
// never placed.

import { CONTAINERS, checkTree, generatedTree, parseContent, splitVersion, versionText } from '../lib/panel-tree.js';
import {
  EditSession,
  KINDS,
  addCard,
  addControl,
  cardOf,
  describeCard,
  editKey,
  editLabels,
  find,
  insertionIndex,
  isPlateCard,
  makeGroup,
  moveCard,
  moveControl,
  newControl,
  paletteEntries,
  placeOf,
  plateCards,
  removeCard,
  removeCardStep,
  removeControl,
  renameCard,
  searchPalette,
  setAttributes,
  setPlace,
  setPlaces,
  setScopeWide,
  setWide,
  ungroup,
} from '../lib/panel-edit.js';
import { defaultPanel } from '../lib/panel.js';
import { GUTTER, MAX_PLATE, PU, blocked, drawnBox, freeze, moved as movedPlace, parsePlate, placeText, plateText, resized, snap } from '../lib/plate.js';
import { ENTRY_CSS, ValueEntry } from './entry-field.js';

/** How many palette results show: the list keeps its height ("Stable layout"). */
const RESULTS = 6;

/** The editor's tools card, in <signal-panel>'s shadow root: fixed sizes throughout. */
export const EDITOR_CSS = `
.tools { box-sizing: border-box; height: 13.5rem; margin: 0 0 .75rem; padding: .5rem 1rem; border-radius: 10px; background: var(--signal-panel, #1f232b); border: 1px dashed var(--signal-focus, #5aa9ff); overflow: hidden; display: flex; flex-direction: column; gap: .35rem; font-size: 11px; }
.tools .line { display: flex; gap: .4rem; align-items: center; height: 1.6rem; white-space: nowrap; }
.tools button, .tools input, .tools select { box-sizing: border-box; height: 1.5rem; font: inherit; color: var(--signal-fg, #d6dbe4); background: var(--signal-track, #2a2f38); border: 1px solid var(--signal-muted, #8a93a3); border-radius: 3px; }
.tools button { flex: none; width: 9ch; padding: 0 .3em; overflow: hidden; text-overflow: ellipsis; cursor: pointer; }
.tools button.long { width: 14ch; }
.tools button.remove-card { width: 22ch; }
.tools button[disabled] { opacity: .4; cursor: default; }
.tools input[type="text"], .tools input:not([type]) { flex: none; width: 16ch; padding: 0 .3em; }
.tools input.search { width: 28ch; }
.tools label { flex: none; display: flex; gap: .3em; align-items: center; color: var(--signal-muted, #8a93a3); }
.tools .sep { flex: none; width: .6rem; }
.tools .message { flex: 1 1 0; min-width: 0; overflow: hidden; text-overflow: ellipsis; color: var(--signal-effective, #ffb74d); }
.tools .address { flex: none; width: 24ch; overflow: hidden; text-overflow: ellipsis; color: var(--signal-accent, #4fc3f7); font-family: ui-monospace, Menlo, monospace; }
.tools .readout { position: relative; display: inline-block; box-sizing: border-box; width: 11ch; height: 1.5rem; line-height: 1.5rem; padding: 0 .3em; border: 1px solid var(--signal-muted, #8a93a3); border-radius: 3px; overflow: hidden; white-space: pre; font-variant-numeric: tabular-nums; cursor: text; }
.tools .results { display: grid; grid-template-columns: repeat(3, minmax(0, 1fr)); grid-template-rows: repeat(2, 1.5rem); gap: .3rem .6rem; margin: 0; padding: 0; list-style: none; }
.tools .results button { width: 100%; text-align: left; font-family: ui-monospace, Menlo, monospace; }
.tools .results button[aria-pressed="true"] { border-color: var(--signal-accent, #4fc3f7); color: var(--signal-accent, #4fc3f7); }
.tools .more { flex: none; width: 14ch; color: var(--signal-muted, #8a93a3); }
.marker { position: fixed; z-index: 10; pointer-events: none; box-sizing: border-box; border: 2px solid var(--signal-accent, #4fc3f7); border-radius: 6px; }
.caret { position: fixed; z-index: 11; pointer-events: none; width: 3px; background: var(--signal-accent, #4fc3f7); }
${ENTRY_CSS}`;

const TOOLS = `
<div class="line">
  <button type="button" class="undo" title="Undo (⌘Z): write the version before">Undo</button>
  <button type="button" class="redo" title="Redo (⇧⌘Z)">Redo</button>
  <span class="sep"></span>
  <button type="button" class="add-card">Add card</button>
  <button type="button" class="remove-card">Remove card</button>
  <button type="button" class="wide">Wide</button>
  <button type="button" class="freeze long" title="Freeze layout: write each card's place on the plate as it is laid out now (ADR 0047)">Freeze layout</button>
  <span class="sep"></span>
  <button type="button" class="group" title="Group the selected controls (Shift-click to select several)">Group</button>
  <button type="button" class="ungroup">Ungroup</button>
  <button type="button" class="remove-control long">Remove control</button>
  <span class="message" role="status"></span>
</div>
<div class="line">
  <label>Card <input class="card-title" placeholder="title" aria-label="Card title"></label>
  <label>Description <input class="card-text" placeholder="none" aria-label="Card description"></label>
</div>
<div class="line">
  <span class="address" title="The selected control's address"></span>
  <label>Label <input class="label" aria-label="Label"></label>
  <label>Taper <select class="taper" aria-label="Taper"><option value="">—</option><option>linear</option><option>log</option><option>db</option></select></label>
  <label>Min <span class="readout min" tabindex="0" role="button" aria-label="Minimum: Enter to type"></span></label>
  <label>Max <span class="readout max" tabindex="0" role="button" aria-label="Maximum: Enter to type"></span></label>
  <label>Skin <input class="skin" placeholder="none" aria-label="Skin"></label>
  <label><input type="checkbox" class="scope-wide"> wide scope</label>
</div>
<div class="line">
  <input class="search" type="text" placeholder="Add a control: search parameters and ports" aria-label="Search parameters and ports">
  <select class="kind" aria-label="Control kind"></select>
  <button type="button" class="add">Add</button>
  <span class="more"></span>
</div>
<ol class="results"></ol>`;

let whitelist = null;

/** The whitelist (ui/lib/panel-whitelist.json), fetched once. */
export function loadWhitelist() {
  whitelist ??= fetch(new URL('../lib/panel-whitelist.json', import.meta.url)).then((r) => {
    if (!r.ok) throw new Error(`the editor's whitelist did not load (${r.status})`);
    return r.json();
  });
  return whitelist;
}

/** A container's or text's class attribute, as the serialiser orders it. */
function classNameOf(required, classes) {
  const first = ['wide', 'scope'].filter((c) => classes.includes(c));
  return [required, ...first, ...classes.filter((c) => !first.includes(c))].filter(Boolean).join(' ');
}

/**
 * Draws `tree` into `panel`'s light DOM, reusing the elements of `elements`
 * (node id → element), so a control that stays keeps its streams; `ids`
 * gets each element's node id. Children are only re-ordered where they
 * changed. `attribute(name, value)`, if given, is what an attribute shows
 * (ADR 0046: a slot's part renders its binds rooted).
 */
export function render(panel, tree, rules, elements, ids, attribute = null) {
  const build = (node) => {
    let el = elements.get(node.id);
    if (node.kind === 'control') {
      if (el?.localName !== node.tag) el = document.createElement(node.tag);
      for (const name of rules.controls[node.tag] ?? []) {
        const value = attribute ? attribute(name, node.attributes[name]) : node.attributes[name];
        if (value === undefined) {
          if (el.hasAttribute(name)) el.removeAttribute(name);
        } else if (el.getAttribute(name) !== value) {
          el.setAttribute(name, value);
        }
      }
    } else if (node.kind === 'h1' || node.kind === 'h2' || node.kind === 'p') {
      if (el?.localName !== node.kind) el = document.createElement(node.kind);
      const name = classNameOf(null, node.classes ?? []);
      if (name) el.className = name;
      else el.removeAttribute('class');
      if (el.textContent !== node.text) el.textContent = node.text;
    } else {
      const { tag, name } = CONTAINERS[node.kind];
      if (el?.localName !== tag) el = document.createElement(tag);
      const className = classNameOf(name, node.classes ?? []);
      if (el.className !== className) el.className = className;
      // ADR 0047: a card's place on the plate.
      if (node.place === undefined || node.place === null) {
        if (el.hasAttribute('place')) el.removeAttribute('place');
      } else if (el.getAttribute('place') !== node.place) el.setAttribute('place', node.place);
      place(el, node.children.map(build));
    }
    elements.set(node.id, el);
    ids.set(el, node.id);
    return el;
  };
  place(panel, tree.children.map(build));
}

/** `parent`'s children made `children`, touching nothing when they already are. */
function place(parent, children) {
  // The text between them goes (the file's line breaks, its comments); an
  // element is moved only when its place changed, so the others stay
  // connected, their streams with them.
  for (const node of [...parent.childNodes]) if (node.nodeType !== 1) node.remove();
  children.forEach((child, i) => {
    const at = parent.children[i];
    if (at !== child) parent.insertBefore(child, at ?? null);
  });
  while (parent.children.length > children.length) parent.lastElementChild.remove();
}

/** Maps the elements already in `panel` to `tree`'s nodes where they match, so they are kept. */
export function adopt(panel, tree, elements) {
  const match = (node, el) => {
    if (!el) return;
    const tag = node.kind === 'control' ? node.tag : CONTAINERS[node.kind]?.tag ?? node.kind;
    if (el.localName !== tag) return;
    elements.set(node.id, el);
    const kids = [...el.children];
    (node.children ?? []).forEach((child, i) => match(child, kids[i]));
  };
  const kids = [...panel.children];
  tree.children.forEach((child, i) => match(child, kids[i]));
}

/** Shows the written panel `html` (a `panel.written` push) in a panel not being edited. */
export async function showWritten(panel, html, attribute = null) {
  const rules = await loadWhitelist();
  const parsed = parseContent(html, rules, { mode: 'read' });
  if (parsed.error) return parsed.error;
  const elements = new Map();
  adopt(panel, parsed.tree, elements);
  render(panel, parsed.tree, rules, elements, new WeakMap(), attribute);
  return null;
}

/**
 * One undo history over several panel files (ADR 0046 §5): the host
 * panel's editor and the editors of its open slot cards' parts, each
 * writing its own file. Undo steps back the editor that wrote last, redo
 * the one undone last; with nothing written in this session, the host's
 * own history.
 */
export class SharedHistory {
  constructor(host) {
    this.host = host;
    this.done = [];
    this.undone = [];
    this.used = false;
  }

  /** `editor` wrote a new version. */
  wrote(editor) {
    this.done.push(editor);
    this.undone = [];
    this.used = true;
  }

  /** Undo (-1) or redo (+1) across the files. */
  step(direction) {
    const from = direction < 0 ? this.done : this.undone;
    const to = direction < 0 ? this.undone : this.done;
    while (from.length && !from[from.length - 1].editing) from.pop();
    const editor = from.pop();
    if (editor) {
      to.push(editor);
      editor.stepOwn(direction);
      return;
    }
    // Nothing written across the files in this session: the host's own history.
    if (!this.used) this.host.stepOwn(direction);
  }
}

/** Whether `event` is about a slot's part (its content or its own tools): its own editor's, not the host's. */
export function inPart(event, panel) {
  for (const n of event.composedPath?.() ?? []) {
    if (n === panel) return false;
    if (n.nodeType !== 1) continue;
    if (n.dataset?.partTools !== undefined) return true;
    if (n.localName !== 'signal-slot' && n.parentElement?.closest?.('signal-slot')) return true;
  }
  return false;
}

export class PanelEditor {
  constructor(panel) {
    this.panel = panel;
    this.editing = false;
    this.tree = null;
    this.selected = [];
    this.pending = null;
    /** What it writes against, for the page's session (EditSession). */
    this.session = null;
    this.picked = [];
    this.elements = new Map();
    this.ids = new WeakMap();
    this.trees = new Map();
    this.stops = [];
  }

  get connection() {
    return this.panel.connection;
  }

  $(selector) {
    return this.panel.shadowRoot.querySelector(selector);
  }

  /** Enters edit mode: reads the panel file (its text and base), or takes the generated panel. A refusal says why. */
  async enter() {
    if (this.editing) return null;
    if (!this.connection) return 'No connection to the server';
    const rules = await loadWhitelist();
    const file = this.panel.file ?? 'main.html';
    const read = await this.connection.request({ type: 'panel.read', file });
    if (!read.planned) return 'The project has no plan: Edit layout writes only what the binding check accepts';
    let tree;
    if (read.exists) {
      if (!read.editable) return `Not editable: ${read.reason}`;
      const parsed = parseContent(read.html, rules, { mode: 'read' });
      // Never stripped: what the whitelist refuses stays, by hand.
      if (parsed.error) return `Not editable here (${parsed.error}); edit the file by hand`;
      tree = parsed.tree;
      // ADR 0047: the plate, on <signal-panel> itself.
      tree.plate = read.plate ?? null;
    } else {
      // ADR 0046: a slot's part starts from its interface parameters.
      tree = this.panel.emptyTree?.() ?? generatedTree(defaultPanel(this.connection.description));
    }
    this.rules = rules;
    this.file = file;
    this.generated = !read.exists;
    this.shownPlate = this.panel.getAttribute?.('plate') ?? null;
    this.session ??= new EditSession((html, plate) => this.readContent(html, plate));
    // The session's history goes on while the file is the version it last
    // wrote (AC7): the tree shown is then that version's, with its ids. It
    // starts at the content as written when that may be written back as it
    // is (ADR 0047: so an undo gives the file back byte for byte), else at
    // its canonical text.
    const text = read.exists && !parseContent(read.html, rules).error ? `${tree.plate ? `plate="${tree.plate}"` : ''}${read.html}` : versionText(tree, rules);
    if (this.session.enter({ ...read, file }, text) && this.trees.has(this.session.history.current)) {
      tree = this.trees.get(this.session.history.current);
    } else {
      this.trees.set(text, tree);
    }
    this.tree = tree;
    this.editing = true;
    this.selected = [];
    this.pending = null;
    this.elements = new Map();
    if (this.generated) this.$('.generated').hidden = true;
    else adopt(this.panel, tree, this.elements);
    this.panel.setAttribute('editing', '');
    this.panel.setAttribute('no-entry', '');
    this.buildTools();
    this.listen();
    this.draw();
    return null;
  }

  /** Leaves edit mode; everything was written already. */
  leave() {
    if (!this.editing) return;
    this.editing = false;
    for (const stop of this.stops.splice(0)) stop();
    this.min?.close();
    this.max?.close();
    this.marker?.remove();
    this.caret?.remove();
    this.panel.removeAttribute('editing');
    this.panel.removeAttribute('no-entry');
    for (const el of this.panel.querySelectorAll('[data-edit], [data-edit-selected]')) {
      el.removeAttribute('data-edit');
      el.removeAttribute('data-edit-selected');
    }
    this.$('.tools').hidden = true;
    this.overlay()?.replaceChildren();
    if (this.generated) {
      // Never written: the generated panel again.
      this.panel.replaceChildren();
      this.panel.setPlate?.(this.shownPlate);
      this.$('.generated').hidden = false;
      this.panel.layOutNow?.();
    }
    this.elements = new Map();
  }

  /** A version's text (its content and plate, `versionText`), remembering its tree (undo draws it with the same elements). */
  remember(tree) {
    const text = versionText(tree, this.rules);
    this.trees.set(text, tree);
    return text;
  }

  /** Another client wrote the panel: take it. */
  written(message) {
    if (!this.editing || message.base === this.session.base) return;
    this.generated = false;
    this.changed(this.session.changed(message), 'Written elsewhere: shown as written');
  }

  /** A panel's content (and its plate) as the session reads it: `{tree, text}` or `{error}`. */
  readContent(html, plate = null) {
    const parsed = parseContent(html, this.rules, { mode: 'read' });
    if (parsed.error) return { error: parsed.error };
    parsed.tree.plate = plate ?? null;
    return { tree: parsed.tree, text: this.remember(parsed.tree) };
  }

  /** Another version is the file (`EditSession.changed`): shown, or writing stopped with why. */
  changed(outcome, note) {
    if (outcome.adopt) {
      this.tree = outcome.adopt;
      this.selected = [];
      this.say(note);
    } else {
      this.say(outcome.blocked);
    }
    if (this.editing) this.draw();
  }

  // Drawing.

  draw() {
    render(this.panel, this.tree, this.rules, this.elements, this.ids, this.panel.attributeValue?.bind(this.panel) ?? null);
    for (const [id, label] of editLabels(this.tree)) {
      const el = this.elements.get(id);
      if (!el) continue;
      el.dataset.edit = label;
      if (this.selected.includes(id)) el.dataset.editSelected = '';
      else delete el.dataset.editSelected;
    }
    // ADR 0047: the plate as the tree has it, laid out at once.
    this.panel.setPlate?.(this.tree.plate ?? null);
    this.panel.layOutNow?.();
    this.drawCorners();
    this.update();
  }

  // The plate (ADR 0047 §2).

  /** The <signal-panel> whose plate this panel is drawn on (a part's: its host's). */
  host() {
    return this.panel.localName === 'signal-panel' ? this.panel : (this.panel.closest?.('signal-panel') ?? null);
  }

  /** The layer the marks are drawn in, over the host's plate (plate px). */
  overlay() {
    return this.host()?.overlay?.() ?? null;
  }

  /** The grid the plate is laid out in: this panel's own (`div.signal-cards`). */
  grid() {
    return this.panel.querySelector?.(':scope > .signal-cards') ?? null;
  }

  /** Where this panel's plate lies on the host's (plate px): its grid's corner. */
  plateOffset() {
    const host = this.host();
    const grid = this.grid();
    if (!host?.plateRect || !grid) return { x: 0, y: 0 };
    const r = host.plateRect(grid);
    if (r.w > 0 || r.h > 0) return { x: r.x, y: r.y };
    // A grid without a box of its own (an open slot card's part, `display: contents`): its cards' corner.
    const cards = [...grid.children].map((c) => host.plateRect(c)).filter((c) => c.w > 0);
    return cards.length ? { x: Math.min(...cards.map((c) => c.x)), y: Math.min(...cards.map((c) => c.y)) } : { x: 0, y: 0 };
  }

  /** A box on this panel's plate (plate px) on the host's plate. */
  onHost(box) {
    const o = this.plateOffset();
    return { x: o.x + box.x, y: o.y + box.y, w: box.w, h: box.h };
  }

  /** The selected card's corners, its resize handles. */
  drawCorners() {
    const layer = this.overlay();
    if (!layer) return;
    for (const c of layer.querySelectorAll(`.corner[data-of="${this.file}"]`)) c.remove();
    const plate = parsePlate(this.tree.plate);
    const id = this.selected.length === 1 ? this.selected[0] : null;
    const found = id ? find(this.tree, id) : null;
    const place = plate && found && isPlateCard(found.node, found.parent) ? placeOf(found.node) : null;
    if (!place) return;
    const box = this.onHost(drawnBox(place));
    for (const [x, y] of [[box.x, box.y], [box.x + box.w, box.y], [box.x, box.y + box.h], [box.x + box.w, box.y + box.h]]) {
      const corner = document.createElement('div');
      corner.className = 'corner';
      corner.dataset.of = this.file;
      Object.assign(corner.style, { left: `${x}px`, top: `${y}px` });
      layer.append(corner);
    }
  }

  /** Where a drag would put a card (`place`), red when refused. */
  showOutline(place, refused) {
    const layer = this.overlay();
    if (!layer) return;
    this.outline ??= Object.assign(document.createElement('div'), { className: 'outline' });
    const box = this.onHost(drawnBox(place));
    Object.assign(this.outline.style, { left: `${box.x}px`, top: `${box.y}px`, width: `${box.w}px`, height: `${box.h}px` });
    this.outline.classList.toggle('refused', Boolean(refused));
    this.outline.title = refused ?? '';
    if (!this.outline.isConnected) layer.append(this.outline);
  }

  /**
   * Freeze layout (ADR 0047 §2): every card's box as laid out now, rounded
   * outward to whole plate units, written as its place, and the plate
   * around them. One undo gives the file back.
   */
  freeze() {
    if (this.tree.plate) return;
    const grids = this.tree.children.filter((c) => c.kind === 'cards');
    if (grids.length !== 1) {
      this.say('Freeze layout places the cards of one grid; this panel has ' + grids.length);
      return;
    }
    const host = this.host();
    const cards = plateCards(this.tree);
    const origin = this.plateOffset();
    const boxes = [];
    for (const { node } of cards) {
      const el = this.elements.get(node.id);
      const box = el?.cardBox?.() ?? el;
      if (!box || !host?.plateRect) {
        this.say('Freeze layout: a card is not drawn; nothing written');
        return;
      }
      // An open slot card: its header and its part's cards (ADR 0046 §5).
      const rects = [box, ...(el.localName === 'signal-slot' ? [...el.querySelectorAll(':scope > .signal-cards > *')] : [])].map((e) => host.plateRect(e)).filter((r) => r.w > 0 && r.h > 0);
      const x = Math.min(...rects.map((r) => r.x));
      const y = Math.min(...rects.map((r) => r.y));
      // Relative to this panel's plate, which an unplaced plate pads by the gutter.
      boxes.push({ x: x - origin.x + GUTTER, y: y - origin.y + GUTTER, w: Math.max(...rects.map((r) => r.x + r.w)) - x, h: Math.max(...rects.map((r) => r.y + r.h)) - y });
    }
    const host_ = this.host();
    const gridWidth = this.grid()?.offsetWidth || Math.max(0, ...boxes.map((b) => b.x + b.w - GUTTER));
    const width = this.panel.localName === 'signal-panel' || !host_ ? undefined : Math.ceil((gridWidth + 2 * GUTTER) / PU);
    const { plate, places } = freeze(boxes, width ? { width } : undefined);
    const result = setPlaces(this.tree, plate, cards.map((c, i) => [c.node.id, places[i]]));
    if (!result.error) this.say(`Frozen: ${cards.length} cards on a plate of ${plate.w} × ${plate.h} units`);
    this.apply(result);
  }

  /**
   * `tree` with every placed card whose content is taller than its card
   * made taller (and the plate with it), measured as drawn: never shrunk
   * under its content.
   */
  refitted(tree) {
    const plate = parsePlate(tree.plate);
    if (!plate) return tree;
    const previous = this.tree;
    this.tree = tree;
    render(this.panel, tree, this.rules, this.elements, this.ids, this.panel.attributeValue?.bind(this.panel) ?? null);
    this.panel.setPlate?.(tree.plate);
    this.panel.layOutNow?.();
    this.tree = previous;
    let out = tree;
    let h = plate.h;
    for (const { node, place } of plateCards(tree)) {
      const el = this.elements.get(node.id);
      if (!place || !el || !(el.clientHeight > 0)) continue;
      const need = Math.ceil((el.scrollHeight + 2 * GUTTER) / PU);
      if (need <= place.h) continue;
      const result = setPlace(out, node.id, { ...place, h: need });
      if (result.error) continue;
      out = result.tree;
      h = Math.max(h, place.y + need);
    }
    if (out !== tree && h > plate.h) out.plate = plateText({ w: plate.w, h });
    return out;
  }

  /**
   * A press on card `subject` on a plate: a drag moves it on the grid, or,
   * from a corner of the selected card, resizes it. Refused drops are
   * outlined red and written never. Whether the press was the plate's.
   */
  plateDown(event, subject) {
    const plate = parsePlate(this.tree.plate);
    const found = plate ? find(this.tree, subject.id) : null;
    const place = found && isPlateCard(subject, found.parent) ? placeOf(subject) : null;
    if (!place) return false;
    const host = this.host();
    const scale = host?.scale || 1;
    const el = this.elements.get(subject.id);
    const r = el?.getBoundingClientRect();
    const near = 12;
    let corner = null;
    if (r && this.selected.length === 1 && this.selected[0] === subject.id) {
      const v = Math.abs(event.clientY - r.top) <= near ? 'n' : Math.abs(event.clientY - r.bottom) <= near ? 's' : '';
      const h = Math.abs(event.clientX - r.left) <= near ? 'w' : Math.abs(event.clientX - r.right) <= near ? 'e' : '';
      if (v && h) corner = v + h;
    }
    const others = plateCards(this.tree).filter((c) => c.node.id !== subject.id).map((c) => c.place);
    // A card goes below the plate's bottom: the plate grows (never past its largest size).
    const room = { w: plate.w, h: MAX_PLATE };
    const start = { x: event.clientX, y: event.clientY };
    let dragging = false;
    let candidate = place;
    let refusal = null;
    const move = (e) => {
      if (!dragging && Math.hypot(e.clientX - start.x, e.clientY - start.y) < 4) return;
      dragging = true;
      const dx = snap(e.clientX - start.x, scale);
      const dy = snap(e.clientY - start.y, scale);
      candidate = corner ? resized(place, corner, dx, dy, room) : movedPlace(place, dx, dy, room);
      refusal = blocked(candidate, others, room);
      this.showOutline(candidate, refusal);
    };
    const up = () => {
      window.removeEventListener('pointermove', move, true);
      window.removeEventListener('pointerup', up, true);
      this.outline?.remove();
      if (!dragging) {
        this.select(subject.id, event.shiftKey);
        return;
      }
      if (refusal) {
        this.say(`Not moved: ${refusal}`);
        return;
      }
      if (placeText(candidate) === placeText(place)) return;
      this.selected = [subject.id];
      const result = setPlace(this.tree, subject.id, candidate);
      if (!result.error && candidate.y + candidate.h > plate.h) result.tree.plate = plateText({ w: plate.w, h: candidate.y + candidate.h });
      this.apply(result);
    };
    window.addEventListener('pointermove', move, true);
    window.addEventListener('pointerup', up, true);
    return true;
  }

  /** The tools' state from the selection. */
  update() {
    const t = this.tree;
    const nodes = this.selected.map((id) => find(t, id)?.node).filter(Boolean);
    const control = nodes.length === 1 && nodes[0].kind === 'control' ? nodes[0] : null;
    const group = nodes.length === 1 && nodes[0].kind === 'group' ? nodes[0] : null;
    const card = nodes.length ? cardOf(t, nodes[0].id) : null;
    const { busy, blocked, history } = this.session;
    this.$('.undo').disabled = busy || Boolean(blocked) || !history.canUndo;
    this.$('.redo').disabled = busy || Boolean(blocked) || !history.canRedo;
    const remove = this.$('.remove-card');
    remove.disabled = !card;
    remove.textContent = this.pending && this.pending === card?.id ? 'Remove card? Click again' : 'Remove card';
    // ADR 0047: on a plate a card's width is its place; Freeze only once.
    this.$('.wide').disabled = !card || Boolean(t.plate);
    this.$('.wide').setAttribute('aria-pressed', String(Boolean(card?.classes.includes('wide'))));
    this.$('.freeze').disabled = busy || Boolean(blocked) || Boolean(t.plate);
    this.$('.group').disabled = !(nodes.length > 1 && nodes.every((n) => n.kind === 'control'));
    this.$('.ungroup').disabled = !group && !(control && find(t, control.id).parent.kind === 'group');
    this.$('.remove-control').disabled = !control;
    const heading = card?.children.find((c) => c.kind === 'h2') ?? card?.children.find((c) => c.kind === 'h1');
    const field = (selector, value, enabled) => {
      const input = this.$(selector);
      if (input !== input.getRootNode().activeElement) input.value = value ?? '';
      input.disabled = !enabled;
    };
    field('.card-title', heading?.text, Boolean(card));
    field('.card-text', card?.children.find((c) => c.kind === 'p')?.text, Boolean(card));
    this.$('.address').textContent = control ? `${control.tag.replace('signal-', '')} ${control.attributes.bind ?? ''}` : '';
    field('.label', control?.attributes.label, Boolean(control));
    field('.skin', control?.attributes.skin, Boolean(control) && this.rules.controls[control.tag].includes('skin'));
    const taper = this.$('.taper');
    taper.value = control?.attributes.taper ?? '';
    taper.disabled = !control || !this.rules.controls[control.tag].includes('taper');
    const ranged = Boolean(control) && this.rules.controls[control.tag].includes('min');
    for (const end of ['min', 'max']) {
      const readout = this.$(`.readout.${end}`);
      if (!this[end]?.isOpen) readout.textContent = ranged ? (control.attributes[end] ?? '(range)') : '';
      readout.setAttribute('aria-disabled', String(!ranged));
    }
    const scope = this.$('.scope-wide');
    scope.disabled = control?.tag !== 'signal-scope';
    scope.checked = Boolean(control?.tag === 'signal-scope' && card?.classes.includes('wide'));
    this.showPalette();
  }

  say(text) {
    const message = this.$('.message');
    message.textContent = text ?? '';
    message.title = text ?? '';
  }

  // The tools.

  buildTools() {
    const tools = this.$('.tools');
    tools.innerHTML = TOOLS;
    tools.hidden = false;
    const on = (selector, type, fn) => this.$(selector).addEventListener(type, fn);
    on('.undo', 'click', () => this.step(-1));
    on('.redo', 'click', () => this.step(1));
    on('.add-card', 'click', () => {
      const card = this.selectedCard();
      const grid = card ? find(this.tree, card.id).parent : null;
      const at = card ? grid.children.indexOf(card) + 1 : Infinity;
      const result = addCard(this.tree, { cards: grid?.id ?? null, index: at, title: 'new card' });
      if (!result.error) this.selected = [result.id];
      this.apply(result);
    });
    on('.remove-card', 'click', () => {
      const card = this.selectedCard();
      if (!card) return;
      const step = removeCardStep(this.pending, this.tree, card.id);
      if (step.action === 'confirm') {
        this.pending = step.pending;
        this.say('The card holds controls: click Remove card again to remove it');
        this.update();
        return;
      }
      this.selected = [];
      this.apply(removeCard(this.tree, card.id));
    });
    on('.wide', 'click', () => {
      const card = this.selectedCard();
      if (card) this.apply(setWide(this.tree, card.id, !card.classes.includes('wide')));
    });
    on('.freeze', 'click', () => this.freeze());
    on('.group', 'click', () => {
      const result = makeGroup(this.tree, this.selected);
      if (!result.error) this.selected = [result.id];
      this.apply(result);
    });
    on('.ungroup', 'click', () => {
      const node = find(this.tree, this.selected[0]);
      const group = node?.node.kind === 'group' ? node.node : node?.parent;
      if (group?.kind === 'group') this.apply(ungroup(this.tree, group.id));
    });
    on('.remove-control', 'click', () => {
      const id = this.selected[0];
      this.selected = [];
      this.apply(removeControl(this.tree, id));
    });
    const text = (selector, op) =>
      on(selector, 'change', (event) => {
        const card = this.selectedCard();
        if (card) this.apply(op(this.tree, card.id, event.target.value.trim()));
      });
    text('.card-title', renameCard);
    text('.card-text', describeCard);
    const attribute = (selector, name) =>
      on(selector, 'change', (event) => {
        const control = this.selectedControl();
        if (control) this.apply(setAttributes(this.tree, control.id, { [name]: event.target.value.trim() || null }));
      });
    attribute('.label', 'label');
    attribute('.skin', 'skin');
    attribute('.taper', 'taper');
    on('.scope-wide', 'change', (event) => {
      const control = this.selectedControl();
      if (control) this.apply(setScopeWide(this.tree, control.id, event.target.checked));
    });
    // Min and max: M2.9-4's value entry, in the parameter's unit and range.
    for (const end of ['min', 'max']) {
      const readout = this.$(`.readout.${end}`);
      this[end] = new ValueEntry(readout, {
        enabled: () => this.selectedControl() && readout.getAttribute('aria-disabled') !== 'true',
        spec: () => this.rangeSpec(),
        value: () => {
          const spec = this.rangeSpec();
          const written = Number(this.selectedControl()?.attributes[end]);
          return Number.isFinite(written) ? written : spec.range[end];
        },
        apply: (value) => {
          const control = this.selectedControl();
          if (control) this.apply(setAttributes(this.tree, control.id, { [end]: String(value) }));
        },
        note: (text) => this.say(text),
        closed: () => this.update(),
        focus: () => readout.focus(),
      });
      readout.addEventListener('click', () => this[end].open());
      readout.addEventListener('keydown', (event) => {
        if (event.key === 'Enter' && !this[end].isOpen) {
          event.preventDefault();
          this[end].open();
        }
      });
    }
    on('.search', 'input', () => {
      this.picked = [];
      this.showPalette();
    });
    on('.kind', 'change', () => this.showPalette());
    on('.add', 'click', () => this.add());
    this.$('.results').replaceChildren(
      ...Array.from({ length: RESULTS }, () => {
        const item = document.createElement('li');
        const button = document.createElement('button');
        button.type = 'button';
        button.addEventListener('click', () => this.pick(button.entry));
        item.append(button);
        return item;
      }),
    );
  }

  /** The selected control's parameter, for its min and max (`parseValue`'s spec). */
  rangeSpec() {
    const control = this.selectedControl();
    const written = (control?.attributes.bind ?? '').split(/[\s,]+/)[0];
    // ADR 0046: a part's relative address, rooted.
    const address = this.panel.absoluteAddress?.(written) ?? written;
    const parameter = (this.connection.description?.controls ?? []).find((c) => c.address === address) ??
      paletteEntries(this.connection.description).find((e) => e.address === address) ?? {};
    return {
      unit: parameter.unit ?? null,
      range: { min: parameter.min ?? -Infinity, max: parameter.max ?? Infinity },
      taper: control?.attributes.taper ?? 'linear',
      kind: null,
    };
  }

  selectedControl() {
    const node = find(this.tree, this.selected[0])?.node;
    return this.selected.length === 1 && node?.kind === 'control' ? node : null;
  }

  selectedCard() {
    return this.selected.length ? cardOf(this.tree, this.selected[0]) : null;
  }

  // The palette.

  /** The palette's entries: a part's are its slot's addresses, relative (ADR 0046). */
  palette() {
    const entries = paletteEntries(this.connection.description);
    return this.panel.paletteEntries?.(entries) ?? entries;
  }

  showPalette() {
    const entries = this.palette();
    const { shown, more } = searchPalette(entries, this.$('.search').value, RESULTS);
    for (const [i, item] of [...this.$('.results').children].entries()) {
      const button = item.firstChild;
      const entry = shown[i] ?? null;
      button.entry = entry;
      button.textContent = entry ? `${entry.address}  ${entry.kinds.join(' ') || '(no kind)'}` : '';
      button.title = entry ? `${entry.address} (${entry.what})` : '';
      button.style.visibility = entry ? 'visible' : 'hidden';
      button.setAttribute('aria-pressed', String(Boolean(entry && this.picked.some((p) => p.address === entry.address))));
    }
    this.$('.more').textContent = more ? `${more} more: narrow the search` : '';
    const kinds = this.picked[0]?.kinds ?? [];
    const select = this.$('.kind');
    const wanted = kinds.join(' ');
    if (select.dataset.kinds !== wanted) {
      select.dataset.kinds = wanted;
      select.replaceChildren(...kinds.map((k) => Object.assign(document.createElement('option'), { value: k, textContent: k === 'xy' ? '2D input' : k })));
    }
    const kind = select.value;
    this.$('.add').disabled = !kinds.length || (kind === 'xy' && this.picked.length < 2);
  }

  /** A palette result clicked: picked (a second one for a 2D input's y). */
  pick(entry) {
    if (!entry) return;
    const xy = this.$('.kind').value === 'xy';
    if (xy && this.picked.length === 1 && this.picked[0].address !== entry.address) this.picked = [this.picked[0], entry];
    else this.picked = [entry];
    this.say(xy && this.picked.length === 1 ? 'A 2D input: pick its y parameter too' : '');
    this.showPalette();
  }

  add() {
    const kind = this.$('.kind').value;
    if (!KINDS[kind]) return;
    const spec = newControl(kind, this.picked, this.connection.description);
    if (spec.error) {
      this.say(spec.error);
      return;
    }
    let tree = this.tree;
    let into = this.selectedControl() ? find(tree, this.selected[0]).parent.id : this.selectedCard()?.id;
    let index = this.selectedControl() ? find(tree, this.selected[0]).index + 1 : Infinity;
    if (!into) {
      // Into the last card, made when there is none.
      const cards = [...walkCards(tree)];
      if (cards.length) into = cards[cards.length - 1].id;
      else {
        const made = addCard(tree, { title: 'new card' });
        tree = made.tree;
        into = made.id;
      }
      index = Infinity;
    }
    if (find(tree, into)?.node.kind === 'card' && this.selectedControl() && find(tree, this.selected[0]).parent.kind === 'card') index = Infinity;
    const result = addControl(tree, spec, { into, index });
    if (!result.error) this.selected = [result.id];
    this.picked = [];
    this.apply(result);
  }

  // Writing.

  /** An operation's result: written, or its error said. */
  apply(result) {
    this.pending = null;
    if (result.error) {
      this.say(result.error);
      this.update();
      return;
    }
    this.write(result.tree, 0);
  }

  /** Undo (-1) or redo (+1): across the files when the history is shared (ADR 0046), else this file's. */
  step(direction) {
    if (!this.editing) return;
    if (this.shared) {
      this.shared.step(direction);
      return;
    }
    this.stepOwn(direction);
  }

  /** Undo (-1) or redo (+1) of this file: writes the version before or after. */
  stepOwn(direction) {
    if (!this.editing) return;
    const history = this.session.history;
    const text = direction < 0 ? history.undo() : history.redo();
    if (text === null) return;
    const { plate, html } = splitVersion(text);
    const tree = this.trees.get(text) ?? { ...parseContent(html, this.rules).tree, plate };
    this.pending = null;
    // The version as it was written (ADR 0047: the first one as the file had it).
    this.write(tree, direction, text);
  }

  /**
   * Writes `tree` (`step` 0: a new version; -1 or +1: undo or redo). Checked
   * against the whitelist before sending; shown at once, and the last
   * written version again when the server refuses it. Nothing is sent while
   * the session's writing is stopped (`EditSession.blocked`).
   */
  async write(tree, step, version = null) {
    if (!this.editing) return;
    // ADR 0047: a placed card is never shorter than its content.
    if (step === 0) tree = this.refitted(tree);
    const checked = checkTree(tree, this.rules);
    if (checked.error) {
      this.say(`Not written: ${checked.error}`);
      // The refit drew it: the version shown again.
      this.draw();
      return;
    }
    const html = version ?? this.remember(tree);
    if (html === this.session.history.current && step === 0) {
      this.tree = tree;
      this.draw();
      return;
    }
    const request = this.session.request(html);
    if (request.refused) {
      this.say(request.refused);
      this.draw();
      return;
    }
    const previous = this.tree;
    this.tree = tree;
    this.draw();
    try {
      const reply = await this.connection.request(request.message);
      this.session.written(reply, html, step);
      if (step === 0) this.shared?.wrote(this);
      if (this.generated) {
        // ui/main.html exists now: this page shows it (left meanwhile: the
        // generated panel that leave() showed again goes; the written one
        // is drawn by its push).
        this.generated = false;
        this.panel.removeAttribute('generated');
        if (!this.editing) this.$('.generated').hidden = true;
      }
      this.say(step < 0 ? 'Undone: written' : step > 0 ? 'Redone: written' : 'Written');
      this.panel.recheck?.();
    } catch (error) {
      const outcome = this.session.refused(error);
      if (outcome.restore) {
        this.tree = previous;
        this.say(outcome.restore);
      } else {
        if (outcome.blocked) this.tree = previous;
        this.changed(outcome, 'The file changed elsewhere: shown as it is now; nothing was written');
        return;
      }
    }
    if (this.editing) this.draw();
  }

  // Pointer and keys.

  /** The node a pointer or key event in the panel's content is about, or null. */
  nodeOf(event) {
    for (const n of event.composedPath()) {
      if (n === this.panel) return null;
      if (n.nodeType === 1 && this.ids.has(n) && this.panel.contains(n)) return this.ids.get(n);
    }
    return null;
  }

  /** Whether an event comes from the panel's content (not its bar or tools, nor a slot's part, which its own editor takes). */
  inContent(event) {
    if (!this.panel.isPart && inPart(event, this.panel)) return false;
    return event.composedPath().some((n) => n !== this.panel && n.nodeType === 1 && this.panel.contains(n));
  }

  listen() {
    const add = (target, type, fn, options = { capture: true }) => {
      target.addEventListener(type, fn, options);
      this.stops.push(() => target.removeEventListener(type, fn, options));
    };
    // Nothing in the content reaches a control: no value is sent.
    const swallow = (event) => {
      if (!this.inContent(event)) return;
      event.stopPropagation();
      if (event.type !== 'wheel') event.preventDefault();
    };
    for (const type of ['click', 'dblclick', 'wheel', 'pointerup', 'pointermove', 'pointercancel', 'touchstart', 'contextmenu']) add(this.panel, type, swallow, type === 'wheel' ? { capture: true, passive: true } : { capture: true });
    add(this.panel, 'keydown', (event) => {
      if (this.inContent(event) && !editKey(event)) swallow(event);
    });
    add(this.panel, 'pointerdown', (event) => this.down(event));
    // A part's editor shares the host's keys (one history, ADR 0046).
    if (this.panel.isPart) return;
    add(this.panel.ownerDocument, 'keydown', (event) => {
      const action = editKey(event);
      if (!action) return;
      event.preventDefault();
      event.stopPropagation();
      if (action === 'leave') this.panel.setEditing(false);
      else this.step(action === 'undo' ? -1 : 1);
    });
  }

  down(event) {
    if (!this.inContent(event)) return;
    event.stopPropagation();
    event.preventDefault();
    const id = this.nodeOf(event);
    const node = id ? find(this.tree, id)?.node : null;
    // Text, rows and grids select their card; a control or group is itself.
    const subject = !node ? null : node.kind === 'control' || node.kind === 'group' || node.kind === 'card' ? node : cardOf(this.tree, id);
    if (!subject) return;
    // ADR 0047: on a plate a card moves and resizes on the grid.
    if (this.plateDown(event, subject)) return;
    const start = { x: event.clientX, y: event.clientY };
    let dragging = false;
    const move = (e) => {
      if (!dragging && Math.hypot(e.clientX - start.x, e.clientY - start.y) < 4) return;
      dragging = true;
      this.showDrop(subject, this.dropFor(subject, e));
    };
    const up = (e) => {
      window.removeEventListener('pointermove', move, true);
      window.removeEventListener('pointerup', up, true);
      this.marker?.remove();
      this.caret?.remove();
      if (!dragging) {
        this.select(subject.id, event.shiftKey);
        return;
      }
      const drop = this.dropFor(subject, e);
      if (!drop) return;
      this.selected = [subject.id];
      if (subject.kind === 'card') this.apply(moveCard(this.tree, subject.id, { cards: drop.into, index: drop.index }));
      else this.apply(moveControl(this.tree, subject.id, drop));
    };
    window.addEventListener('pointermove', move, true);
    window.addEventListener('pointerup', up, true);
  }

  select(id, extend) {
    this.pending = null;
    const node = find(this.tree, id);
    const first = find(this.tree, this.selected[0]);
    if (extend && node?.node.kind === 'control' && first?.node.kind === 'control' && first.parent === node.parent) {
      this.selected = this.selected.includes(id) ? this.selected.filter((s) => s !== id) : [...this.selected, id];
    } else {
      this.selected = [id];
    }
    this.say('');
    this.draw();
  }

  /** Where `subject` dropped at the pointer of `event` goes: `{into, index}`, or null. */
  dropFor(subject, event) {
    const point = { x: event.clientX, y: event.clientY };
    const under = this.panel.ownerDocument.elementsFromPoint(point.x, point.y);
    const nodes = under
      .map((el) => this.ids.get(el))
      .filter(Boolean)
      .map((id) => find(this.tree, id)?.node)
      .filter((n) => n && n.id !== subject.id && !find(subject, n.id));
    const rects = (container) => container.children.map((c) => this.elements.get(c.id)?.getBoundingClientRect()).filter(Boolean);
    if (subject.kind === 'card') {
      const card = nodes.find((n) => n.kind === 'card');
      const grid = nodes.find((n) => n.kind === 'cards') ?? (card ? find(this.tree, card.id).parent : null);
      if (!grid) return null;
      return { into: grid.id, index: insertionIndex(rects(grid), point), rect: this.elements.get(grid.id)?.getBoundingClientRect() };
    }
    let container = nodes.find((n) => n.kind === 'row' || n.kind === 'group' || n.kind === 'card');
    if (!container) return null;
    if (container.kind === 'group' && subject.kind === 'group') container = find(this.tree, container.id).parent;
    if (container.kind === 'card') {
      const row = [...container.children].reverse().find((c) => c.kind === 'row');
      if (row) container = row;
      else return { into: container.id, index: Infinity, rect: this.elements.get(container.id)?.getBoundingClientRect() };
    }
    return { into: container.id, index: insertionIndex(rects(container), point), rect: this.elements.get(container.id)?.getBoundingClientRect() };
  }

  /** The drop's container and place, drawn over the page (fixed, out of the flow: nothing moves). */
  showDrop(subject, drop) {
    if (!drop?.rect) {
      this.marker?.remove();
      this.caret?.remove();
      return;
    }
    this.marker ??= Object.assign(document.createElement('div'), { className: 'marker' });
    this.caret ??= Object.assign(document.createElement('div'), { className: 'caret' });
    const r = drop.rect;
    Object.assign(this.marker.style, { left: `${r.left}px`, top: `${r.top}px`, width: `${r.width}px`, height: `${r.height}px` });
    const container = find(this.tree, drop.into)?.node;
    const children = (container?.children ?? []).filter((c) => c.id !== subject.id);
    const at = Number.isFinite(drop.index) ? container?.children[drop.index] : null;
    const box = (at && this.elements.get(at.id)?.getBoundingClientRect()) ?? null;
    const last = children.length ? this.elements.get(children[children.length - 1].id)?.getBoundingClientRect() : null;
    const caret = box ? { left: box.left - 4, top: box.top, height: box.height } : last ? { left: last.right + 2, top: last.top, height: last.height } : { left: r.left + 4, top: r.top + 4, height: r.height - 8 };
    Object.assign(this.caret.style, { left: `${caret.left}px`, top: `${caret.top}px`, height: `${caret.height}px` });
    // Fixed, so outside the scaled plate (ADR 0047): in the host's shadow root, a part's editor too.
    (this.host() ?? this.panel).shadowRoot.append(this.marker, this.caret);
  }
}

function* walkCards(node) {
  for (const child of node.children ?? []) {
    if (child.kind === 'card') yield child;
    yield* walkCards(child);
  }
}
