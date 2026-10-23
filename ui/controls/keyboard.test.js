import test from 'node:test';
import assert from 'node:assert/strict';

import { LINE_PX, PAGE_SHARE, install, isTextField, registerWheel, runProjectCommand, scrollView, scrollerFor, setViewScroller } from './keyboard.js';
import { setDefaultConnection } from '../signal-ui.js';

const el = (localName, more = {}) => ({ nodeType: 1, localName, ...more });

test('a field being typed in keeps its keys; buttons, checkboxes and ranges do not', () => {
  assert.equal(isTextField(el('input', { type: 'text' })), true);
  assert.equal(isTextField(el('input', { type: 'search' })), true);
  assert.equal(isTextField(el('textarea')), true);
  assert.equal(isTextField(el('select')), true);
  assert.equal(isTextField(el('div', { isContentEditable: true })), true);
  assert.equal(isTextField(el('input', { type: 'checkbox' })), false);
  assert.equal(isTextField(el('input', { type: 'range' })), false);
  assert.equal(isTextField(el('button')), false);
  assert.equal(isTextField(el('signal-knob')), false);
  assert.equal(isTextField(null), false);
});

function box(clientHeight, scrollHeight, overflowY = 'auto') {
  const calls = [];
  const doc = { defaultView: { getComputedStyle: () => ({ overflowY }), matchMedia: () => ({ matches: true }) }, scrollingElement: null };
  return {
    calls, nodeType: 1, localName: 'div', clientHeight, scrollHeight, ownerDocument: doc, parentElement: null,
    scrollBy: (o) => calls.push(['by', o.top]),
    scrollTo: (o) => calls.push(['to', o.top]),
  };
}

test('the view commands scroll a page, a little, or to an end', () => {
  const b = box(400, 2000);
  assert.equal(scrollView(b, 'view.page_down'), true);
  assert.equal(scrollView(b, 'view.page_up'), true);
  assert.equal(scrollView(b, 'view.line_down'), true);
  assert.equal(scrollView(b, 'view.top'), true);
  assert.equal(scrollView(b, 'view.bottom'), true);
  assert.equal(scrollView(b, 'view.wheel_inside'), false);
  assert.equal(scrollView(null, 'view.page_down'), false);
  assert.deepEqual(b.calls, [['by', 400 * PAGE_SHARE], ['by', -400 * PAGE_SHARE], ['by', LINE_PX], ['to', 0], ['to', 2000]]);
});

test('the box scrolled is the scrolling one around the focus, else the page view given', () => {
  const outer = box(400, 2000);
  const inner = { ...box(100, 100), parentElement: outer };
  assert.equal(scrollerFor(inner, outer.ownerDocument), outer, 'the inner box does not scroll');
  const hidden = box(400, 2000, 'hidden');
  const view = box(300, 900);
  setViewScroller(() => view);
  assert.equal(scrollerFor(hidden, hidden.ownerDocument), view);
  setViewScroller(null);
  assert.equal(scrollerFor(hidden, hidden.ownerDocument), null, 'nothing scrolls');
});

test('a control that uses the wheel at some of its points takes a gesture only when it starts at one', () => {
  const listeners = [];
  const doc = { addEventListener: (type, fn, options) => listeners.push({ type, fn, capture: options === true || options?.capture }), defaultView: { innerWidth: 1000, innerHeight: 800 } };
  install(doc);
  // A focused step grid: it owns the wheel, but only its level knobs use it.
  const grid = {
    nodeType: 1, localName: 'signal-step-grid', ownerDocument: doc,
    getAttribute: () => null, matches: (q) => q === ':focus-within',
    getBoundingClientRect: () => ({ width: 400, height: 200 }),
  };
  registerWheel(grid, (event) => event.clientX < 50);
  const first = listeners.find((l) => l.type === 'wheel' && l.capture).fn;
  const last = listeners.find((l) => l.type === 'wheel' && !l.capture).fn;
  const wheel = (clientX, timeStamp) => {
    const event = { currentTarget: doc, clientX, clientY: 10, deltaY: 4, timeStamp, composedPath: () => [grid], prevented: false, preventDefault() { this.prevented = true; } };
    first(event);
    last(event);
    return event.prevented;
  };
  assert.equal(wheel(200, 1000), false, 'over the steps the page scrolls');
  assert.equal(wheel(20, 1100), false, 'and keeps scrolling over a knob in the same gesture');
  assert.equal(wheel(20, 2000), true, 'a gesture starting over a knob is the grid\'s');
  assert.equal(wheel(200, 2100), true, 'and never scrolls the page');
});

/** A connection to studio-transport as `describe` gives it, recording its commits. */
function transportConnection(base = 1) {
  return {
    status: 'open',
    commits: [],
    description: {
      playing: true,
      instances: [{ address: '/transport/playhead', module: 'std.playhead', parameters: [{ name: 'run', address: '/transport/playhead.run' }] }],
      controls: [{ address: '/transport.run', ports: ['/transport/playhead.run'], base, min: 0, max: 1, default: 1, modulated: false }],
    },
    commit(address, value) {
      this.commits.push([address, value]);
      return Promise.resolve({ type: 'committed' });
    },
  };
}

test('Space plays and stops the transport where no control, field or button takes it (ADR 0039)', () => {
  const listeners = [];
  const scrolled = [];
  const doc = {
    addEventListener: (type, fn, options) => listeners.push({ type, fn, capture: options === true || options?.capture }),
    defaultView: { top: null, innerWidth: 1000, innerHeight: 800, getComputedStyle: () => ({ overflowY: 'auto' }), matchMedia: () => ({ matches: true }), dispatchEvent: () => true },
    scrollingElement: { nodeType: 1, clientHeight: 400, scrollHeight: 2000, scrollBy: (o) => scrolled.push(o.top), scrollTo: () => {} },
  };
  doc.defaultView.top = doc.defaultView;
  doc.defaultView.document = doc;
  doc.scrollingElement.ownerDocument = doc;
  install(doc);
  const onKey = listeners.find((l) => l.type === 'keydown' && !l.capture).fn;
  const connection = transportConnection(1);
  setDefaultConnection(connection);
  const press = (target, more = {}) => {
    const event = { key: ' ', code: 'Space', currentTarget: doc, target, composedPath: () => [target], defaultPrevented: false, prevented: false, preventDefault() { this.prevented = true; }, ...more };
    onKey(event);
    return event.prevented;
  };
  const body = el('body', { parentElement: null });
  assert.equal(press(body), true, 'taken');
  assert.deepEqual(connection.commits, [['/transport.run', 0]], 'running: stopped, as a click on its toggle writes');
  assert.equal(press(body), true);
  assert.deepEqual(connection.commits.at(-1), ['/transport.run', 1], 'before the next describe: what was sent counts');
  assert.equal(press(body, { repeat: true }), true, 'a held Space');
  assert.equal(connection.commits.length, 2, 'repeats do nothing');
  assert.deepEqual(scrolled, [], 'Space no longer scrolls');
  // A native button, a text field and a focused control that handled it keep their Space.
  assert.equal(press(el('button')), false);
  assert.equal(press(el('input', { type: 'text' })), false);
  assert.equal(press(body, { defaultPrevented: true }), false);
  assert.equal(connection.commits.length, 2);
  // Page Down still scrolls the page.
  onKey({ key: 'PageDown', currentTarget: doc, target: body, composedPath: () => [body], defaultPrevented: false, preventDefault() {} });
  assert.deepEqual(scrolled, [400 * PAGE_SHARE]);
  setDefaultConnection(null);
});

test('the transport command does nothing without a connection, while nothing plays, or with no transport', () => {
  assert.equal(runProjectCommand('view.page_down', transportConnection()), false, 'not a project command');
  assert.equal(runProjectCommand('transport.toggle', null), true);
  const closed = { ...transportConnection(), status: 'reconnecting' };
  runProjectCommand('transport.toggle', closed);
  assert.deepEqual(closed.commits, []);
  const stopped = transportConnection();
  stopped.description = { ...stopped.description, playing: false };
  runProjectCommand('transport.toggle', stopped);
  assert.deepEqual(stopped.commits, []);
  const none = transportConnection();
  none.description = { ...none.description, instances: [] };
  runProjectCommand('transport.toggle', none);
  assert.deepEqual(none.commits, []);
  // A new description: its base is the truth again.
  const fresh = transportConnection(0);
  runProjectCommand('transport.toggle', fresh);
  fresh.description = { ...fresh.description, controls: fresh.description.controls.map((c) => ({ ...c, base: 1 })) };
  runProjectCommand('transport.toggle', fresh);
  assert.deepEqual(fresh.commits, [['/transport.run', 1], ['/transport.run', 0]]);
});

test('a refused transport write is not counted: the next press goes by the description again', async () => {
  const refusing = transportConnection(1);
  refusing.commit = function (address, value) {
    this.commits.push([address, value]);
    return Promise.reject(Object.assign(new Error('write-failed'), { code: 'write-failed' }));
  };
  runProjectCommand('transport.toggle', refusing);
  await new Promise((resolve) => setTimeout(resolve, 0));
  runProjectCommand('transport.toggle', refusing);
  assert.deepEqual(refusing.commits, [['/transport.run', 0], ['/transport.run', 0]], 'still running as described: stop again');
});
