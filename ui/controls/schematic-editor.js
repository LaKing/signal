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
import { SLOT_DIALOG_CSS, SlotDialogs } from './slot-dialogs.js';
import {
  addSlotOp,
  contentMatch,
  detachOp,
  exportOp,
  frameOutputs,
  frameTargets,
  insideOp,
  insideOps,
  insideOwner,
  insideSlot,
  instancePorts,
  isPreviewLevel,
  isProjectPath,
  isSlotInside,
  levelData,
  loadable,
  loadOps,
  mapRows,
  previewSlot,
  projectEntries,
  templateChoices,
  templateOp,
  templateText,
  templates,
  slotViews,
  unloadOp,
} from '../lib/slot.js';
import { toLocal, toScreen } from '../lib/schematic-view.js';
import { editLevel, shownFrame } from '../lib/schematic-open.js';
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
${BROWSER_CSS}
${SLOT_DIALOG_CSS}`;

/** Which `modules` reply a graph's project would give: its project and the hashes of its files (null when they are not known). */
function modulesKey(graph) {
  return graph?.files ? `${projectKey(graph)}\n${JSON.stringify(graph.files)}` : null;
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
    // ADR 0037 §7: the Load dialog (Load…, Replace…), the module browser in its load mode, and the slot dialogs.
    this.loading = null;
    this.loader = new ModuleBrowser({
      mode: 'load',
      add: (module, _name, options) => this.loadChosen(module, options),
      closed: () => this.loaderClosed(),
      graph: () => this.s.graph,
      defaultName: () => '',
      matchOf: (entry) => contentMatch(entry, this.loading?.slot),
    });
    this.dialogs = new SlotDialogs({ closed: () => this.s.focus?.({ preventScroll: true }) });
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
    root.append(this.rubber, this.notice, this.palette, this.loader.build([...EXPANDED, 's:library', 's:folders']), this.dialogs.build());
    this.updateButtons();
  }

  /** Edit mode on or off (the switch, Escape). */
  setEditing(on) {
    if (!this.session.setEditing(on)) return;
    this.toggle.setAttribute('aria-checked', String(on));
    this.cancelConnect();
    this.closePalette();
    this.closeLoader();
    this.dialogs.close();
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
    // ADR 0037, editable insides: a slot's read-only inside too.
    return levelData(this.s.graph, level) ?? undefined;
  }

  /**
   * Writes `ops` at `level` (`step` 0), or the history's undo (-1) or redo
   * (+1). Resolves to whether it was written (a group's per-level patches
   * go one after the other).
   */
  write(step, level = '', ops = [], { views = [] } = {}) {
    if (!this.editing) return Promise.resolve(false);
    // Only the innermost open level is edited (M2.13-3), undo and redo included (the entry stays for when it is shown);
    // ADR 0037, editable insides: a slot's edits written from inside it (`views`) too.
    const shown = editLevel(this.s.drawn);
    const target = this.session.levelOf(step, level);
    const from = step === 0 ? views : this.session.viewsOf(step);
    if (target !== null && target !== shown.level && !from.includes(shown.level)) {
      this.s.say(`Not ${step < 0 ? 'undone' : step > 0 ? 'redone' : 'edited'}: ${target || 'the top level'} is around the open ${shown.path || 'level'}; ${step ? 'go out to it first' : 'edit it from there'}`);
      return Promise.resolve(false);
    }
    // ADR 0037, editable insides: the first edit of a slot's read-only inside makes the slot's own copy first.
    if (step === 0 && isPreviewLevel(level)) return this.writeOwnCopy(level, ops);
    if (step === 0) ops = insideOps(this.levelGraph(level), ops);
    const asked = step === 0 ? this.session.request(level, ops, 0, views) : step < 0 ? this.session.undo() : this.session.redo();
    return this.send(asked);
  }

  /**
   * ADR 0037, editable insides: `ops` meant for slot inside `key` (read-only,
   * not the slot's own yet): `slot.inside` written at the slot's level
   * first (an undo entry of its own, undone from inside too), then `ops`
   * at the new composite's level, against the base its reply gave.
   */
  writeOwnCopy(key, ops) {
    const { parent, slot } = previewSlot(key);
    const found = insideSlot(this.s.graph, this.levelGraph(key));
    const name = found?.inside?.name ?? null;
    const shaped = insideOps(this.levelGraph(key), ops);
    const asked = this.session.request(parent, [insideOp(slot)], 0, [key, ...(name ? [name] : [])]);
    return this.send(asked).then((ok) => {
      if (!ok) return false;
      const made = this.lastReply?.files?.find((f) => typeof f?.level === 'string' && f.base);
      if (!made) return false;
      this.showNotice('Own inside made', `${slot} now has its own inside: ${made.file}. The content it held stays as it was; Undo takes the copy back.`);
      const again = this.session.request(made.level, shaped, 0, [key]);
      return this.send(again, { keepNotice: true });
    });
  }

  /** Sends `asked` (a `request`'s), keeping the history; resolves to whether it was written. */
  send(asked, { keepNotice = false } = {}) {
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
    this.lastRefusal = null;
    return request.then(
      (reply) => {
        this.session.written(reply, asked.pending);
        this.lastReply = reply;
        if (!keepNotice) this.hideNotice();
        this.s.say('');
        // ADR 0037 §7: what a slot op says it did besides (a Replace's removed map and set entries, panels rebound).
        const said = [...(reply?.notes ?? []), ...((reply?.panels ?? []).length ? [`bindings rewritten in ${reply.panels.join(', ')}`] : [])];
        if (said.length) this.showNotice('Written', said.join('\n'));
        this.updateButtons();
        return true;
      },
      (error) => {
        this.lastRefusal = error?.message ?? String(error);
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

  /** Removes instance or connection `hit` (the selection, or a context menu's target); `views` as `write`'s (an open frame deleted from inside it). */
  remove(hit, { views = [] } = {}) {
    if (hit?.kind === 'instance' && hit.node.kind === 'instance') return this.write(0, hit.level, [removeInstance(hit.node.name)], { views });
    if (hit?.kind === 'wire' && hit.wire.kind === 'connection') return this.write(0, hit.level, [removeConnection(hit.wire.written.from, hit.wire.written.to)]);
    if (hit?.kind === 'wire' && hit.wire.kind === 'export' && hit.wire.written?.role) {
      // ADR 0037, editable insides: a wire of a slot's inside's frame.
      const { role, from, to } = hit.wire.written;
      return this.write(0, hit.level, [exportOp(false, role, hit.wire.written.export ?? (role === 'output' ? to : from), role === 'output' ? from : to)]);
    }
    return Promise.resolve(false);
  }

  /** The sidebar's scale field opened (the context menu's Edit scale…, once the wire is selected). */
  openScale() {
    this.scaleEntry?.open();
  }

  // Drag to connect.

  /** Whether a press on `hit` starts a connection: an instance's output, or (a slot's inside) its frame's input pin, in edit mode. */
  starts(hit) {
    if (!this.editing || hit.kind !== 'port' || hit.port.direction !== 'out') return false;
    return hit.node.kind === 'instance' || (hit.node.kind === 'input' && isSlotInside(this.levelGraph(hit.level)));
  }

  connectStart(hit, at) {
    const levelGraph = this.levelGraph(hit.level);
    let keys;
    if (hit.node.kind === 'input') {
      // ADR 0037, editable insides: a frame pin wired to the inner ports it may reach.
      keys = new Set(frameTargets(levelGraph, hit.port).map((t) => `${hit.path}/${t.instance}.${t.port}`));
    } else {
      const source = { instance: hit.node.name, port: hit.port.name, shape: hit.port.shape };
      const targets = connectTargets(levelGraph, source);
      keys = new Set(targets.map((t) => `${hit.path}/${t.instance}.${t.port}`));
      // ADR 0037, editable insides: and the frame's outputs not wired yet.
      if (isSlotInside(levelGraph)) for (const name of frameOutputs(levelGraph, hit.port.shape)) keys.add(`${hit.path}.${name}`);
    }
    // A panel control's input stands for the address it binds (ui/lib/schematic-controls.js); its output does not take a wire.
    const pins = [...(this.s.groups.get(hit.path)?.g.querySelectorAll('.pin') ?? [])].filter((pin) => keys.has(pin.dataset.address) && pin.dataset.direction !== 'out');
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
    // Dropped on a panel control: the address it binds that takes the wire (the wire is drawn into its input).
    if (c && hit.kind === 'control') {
      const address = hit.path === c.hit.path ? hit.addresses.find((a) => c.keys.has(a)) : null;
      if (!address) {
        this.s.say(`${hit.node.name} binds nothing that can take ${c.hit.address}`);
        return;
      }
      const plain = address.slice(hit.path.length + 1);
      if (c.hit.node.kind === 'input') {
        this.write(0, c.hit.level, [exportOp(true, c.hit.port.role ?? 'input', c.hit.port.export ?? c.hit.port.name, plain)]);
        return;
      }
      this.write(0, c.hit.level, [addConnection(`${c.hit.node.name}.${c.hit.port.name}`, plain)]);
      return;
    }
    if (!c || hit.kind !== 'port') return;
    if (hit.path !== c.hit.path || !c.keys.has(hit.address)) {
      this.s.say(`${hit.address} cannot take ${c.hit.address}`);
      return;
    }
    // ADR 0037, editable insides: a frame pin's wire is an export's.
    if (c.hit.node.kind === 'input') {
      const role = c.hit.port.role ?? 'input';
      this.write(0, c.hit.level, [exportOp(true, role, c.hit.port.export ?? c.hit.port.name, `${hit.node.name}.${hit.port.name}`)]);
      return;
    }
    if (hit.node.kind === 'output') {
      this.write(0, c.hit.level, [exportOp(true, 'output', hit.port.export ?? hit.port.name, `${c.hit.node.name}.${c.hit.port.name}`)]);
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

  /** Whether `event` happened in the Add module browser, the Load dialog or a slot dialog (its wheel scrolls its panes). */
  holds(event) {
    return this.browser.holds(event) || this.loader.holds(event) || this.dialogs.holds(event);
  }

  /** Opens the Add module browser for screen `point` on `level` (a scene level: `paletteLevel`'s). */
  openPalette(point, level) {
    if (!this.editing || !level) return;
    this.placing = { level: level.level, at: toLocal(level.transform, point), names: namesOf(this.levelGraph(level.level)) };
    // The types are asked for once per state of the project's files
    // (`graph.files`): while they are the same, the last reply's show at
    // once and nothing is asked. `inside`: each composite's, for the
    // browser's schematic preview (docs/ui-protocol.md "modules").
    const key = modulesKey(this.s.graph);
    // ADR 0037, editable insides: a slot's own inside is not added as a module (only its slot loads it).
    const known = key !== null && this.modulesKnown?.key === key ? loadable(this.modulesKnown.entries) : null;
    this.browser.open({ where: level.level ? `inside ${level.level}` : 'at the top level', entries: known });
    if (known) return;
    const placing = this.placing;
    this.s.connection?.request?.({ type: 'modules', inside: true }).then(
      (modules) => {
        const entries = browserEntries(modules);
        this.modulesKnown = { key, entries };
        if (this.placing === placing) this.browser.setEntries(loadable(entries));
        this.s.modulesSeen?.(modules);
      },
      (error) => {
        if (this.placing === placing) this.browser.failed(error.message);
      },
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

  // Slots (ADR 0037 §7, phase 3). Each writes a `patch` as the other
  // edits do (undoable); a template is a `file.write` of interfaces/.

  /** Writes `ops` at `level`: a promise of `{ok, error}`, the refusal's text for a dialog to show. */
  writeResult(level, ops, views = []) {
    return this.write(0, level, ops, { views }).then((ok) => ({ ok, error: ok ? null : (this.lastRefusal ?? 'Not written') }));
  }

  /**
   * A slot box (or one of its pins) as the slot ops name it: `{level, name,
   * slot, path, views}`; `views`, when the level shown is that slot's
   * inside (its open frame, 2026-10-29: its slot ops are written at the
   * slot's level from there, as Edit the slot's interface… is), every key
   * its inside may have once written (`slotViews`), so an undo from inside
   * works whichever is shown then.
   */
  slotTarget(hit) {
    const node = hit?.node;
    if (!node?.slot) return null;
    const views = shownFrame(this.s.drawn, hit) ? slotViews(editLevel(this.s.drawn).level, hit.level, node.name, node.slot) : [];
    return { level: hit.level, name: node.name, slot: node.slot, path: hit.path, views };
  }

  /** Whether `level` is the one edited now (the innermost open, M2.13-3), or written from it (`views`); says why not. */
  editable(level, views = []) {
    const shown = editLevel(this.s.drawn);
    if (level === shown.level || views.includes(shown.level)) return true;
    this.s.say(`Not edited: ${level || 'the top level'} is around the open ${shown.path || 'level'}; edit it from there`);
    return false;
  }

  /** Add slot here… (the background, an open composite): the Add slot dialog for screen `point` on `level`. */
  openAddSlot(point, level) {
    if (!this.editing || !level) return;
    const at = toLocal(level.transform, point);
    const names = namesOf(this.levelGraph(level.level));
    this.dialogs.openAddSlot({
      where: level.level ? `Inside ${level.level}` : 'At the top level',
      choices: templateChoices(this.s.graph),
      names,
      defaultName: defaultName('slot', names),
      add: (name, template) => this.writeResult(level.level, [addSlotOp(name, template, at)]),
    });
  }

  /**
   * Load… or Replace… (ADR 0037 §6–7): the Load dialog over the module
   * types and the projects beside this one, sorted by how many of the
   * slot's ports each matches. Until phase 2 a Load is one structural edit
   * (the content's instances fade in as any added instance does) and a
   * Replace another (`docs/adr/0037-slots.md` Notes).
   */
  openLoad(hit) {
    const target = this.slotTarget(hit);
    if (!this.editing || !target || !this.editable(target.level, target.views)) return;
    this.loading = target;
    const title = target.slot.load ? `Replace ${target.slot.load} in ${target.name}` : `Load into ${target.name}`;
    const key = modulesKey(this.s.graph);
    // ADR 0037, editable insides: of the slots' own insides, only this slot's is listed.
    const owner = insideOwner(target.level, target.name);
    const known = key !== null && this.loadKnown?.key === key ? loadable(this.loadKnown.entries, owner) : null;
    this.loader.open({ where: target.level ? `inside ${target.level}` : 'at the top level', entries: known, title });
    if (known) return;
    const loading = this.loading;
    const connection = this.s.connection;
    const modules = connection?.request?.({ type: 'modules', inside: true });
    const projects = connection?.request?.({ type: 'slot.projects' })?.catch?.(() => ({ projects: [] }));
    if (!modules) {
      this.loader.failed('Not connected');
      return;
    }
    Promise.all([modules, projects ?? Promise.resolve({ projects: [] })]).then(
      ([types, folders]) => {
        const entries = [...projectEntries(folders), ...browserEntries(types)];
        this.loadKnown = { key, entries };
        if (this.loading === loading) this.loader.setEntries(loadable(entries, owner));
        this.s.modulesSeen?.(types);
      },
      (error) => {
        if (this.loading === loading) this.loader.failed(error.message);
      },
    );
  }

  /** The Load dialog's choice: `module` loaded, with the content's unmatched ports added first when asked. */
  loadChosen(module, { addUnmatched = false, entry = null } = {}) {
    const target = this.loading;
    if (!target || !module) return null;
    const add = addUnmatched ? contentMatch(entry, target.slot).unmatched : [];
    this.closeLoader();
    // Written from inside, a Load of the slot's own inside shows that next: its undo is let through from there too.
    const known = entry ?? this.loadKnown?.entries?.find((e) => e.module === module);
    const own = target.views.length && known?.insideOf === insideOwner(target.level, target.name);
    const views = own ? [...new Set([...target.views, module])] : target.views;
    this.write(0, target.level, loadOps(target.name, module, { add }), { views });
    return null;
  }

  loaderClosed() {
    this.loading = null;
    this.s.focus?.({ preventScroll: true });
  }

  closeLoader() {
    this.loading = null;
    this.loader.close();
  }

  /** Unload: the slot emptied (its wires stay). */
  unload(hit) {
    const target = this.slotTarget(hit);
    if (target && this.editable(target.level, target.views)) this.write(0, target.level, [unloadOp(target.name)], { views: target.views });
  }

  /** What content `load` offers (a `modules` entry, or a project of `slot.projects`), or null. */
  contentEntry(load) {
    const connection = this.s.connection;
    if (!load || !connection?.request) return Promise.resolve(null);
    const known = this.loadKnown?.entries?.find((e) => e.module === load);
    if (known) return Promise.resolve(known);
    if (isProjectPath(load)) return connection.request({ type: 'slot.projects' }).then((reply) => projectEntries(reply).find((e) => e.module === load) ?? null, () => null);
    return connection.request({ type: 'modules' }).then((reply) => browserEntries(reply).find((e) => e.module === load) ?? null, () => null);
  }

  /** Edit map…: the map editor, its choices the content's names. */
  openMap(hit) {
    const target = this.slotTarget(hit);
    if (!this.editing || !target || !this.editable(target.level, target.views)) return;
    this.contentEntry(target.slot.load).then((entry) => {
      this.dialogs.openMap({ slotName: target.name, load: target.slot.load, rows: mapRows(target.slot, entry), apply: (op) => this.writeResult(target.level, [op], target.views) });
    });
  }

  /** Edit interface… (and a pin's Rename or retype port…: `focus` `{port}`; a "+" pin: `{add}`). */
  openInterface(hit, focus = null) {
    const target = this.slotTarget(hit);
    if (!this.editing || !target || !this.editable(target.level, target.views)) return;
    this.dialogs.openInterface({
      slotName: target.name,
      slot: target.slot,
      focus,
      apply: (op) => this.writeResult(target.level, [op], target.views),
      saveTemplate: () => this.saveTemplate(hit),
      detach: () => this.detach(hit),
    });
  }

  /**
   * ADR 0037, editable insides: Edit the slot's interface… from inside it
   * (the frame's background menu): the interface editor of the slot whose
   * inside is shown, its edits written at the slot's level from here.
   */
  openInsideInterface() {
    const shown = editLevel(this.s.drawn);
    const found = insideSlot(this.s.graph, this.levelGraph(shown.level));
    if (!this.editing || !found) return;
    const at = shown.path.lastIndexOf('/');
    const path = shown.path.slice(0, Math.max(0, at));
    const level = this.s.levelOf?.(path);
    const node = level?.layout.nodes.find((n) => n.kind === 'instance' && n.name === found.name);
    if (!node?.slot) return;
    const views = [shown.level];
    this.dialogs.openInterface({
      slotName: found.name,
      slot: node.slot,
      focus: null,
      apply: (op) => this.write(0, found.parent, [op], { views }).then((ok) => ({ ok, error: ok ? null : (this.lastRefusal ?? 'Not written') })),
      saveTemplate: () => this.saveTemplate({ kind: 'instance', path, level: found.parent, node, address: shown.path }),
      detach: () => this.detach({ kind: 'instance', path, level: found.parent, node, address: shown.path }),
    });
  }

  /** A slot's "+" pin at `address` (`/fx`, `/drums/fx`): the interface editor with a port to add on `side`. */
  addPort(address, side) {
    const at = String(address ?? '').lastIndexOf('/');
    const path = at > 0 ? address.slice(0, at) : '';
    const name = address.slice(at + 1);
    const level = this.s.levelOf?.(path);
    const node = level?.layout.nodes.find((n) => n.kind === 'instance' && n.name === name);
    if (!node?.slot) return;
    this.openInterface({ kind: 'instance', path, level: level.level, node, address }, { add: side === 'out' ? 'out' : 'in' });
  }

  /** A slot pin's Remove port (with its wires: `wires`). */
  removePort(hit, wires = false) {
    const target = this.slotTarget(hit);
    if (!target || !hit.port || !this.editable(target.level, target.views)) return;
    const op = { op: 'slot.interface', slot: target.name, remove: [hit.port.name] };
    if (wires) op.removeWires = true;
    this.write(0, target.level, [op], { views: target.views });
  }

  /** Detach from template: the template's ports written into the slot. */
  detach(hit) {
    const target = this.slotTarget(hit);
    if (!target || !this.editable(target.level, target.views)) return;
    this.dialogs.close();
    this.write(0, target.level, [detachOp(target.name)], { views: target.views });
  }

  /** Writes `interfaces/<name>.toml` for `ports` (a new file): a promise of `{ok, error}`. */
  writeTemplate(name, ports, description) {
    const request = this.s.connection?.request?.({ type: 'file.write', file: `interfaces/${name}.toml`, text: templateText(name, ports, description), base: null });
    if (!request) return Promise.resolve({ ok: false, error: 'Not connected' });
    return request.then(
      () => ({ ok: true, error: null }),
      (error) => ({ ok: false, error: error?.message ?? String(error) }),
    );
  }

  /** The project's own template names (interfaces/), for a new one's name. */
  templateNames() {
    return new Set(templates(this.s.graph).filter((t) => !t.library).map((t) => t.name));
  }

  /** Save interface as template…: the slot's interface written to interfaces/<name>.toml, then the slot set to it. */
  saveTemplate(hit) {
    const target = this.slotTarget(hit);
    if (!this.editing || !target || !this.editable(target.level, target.views)) return;
    this.dialogs.openTemplateName({
      title: `Save interface as template: ${target.name}`,
      note: 'Writes its ports and parameters to interfaces/<name>.toml and sets the slot’s interface to that template (an edit of the file then reaches every slot naming it).',
      defaultName: target.name,
      taken: this.templateNames(),
      save: (name, description) =>
        this.writeTemplate(name, target.slot.interface?.ports ?? [], description).then((written) => (written.ok ? this.writeResult(target.level, [templateOp(target.name, name)], target.views) : written)),
    });
  }

  /** Make interface from this instance… (ADR 0037 §3): any instance's ports and parameters as interfaces/<name>.toml. */
  /**
   * ADR 0046 §8: Group into slot: instance `hit` made a slot holding what
   * it is, with the same names (the plan does not change); at the top
   * level its controls of the main panel move into the slot's part (those
   * selected with it, `controls`, when some are). One
   * undo entry gives every file back.
   */
  groupSlot(hit, controls = null) {
    if (hit?.kind !== 'instance' || hit.node.kind !== 'instance' || hit.node.slot) return Promise.resolve(false);
    // `controls`: the main panel's controls selected with it (`groupControls`), by `panel.check`'s index; none, every one of its own.
    return this.write(0, hit.level, [{ op: 'slot.group', instance: hit.node.name, ...(controls?.length ? { controls } : {}) }]);
  }

  makeInterface(hit) {
    if (!this.editing || hit?.kind !== 'instance') return;
    const instance = (this.levelGraph(hit.level)?.instances ?? []).find((i) => i.name === hit.node.name);
    if (!instance) return;
    const ports = instancePorts(instance);
    this.dialogs.openTemplateName({
      title: `Make interface from ${hit.node.name}`,
      note: `Writes ${ports.length} port${ports.length === 1 ? '' : 's'} and parameters of ${hit.node.name} (${hit.node.module || 'a slot'}) to interfaces/<name>.toml: a template slots may start from.`,
      defaultName: (hit.node.module || hit.node.name).split('.').pop().replace(/[^a-z0-9_]/g, '_').replace(/^[^a-z]/, 'i'),
      taken: this.templateNames(),
      save: (name, description) => this.writeTemplate(name, ports, description),
    });
  }

  // The sidebar.

  /** What the sidebar adds in edit mode: a wire's scale entry; inside a composite, how many instances the edit reaches. */
  cardParts(selection) {
    this.scaleEntry = null;
    if (!this.editing || !selection) return [];
    const parts = [];
    if (isPreviewLevel(selection.level) && selection.kind !== 'background') {
      // ADR 0037, editable insides: nothing is shared once edited: the first edit makes the slot's own copy.
      parts.push(html('div', { className: 'reach', textContent: 'a slot’s read-only inside: an edit makes the slot’s own copy first' }));
    } else if (selection.level && selection.level !== '' && selection.kind !== 'background') {
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
