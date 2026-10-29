// Who takes the wheel (ADR 0034 §2): the page scrolls by default; a
// control that scrolls, zooms or changes a value with the wheel takes it
// only when it owns it: it fills the window (the schematic), it has the
// focus (a click or Tab, until the focus leaves or Escape), or the wheel
// modifier (`view.wheel_inside`, ⌥ by default) is held. A gesture is decided
// once, by its first event, and keeps going where it started: scrolling the
// page over a focused piano roll goes on scrolling the page, and scrolling
// inside a control never runs on into the page (no scroll chaining).
// Plain ES module, no DOM.

/** A pause longer than this between wheel events starts a new gesture (a trackpad's momentum is one gesture). */
export const GESTURE_GAP_MS = 300;

/** No hint while the page is being scrolled: a gesture starting this soon after one of the page's (a mouse's slow notches) shows none. */
export const HINT_QUIET_MS = 1000;

/** The share of the window a control must cover to fill it. */
export const FILLS = Object.freeze({ width: 0.9, height: 0.75 });

/**
 * Whether a control owns the wheel: `fills` (it fills the window, or says
 * `wheel="own"`), `focused` (and not `released` by Escape since), or the
 * wheel `modifier` held. `wheel="page"` (`never`) gives it to the page always.
 */
export function ownsWheel({ fills = false, focused = false, released = false, modifier = false, never = false } = {}) {
  if (never) return false;
  return fills || modifier || (focused && !released);
}

/** Whether a control's box `rect` fills a `viewport` (`{width, height}`). */
export function fillsWindow(rect, viewport) {
  if (!rect || !viewport?.width || !viewport?.height) return false;
  return rect.width >= viewport.width * FILLS.width && rect.height >= viewport.height * FILLS.height;
}

/**
 * The gesture latch of one document: `seen(now)` for every wheel event
 * first; a control asks `claim(element, owns)` (its own answer is taken only
 * by a gesture's first event); `settle()` after the event gives a gesture
 * nobody claimed to the page. `owner`: the element, null (the page), or
 * undefined (not decided yet).
 */
export class WheelLatch {
  constructor(gap = GESTURE_GAP_MS) {
    this.gap = gap;
    this.last = -Infinity;
    this.owner = undefined;
    /** The element whose claim decided the gesture (the hint shows only over it: a page scroll passing over controls shows none). */
    this.decider = undefined;
    /** The hint was shown in this gesture. */
    this.hinted = false;
    /** The page scrolled in the gesture before this one, which ended this soon before it (ms). */
    this.pageBefore = Infinity;
  }

  seen(now) {
    if (now - this.last > this.gap) {
      this.pageBefore = this.owner === null ? now - this.last : Infinity;
      this.owner = undefined;
      this.decider = undefined;
      this.hinted = false;
    }
    this.last = now;
  }

  claim(element, owns) {
    if (this.owner === undefined) {
      this.owner = owns ? element : null;
      this.decider = element;
    }
    return this.owner === element;
  }

  settle() {
    if (this.owner === undefined) this.owner = null;
    if (this.decider === undefined) this.decider = null;
  }

  /** Whether an event outside the owner is held back (the gesture is a control's). */
  holds(inside) {
    return Boolean(this.owner) && !inside;
  }

  /** The hint is due over `element`: the gesture started over it, which did not take it, it was not shown yet, and the page was not just being scrolled. */
  hint(element) {
    if (this.hinted || this.owner !== null || this.decider !== element || this.pageBefore < HINT_QUIET_MS) return false;
    this.hinted = true;
    return true;
  }
}

/** The hint over a control that did not take the wheel. */
export function wheelHint(modifier) {
  return modifier ? `Click to use the wheel here, or hold ${modifier}` : 'Click to use the wheel here';
}

// ------------------------------------------- a mouse or a trackpad

/** A pause longer than this between wheel events ends a gesture's kind (`WheelKinds`): a trackpad's events and its momentum come closer. */
export const KIND_GAP_MS = 150;

/**
 * The least |deltaY| (px) of a mouse wheel's notch. WebKit gives a notch as
 * 40 px times macOS's acceleration (fractional, so a fraction alone says
 * nothing), Chromium 100 px; a trackpad's first event is a few px.
 * An assumption to check on the device.
 */
export const NOTCH_MIN_PX = 32;

/**
 * What sent wheel event `event` (`{deltaX, deltaY, deltaMode, ctrlKey,
 * shiftKey}`): `'pinch'` (ctrl+wheel: how browsers and WKWebView give a
 * trackpad's pinch), `'mouse'` (lines or pages, or a large vertical step
 * alone) or `'trackpad'` (pixels moving sideways too, or small steps). A
 * Shift+wheel's sideways delta (a mouse's on a Mac) is not a trackpad's.
 */
export function wheelKind(event) {
  if (event.ctrlKey) return 'pinch';
  if ((event.deltaMode ?? 0) !== 0) return 'mouse';
  if (event.deltaX && !event.shiftKey) return 'trackpad';
  return Math.abs(event.deltaY || (event.shiftKey ? event.deltaX : 0) || 0) >= NOTCH_MIN_PX ? 'mouse' : 'trackpad';
}

/**
 * A gesture's kind, kept: the first event of a gesture decides
 * (`wheelKind`) and the rest of it (events less than `gap` ms apart) keeps
 * that, so a trackpad's scroll never turns into a zoom when one event of it
 * is large. A pinch is a pinch whenever ctrl is held.
 */
export class WheelKinds {
  constructor(gap = KIND_GAP_MS) {
    this.gap = gap;
    this.last = -Infinity;
    this.current = undefined;
  }

  kind(event, now) {
    const fresh = wheelKind(event);
    if (fresh === 'pinch' || this.current === undefined || this.current === 'pinch' || now - this.last > this.gap) this.current = fresh;
    this.last = now;
    return this.current;
  }
}
