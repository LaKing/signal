// <signal-slot bind="/deck_a">: a slot's card on the panel (ADR 0037 §7,
// phase 3; docs/ui-controls.md "Slots"). Its header names the slot, its
// interface (the template, or "slot") and what it holds (a project with ↗,
// or "empty"), with Load… or Replace… and Unload, which write the slot's
// `load` in project.toml as the schematic's menu does (a `patch`, text
// first). Its body lists the interface's ports, each matched (filled) or
// not (hollow, the reason as its tooltip), with a level while Levels is on,
// and the interface's parameters as knobs, which write the slot's entry. A
// module name or a project path dropped on it loads it. What it shows comes
// from `describe` (`content.slots`, `controls`); the rest is
// ui/lib/slot.js's.
//
// ADR 0046 §5: the slot's own part (`ui/parts/<slot>.html`, read with
// `panel.read`) is drawn under it, in its light DOM, its binds rooted at the
// slot (ui/lib/parts.js), so the page's own CSS styles its cards. Placed as
// a card (a direct child of `div.signal-cards`) it is the **open slot
// card**: the header is a card of the host's grid and the part's cards are
// cards of that grid too, framed in the accent colour; placed in a row it is
// the compact card, its part's cards beneath it. Either folds (▾ / ▸; view
// state, open by default, not written). With a part, the header shows no
// interface knobs (the part has its controls); without one, as before. In
// the host's Edit layout the part is edited here (a panel editor of its
// own, the history shared with the host's).

import { browserEntries } from '../lib/module-browser.js';
import { BindingCheck, formatDiagnostic } from '../lib/panel.js';
import { emptyPartTree, opensAsCard, partFile, partPalette, rootAddress, rootedAttribute } from '../lib/parts.js';
import { cardTitle, contentMatch, dropLoad, loadOps, loadable, projectEntries, slotCard, unloadOp } from '../lib/slot.js';
import { SignalElement } from './base.js';
import { BROWSER_CSS, ModuleBrowser } from './module-browser.js';
import { EDITOR_CSS, PanelEditor, showWritten } from './panel-editor.js';
import './knob.js';

/** At most this many ports show a level at once (each takes one of the run's scope slots). */
export const LEVEL_PORTS = 4;

/** The levels' rate: a decimated scope, enough for a lamp. */
const LEVEL_RATE = 480;

export class SignalSlot extends SignalElement {
  static role = 'group';
  static observedAttributes = ['bind', 'skin'];
  static css = `
:host { display: block; min-width: 0; }
/* ADR 0046 §5: the open slot card: its header and its part's cards are cards of the host's grid. */
:host([data-card]) { display: contents; }
:host([data-card]) .card { box-sizing: border-box; min-width: 0; padding: .75rem 1rem; border-radius: 10px; background: var(--signal-panel, #1f232b); border: 1px solid var(--signal-accent, #4fc3f7); }
.card { display: flex; flex-direction: column; gap: 6px; min-width: 0; }
.head { display: flex; flex-wrap: wrap; gap: 6px; align-items: baseline; min-width: 0; }
.title { flex: 1 1 12rem; min-width: 0; margin: 0; font-size: 11px; font-weight: 600; letter-spacing: 0.08em; color: var(--signal-muted, #8a93a3); white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }
.buttons { display: flex; gap: 4px; }
button { font: inherit; color: var(--signal-fg, #d6dbe4); background: var(--signal-track, #2a2f38); border: 1px solid var(--signal-knob-edge, #3a414d); border-radius: 3px; padding: 1px 6px; cursor: pointer; }
button:disabled { opacity: 0.4; cursor: default; }
button[aria-pressed="true"] { border-color: var(--signal-accent, #4fc3f7); color: var(--signal-accent, #4fc3f7); }
.fold { flex: none; width: 2.2em; }
.state { min-height: 1.3em; color: var(--signal-effective, #ffb74d); white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }
.ports { list-style: none; margin: 0; padding: 0; display: grid; grid-template-columns: repeat(auto-fill, minmax(9rem, 1fr)); gap: 2px 8px; }
.ports li { display: flex; gap: 6px; align-items: center; min-width: 0; white-space: nowrap; }
.dot { flex: none; width: 8px; height: 8px; border-radius: 50%; box-sizing: border-box; border: 1.5px solid var(--signal-accent, #4fc3f7); background: var(--signal-accent, #4fc3f7); }
.dot.events { border-color: var(--signal-meter-rms, #3ecf8e); background: var(--signal-meter-rms, #3ecf8e); }
.dot.vector { border-color: var(--signal-effective, #ffb74d); background: var(--signal-effective, #ffb74d); }
.dot.unmatched { background: transparent; border-style: dashed; border-color: var(--signal-muted, #8a93a3); }
.name { overflow: hidden; text-overflow: ellipsis; }
.dir { color: var(--signal-muted, #8a93a3); }
.level { flex: none; width: 28px; height: 3px; background: var(--signal-track, #2a2f38); position: relative; overflow: hidden; }
.level > i { position: absolute; left: 0; top: 0; bottom: 0; width: 0; background: var(--signal-meter-rms, #3ecf8e); }
.params { display: flex; flex-wrap: wrap; gap: 8px; }
.empty-note, .note { color: var(--signal-muted, #8a93a3); }
.problems { margin: 0; padding: 0 0 0 1.2em; font: 11px/1.5 ui-monospace, Menlo, monospace; }
.problems .error { color: var(--signal-clip, #ff5252); }
.problems .warning { color: var(--signal-effective, #ffb74d); }
.card > .generated { display: none !important; }
:host(.drop) .card { outline: 1px dashed var(--signal-focus, #5aa9ff); outline-offset: 4px; }
${EDITOR_CSS}
${BROWSER_CSS}`;

  constructor() {
    super();
    this.card = null;
    this.shownKey = null;
    this.levels = false;
    this.levelStops = [];
    this.busy = null;
    /**
     * ADR 0046: the part as last read (`{exists, html, error}`), and whether it is folded.
     * Never `this.part`: an element's `part` is the DOM's own (`::part()` names), and setting it writes the attribute.
     */
    this.partState = null;
    this.folded = false;
    this.partEditor = null;
    this.partReading = false;
    this.partCheck = null;
    this.marked = [];
    /** For the panel editor (ui/controls/panel-editor.js): this element edits a part. */
    this.isPart = true;
  }

  template() {
    return `<div class="card" part="card">
  <div class="head"><button type="button" class="fold" aria-expanded="true" title="Fold the slot's part">▾</button><h3 class="title"></h3><span class="buttons"><button type="button" class="load"></button><button type="button" class="unload">Unload</button><button type="button" class="levels" aria-pressed="false" title="Show the ports' levels (each takes one of the run's scope slots)">Levels</button></span></div>
  <div class="state" role="status"></div>
  <ul class="ports"></ul>
  <div class="params"></div>
  <div class="note"></div>
  <ol class="problems" hidden></ol>
  <div class="tools" data-part-tools hidden></div>
  <div class="message" hidden></div>
  <div class="generated"></div>
</div><slot></slot>`;
  }

  /** The slot's name, from `bind` (`/deck_a`). */
  get slotName() {
    return /^\/([a-z][a-z0-9_]*)$/.exec(this.getAttribute('bind')?.trim() ?? '')?.[1] ?? null;
  }

  /** ADR 0046: the slot's part, relative to the project's `ui/` (what the panel editor reads and writes). */
  get file() {
    return this.slotName ? partFile(this.slotName) : null;
  }

  /** The part's root: the slot's address. */
  get root() {
    return `/${this.slotName}`;
  }

  /** What a part's control attribute shows: its addresses rooted (the panel editor's hook). */
  attributeValue(name, value) {
    return rootedAttribute(name, value, this.root);
  }

  /** A part's address as the plan names it (the panel editor's range lookup). */
  absoluteAddress(address) {
    return rootAddress(address, this.root) ?? address;
  }

  /** The panel editor's palette in a part: the slot's addresses, relative. */
  paletteEntries(entries) {
    return partPalette(entries, this.root);
  }

  /** The tree a part starts from: the interface's parameters as knobs. */
  emptyTree() {
    return emptyPartTree(this.card);
  }

  /** The card box the app orders and hides (ui/app/lib/cards.js): the open slot card's own box. */
  cardBox() {
    return this.hasAttribute('data-card') ? this.$('.card') : this;
  }

  setup() {
    this.$('.load').addEventListener('click', () => this.openLoad());
    this.$('.unload').addEventListener('click', () => this.write([unloadOp(this.slotName)], 'Unloading…'));
    this.$('.levels').addEventListener('click', () => this.setLevels(!this.levels));
    this.$('.fold').addEventListener('click', () => this.setFolded(!this.folded));
    // A module name or a project path dropped on the card (its header, not its part) loads it.
    const onCard = (event) => event.composedPath?.().includes(this.$('.card')) ?? true;
    this.addEventListener('dragover', (event) => {
      if (!onCard(event)) return;
      event.preventDefault();
      this.classList.add('drop');
    });
    this.addEventListener('dragleave', () => this.classList.remove('drop'));
    this.addEventListener('drop', (event) => {
      if (!onCard(event)) return;
      event.preventDefault();
      this.classList.remove('drop');
      const data = event.dataTransfer;
      const uri = data?.getData?.('text/uri-list')?.split(/\r?\n/).find((l) => l && !l.startsWith('#')) ?? '';
      const text = uri.startsWith('file://') ? decodeURIComponent(new URL(uri).pathname).replace(/\/$/, '') : data?.getData?.('text/plain');
      const load = dropLoad(text);
      if (load) this.write(loadOps(this.slotName, load), `Loading ${load}…`);
      else this.say(`Not loaded: a dropped module name or project folder loads (${String(text ?? '').slice(0, 40) || 'nothing'} is neither)`);
    });
    this.loader = new ModuleBrowser({
      mode: 'load',
      add: (module, _name, options) => this.loadChosen(module, options),
      closed: () => this.focus?.({ preventScroll: true }),
      graph: () => null,
      defaultName: () => '',
      matchOf: (entry) => contentMatch(entry, this.slotShape()),
    });
    this.shadowRoot.append(this.loader.build(['s:project', 's:library', 's:folders', 's:std']));
  }

  connectedCallback() {
    super.connectedCallback();
    // ADR 0046 §5: a card of the host's grid opens as a card; in a row, compact.
    this.toggleAttribute('data-card', opensAsCard(this.parentElement));
  }

  /** The card's slot as `contentMatch` reads it: its interface, its map. */
  slotShape() {
    const slot = (this.description?.content?.slots ?? []).find((s) => s.slot === `/${this.slotName}`);
    return slot ? { interface: slot.interface, map: null } : null;
  }

  described(description) {
    this.description = description;
    this.card = slotCard(description, this.slotName);
    this.update();
    // Described means open: a part not read yet (or whose read the connection dropped) is read now.
    if (this.partState === null && !this.partReading) this.readPart();
    if (this.partState?.exists && this.partCheck && this.file) this.partCheck.described(description, this.file);
  }

  streams(connection) {
    const stops = [() => this.stopLevels(), ...(this.levels ? [this.startLevels(connection)] : [])];
    // ADR 0046: the part, read now and again whenever it is written.
    this.partCheck = new BindingCheck({
      request: (message) => connection.request(message),
      show: (reply, error) => this.partChecked(reply, error),
    });
    if (connection.on) {
      stops.push(connection.on('panel.written', (message) => this.partWritten(message)));
      // A request before the connection is open is refused (`closed`): the
      // part is read once it opens (the operator's check, 2026-10-30: a page
      // attached its controls while connecting, and its read failed).
      stops.push(
        connection.on('status', (status) => {
          if (status === 'open' && !this.partEditor?.editing) this.readPart(connection);
        }),
      );
    }
    if (connection.status === undefined || connection.status === 'open') this.readPart(connection);
    return stops;
  }

  say(text) {
    const state = this.$('.state');
    if (state) state.textContent = text ?? '';
  }

  // ADR 0046 §5: the part.

  /** Reads the part (`panel.read`) and shows it; none, the slot card as before. */
  async readPart(connection = this.connection) {
    const file = this.file;
    if (this.partEditor?.editing || !file) return;
    // A connection that reads no file (the gallery's): no part, the card as before.
    if (!connection?.request) {
      this.partState = { exists: false, html: null, error: null };
      this.update();
      return;
    }
    let read;
    this.partReading = true;
    try {
      read = await connection.request({ type: 'panel.read', file });
    } catch (error) {
      // The connection dropped (or was not open yet): not read; again when it opens.
      if (error?.code === 'closed') return;
      // No part (`unknown-panel` for a slot the server does not know yet):
      // the card as before; any other failure is said, not hidden by the
      // interface's knobs.
      this.partState = { exists: false, html: null, error: error?.code === 'unknown-panel' ? null : (error?.message ?? String(error)) };
      this.showPart();
      return;
    } finally {
      this.partReading = false;
    }
    if (file !== this.file) return;
    this.partState = { exists: Boolean(read?.exists), html: read?.html ?? null, error: read?.exists && !read?.editable ? read.reason : null };
    await this.showPart();
    if (this.partState.exists && this.description) {
      this.partCheck.checked = undefined;
      this.partCheck.asking = undefined;
      this.partCheck.described(this.description, file);
    }
  }

  /** Draws the part as last read into the light DOM, rooted. */
  async showPart() {
    if (this.partEditor?.editing) return;
    if (!this.partState?.exists || this.partState.html === null) {
      this.replaceChildren();
    } else {
      const error = await showWritten(this, this.partState.html, (name, value) => this.attributeValue(name, value));
      if (error) this.partState.error = error;
    }
    this.showProblems(this.partState?.error ? [{ severity: 'error', text: `${this.file}: ${this.partState.error}` }] : []);
    this.shownKey = null;
    this.update();
  }

  /** A `panel.written` push: the part another client (or a patch) wrote. */
  partWritten(message) {
    if (message?.file !== this.file) return;
    if (this.partEditor?.editing) {
      this.partEditor.written(message);
      return;
    }
    this.partState = message.html === null || message.html === undefined ? { exists: false, html: null, error: null } : { exists: true, html: message.html, error: null };
    this.showPart().then(() => this.recheck());
  }

  /** The part's binding check again (the panel editor asks after a write). */
  recheck() {
    if (!this.partCheck || !this.connection?.description || !this.file || !this.partState?.exists) return;
    this.partCheck.checked = undefined;
    this.partCheck.asking = undefined;
    this.partCheck.described(this.connection.description, this.file);
  }

  /** The part's `panel.check`: its problems listed in the card, its controls marked. */
  partChecked(reply, error) {
    for (const element of this.marked.splice(0)) {
      element.style.removeProperty('outline');
      element.removeAttribute('aria-invalid');
    }
    if (error) {
      this.showProblems([{ severity: 'error', text: `${this.file}: the binding check failed: ${error.message}` }]);
      return;
    }
    const controls = [...this.querySelectorAll('*')].filter((e) => e.localName.startsWith('signal-'));
    const problems = [];
    for (const d of reply?.diagnostics ?? []) {
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
    this.showProblems(problems);
  }

  showProblems(problems) {
    const list = this.$('.problems');
    if (!list) return;
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

  /** Folds the part away or opens it (view state). */
  setFolded(folded) {
    this.folded = folded;
    const fold = this.$('.fold');
    fold.textContent = folded ? '▸' : '▾';
    fold.setAttribute('aria-expanded', String(!folded));
    this.$('slot').hidden = folded;
  }

  /**
   * The host's Edit layout entered (`on`) or left: the part edited here,
   * its history `shared` with the host's (one undo across the files). A
   * slot whose part cannot be edited says why.
   */
  async editPart(on, shared = null) {
    if (!on) {
      if (this.partEditor?.editing) {
        this.partEditor.leave();
        this.removeAttribute('editing');
        this.removeAttribute('no-entry');
        this.readPart();
      }
      return;
    }
    if (!this.slotName || !this.connection) return;
    this.setFolded(false);
    this.partEditor ??= new PanelEditor(this);
    this.partEditor.shared = shared;
    let refused;
    try {
      refused = await this.partEditor.enter();
    } catch (error) {
      refused = error?.message ?? String(error);
    }
    if (refused) this.say(`Edit layout of ${this.file}: ${refused}`);
  }

  /** The panel editor's Escape: the host's Edit layout ends. */
  setEditing(on) {
    this.closest?.('signal-panel')?.setEditing?.(on);
  }

  update() {
    if (!this.shadowRoot.firstChild) return;
    const card = this.card;
    const title = this.$('.title');
    const load = this.$('.load');
    const unload = this.$('.unload');
    const fold = this.$('.fold');
    if (fold) fold.hidden = !this.partState?.exists && !this.partEditor?.editing;
    if (!card) {
      title.textContent = this.slotName ? `${this.slotName.toUpperCase()}: no such slot` : 'no slot bound';
      load.textContent = 'Load…';
      load.disabled = true;
      unload.disabled = true;
      this.$('.ports').replaceChildren();
      this.$('.params').replaceChildren();
      this.$('.note').textContent = this.description ? `The project has no slot ${this.getAttribute('bind') ?? ''}` : '';
      return;
    }
    title.textContent = cardTitle(card);
    title.title = cardTitle(card);
    load.textContent = card.empty ? 'Load…' : 'Replace…';
    load.disabled = Boolean(this.busy);
    unload.disabled = card.empty || Boolean(this.busy);
    if (!this.hasAttribute('aria-label')) this.setAttribute('aria-label', `slot ${card.name}`);
    // ADR 0046 §5: with a part, its controls are the part's; without, the interface's knobs.
    // The interface's knobs only once it is known that the slot has no part:
    // not while its part is read, nor when reading it failed (said above).
    const withPart = this.partState === null || Boolean(this.partState.exists) || Boolean(this.partState.error);
    const key = JSON.stringify([card, withPart]);
    if (key === this.shownKey) return;
    this.shownKey = key;
    this.$('.ports').replaceChildren(
      ...card.ports.map((p) => {
        const li = document.createElement('li');
        li.title = p.matched ? `${p.address}: ${p.kind}` : `${p.address}: unmatched, ${p.reason}`;
        const dot = document.createElement('span');
        dot.className = ['dot', p.shape, p.matched ? '' : 'unmatched'].filter(Boolean).join(' ');
        const name = document.createElement('span');
        name.className = 'name';
        name.textContent = p.name;
        const dir = document.createElement('span');
        dir.className = 'dir';
        dir.textContent = p.role === 'output' ? 'out' : 'in';
        const level = document.createElement('span');
        level.className = 'level';
        level.dataset.address = p.address;
        level.hidden = !(this.levels && p.matched && p.shape === 'signal');
        level.append(document.createElement('i'));
        li.append(dot, name, dir, level);
        return li;
      }),
    );
    const connection = this.connection;
    this.$('.params').replaceChildren(
      ...(withPart ? [] : card.parameters).map((p) => {
        if (!p.control) {
          const span = document.createElement('span');
          span.className = 'empty-note';
          span.textContent = `${p.name}: ${p.reason ?? 'unmatched'}`;
          return span;
        }
        const knob = document.createElement('signal-knob');
        knob.setAttribute('bind', p.address);
        knob.setAttribute('label', p.name);
        if (this.ownConnection) knob.useConnection?.(this.ownConnection);
        return knob;
      }),
    );
    const notes = [];
    if (card.empty) notes.push('Empty: its wires read 0. Load… or drop a module name or a project folder here.');
    if (card.unheard.length) notes.push(`${card.unheard.length} output${card.unheard.length === 1 ? '' : 's'} not in this slot: unheard (${card.unheard.join(', ')})`);
    this.$('.note').textContent = notes.join(' ');
    if (connection && this.levels) this.attach(connection);
  }

  // Load, Replace, Unload: `patch`es of project.toml, against its current SHA-256.

  /** Writes `ops` into project.toml (base from `file.read`), saying `doing` meanwhile. */
  async write(ops, doing) {
    const connection = this.connection;
    if (!connection?.request || !this.slotName) {
      this.say('Not connected');
      return false;
    }
    this.busy = doing;
    this.say(doing);
    this.update();
    try {
      const read = await connection.request({ type: 'file.read', file: 'project.toml' });
      const reply = await connection.request({ type: 'patch', level: '', base: read.base, ops });
      // Until ADR 0037 phase 2, a Load or Replace is one structural edit: the reload applies it.
      this.say(reply?.notes?.length ? reply.notes.join('; ') : '');
      return true;
    } catch (error) {
      this.say(`Not written: ${error?.message ?? error}`);
      return false;
    } finally {
      this.busy = null;
      this.update();
    }
  }

  /** Load… or Replace…: the Load dialog (the schematic's), its entries the module types and the projects beside this one. */
  openLoad() {
    const connection = this.connection;
    if (!this.card || !connection?.request) return;
    this.loader.open({ where: 'at the top level', entries: null, title: this.card.empty ? `Load into ${this.card.name}` : `Replace ${this.card.content} in ${this.card.name}` });
    Promise.all([connection.request({ type: 'modules' }), connection.request({ type: 'slot.projects' }).catch(() => ({ projects: [] }))]).then(
      ([types, folders]) => this.loader.setEntries(loadable([...projectEntries(folders), ...browserEntries(types)], this.card.name)),
      (error) => this.loader.failed(error?.message ?? String(error)),
    );
  }

  loadChosen(module, { addUnmatched = false, entry = null } = {}) {
    const add = addUnmatched ? contentMatch(entry, this.slotShape()).unmatched : [];
    this.loader.close();
    this.write(loadOps(this.slotName, module, { add }), `${this.card?.empty ? 'Loading' : 'Replacing with'} ${module}…`);
    return null;
  }

  // Levels: a scope stream per matched signal port shown, at most LEVEL_PORTS.

  setLevels(on) {
    this.levels = on;
    this.$('.levels').setAttribute('aria-pressed', String(on));
    this.shownKey = null;
    if (this.connection) this.attach(this.connection);
    else this.update();
  }

  startLevels(connection) {
    const ports = (this.card?.ports ?? []).filter((p) => p.matched && p.shape === 'signal').slice(0, LEVEL_PORTS);
    const stops = ports.map((p) =>
      connection.subscribe(
        { stream: 'scope', port: p.address, rate: LEVEL_RATE },
        (frame) => {
          const peak = Math.max(0, ...(frame.values ?? []).map((v) => Math.abs(v)));
          const fill = this.shadowRoot.querySelector(`.level[data-address="${p.address}"] > i`);
          if (fill) fill.style.width = `${Math.round(Math.min(1, peak) * 100)}%`;
        },
        { onError: (error) => this.say(`${p.name}: no level (${error?.message ?? error})`) },
      ),
    );
    this.levelStops = stops;
    return () => this.stopLevels();
  }

  stopLevels() {
    for (const stop of this.levelStops.splice(0)) stop?.();
  }
}

customElements.get('signal-slot') ?? customElements.define('signal-slot', SignalSlot);
