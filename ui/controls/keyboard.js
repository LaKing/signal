// The keyboard and the wheel on a page (ADR 0034, docs/ui-controls.md
// "Keys and the wheel"). Importing it (every control does, through
// ./base.js) sets up, once per document:
// - the keymap: the built-in one (ui/lib/keys.default.js) until the
//   server's `keys` arrives (`keys.get` on each connect, pushed on every
//   change of the person's keys.toml, docs/ui-protocol.md);
// - the view's keys: where no focused control takes a key, Page Up, Page
//   Down, Space, Home, End and the arrows scroll the page (the scrolling box
//   around the focus, else the page's view: `setViewScroller`); an open text
//   field keeps every key;
// - Signal.app's commands (scope `app`): inside Signal.app the native menu
//   takes each command's first chord, and another chord is handed to it
//   (wry's IPC, `menu.run`); in a browser the page is told (`signal-command`
//   on the top window, ui/app/app.js runs it);
// - who takes the wheel (ui/lib/wheel.js): `takesWheel(element, event)` for
//   a control's wheel listener, the gesture latch, Escape giving the wheel
//   back, and a one-line hint over a control that did not take it.

import { getDefaultConnection, onDefaultConnection } from '../signal-ui.js';
import { applyKeys, eventChord, formatChord, isMac, keyCommand, keymap, splitChord, wheelModifierHeld } from '../lib/keys.js';
import { WheelLatch, fillsWindow, ownsWheel, wheelHint } from '../lib/wheel.js';

/** How far the arrows scroll the page, in CSS pixels. */
export const LINE_PX = 40;
/** A page is this share of the box's height (some context stays in sight). */
export const PAGE_SHARE = 0.875;

const installed = new WeakSet();
const connected = new WeakSet();
let viewScroller = null;

/** Gives the page's view, where the focus is in no scrolling box: `fn(document)` → an element, or null (the document's). */
export function setViewScroller(fn) {
  viewScroller = fn;
}

/** Asks the server for the keymap on `connection` (now if open, and after every reconnect) and takes its pushes. */
function follow(connection) {
  if (!connection || connected.has(connection)) return;
  connected.add(connection);
  const ask = () => connection.request?.({ type: 'keys.get' }).then(applyKeys, () => {});
  connection.on?.('keys', applyKeys);
  connection.on?.('welcome', ask);
  if (connection.status === 'open') ask();
}

/** Whether `target` is a field being typed in: it keeps every key. */
export function isTextField(target) {
  if (!target || target.nodeType !== 1) return false;
  if (target.isContentEditable) return true;
  const tag = target.localName;
  if (tag === 'textarea' || tag === 'select') return true;
  if (tag !== 'input') return false;
  return !['button', 'checkbox', 'radio', 'range', 'color', 'file', 'submit', 'reset', 'image'].includes(String(target.type).toLowerCase());
}

/** Whether `target` is a native button or link, whose Space and Enter are its own. */
function isPressable(target) {
  if (!target || target.nodeType !== 1) return false;
  return ['button', 'a', 'summary'].includes(target.localName) || (target.localName === 'input' && !isTextField(target));
}

function scrollable(element) {
  const view = element.ownerDocument?.defaultView;
  if (!view) return false;
  if (element === element.ownerDocument.scrollingElement) return element.scrollHeight > element.clientHeight + 1;
  const style = view.getComputedStyle(element);
  return /(auto|scroll|overlay)/.test(style.overflowY) && element.scrollHeight > element.clientHeight + 1;
}

/** The box the view's keys scroll for an event at `start`: the scrolling box around it, else the page's view. */
export function scrollerFor(start, doc) {
  for (let at = start; at; at = at.parentElement ?? at.getRootNode?.()?.host ?? null) {
    if (at.nodeType === 1 && scrollable(at)) return at;
  }
  const given = viewScroller?.(doc);
  if (given) return given;
  const root = doc.scrollingElement;
  return root && scrollable(root) ? root : null;
}

function reduced(doc) {
  return Boolean(doc.defaultView?.matchMedia?.('(prefers-reduced-motion: reduce)').matches);
}

/** Scrolls `box` for view command `id`; whether it moved anything. */
export function scrollView(box, id) {
  if (!box) return false;
  const page = Math.max(LINE_PX, box.clientHeight * PAGE_SHARE);
  const top = {
    'view.page_down': page,
    'view.page_up': -page,
    'view.line_down': LINE_PX,
    'view.line_up': -LINE_PX,
  }[id];
  const behavior = reduced(box.ownerDocument) ? 'auto' : 'smooth';
  if (top !== undefined) box.scrollBy({ top, behavior });
  else if (id === 'view.top') box.scrollTo({ top: 0, behavior });
  else if (id === 'view.bottom') box.scrollTo({ top: box.scrollHeight, behavior });
  else return false;
  return true;
}

/** The window Signal.app's commands are told on: the top one of the same origin. */
function commandWindow(view) {
  try {
    return view.top && view.top.document ? view.top : view;
  } catch {
    return view;
  }
}

function onKey(event) {
  if (event.defaultPrevented || event.isComposing) return;
  const doc = event.currentTarget;
  const view = doc.defaultView;
  const target = event.composedPath?.()[0] ?? event.target;
  const chord = eventChord(event);
  if (!chord) return;
  // A field being typed in, and a native range, checkbox or radio, keep their
  // keys; Cmd or Ctrl chords still reach the app's commands (the field commits
  // a changed text first: ./entry-field.js, docs/ui-controls.md "Committing").
  const field = isTextField(target) || target?.localName === 'input';
  const { mods } = splitChord(chord);
  if (field && !mods.has('Cmd') && !mods.has('Ctrl')) return;
  const viewCommand = field ? null : keyCommand('view', event);
  if (viewCommand && !(isPressable(target) && /(^|\+)(Space|Enter)$/.test(chord))) {
    if (scrollView(scrollerFor(target, doc), viewCommand.id)) event.preventDefault();
    return;
  }
  const app = keyCommand('app', event);
  const command = app && keymap().commands.get(app.id);
  if (!command || command.fixed) return;
  // Signal.app's page is the top one (a panel frame's keys reach the app through it).
  const top = commandWindow(view);
  if (top.signalApp?.menu) {
    // The native menu takes its item's chord itself; the others are handed to it, once per press.
    if (command.menu && command.keys[0] === chord) return;
    if (!top.ipc?.postMessage) return;
    event.preventDefault();
    if (!event.repeat) top.ipc.postMessage(JSON.stringify({ type: 'menu.run', command: command.id }));
    return;
  }
  // In a browser the page runs what it can (it cancels the event); the browser keeps the rest (⌘N, ⌘Q, …).
  // `repeat`: a held chord's repeats are taken but not run again.
  const asked = new CustomEvent('signal-command', { detail: command.id, cancelable: true });
  asked.repeat = event.repeat;
  if (!top.dispatchEvent(asked)) event.preventDefault();
}

// ------------------------------------------------------------ the wheel

const latches = new WeakMap();
/** Controls given the wheel back by Escape, until they are pressed or focused again. */
const released = new WeakSet();
const hints = new WeakMap();

function latchOf(doc) {
  let latch = latches.get(doc);
  if (!latch) {
    latch = new WheelLatch();
    latches.set(doc, latch);
  }
  return latch;
}

function focusedWithin(element) {
  return element.matches?.(':focus-within') || element.ownerDocument?.activeElement === element;
}

/**
 * A wheel event's vertical travel: deltaY, or deltaX with Shift only (a
 * mouse's Shift+wheel arrives as deltaX on a Mac). A sideways trackpad
 * swipe is never a value change or a zoom.
 */
export function verticalDelta(event) {
  return event.deltaY || (event.shiftKey ? event.deltaX : 0);
}

/** Controls that use the wheel (scroll, zoom or change a value): the gesture's first event decides whether one takes it. */
const wheelControls = new WeakSet();
/** A control that uses the wheel only at some of its points: whether `event` is at one. */
const wheelParts = new WeakMap();

/**
 * Marks `element` as a control that uses the wheel: every part of it, or,
 * with `uses(event)`, only where that says so (the step grid's level
 * knobs). A gesture starting elsewhere on it is the page's. A gesture it
 * takes never scrolls the page.
 */
export function registerWheel(element, uses) {
  wheelControls.add(element);
  if (uses) wheelParts.set(element, uses);
  install(element.ownerDocument);
}

/** Whether `event` is at a point of `element` that uses the wheel. */
function usesWheelAt(element, event) {
  const uses = wheelParts.get(element);
  return !uses || Boolean(uses(event));
}

/** Whether `element` owns the wheel for `event` (ui/lib/wheel.js). */
function ownsFor(element, event) {
  const mode = element.getAttribute?.('wheel');
  const view = element.ownerDocument?.defaultView;
  return ownsWheel({
    never: mode === 'page',
    fills: mode === 'own' || (mode !== 'page' && fillsWindow(element.getBoundingClientRect?.(), { width: view?.innerWidth, height: view?.innerHeight })),
    focused: focusedWithin(element),
    released: released.has(element),
    modifier: wheelModifierHeld(event),
  });
}

/** A new gesture's first event over `element`: it takes the gesture, or (once, when due) the hint says how. */
function decide(latch, element, event) {
  // Over a part without a wheel use the gesture is the page's, without a hint.
  if (!usesWheelAt(element, event)) {
    latch.claim(element, false);
    return;
  }
  const owns = ownsFor(element, event);
  latch.claim(element, owns);
  if (!owns && latch.hint(element)) showHint(element.ownerDocument, event);
}

/**
 * Whether `element` takes this wheel event (call it from its wheel
 * listener; when false, leave the event alone: the page scrolls). The
 * gesture's first event decides, over the whole control (`registerWheel`),
 * in the document's capture listener: it takes the gesture when it owns the
 * wheel. Otherwise, when the gesture started over it, a hint says how to
 * give it the wheel (once per gesture, not while the page is being scrolled).
 */
export function takesWheel(element, event) {
  if (!wheelControls.has(element)) registerWheel(element);
  const latch = latchOf(element.ownerDocument);
  if (latch.owner === undefined) decide(latch, element, event);
  return latch.owner === element;
}

function showHint(doc, event) {
  const keys = keymap().commands.get('view.wheel_inside')?.keys ?? [];
  const modifier = keys.length ? formatChord(keys[0], { mac: isMac() }).replace(/\s*Wheel$/, '') : '';
  let hint = hints.get(doc);
  if (!hint) {
    hint = doc.createElement('div');
    hint.className = 'signal-wheel-hint';
    hint.setAttribute('aria-hidden', 'true');
    Object.assign(hint.style, {
      position: 'fixed', zIndex: '2147483000', pointerEvents: 'none', padding: '3px 8px', borderRadius: '4px',
      font: '12px/1.3 -apple-system, BlinkMacSystemFont, "Helvetica Neue", sans-serif', whiteSpace: 'nowrap',
      background: 'var(--signal-panel, #222)', color: 'var(--signal-fg, #ddd)', border: '1px solid var(--signal-line, #444)',
      opacity: '0', transition: 'opacity 160ms ease-out', left: '0', top: '0',
    });
    doc.body?.append(hint);
    hints.set(doc, hint);
  }
  hint.textContent = wheelHint(modifier);
  const view = doc.defaultView;
  const x = Math.min(event.clientX + 14, (view?.innerWidth ?? 0) - 260);
  const y = Math.min(event.clientY + 16, (view?.innerHeight ?? 0) - 30);
  hint.style.left = `${Math.max(4, x)}px`;
  hint.style.top = `${Math.max(4, y)}px`;
  hint.style.opacity = '0.92';
  clearTimeout(hint.timer);
  hint.timer = setTimeout(() => (hint.style.opacity = '0'), 1400);
}

function onWheelFirst(event) {
  const latch = latchOf(event.currentTarget);
  latch.seen(event.timeStamp || performance.now());
  if (latch.owner === undefined) {
    const control = event.composedPath().find((n) => wheelControls.has(n));
    if (control) decide(latch, control, event);
  }
  // The gesture is a control's: the page never scrolls, wherever the pointer
  // is (outside it, or over a part of it without a wheel use of its own).
  if (latch.owner) event.preventDefault();
}

function onWheelLast(event) {
  latchOf(event.currentTarget).settle();
}

/** The control a press or focus at `event` reaches (the innermost custom element in its path). */
function controlOf(event) {
  return event.composedPath().find((n) => n.nodeType === 1 && n.localName?.includes('-')) ?? null;
}

/** Sets up `doc` once: its keys, the wheel latch, Escape. */
export function install(doc = globalThis.document) {
  if (!doc || installed.has(doc)) return;
  installed.add(doc);
  doc.addEventListener('keydown', onKey);
  // Escape gives the wheel back to the page (the control keeps its own Escape).
  doc.addEventListener('keydown', (event) => {
    if (event.key !== 'Escape') return;
    const control = controlOf(event);
    if (control) released.add(control);
  }, true);
  const engage = (event) => {
    const control = controlOf(event);
    if (control) released.delete(control);
  };
  doc.addEventListener('pointerdown', engage, true);
  doc.addEventListener('focusin', engage, true);
  doc.addEventListener('wheel', onWheelFirst, { capture: true, passive: false });
  doc.addEventListener('wheel', onWheelLast, { passive: true });
}

if (globalThis.document) {
  install(globalThis.document);
  follow(getDefaultConnection());
  onDefaultConnection(follow);
}
