// The right or the middle button held pans the view at once (ADR 0051's
// amendment of 2026-11-02, the owner: "the same on every layer"), shared by
// the Deck (`<signal-plane>`) and its Engine board (`<signal-schematic
// plane>`). A right click that does not move opens the context menu on its
// release: the system's own right click (macOS sends it on the press, others
// on the release) is held back while the button is down and sent again,
// where the button was pressed, when it is let go without a move. A second
// right click soon after, near the first, is a double right click (the
// zoom lock's toggle): no menu then. The pointer's motion is passed on as
// screen deltas; no layout is read here (rule: never per frame).

/** A press moved this far (screen px) is a pan, not a click. */
export const PAN_SLOP = 4;
/** A second right click within this long (ms) and this near (px) of the first is a double right click. */
export const DOUBLE_RIGHT_MS = 400;
export const DOUBLE_RIGHT_PX = 8;

/** Whether a right click at `at` (`{x, y}`) at `now` (ms) is the second of a double right click after `last` (`{at, x, y}` or null). */
export function isDoubleRight(last, now, at) {
  return Boolean(last) && now - last.at < DOUBLE_RIGHT_MS && Math.hypot(at.x - last.x, at.y - last.y) < DOUBLE_RIGHT_PX;
}

/**
 * Button panning on `host` (an element whose `dataset.panning` is set while
 * it pans): `capture` the element that takes the pointer, `pan(dx, dy)`
 * the view panned by screen pixels, `doubleRight()` a double right click,
 * `skip(event)` true for a press that is not this pan's (an editor's, a
 * layer's own). `down`, `move` and `up` are the stage's capture listeners'
 * (`up` for `pointercancel` too); each returns whether it took the event.
 */
export class ButtonPan {
  constructor(host, { capture, pan, doubleRight = null, skip = null, closeMenu = null }) {
    this.host = host;
    this.capture = capture;
    this.onPan = pan;
    this.onDoubleRight = doubleRight;
    this.skip = skip;
    this.closeMenu = closeMenu;
    this.active = null;
    this.lastRight = null;
  }

  down(event) {
    if (event.button !== 1 && event.button !== 2) return false;
    if (this.skip?.(event)) return false;
    event.preventDefault();
    this.capture?.setPointerCapture?.(event.pointerId);
    if (this.host.dataset) this.host.dataset.panning = '';
    const at = { x: event.clientX, y: event.clientY };
    const pan = { id: event.pointerId, button: event.button, target: event.composedPath?.()[0] ?? event.target, start: at, last: at, moved: false };
    pan.swallow = (e) => {
      if (e.signalReplayed) return;
      e.preventDefault();
      e.stopPropagation();
    };
    this.doc()?.addEventListener('contextmenu', pan.swallow, true);
    this.active = pan;
    return true;
  }

  move(event) {
    const pan = this.active;
    if (!pan || event.pointerId !== pan.id) return false;
    event.stopPropagation();
    if (!pan.moved && Math.hypot(event.clientX - pan.start.x, event.clientY - pan.start.y) >= PAN_SLOP) pan.moved = true;
    const dx = event.clientX - pan.last.x;
    const dy = event.clientY - pan.last.y;
    pan.last = { x: event.clientX, y: event.clientY };
    if (dx || dy) this.onPan(dx, dy);
    return true;
  }

  up(event) {
    const pan = this.active;
    if (!pan || event.pointerId !== pan.id) return false;
    this.active = null;
    if (this.host.dataset) delete this.host.dataset.panning;
    event.stopPropagation();
    // A release on other platforms brings its contextmenu after pointerup: still held back for a moment.
    const doc = this.doc();
    setTimeout(() => doc?.removeEventListener('contextmenu', pan.swallow, true), 0);
    if (event.type === 'pointercancel' || pan.moved || pan.button !== 2) return true;
    const now = event.timeStamp ?? performance.now();
    if (this.onDoubleRight && isDoubleRight(this.lastRight, now, pan.start)) {
      this.lastRight = null;
      this.closeMenu?.();
      this.onDoubleRight();
      return true;
    }
    this.lastRight = { at: now, x: pan.start.x, y: pan.start.y };
    // A right click without a move: its menu, where it was pressed.
    const replay = new MouseEvent('contextmenu', { bubbles: true, composed: true, cancelable: true, clientX: pan.start.x, clientY: pan.start.y, button: 2 });
    replay.signalReplayed = true;
    pan.target.dispatchEvent(replay);
    return true;
  }

  doc() {
    return this.host.ownerDocument ?? globalThis.document ?? null;
  }
}

/** Closes Signal.app's page menu (the first right click's) when there is one. */
export function closePageMenu(doc = globalThis.document) {
  doc?.querySelector?.('signal-app-context-menu')?.close?.();
}
