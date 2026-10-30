// <signal-panel>: the loader of a project panel (docs/ui-controls.md
// "Panels"). It connects with the page's URL (`#ws=…&token=…`), makes that
// the controls' connection, and once described asks the server's binding
// check (`panel.check`, the check `signal ui check` runs) for its file,
// showing every diagnostic in the page and marking the controls they are
// about. With `generated` it builds the default panel from `describe`
// instead. Attributes: `file` (the panel file; by default the page's
// `/project/<file>.html`), `generated`, `plate` (ADR 0047 §2: the plate's
// width and height in plate units, its cards at their `place`). Its bar's
// Edit layout switch is the panel editor (./panel-editor.js, M2.9-2); a
// panel another client wrote (`panel.written`) is shown as written, without
// reloading the page.
//
// ADR 0047: the panel is a fixed plate. Its cards are laid out once at the
// plate's own size (a panel without places at 1280 px, the reference width:
// the implicit freeze) and the plate is scaled whole to the window by one
// transform: fit the width and scroll down, never under a scale of 0.5
// (then it pans sideways too); ⌘+ and ⌘− zoom, ⌘0 shows the whole plate (and
// fits the width again). The window changes the scale, never the
// arrangement. The scale is view state, never written.

import { connectFromPage, getDefaultConnection, setDefaultConnection } from '../signal-ui.js';
import { BindingCheck, defaultPanel, formatDiagnostic, panelFile } from '../lib/panel.js';
import { GUTTER, PU, REFERENCE_PX, defaultPlate, fitScale, pack, plateText, zoomed } from '../lib/plate.js';
import { PLATE_CSS, layOutCards, overflowing } from './plate.js';
import { unsavedLine } from '../lib/unsaved.js';
import { EDITOR_CSS, PanelEditor, SharedHistory, showWritten } from './panel-editor.js';

/** The generated panel's grid of cards (`generatedSections`), also the Add module browser's panel preview. */
export const GENERATED_CSS = `
.generated { display: grid; grid-template-columns: repeat(auto-fill, minmax(min(var(--signal-card-min, 16rem), 100%), 1fr)); gap: .75rem; }
.generated:empty { display: none; }
.generated section { min-width: 0; padding: .75rem 1rem; display: flex; flex-direction: column; gap: .6rem; }
.generated section.wide { grid-column: 1 / -1; }
.generated h2 { margin: 0; font-size: 11px; font-weight: 600; letter-spacing: .08em; text-transform: uppercase; color: var(--signal-muted, #8a93a3); white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }
.generated h2 small { font-weight: 400; letter-spacing: 0; text-transform: none; margin-left: .5em; }
.generated .row { display: flex; flex-wrap: wrap; gap: .75rem 1.25rem; align-items: flex-end; }
.generated .signal-group { display: flex; flex: none; flex-wrap: nowrap; gap: .5rem; align-items: flex-end; }
.generated signal-scope { flex: 1 1 20rem; min-width: 16rem; }
.generated signal-slot .signal-cards { display: grid; grid-template-columns: repeat(auto-fill, minmax(min(var(--signal-card-min, 16rem), 100%), 1fr)); gap: .75rem; margin-top: .6rem; }
.generated signal-slot .signal-card { min-width: 0; padding: .75rem 1rem; display: flex; flex-direction: column; gap: .6rem; border-radius: 10px; border: 1px solid var(--signal-accent, #4fc3f7); }
.generated signal-slot .signal-card > h2 { margin: 0; font-size: 11px; font-weight: 600; letter-spacing: .08em; text-transform: uppercase; color: var(--signal-muted, #8a93a3); }
.generated signal-slot .row { display: flex; flex-wrap: wrap; gap: .75rem 1.25rem; align-items: flex-end; }
.generated signal-slot .signal-group { display: flex; flex: none; gap: .5rem; align-items: flex-end; }`;

const CSS = `
:host { display: block; color: var(--signal-fg, #d6dbe4); font: 13px/1.45 -apple-system, BlinkMacSystemFont, "Helvetica Neue", sans-serif; }
/* Cards (docs/ui-controls.md "Panels"): the bar, the problems and the
   generated panel's groups are cards too, so no text is outside one. The
   bar is one line of fixed height whatever it says ("Stable layout"). */
.bar, .problems, .generated section { box-sizing: border-box; border-radius: 10px; background: var(--signal-panel, #1f232b); }
.bar { display: flex; gap: .75rem; align-items: center; height: 2rem; margin: 0 0 .75rem; padding: 0 1rem; color: var(--signal-muted, #8a93a3); font-size: 11px; white-space: nowrap; overflow: hidden; }
.bar > span { overflow: hidden; text-overflow: ellipsis; }
.status { flex: none; width: 11ch; color: var(--signal-accent, #4fc3f7); }
.status.failed, .status.closed { color: var(--signal-clip, #ff5252); }
.title { flex: none; width: 16ch; min-width: 0; }
.summary { min-width: 0; }
.problems { margin: 0 0 .75rem; padding: .5rem .75rem .5rem 2rem; border: 1px solid var(--signal-clip, #ff5252); font: 11px/1.5 ui-monospace, Menlo, monospace; }
.problems .warning { color: var(--signal-effective, #ffb74d); }
.problems .error { color: var(--signal-clip, #ff5252); }
${GENERATED_CSS}
/* ADR 0047: the plate, laid out at its own size and scaled whole into the
   viewport (one transform); the viewport takes the scaled size, so the page
   scrolls as far as the plate reaches. Unplaced, the plate is 1280 px wide,
   padded half a plate unit. */
.viewport { position: relative; }
.viewport.cropped { overflow: hidden; }
.plate { position: absolute; left: 0; top: 0; box-sizing: border-box; width: ${REFERENCE_PX}px; padding: ${GUTTER}px; transform-origin: 0 0; }
.plate.placed { padding: 0; }
.plate.placed .generated { gap: 0; }
/* The editor's marks on the plate (a card's corners, where a drag would put it), in plate px. */
.plate-overlay { position: absolute; left: 0; top: 0; width: 0; height: 0; z-index: 20; pointer-events: none; }
.plate-overlay > * { position: absolute; box-sizing: border-box; }
.plate-overlay .corner { width: 10px; height: 10px; margin: -5px 0 0 -5px; border-radius: 2px; background: var(--signal-accent, #4fc3f7); border: 1px solid var(--signal-bg, #16181d); }
.plate-overlay .outline { border: 2px dashed var(--signal-accent, #4fc3f7); border-radius: 10px; }
.plate-overlay .outline.refused { border-color: var(--signal-clip, #ff5252); background: color-mix(in srgb, var(--signal-clip, #ff5252) 12%, transparent); }
${PLATE_CSS}
/* The unsaved indicator is a box of fixed width at the bar's end, its text cut
   to its own fixed width: the count or the error changing moves nothing. */
.unsaved { flex: none; width: 46ch; margin-left: auto; display: flex; align-items: center; color: var(--signal-unsaved, #ffb74d); }
.unsaved-text { flex: none; width: 28ch; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
/* Edit layout: a switch of fixed width, its state in its colour only. */
.edit { flex: none; width: 12ch; font: inherit; color: var(--signal-fg, #d6dbe4); background: var(--signal-track, #2a2f38); border: 1px solid var(--signal-muted, #8a93a3); border-radius: 3px; cursor: pointer; overflow: hidden; text-overflow: ellipsis; }
.edit[aria-checked="true"] { border-color: var(--signal-accent, #4fc3f7); color: var(--signal-accent, #4fc3f7); }
.edit[aria-disabled="true"] { opacity: .5; cursor: default; }
.unsaved button { margin-left: .4em; font: inherit; color: var(--signal-fg, #d6dbe4); background: var(--signal-track, #2a2f38); border: 1px solid var(--signal-unsaved, #ffb74d); border-radius: 3px; cursor: pointer; }
[hidden] { display: none !important; }
${EDITOR_CSS}
`;

/**
 * The controls of `root` in document order (the check's indexes): a slot's
 * part, drawn inside its `<signal-slot>`, is a file of its own (ADR 0046).
 */
export function controlsOf(root) {
  return [...root.querySelectorAll('*')].filter((e) => e.localName.startsWith('signal-') && e.localName !== 'signal-panel' && !e.parentElement?.closest('signal-slot'));
}

export class SignalPanel extends HTMLElement {
  constructor() {
    super();
    this.attachShadow({ mode: 'open' });
    this.shadowRoot.innerHTML = `<style>${CSS}</style>
<div class="bar" part="bar"><button type="button" class="edit" role="switch" aria-checked="false" aria-disabled="true" title="Edit layout: arrange the cards and controls, written to the panel file">Edit layout</button><span class="status">starting…</span><span class="title"></span><span class="summary"></span><span class="unsaved" part="unsaved" role="status" style="visibility: hidden"><span class="unsaved-text"></span><button type="button" class="retry-all">Retry all</button><button type="button" class="revert-all">Revert all</button></span></div>
<div class="tools" part="tools" hidden></div>
<ol class="problems" part="problems" role="alert" hidden></ol>
<div class="viewport" part="viewport"><div class="plate" part="plate"><div class="generated" part="generated"></div><slot></slot><div class="plate-overlay"></div></div></div>`;
    this.connection = null;
    this.stops = [];
    /** The structure of the generated panel shown (`defaultPanel`), with `generated`. */
    this.generated = undefined;
    this.marked = [];
    this.editor = new PanelEditor(this);
    /** ADR 0047: the view's scale mode (`fitScale`'s), the scale shown, the crop (a panel window's cards, plate px) and the plate's size. */
    this.zoom = { kind: 'width' };
    this.scale = 1;
    this.crop = null;
    this.plateSize = { w: REFERENCE_PX, h: 0 };
    this.checkProblems = [];
    this.plateProblems = [];
    this.laying = 0;
  }

  static observedAttributes = ['plate'];

  attributeChangedCallback() {
    this.scheduleLayOut();
  }

  $(selector) {
    return this.shadowRoot.querySelector(selector);
  }

  /** The panel file the check is asked for. */
  get file() {
    return this.getAttribute('file') ?? panelFile(globalThis.location?.pathname);
  }

  connectedCallback() {
    this.watchPlate();
    if (this.connection) return;
    const connection = getDefaultConnection() ?? connectFromPage();
    if (!connection) {
      this.showStatus('not connected');
      this.showProblems([{ severity: 'error', text: 'no #ws=…&token=… in the URL: open the panel URL that `signal ui <project>` prints' }]);
      return;
    }
    this.connection = connection;
    this.binding = new BindingCheck({
      request: (message) => connection.request(message),
      show: (reply, error) => this.checked(reply, error),
    });
    if (getDefaultConnection() !== connection) setDefaultConnection(connection);
    this.stops.push(connection.on('status', (s) => this.showStatus(s)));
    this.stops.push(connection.on('describe', (d) => this.described(d)));
    this.stops.push(connection.on('unsaved', (u) => this.showUnsaved(u)));
    this.stops.push(connection.on('panel.written', (m) => this.written(m)));
    this.$('.edit').onclick = () => this.setEditing(!this.editor.editing);
    this.$('.retry-all').onclick = () => this.everyUnsaved((address) => connection.retry(address), (p) => p.control);
    this.$('.revert-all').onclick = () => this.everyUnsaved((address) => connection.revert(address), () => true);
    this.showStatus(connection.status);
    if (connection.description) this.described(connection.description);
  }

  /**
   * The project's unsaved parameters (docs/ui-controls.md "Unsaved"):
   * "N unsaved parameters — last error", with Retry all and Revert all.
   */
  showUnsaved(state, note = null) {
    this.unsaved = state;
    const line = unsavedLine(state);
    this.$('.unsaved-text').textContent = note ? `${line} (${note})` : line;
    this.$('.unsaved-text').title = this.$('.unsaved-text').textContent;
    // Its space is kept: the indicator appearing moves nothing.
    this.$('.unsaved').style.visibility = line ? 'visible' : 'hidden';
  }

  /** `request(address)` for every listed parameter `which` takes; a refusal is shown. */
  everyUnsaved(request, which) {
    for (const p of (this.unsaved?.parameters ?? []).filter(which)) {
      request(p.address).catch((error) => {
        if (error.code !== 'not-unsaved') this.showUnsaved(this.unsaved, `${p.address}: ${error.message}`);
      });
    }
  }

  disconnectedCallback() {
    this.editor.leave();
    for (const stop of this.stops.splice(0)) stop();
    this.observers?.forEach((o) => o.disconnect());
    this.observers = null;
  }

  // ADR 0047: the plate.

  /** Lays out the plate as its content changes, and scales it as the window does. */
  watchPlate() {
    if (this.observers) return;
    this.observers = [];
    const View = this.ownerDocument?.defaultView ?? globalThis;
    if (View.ResizeObserver) {
      const sizes = new View.ResizeObserver(() => this.fit());
      sizes.observe(this);
      sizes.observe(this.$('.plate'));
      this.observers.push(sizes);
      // Cards whose height is their content's (unplaced on a plate, the generated panel's): laid out again as they grow.
      this.cardSizes = new View.ResizeObserver(() => this.scheduleLayOut());
      this.observers.push(this.cardSizes);
    }
    if (View.MutationObserver) {
      // The cards and their places (the editor's draws, a written panel); not the styles the layout sets.
      const changes = new View.MutationObserver(() => this.scheduleLayOut());
      changes.observe(this, { childList: true, subtree: true, attributes: true, attributeFilter: ['place', 'class', 'data-card'] });
      this.observers.push(changes);
    }
    const resize = () => this.fit();
    View.addEventListener?.('resize', resize);
    this.observers.push({ disconnect: () => View.removeEventListener?.('resize', resize) });
    this.layOut();
  }

  scheduleLayOut() {
    if (this.laying) return;
    this.laying = 1;
    queueMicrotask(() => {
      this.laying = 0;
      this.layOut();
    });
  }

  /**
   * Lays the cards out on the plate: at their places with a plate, else as
   * the grid flows at the reference width; then measures it and scales it.
   */
  layOut() {
    this.laying = 0;
    const plate = this.$('.plate');
    if (!plate) return;
    const generated = this.$('.generated');
    // ADR 0047 §4: each slot's part on its own plate first (it sizes its card).
    for (const slot of this.querySelectorAll('signal-slot')) slot.layOutPart?.();
    let placed = null;
    if (this.generatedPlaces && !generated.hidden) {
      placed = layOutCards(generated, this.generatedPlate, this.generatedPlaces);
      if (this.refitGenerated(generated)) placed = layOutCards(generated, this.generatedPlate, this.generatedPlaces);
    } else layOutCards(generated, null);
    const grid = this.querySelector(':scope > .signal-cards');
    const own = layOutCards(grid, this.getAttribute('plate'));
    placed ??= own;
    this.cardSizes?.disconnect();
    for (const card of [...(own?.unplaced ?? []), ...(placed && !own ? generated.children : [])]) this.cardSizes?.observe(card);
    plate.classList.toggle('placed', Boolean(placed));
    plate.style.width = placed ? `${placed.w}px` : '';
    this.plateProblems = this.measureProblems(placed ? (own ? grid : generated) : null, placed?.unplaced ?? []);
    this.renderProblems();
    this.fit();
  }

  /**
   * The generated panel's cards made as tall as their content where the
   * packing's estimate was short (a slot's part, a skin), packed again in
   * their order: never one over the next shelf. Whether anything changed.
   */
  refitGenerated(grid) {
    const sections = [...grid.children];
    const places = sections.map((s) => this.generatedPlaces.get(s));
    if (!places.every(Boolean)) return false;
    const needs = sections.map((s) => Math.ceil((s.scrollHeight + 2 * GUTTER) / PU));
    if (!needs.some((n, i) => n > places[i].h)) return false;
    const { plate, places: packed } = pack(places.map((p, i) => ({ w: p.w, h: Math.max(p.h, needs[i]) })));
    this.generatedPlate = plateText(plate);
    this.generatedPlaces = new Map(sections.map((s, i) => [s, packed[i]]));
    return true;
  }

  /** The page's own findings about the plate (ADR 0047 §2): a card whose content is taller than its card. */
  measureProblems(grid, unplaced) {
    const problems = [];
    for (const { card } of overflowing(grid)) {
      const title = card.querySelector?.('h1, h2')?.textContent?.trim() || card.getAttribute?.('bind') || 'a card';
      problems.push({ severity: 'warning', text: `${title}: its content is taller than its card (place="${card.getAttribute('place') ?? ''}"); Edit layout makes it taller` });
    }
    if (unplaced.length && grid === this.querySelector(':scope > .signal-cards')) {
      problems.push({ severity: 'warning', text: `${unplaced.length} card${unplaced.length === 1 ? ' has' : 's have'} no place on the plate: shown below the placed ones` });
    }
    return problems;
  }

  /** The plate scaled to the view (`zoom`), cropped to `crop` in a panel window: one transform. */
  fit() {
    const plate = this.$('.plate');
    const viewport = this.$('.viewport');
    if (!plate || !viewport) return;
    const size = { w: plate.offsetWidth || REFERENCE_PX, h: plate.offsetHeight || 0 };
    this.plateSize = size;
    const crop = this.crop ?? { x: 0, y: 0, ...size };
    const width = this.clientWidth || size.w;
    const top = (viewport.getBoundingClientRect?.().top ?? 0) + (this.ownerDocument?.defaultView?.scrollY ?? 0);
    const height = Math.max(120, (this.ownerDocument?.defaultView?.innerHeight ?? 800) - Math.max(0, top) - 12);
    const s = fitScale(this.zoom, { w: width, h: height }, crop);
    this.scale = s;
    const transform = `translate(${-crop.x * s}px, ${-crop.y * s}px) scale(${s})`;
    if (plate.style.transform !== transform) plate.style.transform = transform;
    viewport.style.width = `${Math.round(crop.w * s)}px`;
    viewport.style.height = `${Math.ceil(crop.h * s)}px`;
    viewport.classList.toggle('cropped', Boolean(this.crop));
  }

  /**
   * The plate's view commands (ADR 0047 §2, ADR 0034's keymap): ⌘+ and ⌘−
   * zoom; ⌘0 the whole plate, again the width. Whether `id` was one.
   */
  plateCommand(id) {
    if (id === 'view.plate_zoom_in' || id === 'view.plate_zoom_out') this.zoom = zoomed(this.scale, id.endsWith('in') ? 1 : -1);
    else if (id === 'view.plate_whole') this.zoom = this.zoom.kind === 'whole' ? { kind: 'width' } : { kind: 'whole' };
    else return false;
    this.fit();
    return true;
  }

  /** Where the plate's origin is on screen and its scale: what turns a pointer into plate px. */
  plateOrigin() {
    const r = this.$('.plate').getBoundingClientRect();
    return { left: r.left, top: r.top, scale: this.scale || 1 };
  }

  /** Element `element`'s box in plate px (`{x, y, w, h}`). */
  plateRect(element) {
    const o = this.plateOrigin();
    const r = element.getBoundingClientRect();
    return { x: (r.left - o.left) / o.scale, y: (r.top - o.top) / o.scale, w: r.width / o.scale, h: r.height / o.scale };
  }

  /** A panel window shows only its cards (ADR 0047 §3): the plate cropped to `rect` (plate px), or whole (null). */
  setCrop(rect) {
    const same = (a, b) => (a === null && b === null) || (a && b && a.x === b.x && a.y === b.y && a.w === b.w && a.h === b.h);
    if (same(this.crop, rect ?? null)) return;
    this.crop = rect ?? null;
    this.fit();
  }

  /** The editor's plate (`"W H"` or null), shown at once. */
  setPlate(plate) {
    if ((plate ?? null) === this.getAttribute('plate')) return;
    if (plate) this.setAttribute('plate', plate);
    else this.removeAttribute('plate');
  }

  /** The layer over the plate the editor draws its marks in (plate px). */
  overlay() {
    return this.$('.plate-overlay');
  }

  /** The editor's draw: laid out now, so it can measure. */
  layOutNow() {
    this.layOut();
  }

  /**
   * Edit layout on or off (the bar's switch, Escape). Without a plan the
   * editor cannot check bindings, so it does not start, and says why.
   */
  async setEditing(on) {
    const edit = this.$('.edit');
    if (!on) {
      this.editor.leave();
      // ADR 0046 §5: the open slot cards' parts with it.
      for (const slot of this.querySelectorAll?.('signal-slot') ?? []) slot.editPart?.(false);
      edit.setAttribute('aria-checked', 'false');
      return;
    }
    if (edit.getAttribute('aria-disabled') === 'true' || this.entering) return;
    // One entry at a time: a double click must not start it twice.
    this.entering = true;
    let refused;
    try {
      refused = await this.editor.enter();
    } catch (error) {
      refused = error.message ?? String(error);
    } finally {
      this.entering = false;
    }
    if (refused) {
      edit.title = refused;
      this.showProblems([{ severity: 'warning', text: `Edit layout: ${refused}` }]);
      return;
    }
    edit.setAttribute('aria-checked', 'true');
    // ADR 0046 §5: Edit layout in an open slot card edits its part, one undo history with the host's.
    this.editor.shared = new SharedHistory(this.editor);
    for (const slot of this.querySelectorAll?.('signal-slot') ?? []) slot.editPart?.(true, this.editor.shared);
  }

  /** The switch is usable once there is a plan to check writes against. */
  editable(description) {
    const planned = Array.isArray(description?.instances) && description.instances.length > 0;
    const edit = this.$('.edit');
    edit.setAttribute('aria-disabled', String(!planned));
    edit.title = planned
      ? 'Edit layout: arrange the cards and controls, written to the panel file'
      : 'The project has no plan: Edit layout writes only what the binding check accepts';
  }

  /** Another client (or this page) wrote a panel file: shown as written. */
  written(message) {
    if (message.file !== (this.file ?? 'main.html')) return;
    if (this.editor.editing) {
      this.editor.written(message);
      return;
    }
    // The generated panel's file now exists: the page is that file.
    if (this.hasAttribute('generated')) {
      globalThis.location?.reload();
      return;
    }
    showWritten(this, message.html ?? '').then((error) => {
      if (error) this.showProblems([{ severity: 'warning', text: `the written panel is not shown: ${error}` }]);
      else {
        // ADR 0047: the plate as written.
        if (message.plate !== undefined) this.setPlate(message.plate);
        this.recheck();
      }
    });
  }

  /** The binding check again (the controls' indexes changed with a write). */
  recheck() {
    const file = this.file;
    if (!file || !this.connection?.description || !this.binding) return;
    this.binding.checked = undefined;
    this.binding.asking = undefined;
    this.binding.described(this.connection.description, file);
  }

  showStatus(status) {
    const element = this.$('.status');
    element.textContent = status;
    element.className = `status ${status}`;
  }

  /** A pushed or requested describe: check again (or rebuild) when the plan changed. */
  described(description) {
    // The name comes only while playing; the title's fixed width keeps the
    // bar still when Play and Stop set and clear it ("Stable layout").
    this.$('.title').textContent = description.project ?? '';
    if (description.unsaved) this.showUnsaved(description.unsaved);
    this.editable(description);
    if (this.hasAttribute('generated')) {
      // While it is edited the editor draws it.
      if (this.editor.editing) return;
      // Rebuilt only when what it shows changes (docs/ui-controls.md
      // "Stable layout"): a new plan version with the same controls, meters
      // and scope keeps its elements, which take the new values themselves.
      // Only what generate() builds from: not the project's name, which a
      // describe gives only while playing (Play and Stop change nothing here).
      const { groups, meters, scope } = defaultPanel(description);
      const structure = JSON.stringify({ groups, meters, scope });
      if (structure === this.generated) return;
      this.generated = structure;
      this.generate(description);
      return;
    }
    const file = this.file;
    if (!file) {
      this.showProblems([{ severity: 'warning', text: 'this page is not a project panel (/project/<file>.html): its bindings are not checked' }]);
      return;
    }
    this.binding.described(description, file);
  }

  /** The binding check's reply (or its refusal): shown in the page. */
  checked(reply, error) {
    if (error) {
      this.showProblems([{ severity: 'error', text: `the binding check failed: ${error.message}` }]);
      return;
    }
    const controls = controlsOf(this.ownerDocument);
    for (const element of this.marked.splice(0)) {
      element.style.removeProperty('outline');
      element.removeAttribute('title');
      element.removeAttribute('aria-invalid');
    }
    const problems = [];
    for (const d of reply.diagnostics ?? []) {
      const text = formatDiagnostic(d);
      problems.push({ severity: d.severity, text });
      const element = d.control === null || d.control === undefined ? null : controls[d.control];
      if (element) {
        element.style.outline = `2px solid var(${d.severity === 'error' ? '--signal-clip, #ff5252' : '--signal-effective, #ffb74d'})`;
        element.title = element.title ? `${element.title}\n${text}` : text;
        if (d.severity === 'error') element.setAttribute('aria-invalid', 'true');
        this.marked.push(element);
      }
    }
    this.$('.summary').textContent = `${reply.file}: ${reply.controls} controls, ${reply.bindings} bindings${reply.ok ? '' : ', with errors'}`;
    this.showProblems(problems);
  }

  showProblems(problems) {
    this.checkProblems = problems;
    this.renderProblems();
  }

  /** The check's problems and the plate's own (measured here). */
  renderProblems() {
    const problems = [...(this.checkProblems ?? []), ...(this.plateProblems ?? [])];
    const list = this.$('.problems');
    const key = JSON.stringify(problems);
    if (key === this.problemsKey) return;
    this.problemsKey = key;
    list.replaceChildren(
      ...problems.map(({ severity, text }) => {
        const item = document.createElement('li');
        item.className = severity;
        item.textContent = text;
        return item;
      }),
    );
    list.hidden = problems.length === 0;
  }

  /** Builds the default panel of `description`, its cards packed on the plate (ADR 0047 §2). */
  generate(description) {
    const spec = defaultPanel(description);
    const sections = generatedSections(spec);
    const { plate, places } = defaultPlate(spec);
    this.generatedPlate = plateText(plate);
    this.generatedPlaces = new Map(sections.map((s, i) => [s, places[i]]));
    this.$('.generated').replaceChildren(...sections);
    this.$('.summary').textContent = 'generated panel (the project has no ui/main.html)';
    this.layOut();
  }
}

/**
 * The generated panel's cards for `spec` (`defaultPanel`'s): a section per
 * instance, its controls in a row, and a wide output card with the sink's
 * meters and the scope. Also the Add module browser's panel preview
 * (./module-browser.js), styled by `GENERATED_CSS` in a `.generated` grid.
 */
export function generatedSections(spec) {
  const element = (name, attributes) => {
    const e = document.createElement(name);
    for (const [key, value] of Object.entries(attributes)) e.setAttribute(key, value);
    return e;
  };
  const section = (title, note, children) => {
    const s = document.createElement('section');
    const h = document.createElement('h2');
    h.textContent = title;
    if (note) {
      const small = document.createElement('small');
      small.textContent = note;
      h.append(small);
    }
    const row = document.createElement('div');
    row.className = 'row';
    row.append(...children);
    s.append(h, row);
    return s;
  };
  const sections = spec.groups.map((g) =>
    section(g.instance, g.module, g.controls.map((c) => element(c.element, { bind: c.bind, label: c.label, taper: c.taper }))),
  );
  // ADR 0037 §7: a card per slot, loaded or not.
  for (const name of spec.slots ?? []) {
    const card = document.createElement('section');
    card.className = 'slot';
    card.append(element('signal-slot', { bind: `/${name}` }));
    sections.push(card);
  }
  // The sink's meters side by side, never broken across lines.
  const meters = document.createElement('div');
  meters.className = 'signal-group';
  meters.append(...spec.meters.map((m) => element('signal-meter', { bind: m.bind, label: m.label })));
  const output = [
    ...(spec.meters.length ? [meters] : []),
    ...(spec.scope ? [element('signal-scope', { bind: spec.scope.bind, label: spec.scope.label })] : []),
  ];
  // The output card (meters and the scope) spans the grid's width.
  if (output.length) sections.push(Object.assign(section('output', null, output), { className: 'wide' }));
  return sections;
}

customElements.get('signal-panel') ?? customElements.define('signal-panel', SignalPanel);
