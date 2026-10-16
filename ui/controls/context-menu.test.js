// The context menu's element (docs/ui-controls.md "Schematic", the context
// menu) on the least of a DOM it needs: its keys (none reaches the page
// below), activation, the focus given back, the reason of a disabled item
// for assistive technology, and a page menu claiming its document.

import test from 'node:test';
import assert from 'node:assert/strict';
import { ContextMenu, claimMenus, focusable, menusClaimed } from './context-menu.js';

class Node {
  constructor(doc, tag) {
    this.doc = doc;
    this.localName = tag;
    this.nodeType = 1;
    this.tabIndex = tag === 'button' ? 0 : -1;
    this.attributes = new Map();
    this.listeners = {};
    this.children = [];
    this.parent = null;
    this.style = {};
    this.hidden = false;
  }
  setAttribute(name, value) {
    this.attributes.set(name, String(value));
  }
  getAttribute(name) {
    return this.attributes.get(name) ?? null;
  }
  addEventListener(type, fn) {
    (this.listeners[type] ??= []).push(fn);
  }
  dispatch(type, init = {}) {
    const event = { type, stopped: false, prevented: false, stopPropagation() { this.stopped = true; }, preventDefault() { this.prevented = true; }, ...init };
    for (let at = this; at && !event.stopped; at = at.parent) for (const fn of at.listeners[type] ?? []) fn(event);
    return event;
  }
  replaceChildren(...nodes) {
    for (const child of this.children) child.parent = null;
    this.children = nodes;
    for (const child of nodes) child.parent = this;
  }
  contains(node) {
    for (let at = node; at; at = at.parent) if (at === this) return true;
    return false;
  }
  focus() {
    this.doc.activeElement = this;
  }
  blur() {
    if (this.doc.activeElement === this) this.doc.activeElement = null;
  }
  getRootNode() {
    return this.doc;
  }
  getBoundingClientRect() {
    return { width: 200, height: 100 };
  }
}

function fakeDocument() {
  const doc = { activeElement: null };
  doc.createElement = (tag) => new Node(doc, tag);
  return doc;
}

const ITEMS = [
  { id: 'inspect', label: 'Inspect' },
  { id: 'delete', label: 'Delete instance', disabled: true, title: 'Turn on Edit patch to change the patch' },
  { id: 'separator' },
  { id: 'fit', label: 'Fit' },
];

function opened() {
  const doc = fakeDocument();
  const page = doc.createElement('signal-schematic');
  page.tabIndex = 0;
  const menu = new ContextMenu({ label: 'Schematic', doc });
  page.append = (node) => (node.parent = page);
  page.append(menu.list);
  const picked = [];
  let closed = 0;
  page.focus();
  menu.show(ITEMS, { x: 10, y: 10, pick: (id) => picked.push(id), closed: () => closed++, back: page });
  return { doc, page, menu, picked, closes: () => closed };
}

test('the menu is named, its items menuitems, a disabled one carries its reason for a screen reader too', () => {
  const { menu, doc } = opened();
  assert.equal(menu.list.getAttribute('role'), 'menu');
  assert.equal(menu.list.getAttribute('aria-label'), 'Schematic');
  const [inspect, del, hr] = menu.list.children;
  assert.equal(inspect.getAttribute('role'), 'menuitem');
  assert.equal(del.getAttribute('aria-disabled'), 'true');
  assert.equal(del.getAttribute('aria-description'), 'Turn on Edit patch to change the patch');
  assert.equal(hr.getAttribute('role'), 'separator');
  assert.equal(doc.activeElement, inspect, 'the first enabled item has the focus');
});

test('no key pressed in the menu reaches the page: Delete and ⌘Z act on nothing below', () => {
  const { doc, page, menu, picked } = opened();
  const below = [];
  page.addEventListener('keydown', (event) => below.push(event.key));
  for (const key of ['Delete', 'Backspace', 'z', 'ArrowDown', 'Escape']) doc.activeElement.dispatch('keydown', { key, metaKey: key === 'z' });
  assert.deepEqual(below, []);
  assert.deepEqual(picked, []);
  assert.equal(menu.isOpen, false, 'Escape closed it');
});

test('arrows move onto a disabled item, which Enter does not activate; an enabled one runs after the menu closes, the focus given back', () => {
  const { doc, page, menu, picked, closes } = opened();
  doc.activeElement.dispatch('keydown', { key: 'ArrowDown' });
  assert.equal(doc.activeElement, menu.list.children[1]);
  doc.activeElement.dispatch('keydown', { key: 'Enter' });
  assert.deepEqual(picked, []);
  assert.equal(menu.isOpen, true);
  doc.activeElement.dispatch('keydown', { key: 'ArrowDown' });
  doc.activeElement.dispatch('keydown', { key: 'Enter' });
  assert.deepEqual(picked, ['fit']);
  assert.equal(menu.isOpen, false);
  assert.equal(closes(), 1);
  assert.equal(doc.activeElement, page, 'not left on a removed item');
});

test('Tab, or a close by its owner (a push without its target), gives the focus back', () => {
  const tab = opened();
  tab.doc.activeElement.dispatch('keydown', { key: 'Tab' });
  assert.equal(tab.doc.activeElement, tab.page);
  const pushed = opened();
  pushed.menu.close();
  assert.equal(pushed.doc.activeElement, pushed.page);
  assert.equal(pushed.closes(), 1);
  pushed.menu.close();
  assert.equal(pushed.closes(), 1, 'closed once');
});

test('a page menu claims its document; focus is duck-typed (another frame’s nodes)', () => {
  const doc = {};
  assert.equal(menusClaimed(doc), false);
  const release = claimMenus(doc);
  assert.equal(menusClaimed(doc), true);
  release();
  assert.equal(menusClaimed(doc), false);
  assert.equal(menusClaimed(null), false);
  assert.equal(focusable({ nodeType: 1, tabIndex: 0, focus() {} }), true);
  assert.equal(focusable({ nodeType: 1, tabIndex: -1, focus() {} }), false, 'an SVG element');
  assert.equal(focusable({ nodeType: 9, tabIndex: 0, focus() {} }), false);
});
