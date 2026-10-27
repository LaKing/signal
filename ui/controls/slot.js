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

import { browserEntries } from '../lib/module-browser.js';
import { cardTitle, contentMatch, dropLoad, loadOps, projectEntries, slotCard, unloadOp } from '../lib/slot.js';
import { SignalElement } from './base.js';
import { BROWSER_CSS, ModuleBrowser } from './module-browser.js';
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
.card { display: flex; flex-direction: column; gap: 6px; min-width: 0; }
.head { display: flex; flex-wrap: wrap; gap: 6px; align-items: baseline; min-width: 0; }
.title { flex: 1 1 12rem; min-width: 0; margin: 0; font-size: 11px; font-weight: 600; letter-spacing: 0.08em; color: var(--signal-muted, #8a93a3); white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }
.buttons { display: flex; gap: 4px; }
button { font: inherit; color: var(--signal-fg, #d6dbe4); background: var(--signal-track, #2a2f38); border: 1px solid var(--signal-knob-edge, #3a414d); border-radius: 3px; padding: 1px 6px; cursor: pointer; }
button:disabled { opacity: 0.4; cursor: default; }
button[aria-pressed="true"] { border-color: var(--signal-accent, #4fc3f7); color: var(--signal-accent, #4fc3f7); }
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
:host(.drop) .card { outline: 1px dashed var(--signal-focus, #5aa9ff); outline-offset: 4px; }
${BROWSER_CSS}`;

  constructor() {
    super();
    this.card = null;
    this.shownKey = null;
    this.levels = false;
    this.levelStops = [];
    this.busy = null;
  }

  template() {
    return `<div class="card" part="card">
  <div class="head"><h3 class="title"></h3><span class="buttons"><button type="button" class="load"></button><button type="button" class="unload">Unload</button><button type="button" class="levels" aria-pressed="false" title="Show the ports' levels (each takes one of the run's scope slots)">Levels</button></span></div>
  <div class="state" role="status"></div>
  <ul class="ports"></ul>
  <div class="params"></div>
  <div class="note"></div>
</div>`;
  }

  /** The slot's name, from `bind` (`/deck_a`). */
  get slotName() {
    return /^\/([a-z][a-z0-9_]*)$/.exec(this.getAttribute('bind')?.trim() ?? '')?.[1] ?? null;
  }

  setup() {
    this.$('.load').addEventListener('click', () => this.openLoad());
    this.$('.unload').addEventListener('click', () => this.write([unloadOp(this.slotName)], 'Unloading…'));
    this.$('.levels').addEventListener('click', () => this.setLevels(!this.levels));
    // A module name or a project path dropped on the card loads it.
    this.addEventListener('dragover', (event) => {
      event.preventDefault();
      this.classList.add('drop');
    });
    this.addEventListener('dragleave', () => this.classList.remove('drop'));
    this.addEventListener('drop', (event) => {
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

  /** The card's slot as `contentMatch` reads it: its interface, its map. */
  slotShape() {
    const slot = (this.description?.content?.slots ?? []).find((s) => s.slot === `/${this.slotName}`);
    return slot ? { interface: slot.interface, map: null } : null;
  }

  described(description) {
    this.description = description;
    this.card = slotCard(description, this.slotName);
    this.update();
  }

  streams(connection) {
    return [() => this.stopLevels(), ...(this.levels ? [this.startLevels(connection)] : [])];
  }

  say(text) {
    const state = this.$('.state');
    if (state) state.textContent = text ?? '';
  }

  update() {
    if (!this.shadowRoot.firstChild) return;
    const card = this.card;
    const title = this.$('.title');
    const load = this.$('.load');
    const unload = this.$('.unload');
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
    const key = JSON.stringify(card);
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
      ...card.parameters.map((p) => {
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
      ([types, folders]) => this.loader.setEntries([...projectEntries(folders), ...browserEntries(types)]),
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
