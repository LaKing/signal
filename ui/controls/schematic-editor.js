// The schematic's Edit patch mode (M2.9-3, ADR 0026 §6, docs/ui-controls.md
// "Editing the patch"): the switch, Undo and Redo in <signal-schematic>'s
// bar; dragging from an output to a compatible input or parameter connects
// them; a double-click on the background opens the add palette there;
// Delete or Backspace removes the selection (several parts: one patch per
// level); a wire's scale is typed in the sidebar. The context menu's edits (ui/lib/schematic-menu.js)
// come here too: Delete, Edit scale…, Disconnect all, Add module here…. Every change is a `patch` written at once
// (docs/ui-protocol.md "patch"); the drawing changes with the `graph` push
// that follows, never before. The state and the history are
// ui/lib/schematic-edit.js, the ops and rules ui/lib/schematic-patch.js.
// Off, nothing here takes a pointer or a key.

import { ValueEntry } from './entry-field.js';
import { PatchSession, patchKey } from '../lib/schematic-edit.js';
import {
  addConnection,
  addInstance,
  connectTargets,
  defaultName,
  instanceCount,
  nameError,
  namesOf,
  paletteEntries,
  removeConnection,
  removeInstance,
  scaleConnection,
} from '../lib/schematic-patch.js';
import { EXPANDED, moveCursor, paletteKey, paletteRows, placeCursor, toggled, windowRows } from '../lib/schematic-palette.js';
import { toLocal, toScreen } from '../lib/schematic-view.js';
import { cordCurve, curveData } from '../lib/schematic-wire.js';
import { formatFixed } from '../lib/format.js';

/** The rows the palette shows at once (the list keeps its height and never scrolls: the cursor moves its window). */
const PALETTE_ROWS = 12;

/** The editor's look, in <signal-schematic>'s shadow root: fixed sizes, absolutely placed. */
export const EDITOR_CSS = `
.edit { flex: none; width: 11ch; color: var(--signal-fg, #d6dbe4); background: var(--signal-track, #2a2f38); border-color: var(--signal-muted, #8a93a3); }
.edit[aria-checked="true"] { border-color: var(--signal-accent, #4fc3f7); color: var(--signal-accent, #4fc3f7); }
.history { flex: none; width: 6ch; }
.history.off { visibility: hidden; }
.pin.target, .pin.parameter.target:not(.connected) { stroke: var(--signal-focus, #5aa9ff); stroke-width: 3; }
svg.rubber { pointer-events: none; }
svg.rubber path { fill: none; stroke: var(--signal-focus, #5aa9ff); stroke-width: 1.5; stroke-dasharray: 4 3; }
.palette { position: absolute; left: 0; top: 0; width: 22rem; box-sizing: border-box; padding: 6px; visibility: hidden; z-index: 2;
  background: var(--signal-panel, #1f232b); border: 1px solid var(--signal-focus, #5aa9ff); border-radius: var(--signal-radius, 6px); }
.palette input { width: 100%; box-sizing: border-box; font: inherit; color: inherit; background: var(--signal-bg, #16181d); border: 1px solid var(--signal-knob-edge, #3a414d); border-radius: 3px; margin: 0 0 4px; }
.palette ol { list-style: none; margin: 0 0 4px; padding: 0; height: calc(${PALETTE_ROWS} * 1.6em); overflow: hidden; }
.palette li { height: 1.6em; line-height: 1.6em; white-space: nowrap; overflow: hidden; text-overflow: ellipsis; cursor: pointer; padding: 0 4px 0 1.6em; border-radius: 3px; }
.palette li.group { display: flex; gap: 0.6ch; padding-left: 4px; color: var(--signal-muted, #8a93a3); font-weight: 600; text-transform: uppercase; letter-spacing: 0.04em; font-size: 0.92em; }
.palette li.group .twisty { flex: none; width: 1.2em; text-align: center; }
.palette li.group .name { flex: 1; overflow: hidden; text-overflow: ellipsis; }
.palette li.group .count { flex: none; font-weight: normal; font-variant-numeric: tabular-nums; }
.palette li[aria-selected="true"] { background: var(--signal-knob, #262b34); color: var(--signal-accent, #4fc3f7); }
.palette li .what { color: var(--signal-muted, #8a93a3); margin-left: 1ch; }
.palette .about { height: 3.9em; line-height: 1.3em; overflow: hidden; color: var(--signal-fg, #d6dbe4); margin: 0 0 4px; overflow-wrap: anywhere; }
.palette .about .group-name { color: var(--signal-muted, #8a93a3); }
.palette .more, .palette .why { height: 1.3em; color: var(--signal-muted, #8a93a3); white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }
.palette .why { color: var(--signal-effective, #ffb74d); }
.palette .buttons { display: flex; gap: 6px; justify-content: flex-end; }
.notice { position: absolute; left: 8px; right: 8px; bottom: 28px; max-height: 30%; overflow: hidden; visibility: hidden; padding: 6px 8px; white-space: pre-wrap;
  background: var(--signal-panel, #1f232b); border: 1px solid var(--signal-effective, #ffb74d); border-radius: var(--signal-radius, 6px); }
.notice h3 { margin: 0 0 4px; font-size: 12px; color: var(--signal-effective, #ffb74d); }
.side .reach { color: var(--signal-effective, #ffb74d); margin-top: 4px; }
`;

function html(name, properties = {}) {
  return Object.assign(document.createElement(name), properties);
}

function svgEl(name, attributes = {}) {
  const node = document.createElementNS('http://www.w3.org/2000/svg', name);
  for (const [key, value] of Object.entries(attributes)) node.setAttribute(key, String(value));
  return node;
}

export class SchematicEditor {
  /** `schematic`: the <signal-schematic> it edits. */
  constructor(schematic) {
    this.s = schematic;
    this.session = new PatchSession();
    this.connecting = null;
    this.placing = null;
    this.modules = null;
    // The palette's open groups, kept for the page's session (memory only).
    this.expanded = new Set(EXPANDED);
  }

  get editing() {
    return this.session.editing;
  }

  /** Its switch and buttons at the start of `bar`, its palette, rubber band and notice in `root`. */
  build(bar, root) {
    this.toggle = html('button', { type: 'button', className: 'edit', textContent: 'Edit patch', title: 'Edit patch: add and remove instances and connections, written to the project’s files' });
    this.toggle.setAttribute('role', 'switch');
    this.toggle.setAttribute('aria-checked', 'false');
    this.toggle.addEventListener('click', () => this.setEditing(!this.editing));
    this.undoButton = html('button', { type: 'button', className: 'history off', textContent: 'Undo', title: 'Undo (⌘Z)' });
    this.redoButton = html('button', { type: 'button', className: 'history off', textContent: 'Redo', title: 'Redo (⇧⌘Z)' });
    this.undoButton.addEventListener('click', () => this.write(-1));
    this.redoButton.addEventListener('click', () => this.write(+1));
    bar.prepend(this.toggle, this.undoButton, this.redoButton);
    this.rubber = svgEl('svg', { class: 'rubber' });
    // The cord being dragged: a wire's curve, in screen pixels.
    this.cord = svgEl('path', { visibility: 'hidden' });
    this.rubber.append(this.cord);
    this.notice = html('section', { className: 'notice' });
    this.palette = this.buildPalette();
    root.append(this.rubber, this.notice, this.palette);
    this.updateButtons();
  }

  /** Edit mode on or off (the switch, Escape). */
  setEditing(on) {
    if (!this.session.setEditing(on)) return;
    this.toggle.setAttribute('aria-checked', String(on));
    this.cancelConnect();
    this.closePalette();
    if (!on) this.hideNotice();
    this.s.toggleAttribute('editing', on);
    this.updateButtons();
    this.s.showSide();
  }

  graphSeen(graph) {
    const result = this.session.graphSeen(graph);
    if (result?.cleared) this.showNotice('Undo history emptied', result.cleared);
    this.updateButtons();
  }

  updateButtons() {
    const { editing, history, pending } = this.session;
    for (const button of [this.undoButton, this.redoButton]) button.classList.toggle('off', !editing);
    this.undoButton.disabled = !editing || Boolean(pending) || !history.canUndo;
    this.redoButton.disabled = !editing || Boolean(pending) || !history.canRedo;
  }

  levelGraph(level) {
    const graph = this.s.graph;
    return level === '' ? graph?.top : graph?.composites?.[level];
  }

  /**
   * Writes `ops` at `level` (`step` 0), or the history's undo (-1) or redo
   * (+1). Resolves to whether it was written (a group's per-level patches
   * go one after the other).
   */
  write(step, level = '', ops = []) {
    if (!this.editing) return Promise.resolve(false);
    const asked = step === 0 ? this.session.request(level, ops) : step < 0 ? this.session.undo() : this.session.redo();
    if (!asked) return Promise.resolve(false);
    if (asked.refused) {
      this.s.say(asked.refused);
      return Promise.resolve(false);
    }
    this.updateButtons();
    const request = this.s.connection?.request?.(asked.message);
    if (!request) {
      this.session.refused({ message: 'Not connected' });
      this.updateButtons();
      return Promise.resolve(false);
    }
    return request.then(
      (reply) => {
        this.session.written(reply, asked.pending);
        this.hideNotice();
        this.s.say('');
        this.updateButtons();
        return true;
      },
      (error) => {
        const result = this.session.refused(error, asked.pending);
        if (result.cleared) this.showNotice('Undo history emptied', result.cleared);
        else this.showNotice('Not written', [result.reason, ...result.references.map((r) => `${r.file}${r.line ? `:${r.line}` : ''}: ${r.text}`)].join('\n'));
        this.updateButtons();
        return false;
      },
    );
  }

  showNotice(title, text) {
    this.notice.replaceChildren(html('h3', { textContent: title }), document.createTextNode(text));
    this.notice.style.visibility = 'visible';
  }

  hideNotice() {
    this.notice.style.visibility = 'hidden';
  }

  /** A key in edit mode: whether it was the editor's (Escape closes the palette, then leaves, before the level navigation). */
  key(event) {
    if (!this.editing) return false;
    const action = patchKey(event, { palette: Boolean(this.placing) });
    if (!action) return false;
    event.preventDefault();
    if (action === 'close') {
      // The hidden palette's field lets the focus go: the next Escape is the schematic's.
      this.closePalette();
      this.s.focus({ preventScroll: true });
    } else if (action === 'leave') this.setEditing(false);
    else if (action === 'undo') this.write(-1);
    else if (action === 'redo') this.write(+1);
    else if (action === 'delete') this.removeSelection();
    return true;
  }

  /** Delete or Backspace: the selection removed (several parts: one patch per level). */
  removeSelection() {
    this.s.deleteSelection();
  }

  /** Removes instance or connection `hit` (the selection, or a context menu's target). */
  remove(hit) {
    if (hit?.kind === 'instance' && hit.node.kind === 'instance') this.write(0, hit.level, [removeInstance(hit.node.name)]);
    else if (hit?.kind === 'wire' && hit.wire.kind === 'connection') this.write(0, hit.level, [removeConnection(hit.wire.written.from, hit.wire.written.to)]);
  }

  /** The sidebar's scale field opened (the context menu's Edit scale…, once the wire is selected). */
  openScale() {
    this.scaleEntry?.open();
  }

  // Drag to connect.

  /** Whether a press on `hit` starts a connection: an instance's output, in edit mode. */
  starts(hit) {
    return this.editing && hit.kind === 'port' && hit.node.kind === 'instance' && hit.port.direction === 'out';
  }

  connectStart(hit, at) {
    const source = { instance: hit.node.name, port: hit.port.name, shape: hit.port.shape };
    const targets = connectTargets(this.levelGraph(hit.level), source);
    const keys = new Set(targets.map((t) => `${hit.path}/${t.instance}.${t.port}`));
    const pins = [...(this.s.groups.get(hit.path)?.g.querySelectorAll('.pin') ?? [])].filter((pin) => keys.has(pin.dataset.address));
    for (const pin of pins) pin.classList.add('target');
    // From the pin itself, as the wire will be drawn.
    const level = this.s.levelOf(hit.path);
    const start = level ? toScreen(level.transform, hit.port) : at;
    this.connecting = { hit, keys, pins, start };
    this.connectMove(at);
  }

  connectMove(at) {
    const c = this.connecting;
    if (!c) return;
    this.cord.setAttribute('d', curveData(cordCurve(c.start, at)));
    this.cord.setAttribute('visibility', 'visible');
  }

  /** The drag ended over `hit`: a compatible input or parameter of the same level is connected. */
  connectEnd(hit) {
    const c = this.connecting;
    this.cancelConnect();
    if (!c || hit.kind !== 'port') return;
    if (hit.path !== c.hit.path || !c.keys.has(hit.address)) {
      this.s.say(`${hit.address} cannot take ${c.hit.address}`);
      return;
    }
    this.write(0, c.hit.level, [addConnection(`${c.hit.node.name}.${c.hit.port.name}`, `${hit.node.name}.${hit.port.name}`)]);
  }

  cancelConnect() {
    for (const pin of this.connecting?.pins ?? []) pin.classList.remove('target');
    this.connecting = null;
    this.cord?.setAttribute('visibility', 'hidden');
  }

  // The add palette.

  buildPalette() {
    const box = html('section', { className: 'palette' });
    this.search = html('input', { type: 'text', placeholder: 'Search modules', spellcheck: false, autocomplete: 'off' });
    this.list = html('ol', { role: 'listbox' });
    this.list.setAttribute('aria-label', 'Module types by group');
    this.about = html('div', { className: 'about' });
    this.more = html('div', { className: 'more' });
    this.name = html('input', { type: 'text', placeholder: 'name', spellcheck: false, autocomplete: 'off' });
    this.why = html('div', { className: 'why' });
    const add = html('button', { type: 'button', textContent: 'Add' });
    const cancel = html('button', { type: 'button', textContent: 'Cancel' });
    const buttons = html('div', { className: 'buttons' });
    buttons.append(cancel, add);
    box.append(this.search, this.list, this.more, this.about, this.name, this.why, buttons);
    this.search.addEventListener('input', () => this.showEntries());
    this.search.addEventListener('keydown', (event) => {
      const placing = this.placing;
      if (!placing || !this.view) return;
      const action = paletteKey(event, this.view, placing.cursor);
      if (!action) return;
      event.preventDefault();
      if (action === 'add') this.add();
      else if (action.toggle) this.toggleGroup(action.toggle);
      else if (action.move) {
        placing.cursor = moveCursor(this.view, placing.cursor, action.move);
        this.showEntries();
      }
    });
    this.name.addEventListener('keydown', (event) => {
      if (event.key === 'Enter') this.add();
    });
    this.name.addEventListener('input', () => (this.why.textContent = ''));
    add.addEventListener('click', () => this.add());
    cancel.addEventListener('click', () => this.closePalette());
    // The wheel over the list moves its window (it never scrolls).
    this.list.addEventListener('wheel', (event) => {
      if (!this.placing || !this.view) return;
      event.preventDefault();
      this.placing.top = Math.max(0, Math.min(this.view.rows.length - PALETTE_ROWS, this.placing.top + Math.sign(event.deltaY) * 3));
      this.showEntries({ follow: false });
    });
    // Its pointer and wheel are its own, not the schematic's.
    for (const type of ['pointerdown', 'pointerup', 'pointermove', 'dblclick', 'wheel']) box.addEventListener(type, (event) => event.stopPropagation());
    return box;
  }

  /** Opens the palette at screen `point` on `level` (a scene level: `paletteLevel`'s). */
  openPalette(point, level) {
    if (!this.editing || !level) return;
    this.placing = { level: level.level, at: toLocal(level.transform, point), names: namesOf(this.levelGraph(level.level)), module: null, cursor: null, top: 0 };
    const left = Math.max(0, Math.min(point.x, this.s.viewport.w - 352));
    const top = Math.max(0, Math.min(point.y, this.s.viewport.h - 420));
    this.palette.style.transform = `translate(${Math.round(left)}px, ${Math.round(top)}px)`;
    this.palette.style.visibility = 'visible';
    this.search.value = '';
    this.name.value = '';
    this.why.textContent = '';
    this.showEntries();
    this.search.focus({ preventScroll: true });
    this.s.connection?.request?.({ type: 'modules' }).then(
      (modules) => {
        this.modules = paletteEntries(modules);
        this.s.modulesSeen?.(modules);
        this.showEntries();
      },
      (error) => (this.why.textContent = error.message),
    );
  }

  /**
   * The list built again: the groups in their order, a search's matches
   * under their (open) groups, the cursor's row marked and scrolled into
   * view (unless `follow` is false: the wheel moved the window), and what
   * the chosen type is under it.
   */
  showEntries({ follow = true } = {}) {
    const placing = this.placing;
    if (!placing) return;
    const view = paletteRows(this.modules ?? [], this.search.value, this.expanded);
    this.view = view;
    placing.cursor = placeCursor(view, placing.cursor);
    const row = view.rows.find((r) => r.key === placing.cursor);
    // The cursor on an entry chooses it; on a heading, the last choice stays.
    if (row?.kind === 'entry') {
      if (row.entry.module !== placing.module) this.choose(row.entry);
    } else if (!row && placing.module !== null) this.choose(null);
    // The window of rows shown, moved to keep the cursor's in sight, its group's heading first.
    const { top, shown, above, below } = windowRows(view.rows, follow ? view.rows.indexOf(row) : -1, placing.top, PALETTE_ROWS);
    placing.top = top;
    this.list.replaceChildren(
      ...shown.map((r) => {
        const item = r.kind === 'group' ? this.groupItem(r, view) : this.entryItem(r.entry);
        item.setAttribute('aria-selected', String(r.key === placing.cursor));
        return item;
      }),
    );
    const hidden = above || below ? ` (${[above ? `${above} above` : '', below ? `${below} below` : ''].filter(Boolean).join(', ')})` : '';
    const chosen = this.modules?.find((e) => e.module === placing.module) ?? null;
    this.about.replaceChildren(
      ...(chosen ? [html('span', { className: 'group-name', textContent: `${chosen.group} · ` }), document.createTextNode(chosen.description ?? (chosen.composite ? 'a composite of this project' : 'no description'))] : []),
    );
    this.about.title = chosen?.description ?? '';
    this.more.textContent =
      this.modules === null
        ? 'Loading…'
        : view.searching
          ? view.matched
            ? `${view.matched} match${view.matched === 1 ? '' : 'es'}${hidden}`
            : 'No module matches'
          : `${view.matched} types${hidden}: ↑↓ move, Enter adds or opens`;
  }

  groupItem(r, view) {
    const item = html('li', { className: 'group' });
    item.setAttribute('role', 'option');
    if (!view.searching) item.setAttribute('aria-expanded', String(r.expanded));
    item.append(html('span', { className: 'twisty', textContent: r.expanded ? '▾' : '▸' }), html('span', { className: 'name', textContent: r.group }), html('span', { className: 'count', textContent: String(r.count) }));
    item.addEventListener('click', () => {
      this.placing.cursor = r.key;
      if (!view.searching) this.toggleGroup(r.group);
      else this.showEntries();
      this.search.focus({ preventScroll: true });
    });
    return item;
  }

  entryItem(entry) {
    const item = html('li', { title: entry.label });
    item.setAttribute('role', 'option');
    item.append(document.createTextNode(entry.module), html('span', { className: 'what', textContent: entry.label }));
    item.addEventListener('click', () => {
      this.placing.cursor = `m:${entry.module}`;
      this.choose(entry);
      this.showEntries();
      this.search.focus({ preventScroll: true });
    });
    return item;
  }

  /** Group `group` opened or closed (kept for the session). */
  toggleGroup(group) {
    this.expanded = toggled(this.expanded, group);
    this.showEntries();
  }

  /** `entry` chosen: its default name, editable before adding. */
  choose(entry) {
    const placing = this.placing;
    placing.module = entry?.module ?? null;
    this.name.value = entry ? defaultName(entry.module, placing.names) : '';
  }

  add() {
    const placing = this.placing;
    if (!placing?.module) return;
    const name = this.name.value.trim();
    const error = nameError(name, placing.names);
    if (error) {
      this.why.textContent = error;
      return;
    }
    this.closePalette();
    // The hidden palette's field lets the focus go: ⌘Z, Delete and Escape are the schematic's again.
    this.s.focus({ preventScroll: true });
    this.write(0, placing.level, [addInstance(name, placing.module, placing.at)]);
  }

  closePalette() {
    this.placing = null;
    if (this.palette) this.palette.style.visibility = 'hidden';
  }

  // The sidebar.

  /** What the sidebar adds in edit mode: a wire's scale entry; inside a composite, how many instances the edit reaches. */
  cardParts(selection) {
    this.scaleEntry = null;
    if (!this.editing || !selection) return [];
    const parts = [];
    if (selection.level && selection.level !== '' && selection.kind !== 'background') {
      const count = instanceCount(this.s.graph, selection.level);
      parts.push(html('div', { className: 'reach', textContent: `edits every instance of ${selection.level}: ${count}` }));
    }
    if (selection.kind === 'wire' && selection.wire.kind === 'connection') {
      const { from, to } = selection.wire.written;
      const scale = selection.wire.scale ?? 1;
      const row = html('div', { className: 'row' });
      const readout = html('span', { className: 'readout', title: 'scale: double-click to type', textContent: formatFixed(scale, null) });
      row.append(html('span', { textContent: 'scale' }), readout);
      this.scaleEntry = new ValueEntry(readout, {
        spec: () => ({ unit: null, range: {}, taper: 'linear', kind: null }),
        value: () => scale,
        apply: (value) => {
          if (Number.isFinite(value)) this.write(0, selection.level, [scaleConnection(from, to, value)]);
        },
        note: (text) => this.s.say(text ?? ''),
        closed: () => {},
        focus: () => this.s.focus({ preventScroll: true }),
      });
      parts.push(row);
    }
    return parts;
  }
}
