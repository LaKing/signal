import test from 'node:test';
import assert from 'node:assert/strict';

import { LINE_PX, PAGE_SHARE, isTextField, scrollView, scrollerFor, setViewScroller } from './keyboard.js';

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
