// The adapter between a pattern control and the server (ADR 0028 §7). No DOM.
//
// The protocol messages are built elsewhere; a control uses them only when
// its connection offers these two methods (`ui/signal-ui.js` is expected to
// add them over the messages, as `panel.read`/`panel.write` work):
//
//   connection.readPattern(name)
//     → `{"type": "pattern.read", "name": "<name>"}`
//     ← resolves `{ text, base }`: the whole `patterns/<name>.toml` and its
//       SHA-256 (hex), the base a write names.
//   connection.writePattern(name, text, base)
//     → `{"type": "pattern.write", "name", "text", "base"}`
//     ← resolves `{ base }`, the written file's SHA-256; rejects with an
//       error whose `code` is the server's (`conflict`: the file changed
//       since `base`; its `reply` may carry the current `text` and `base`,
//       as `panel.write`'s conflict does), the server having parsed and
//       validated the text before an atomic write (ADR 0028 §7).
//
// The server pushes `{"type": "pattern.written", "name", "base", "text"}`
// to every client after each `pattern.write`. An idle control (loaded,
// nothing pending, no write in flight) whose pattern it names and whose
// base differs shows that text at once: another client's edit appears
// without waiting for a conflict. Its own write's push is ignored (the
// write is still in flight, or its base is already ours); with an edit
// pending, the push is left to that edit's write, which meets it as a
// conflict. A file changed on disk by another program is not pushed: it
// shows on the next write's `conflict` or the next connect.
//
// `start()` reads the file when the connection is open (`status`
// 'open', after the server's `welcome`: a request sent earlier is refused
// at once) and again whenever it opens after a drop; a write refused by the
// drop is sent again then.
//
// Until the first read has arrived the control takes no edits
// (`loaded` false), so nothing is written over a file it has not shown.
// Writes are debounced (`delay` ms after the last edit), one at a time,
// each naming the base the last read or write returned; text equal to the
// file's is not written. On `conflict` the file is read again: the user's
// edit is kept (written again on the new base) only when the file still
// means what the edit started from, i.e. its sha is the edit's base or its
// pattern is the same; otherwise the file's version wins, is loaded into
// the control, and `onNotice` says the edit was replaced. After 3
// conflicts or failed re-reads in a row it stops until the next edit. Any
// other refusal is reported and the edit stays pending until the next edit
// or `retry()`. `close()` drains: what is pending (also an edit made while
// a write was in flight) is still written, stopping only at a refusal.

import { parsePattern, samePattern } from './pattern-text.js';

export const WRITE_DELAY_MS = 300;

/** The notice of a conflict the file's version won. */
export const CHANGED_ELSEWHERE = 'the pattern file changed elsewhere: its version is shown, your last edit was not written';

/**
 * A notice as a control's toolbar shows it, short enough not to be cut
 * (the full text is its title and its `role=status` text): a conflict the
 * file won is "changed elsewhere — reloaded", a stop after MAX_CONFLICTS
 * "not written — edit again to retry"; anything else as it is.
 */
export function shortNotice(text) {
  if (!text) return '';
  if (text === CHANGED_ELSEWHERE) return 'changed elsewhere — reloaded';
  if (text.startsWith('not written: ') && text.endsWith('your next edit tries again')) return 'not written — edit again to retry';
  return text;
}
/** Conflicts (or failed re-reads) in a row before the adapter stops retrying. */
export const MAX_CONFLICTS = 3;

/** Whether a connection offers the pattern methods. */
export function canSyncPatterns(connection) {
  return Boolean(connection && typeof connection.readPattern === 'function' && typeof connection.writePattern === 'function');
}

export class PatternSync {
  /**
   * `onLoad(pattern, text)` shows a pattern read from the file;
   * `onNotice(text|null)` shows the state (an error, a replaced edit).
   */
  constructor({ name, connection, kind = null, onLoad = () => {}, onNotice = () => {}, delay = WRITE_DELAY_MS, timers = globalThis }) {
    this.name = name;
    this.connection = connection;
    this.kind = kind;
    this.onLoad = onLoad;
    this.onNotice = onNotice;
    this.delay = delay;
    this.timers = timers;
    this.loaded = false; // the first read arrived: edits are taken
    this.base = null; // the sha of the file as last read or written
    this.text = null; // that file's text
    this.pattern = null; // that file's pattern
    this.pending = null; // text waiting to be written
    this.pendingPattern = null; // its pattern, when the control gave it
    this.pendingBase = null; // the base the pending edit started from
    this.writing = null; // the write in flight (a promise)
    this.timer = null;
    this.conflicts = 0;
    this.stalled = false; // stopped after MAX_CONFLICTS until the next edit
    this.offline = false; // a write was refused because the connection dropped
    this.closed = false;
  }

  /** Tells the control; a failure there never changes what was written. */
  notify(text) {
    try {
      this.onNotice(text);
    } catch (error) {
      console.error(error);
    }
  }

  /** The file's pattern from `text`, or a notice and null (a wrong kind included). */
  parse(text, why) {
    try {
      const pattern = parsePattern(text);
      if (this.kind && pattern.kind !== this.kind) throw new Error(`it is a ${pattern.kind} pattern, not ${this.kind}`);
      return pattern;
    } catch (error) {
      this.notify(`pattern ${this.name}${why}: ${error.message ?? error}`);
      return null;
    }
  }

  /**
   * Reads the file once the connection is open (now, when it already is; a
   * connection without a `status` counts as open), and again each time it
   * opens after a drop. A request sent before the server's `welcome` is
   * refused at once (`connection closed`), so a control attached while its
   * page connects would otherwise never load. Returns the stop.
   */
  start() {
    const connection = this.connection;
    const stops = [];
    if (typeof connection.on === 'function') {
      stops.push(connection.on('status', (status) => status === 'open' && this.opened()));
      stops.push(connection.on('pattern.written', (message) => this.pushed(message)));
    }
    if (connection.status === undefined || connection.status === 'open') this.load();
    return () => {
      for (const stop of stops) stop();
    };
  }

  /**
   * A `pattern.written` push: another client wrote this pattern. Shown at
   * once when the control is idle and the base is not its own (the pushed
   * text, or the file read again when the push carries none).
   */
  pushed(message) {
    if (this.closed || !this.loaded || message?.name !== this.name) return;
    if (!message.base || message.base === this.base) return;
    if (this.pending !== null || this.writing) return;
    if (typeof message.text !== 'string') {
      this.load();
      return;
    }
    const pattern = this.parse(message.text, ' written elsewhere');
    if (!pattern) return;
    this.adopt({ text: message.text, base: message.base }, pattern);
    this.conflicts = 0;
    this.stalled = false;
    this.onLoad(pattern, message.text);
    this.notify(null);
  }

  /** The connection opened (again): load, or write what the drop held back, or read the file again. */
  opened() {
    if (this.closed) return;
    if (!this.loaded) {
      this.load();
    } else if (this.offline) {
      this.offline = false;
      this.retry();
    } else if (this.pending === null && !this.writing) {
      this.load();
    }
  }

  /**
   * Reads the file and shows it. Resolves to the pattern (or null on
   * failure). Read again after a reconnect, an unchanged file is not shown
   * again (the control keeps its history), and an edit made meanwhile wins
   * until its write meets the file (a conflict).
   */
  async load() {
    let reply;
    try {
      reply = await this.connection.readPattern(this.name);
    } catch (error) {
      if (!this.closed) this.notify(`pattern ${this.name}: ${error?.message ?? error}`);
      return null;
    }
    if (this.closed) return null;
    if (this.loaded && (this.pending !== null || this.writing)) return this.pattern;
    const pattern = this.parse(reply.text, '');
    if (!pattern) return null;
    const same = this.loaded && reply.text === this.text;
    this.adopt(reply, pattern);
    this.loaded = true;
    if (!same) this.onLoad(pattern, reply.text);
    this.notify(null);
    return pattern;
  }

  adopt({ text, base }, pattern) {
    this.base = base;
    this.text = text;
    this.pattern = pattern;
  }

  /** The control changed the pattern to `text` (`pattern` its object): written after the delay. */
  edited(text, pattern = null) {
    if (this.closed || !this.loaded) return;
    if (this.pending === null) this.pendingBase = this.base;
    this.pending = text;
    this.pendingPattern = pattern;
    this.conflicts = 0;
    this.stalled = false;
    this.schedule();
  }

  schedule() {
    this.timers.clearTimeout(this.timer);
    this.timer = this.timers.setTimeout(() => {
      this.timer = null;
      this.write();
    }, this.delay);
  }

  /** Writes what is pending now (also on blur and before leaving); resolves when written or refused. */
  async flush() {
    this.timers.clearTimeout(this.timer);
    this.timer = null;
    if (this.writing) await this.writing;
    return this.write();
  }

  retry() {
    this.stalled = false;
    this.conflicts = 0;
    return this.flush();
  }

  /** Writes the pending text, then what became pending meanwhile; one write at a time. */
  write() {
    if (this.writing) return this.writing;
    if (this.pending === null || this.stalled) return Promise.resolve();
    this.writing = this.writeOnce().finally(() => {
      this.writing = null;
    });
    return this.writing.then(() => {
      if (this.pending === null || this.stalled) return undefined;
      // Closed: drain now; open: after the delay, as any edit.
      if (this.closed) return this.write();
      if (this.timer === null) this.schedule();
      return undefined;
    });
  }

  async writeOnce() {
    const text = this.pending;
    const editBase = this.pendingBase;
    const given = this.pendingPattern;
    this.pending = null;
    this.pendingPattern = null;
    if (text === this.text) return; // the file already says this
    let reply;
    try {
      reply = await this.connection.writePattern(this.name, text, this.base);
    } catch (error) {
      if (error?.code === 'conflict') await this.conflict(text, editBase, given);
      else {
        // Kept: the next edit or a retry writes it; a drain stops here.
        if (this.pending === null) {
          this.pending = text;
          this.pendingPattern = given;
          this.pendingBase = editBase;
        }
        this.stalled = true;
        // The connection dropped: written again when it opens (`opened`).
        if (error?.code === 'closed') this.offline = true;
        this.notify(`not written: ${error?.message ?? error}`);
      }
      return;
    }
    // Written: whatever follows (parsing for the record, the notice) cannot make it unwritten.
    this.conflicts = 0;
    this.offline = false;
    this.base = reply?.base ?? this.base;
    this.text = text;
    let pattern = given;
    if (!pattern) {
      try {
        pattern = parsePattern(text);
      } catch {
        pattern = null;
      }
    }
    this.pattern = pattern;
    this.notify(null);
  }

  /** Counts a conflict; after MAX_CONFLICTS in a row, stops until the next edit. */
  giveUp(text, given, why) {
    this.conflicts++;
    if (this.conflicts < MAX_CONFLICTS) return false;
    if (this.pending === null) {
      this.pending = text;
      this.pendingPattern = given;
    }
    this.stalled = true;
    this.notify(`not written: ${why} ${MAX_CONFLICTS} times; your next edit tries again`);
    return true;
  }

  /** A write was refused as `conflict`: read again, keep the edit only on the same base or meaning. */
  async conflict(text, editBase, given) {
    const startedFrom = this.pattern;
    let current;
    try {
      current = await this.connection.readPattern(this.name);
    } catch (error) {
      if (!this.giveUp(text, given, 'the file changed elsewhere and could not be read again')) {
        if (this.pending === null) {
          this.pending = text;
          this.pendingPattern = given;
        }
        this.notify(`the pattern changed elsewhere and could not be read again: ${error?.message ?? error}`);
      }
      return;
    }
    const pattern = this.parse(current.text, ' changed elsewhere and');
    if (!pattern) {
      // Not a file this control can show (it does not parse, or another kind): nothing more is written.
      this.pending = null;
      this.stalled = true;
      return;
    }
    const sameBase = current.base === editBase;
    const sameMeaning = startedFrom !== null && samePattern(pattern, startedFrom);
    this.adopt(current, pattern);
    if (sameBase || sameMeaning) {
      if (this.giveUp(text, given, 'the server answered conflict')) return;
      // Nothing the user saw was changed: the edit goes on the new base.
      if (this.pending === null) {
        this.pending = text;
        this.pendingPattern = given;
        this.pendingBase = current.base;
      }
      return;
    }
    this.pending = null;
    this.pendingPattern = null;
    if (this.closed) return;
    this.onLoad(pattern, current.text);
    this.notify(CHANGED_ELSEWHERE);
  }

  /** No more edits; what is pending is still written (drained). Resolves when done. */
  close() {
    this.timers.clearTimeout(this.timer);
    this.timer = null;
    this.closed = true;
    return this.write();
  }
}
