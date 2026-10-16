// A context menu's element (2026-10-17): Signal.app's menu and
// <signal-schematic>'s own in a browser are this one, so both look and
// behave alike. `role="menu"`, its items `menuitem` buttons; an item that
// does not apply is `aria-disabled` (it takes the focus, its title says
// why, it does nothing). Arrow keys, Home and End move, Enter or Space
// activates, Escape or Tab closes (ui/lib/context-menu.js). Placed `fixed`
// at the click, inside the window, over everything and never in the flow.
// Closing on a press outside, a scroll or the window's blur is its owner's.
// Every key pressed in it is its own: none reaches the page below (the
// schematic's Delete or ⌘Z would act on its selection, not the menu's
// target). A page menu (Signal.app's) claims its document (`claimMenus`):
// an element with items of its own (<signal-schematic>) then gives them to
// it instead of opening its own menu.

import { firstIndex, menuKey, placeMenu } from '../lib/context-menu.js';

/** Its look, for a shadow root (Signal.app's is in ui/app/app.css). */
export const MENU_CSS = `
.context-menu { position: fixed; left: 0; top: 0; z-index: 100; min-width: 13rem; padding: 0.3rem; display: flex; flex-direction: column; box-sizing: border-box;
  background: var(--signal-panel, #1f232b); border: 1px solid var(--signal-track, #2a2f38); border-radius: 8px; box-shadow: 0 8px 24px rgba(0, 0, 0, 0.45); }
.context-menu[hidden] { display: none; }
.context-menu button { font: inherit; color: var(--signal-fg, #d6dbe4); text-align: left; border: 0; background: transparent; padding: 0.3rem 0.6rem; border-radius: 4px; white-space: nowrap; }
.context-menu button:hover:not([aria-disabled="true"]), .context-menu button:focus { background: var(--signal-knob, #262b34); outline: none; }
.context-menu button[aria-disabled="true"] { opacity: 0.45; }
.context-menu hr { border: 0; border-top: 1px solid var(--signal-track, #2a2f38); margin: 0.25rem 0.2rem; }
`;

/** The documents whose right clicks a page menu takes. */
const claimed = new WeakSet();

/** A page menu takes the right clicks of `doc`; returns the remover. */
export function claimMenus(doc) {
  claimed.add(doc);
  return () => claimed.delete(doc);
}

/** Whether a page menu takes the right clicks of `doc`. */
export function menusClaimed(doc) {
  return Boolean(doc) && claimed.has(doc);
}

/** Whether `node` can take the focus back when the menu closes (duck-typed: a node of another frame's realm too). */
export function focusable(node) {
  return node?.nodeType === 1 && typeof node.focus === 'function' && node.tabIndex >= 0;
}

export class ContextMenu {
  /** `label`: the menu's accessible name; `doc`: the document it is built in. */
  constructor({ label = 'Context menu', doc = document } = {}) {
    this.doc = doc;
    this.list = Object.assign(doc.createElement('div'), { className: 'context-menu', hidden: true });
    this.list.setAttribute('role', 'menu');
    this.list.setAttribute('aria-label', label);
    this.items = [];
    this.buttons = [];
    this.at = -1;
    this.pick = null;
    this.closed = null;
    this.list.addEventListener('keydown', (event) => this.key(event));
    // Its pointer, wheel and right clicks are its own, not the page's below.
    for (const type of ['pointerdown', 'pointerup', 'pointermove', 'click', 'dblclick', 'wheel']) this.list.addEventListener(type, (event) => event.stopPropagation());
    this.list.addEventListener('contextmenu', (event) => {
      event.preventDefault();
      event.stopPropagation();
    });
  }

  get isOpen() {
    return !this.list.hidden;
  }

  /**
   * Shows `items` at client point `x`, `y`: `pick(id)` runs an enabled
   * item's action after the menu closes; `closed()` runs once when it
   * closes, however; `back` takes the focus again when the menu closes
   * with the focus in it (an item, Escape, Tab, or closed by its owner).
   */
  show(items, { x, y, pick, closed = null, back = null }) {
    this.close();
    this.items = items;
    this.pick = pick;
    this.closed = closed;
    this.back = back;
    this.buttons = items.map((item, index) => {
      if (item.id === 'separator') {
        const hr = this.doc.createElement('hr');
        hr.setAttribute('role', 'separator');
        return hr;
      }
      const button = Object.assign(this.doc.createElement('button'), { type: 'button', textContent: item.label, tabIndex: -1 });
      button.setAttribute('role', 'menuitem');
      if (item.title) {
        button.title = item.title;
        // The reason read out with the item, not only shown on hover.
        button.setAttribute('aria-description', item.title);
      }
      if (item.disabled) button.setAttribute('aria-disabled', 'true');
      button.addEventListener('focus', () => (this.at = index));
      button.addEventListener('click', () => this.activate(index));
      return button;
    });
    this.list.replaceChildren(...this.buttons);
    this.list.hidden = false;
    const box = this.list.getBoundingClientRect();
    const { left, top } = placeMenu(x, y, box.width, box.height, globalThis.innerWidth ?? 0, globalThis.innerHeight ?? 0);
    this.list.style.left = `${left}px`;
    this.list.style.top = `${top}px`;
    this.focusAt(firstIndex(items));
  }

  focusAt(index) {
    this.at = index;
    this.buttons[index]?.focus({ preventScroll: true });
  }

  activate(index) {
    const item = this.items[index];
    if (!item || item.id === 'separator' || item.disabled) return;
    const pick = this.pick;
    this.close();
    pick?.(item.id);
  }

  key(event) {
    // No key pressed in the menu is the page's (the schematic's Escape
    // leaves Edit patch, its Delete removes the selection).
    event.stopPropagation();
    const action = menuKey(event.key, this.at, this.items);
    if (!action) return;
    event.preventDefault();
    if ('focus' in action) this.focusAt(action.focus);
    else if (action.activate !== undefined) this.activate(this.at);
    else if (action.close) this.close();
  }

  /** Closed; the focus, if it was in the menu, back on `back`, never left on a removed item. */
  close() {
    if (this.list.hidden) return;
    const root = this.list.getRootNode?.() ?? this.doc;
    const inside = Boolean(root.activeElement) && this.list.contains(root.activeElement);
    const back = this.back;
    this.back = null;
    if (inside && focusable(back)) back.focus({ preventScroll: true });
    else if (inside) root.activeElement.blur?.();
    this.list.hidden = true;
    this.list.replaceChildren();
    this.buttons = [];
    this.at = -1;
    const closed = this.closed;
    this.closed = null;
    closed?.();
  }
}
