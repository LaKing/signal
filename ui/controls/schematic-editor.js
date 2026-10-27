// The schematic's Edit patch mode (M2.9-3, ADR 0026 §6, docs/ui-controls.md
// "Editing the patch"): the switch, Undo and Redo in <signal-schematic>'s
// bar; dragging from an output to a compatible input or parameter connects
// them; a double-click on the background opens the Add module browser
// (./module-browser.js) for there;
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
  removeConnection,
  removeInstance,
  scaleConnection,
} from '../lib/schematic-patch.js';
import { EXPANDED, browserEntries } from '../lib/module-browser.js';
import { projectKey } from '../lib/schematic-palette.js';
import { BROWSER_CSS, ModuleBrowser } from './module-browser.js';
import { toLocal, toScreen } from '../lib/schematic-view.js';
import { editLevel } from '../lib/schematic-open.js';
import { cordCurve, curveData } from '../lib/schematic-wire.js';
import { formatFixed } from '../lib/format.js';

/** The editor's look, in <signal-schematic>'s shadow root: fixed sizes, absolutely placed. */
export const EDITOR_CSS = `
.edit { flex: none; width: 11ch; color: var(--signal-fg, #d6dbe4); background: var(--signal-track, #2a2f38); border-color: var(--signal-muted, #8a93a3); }
.edit[aria-checked="true"] { border-color: var(--signal-accent, #4fc3f7); color: var(--signal-accent, #4fc3f7); }
.history { flex: none; width: 6ch; }
.history.off { visibility: hidden; }
.pin.target, .pin.parameter.target:not(.connected) { stroke: var(--signal-focus, #5aa9ff); stroke-width: 3; }
svg.rubber { pointer-events: none; }
svg.rubber path { fill: none; stroke: var(--signal-focus, #5aa9ff); stroke-width: 1.5; stroke-dasharray: 4 3; }
.notice { position: absolute; left: 8px; right: 8px; bottom: 28px; max-height: 30%; overflow: hidden; visibility: hidden; padding: 6px 8px; white-space: pre-wrap;
  background: var(--signal-panel, #1f232b); border: 1px solid var(--signal-effective, #ffb74d); border-radius: var(--signal-radius, 6px); }
.notice h3 { margin: 0 0 4px; font-size: 12px; color: var(--signal-effective, #ffb74d); }
.side .reach { color: var(--signal-effective, #ffb74d); margin-top: 4px; }
${BROWSER_CSS}`;

/** Which `modules` reply a graph's project would give: its project and the hashes of its files. */
function modulesKey(graph) {
  return graph ? `${projectKey(graph)}\n${JSON.stringify(graph.files ?? null)}` : null;
}

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
    this.browser = new ModuleBrowser({
      add: (module, name) => this.add(module, name),
      closed: () => this.browserClosed(),
      graph: () => this.s.graph,
      defaultName: (module) => (this.placing ? defaultName(module, this.placing.names) : ''),
    });
  }

  get editing() {
    return this.session.editing;
  }

  /** Its switch and buttons at the start of `bar`, its Add module browser, rubber band and notice in `root`. */
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
    // The browser's open branches are kept for the page's session (memory only).
    this.palette = this.browser.build(EXPANDED);
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
    // Only the innermost open level is edited (M2.13-3), undo and redo included (the entry stays for when it is shown).
    const shown = editLevel(this.s.drawn);
    const target = this.session.levelOf(step, level);
    if (target !== null && target !== shown.level) {
      this.s.say(`Not ${step < 0 ? 'undone' : step > 0 ? 'redone' : 'edited'}: ${target || 'the top level'} is around the open ${shown.path || 'level'}; ${step ? 'go out to it first' : 'edit it from there'}`);
      return Promise.resolve(false);
    }
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

  // The Add module browser.

  /** Whether `event` happened in the Add module browser (its wheel scrolls its panes). */
  holds(event) {
    return this.browser.holds(event);
  }

  /** Opens the Add module browser for screen `point` on `level` (a scene level: `paletteLevel`'s). */
  openPalette(point, level) {
    if (!this.editing || !level) return;
    this.placing = { level: level.level, at: toLocal(level.transform, point), names: namesOf(this.levelGraph(level.level)) };
    // The last reply's types show at once while the project's files are the same; the reply that follows replaces them.
    const key = modulesKey(this.s.graph);
    const known = this.modulesKnown?.key === key ? this.modulesKnown.entries : null;
    this.browser.open({ where: level.level ? `inside ${level.level}` : 'at the top level', entries: known });
    this.s.connection?.request?.({ type: 'modules' }).then(
      (modules) => {
        const entries = browserEntries(modules);
        this.modulesKnown = { key, entries };
        if (this.placing) this.browser.setEntries(entries);
        this.s.modulesSeen?.(modules);
      },
      (error) => this.browser.failed(error.message),
    );
  }

  /** Adds `module` named `name` where the browser was opened: an error text when the name is refused, else null. */
  add(module, name) {
    const placing = this.placing;
    if (!placing || !module) return null;
    const error = nameError(name, placing.names);
    if (error) return error;
    this.closePalette();
    this.write(0, placing.level, [addInstance(name, module, placing.at)]);
    return null;
  }

  /** The browser closed (Cancel, Escape, Add): the focus back on the schematic, so ⌘Z, Delete and Escape are its own again. */
  browserClosed() {
    this.placing = null;
    this.s.focus?.({ preventScroll: true });
  }

  /** Closes the browser (Add, Edit patch left, Escape). */
  closePalette() {
    this.placing = null;
    this.browser.close();
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
