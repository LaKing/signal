// The schematic's Edit patch mode (M2.9-3, docs/ui-controls.md "Editing the
// patch"): its state, the history of the patches written in this session,
// and its keys. As the panel editor's (ui/lib/panel-edit.js): every change
// is written at once, undo and redo write again; here what is kept is each
// patch's ops and the inverse the server answered with, and an undo sends
// that inverse as a patch against the newest base the page knows (its own
// `patched` replies and `graph.files`). No DOM.

import { editKey } from './panel-edit.js';
import { keyCommand } from './keys.js';
import { patchMessage } from './schematic-patch.js';

/**
 * The patches written in this session: `{level, forward, backward}` each,
 * and how many of them the files hold (`at`). Undo sends an entry's
 * `backward`, redo its `forward`; the reply's inverse replaces the other
 * one (the server's, with the positions it needs). A new patch drops what
 * redo would have written.
 */
export class PatchHistory {
  constructor() {
    this.entries = [];
    this.at = 0;
  }

  get canUndo() {
    return this.at > 0;
  }

  get canRedo() {
    return this.at < this.entries.length;
  }

  /** The entry undo writes, or null. */
  undo() {
    return this.canUndo ? this.entries[this.at - 1] : null;
  }

  /** The entry redo writes, or null. */
  redo() {
    return this.canRedo ? this.entries[this.at] : null;
  }

  push(entry) {
    this.entries.splice(this.at);
    this.entries.push(entry);
    this.at = this.entries.length;
  }

  /** Undo (-1) or redo (+1) was written; `inverse` is the server's answer. */
  moved(step, inverse) {
    if (step < 0 && this.canUndo) {
      this.at -= 1;
      this.entries[this.at].forward = inverse;
    } else if (step > 0 && this.canRedo) {
      this.entries[this.at].backward = inverse;
      this.at += 1;
    }
  }

  clear() {
    this.entries = [];
    this.at = 0;
  }
}

/**
 * What a key does in Edit patch (the `schematic_edit` scope,
 * ui/lib/keys.default.js): `undo`, `redo`, `leave` (Escape, a field
 * included) and `delete` (Delete, Backspace; not in a field). With the add
 * palette open (`palette`), Escape closes it (`close`): a second Escape leaves.
 */
export function patchKey(event, { palette = false } = {}) {
  const action = editKey(event, 'schematic_edit');
  if (action === 'leave' && palette) return 'close';
  if (action) return action;
  const target = event?.composedPath?.()[0] ?? event?.target;
  if (['input', 'textarea', 'select'].includes(target?.localName ?? '')) return null;
  if (keyCommand('schematic_edit', event)?.id === 'schematic_edit.delete') return 'delete';
  return null;
}

/**
 * Edit mode and what it writes against, kept for the page's session: the
 * newest base of each level, the history, a write in flight (one at a
 * time) and the last notice.
 */
export class PatchSession {
  constructor() {
    this.editing = false;
    this.bases = new Map();
    this.history = new PatchHistory();
    this.pending = null;
    this.notice = null;
  }

  /** Edit mode on or off (`toggle` when `on` is not given); whether it changed. */
  setEditing(on = !this.editing) {
    if (on === this.editing) return false;
    this.editing = on;
    return true;
  }

  /**
   * A `graph`'s files: the newest bases (another client's or a commit's
   * write included). Another project (`graph.projectId`, which tells apart
   * folders of the same name and projects of the same name): the history,
   * the bases and a write in flight are let go with a notice (`{cleared}`),
   * so no inverse is ever sent to a project it was not written in.
   */
  graphSeen(graph) {
    const project = graph?.projectId ?? `${graph?.folder ?? ''}\n${graph?.project ?? ''}`;
    let result = null;
    if (this.project !== undefined && project !== this.project) {
      const had = this.history.entries.length > 0;
      this.history.clear();
      this.bases.clear();
      this.pending = null;
      if (had) {
        this.notice = 'Another project: the undo history was emptied';
        result = { cleared: this.notice };
      }
    }
    this.project = project;
    for (const [level, base] of Object.entries(graph?.files ?? {})) this.bases.set(level, base);
    return result;
  }

  /** The `patch` of `ops` (step 0: a new patch, -1 undo, +1 redo), or `{refused}` while one is written. */
  request(level, ops, step = 0) {
    if (this.pending) return { refused: 'Writing the last change: try again' };
    const base = this.bases.get(level);
    if (!base) return { refused: 'The level’s file is not known yet' };
    this.pending = { level, ops, step, project: this.project };
    return { message: patchMessage(level, base, ops), pending: this.pending };
  }

  /** The level a write of `step` (0: a new patch at `level`, -1 undo, +1 redo) would patch, or null when there is none to write. */
  levelOf(step, level = '') {
    if (step === 0) return level;
    return (step < 0 ? this.history.undo() : this.history.redo())?.level ?? null;
  }

  undo() {
    const entry = this.history.undo();
    return entry ? this.request(entry.level, entry.backward, -1) : null;
  }

  redo() {
    const entry = this.history.redo();
    return entry ? this.request(entry.level, entry.forward, +1) : null;
  }

  /**
   * The pending patch was written (`patched`). `pending`: the request it
   * answers; a reply to one let go (another project) changes nothing.
   */
  written(reply, pending = this.pending) {
    if (!pending || pending !== this.pending) return;
    this.pending = null;
    this.bases.set(reply.level ?? pending.level, reply.base);
    if (pending.step === 0) this.history.push({ level: pending.level, forward: pending.ops, backward: reply.inverse });
    else this.history.moved(pending.step, reply.inverse);
  }

  /**
   * The pending patch was refused (`error`): a conflict, or an undo or redo
   * the server cannot apply any more, empties the history with a notice
   * (`{cleared}`); otherwise `{reason, references}`, the history kept.
   */
  refused(error, pending = this.pending) {
    if (pending !== this.pending) return { reason: error?.message ?? String(error), references: [] };
    this.pending = null;
    const reply = error?.reply ?? {};
    if (reply.code === 'conflict' && reply.level !== undefined && reply.base) this.bases.set(reply.level, reply.base);
    const undoing = pending && pending.step !== 0;
    if (error?.code === 'conflict' || (undoing && error?.code === 'bad-value')) {
      this.history.clear();
      this.notice = `${error.code === 'conflict' ? 'The file changed elsewhere' : 'That change can no longer be undone'}: the undo history was emptied`;
      return { cleared: this.notice };
    }
    return { reason: error?.message ?? String(error), references: reply.references ?? [] };
  }
}
