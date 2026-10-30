// The project panel's cards across Signal.app's windows (ADR 0032,
// docs/ui-controls.md "Cards across windows"). The project's main panel
// (`ui/main.html`) stays the one source of its cards; each window shows a
// subset: a panel window the cards it holds, in its order (none until told,
// so a new panel window is empty), and a main window's Panel tab every card,
// those another window holds as a compact placeholder in their place ("—
// in Panel window 2 (screen 1)", Show, Bring back). Which window holds which
// card is the project's `ui/windows.toml`, kept by the app (the native
// side); a page only shows what it is told (`signal-window`) and says what
// the person chose (`card.move`, `card.order`, `window.show`, `window.fit`).
//
// A card is moved from its context menu (owner's decision, 2026-10-22: "not
// drag and drop — the panel elements should be movable from the context
// menu"): a right click on a card (its background or heading; a control
// bound to a parameter keeps its own menu, a text field the system's) shows
// Move to each other window, Move to new panel window, in a panel window
// Bring back to main window, and Move up / Move down where it has
// neighbours. In a main window these items come first in the app's own
// menu (the card is a `part` of it, as the schematic is); in a panel window
// the same menu element (ui/controls/context-menu.js) opens in the page.
// Not while the panel's layout is edited.
//
// Nothing is added to the panel's own DOM: the placeholders are an overlay
// in the frame's body, outside `<signal-panel>`, and cards are only hidden,
// ordered and collapsed with inline CSS, so the panel file, its editor and
// `panel.written` see the panel as written. A card's id is its `id`
// attribute when it has one, else the slug of its first heading (`kick`,
// `output`; a second card of the same heading `kick-2`), else `card`
// (`card-2`, …), in the panel's order. ADR 0046 §5: an open slot card
// (`<signal-slot>` among the cards) is a card titled by its slot, and the
// cards of its part are cards of their own, their ids counted in the part
// and prefixed with the slot's name (`synth:mono-synth`), as the server
// names them when Group into slot moves a card (`ui/windows.toml`).

import { ContextMenu, menusClaimed } from '../../controls/context-menu.js';
import { keepsSystemMenu } from './context.js';

/** A heading's slug: lowercase letters and digits, the rest as single `-`. */
export function slug(text) {
  const s = String(text ?? '')
    .normalize('NFKD')
    .replace(/[̀-ͯ]/g, '')
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, '-')
    .replace(/^-+|-+$/g, '')
    .slice(0, 64);
  return s || 'card';
}

/** Whether `id` is a card id the app keeps (as `layout::is_card_id`). */
export function isCardId(id) {
  return typeof id === 'string' && id.length > 0 && id.length <= 128 && /^[A-Za-z0-9_.:-]+$/.test(id);
}

/**
 * The ids of cards `cards` (each `{ id, title }`, in the panel's order):
 * its own id when valid, else its title's slug, made unique with `-2`, `-3`.
 */
export function cardIds(cards) {
  const taken = new Set();
  const own = new Set(cards.map((c) => c.id).filter(isCardId));
  return cards.map((card) => {
    if (isCardId(card.id) && !taken.has(card.id)) {
      taken.add(card.id);
      return card.id;
    }
    const base = slug(card.title);
    let id = base;
    for (let n = 2; taken.has(id) || own.has(id); n += 1) id = `${base}-${n}`;
    taken.add(id);
    return id;
  });
}

/** What a panel window shows before the app says: nothing. */
export const EMPTY_PANEL = Object.freeze({ view: 'panel', cards: [] });

/**
 * What a window shows of the cards `ids` (in the panel's order), told by
 * the app as `state`: a panel window (`{ view: 'panel', cards }`) its cards
 * in its order, none without them; a main window (`{ view: 'main', held,
 * order }`) every card, those in `order` first, a `held` one (`{ card,
 * where }`, `where` the window holding it) as a placeholder. `null` (a
 * browser, or a main window not told yet): every card. Returns `[{ id,
 * shown, order, held }]`, `held` the placeholder's text or null.
 */
export function arrangement(ids, state) {
  const panel = state?.view === 'panel';
  const listed = panel ? (state.cards ?? []) : (state?.order ?? []);
  const held = new Map(panel ? [] : (state?.held ?? []).map((h) => (typeof h === 'string' ? [h, ''] : [h.card, h.where ?? ''])));
  const rank = new Map(listed.map((id, i) => [id, i]));
  return ids.map((id, i) => ({
    id,
    shown: panel ? rank.has(id) : true,
    order: rank.has(id) ? rank.get(id) : listed.length + i,
    held: held.has(id) ? held.get(id) : null,
  }));
}

/**
 * The order of a window's own cards (`placed`, `arrangement`'s, without
 * the hidden and held ones) with `card` moved one place by `step` (-1 up,
 * +1 down); null when it cannot move that way.
 */
export function moved(placed, card, step) {
  const order = placed
    .filter((c) => c.shown && c.held === null)
    .sort((a, b) => a.order - b.order)
    .map((c) => c.id);
  const at = order.indexOf(card);
  const to = at + step;
  if (at < 0 || to < 0 || to >= order.length) return null;
  [order[at], order[to]] = [order[to], order[at]];
  return order;
}

/**
 * The card menu's items for `card` in a window told `state` (`placed` its
 * arrangement): Move to each other window (`state.targets`, `{ index,
 * label }`), Move to new panel window, in a panel window Bring back to main
 * window, and Move up / Move down when the window has other cards. Items
 * are ui/lib/context-menu.js's (`{ id, label, disabled? }`).
 */
export function cardMenu(state, placed, card) {
  const items = (state?.targets ?? []).map((t) => ({ id: `move:${t.index}`, label: `Move to ${t.label}` }));
  items.push({ id: 'new', label: 'Move to new panel window' });
  if (state?.view === 'panel') items.push({ id: 'main', label: 'Bring back to main window' });
  const own = placed.filter((c) => c.shown && c.held === null).length;
  if (own > 1) {
    items.push({ id: 'separator' });
    items.push({ id: 'up', label: 'Move up', disabled: !moved(placed, card, -1) });
    items.push({ id: 'down', label: 'Move down', disabled: !moved(placed, card, 1) });
  }
  return items;
}

/**
 * What choosing `id` of the card menu for `card` sends the app (`size`:
 * the window a new one needs), or null.
 */
export function menuMessage(id, card, placed, size) {
  if (id === 'up' || id === 'down') {
    const cards = moved(placed, card, id === 'up' ? -1 : 1);
    return cards ? { type: 'card.order', cards } : null;
  }
  if (id === 'new' || id === 'main') return { type: 'card.move', card, to: id, size };
  if (id.startsWith('move:')) {
    const to = Number(id.slice(5));
    return Number.isInteger(to) && to >= 0 ? { type: 'card.move', card, to } : null;
  }
  return null;
}

/** A placeholder's height (the held card's place). */
export const PLACEHOLDER_HEIGHT = '2.25rem';

/**
 * The cards of panel document `doc`: its `.signal-card`s and its open slot
 * cards (ADR 0046 §5, `div.signal-cards > signal-slot`) with their parts'
 * cards, or a generated panel's groups.
 */
export function findCards(doc) {
  const own = [...doc.querySelectorAll('.signal-card, .signal-cards > signal-slot')].filter((c) => !c.parentElement?.closest('.signal-card'));
  if (own.length) return own;
  return [...doc.querySelectorAll('signal-panel')].flatMap((p) => [...(p.shadowRoot?.querySelectorAll('.generated > section') ?? [])]);
}

function heading(card) {
  return [...card.children].find((c) => /^H[1-6]$/.test(c.tagName)) ?? null;
}

/** The slot a `<signal-slot>` binds (`/synth` → `synth`), or null. */
function slotName(element) {
  return /^\/([a-z][a-z0-9_]*)$/.exec(element.getAttribute?.('bind')?.trim() ?? '')?.[1] ?? null;
}

/** The `<signal-slot>` whose part holds card `element` (ADR 0046), or null for a card of the host's panel. */
export function slotOfCard(element) {
  return element.parentElement?.closest?.('signal-slot') ?? null;
}

/** A card's title: its first heading's text, an open slot card's its slot. */
function titleOf(element) {
  if (element.localName === 'signal-slot') return slotName(element) ?? '';
  return (heading(element)?.textContent ?? '').trim();
}

/** `[{ element, id, title }]` of `doc`'s cards; a part's ids `<slot>:<id>`. */
export function cardsOf(doc) {
  const elements = findCards(doc);
  const titles = elements.map(titleOf);
  const groups = new Map();
  elements.forEach((element, i) => {
    const slot = slotOfCard(element);
    if (!groups.has(slot)) groups.set(slot, []);
    groups.get(slot).push(i);
  });
  const ids = [];
  for (const [slot, indexes] of groups) {
    const own = cardIds(indexes.map((i) => ({ id: elements[i].id, title: titles[i] })));
    const prefix = slot ? `${slotName(slot)}:` : '';
    indexes.forEach((i, n) => {
      ids[i] = `${prefix}${own[n]}`;
    });
  }
  // An open slot card is laid out by its own box (ADR 0046 §5: its element takes no box of its own).
  return elements.map((element, i) => ({ element: element.cardBox?.() ?? element, id: ids[i], title: titles[i] || ids[i] }));
}

/** The attribute of a card another window holds (HELD_CSS hides all of it). */
export const HELD_MARK = 'data-signal-app-held';

/** A held card's inline style: its place kept as one line, nothing of it drawn. */
export const HELD = Object.freeze({ visibility: 'hidden', height: PLACEHOLDER_HEIGHT, 'min-height': '0', 'max-height': PLACEHOLDER_HEIGHT, overflow: 'hidden', padding: '0' });

/** In the panel's document: no part of a held card shows, whatever its own CSS makes visible. */
export const HELD_CSS = `[${HELD_MARK}], [${HELD_MARK}] *, [${HELD_MARK}] *::before, [${HELD_MARK}] *::after, [${HELD_MARK}]::before, [${HELD_MARK}]::after { visibility: hidden !important; }`;

/** Shows `doc`'s cards as `state` says (hidden, ordered and collapsed with CSS only); returns `[cards, placed]`. */
export function applyCards(doc, state) {
  const cards = cardsOf(doc);
  const placed = arrangement(cards.map((c) => c.id), state);
  cards.forEach((card, i) => {
    const { element } = card;
    const p = placed[i];
    card.held = p.held;
    element.style.display = p.shown ? '' : 'none';
    element.style.order = state ? String(p.order) : '';
    // A card another window holds keeps only its place (one line), wholly
    // hidden, its placeholder drawn over it; nothing of the card shows.
    const held = p.held !== null;
    for (const [name, value] of Object.entries(HELD)) element.style.setProperty(name, held ? value : '', held ? 'important' : '');
    if (held) element.setAttribute?.(HELD_MARK, '');
    else element.removeAttribute?.(HELD_MARK);
  });
  return [cards, placed];
}

/**
 * Whether a right click along `path` (the event's composed path, from the
 * target) is the card's: not on a control bound to a parameter (it keeps
 * its own menu), a text field (the system's), or while the layout is
 * edited.
 */
export function cardClick(path, card) {
  for (const node of path) {
    if (node === card) return true;
    if (node?.binding?.address || keepsSystemMenu(node)) return false;
    if (node?.localName === 'signal-panel' && node.hasAttribute?.('editing')) return false;
  }
  return false;
}

/** The app's message channel (wry's `window.ipc`), or null outside Signal.app. */
export function appChannel(target = globalThis) {
  const ipc = target?.ipc;
  return ipc && typeof ipc.postMessage === 'function' ? (message) => ipc.postMessage(JSON.stringify(message)) : null;
}

const STYLE = {
  layer: 'position:absolute;left:0;top:0;width:0;height:0;z-index:2147483000;',
  placeholder: 'position:absolute;box-sizing:border-box;display:flex;gap:.6rem;align-items:center;padding:0 .75rem;border:1px solid var(--signal-track,#2a2f38);border-radius:8px;background:var(--signal-bg,#15181d);color:var(--signal-muted,#8a93a3);font:12px/1.3 -apple-system,BlinkMacSystemFont,sans-serif;overflow:hidden;white-space:nowrap;visibility:visible;',
  button: 'font:inherit;color:var(--signal-fg,#d6dbe4);background:var(--signal-track,#2a2f38);border:1px solid var(--signal-muted,#8a93a3);border-radius:3px;padding:.1rem .6rem;cursor:pointer;',
};

function make(doc, tag, style, text) {
  const e = doc.createElement(tag);
  e.setAttribute('style', style);
  e.dataset.signalApp = '';
  if (text !== undefined) e.textContent = text;
  return e;
}

/**
 * Connects panel frame `frame` (a same-origin iframe of the project's
 * panel) of this window to the app: tells it the window is ready, shows its
 * cards as each `signal-window` says (`initial` until then: EMPTY_PANEL in
 * a panel window, null, every card, in a main window), draws the
 * placeholders, and gives each card its menu: as a part of the app's menu
 * where the page has one (a main window), else in `menuHost`'s document
 * (a panel window). `shownCount` is told how many cards it shows. Outside
 * Signal.app it does nothing. Returns the remover.
 */
export function connectCards(frame, { target = globalThis, initial = null, shownCount = () => {}, menuHost = null } = {}) {
  const post = appChannel(target);
  if (!post) return () => {};
  let state = initial;
  let cards = [];
  let placed = [];
  let layer = null;
  let stops = [];
  let frameId = 0;
  let sizes = null;
  let menu = null;
  const doc = () => frame.contentDocument;

  // The window a card of `rect` needs: its size with the page's margins
  // and what is above the cards (the panel's bar).
  const sizeOf = (rect) => {
    const d = doc();
    const grid = d?.querySelector('.signal-cards') ?? d?.querySelector('signal-panel');
    const above = Math.max(0, (grid?.getBoundingClientRect().top ?? 0) + (d?.defaultView?.scrollY ?? 0));
    return { width: Math.ceil(rect.width + 24), height: Math.ceil(rect.height + above + 24) };
  };

  // Each card's menu, as a part of a page menu (`contextMenu`, as
  // SignalAppContextMenu.part asks).
  const partOf = (card) => (event) => {
    if (!cardClick(event.composedPath?.() ?? [], card.element)) return null;
    return {
      items: cardMenu(state, placed, card.id),
      act: (id) => {
        const message = menuMessage(id, card.id, placed, sizeOf(card.element.getBoundingClientRect()));
        if (message) post(message);
      },
    };
  };

  // The placeholders, over the held cards where they are now.
  const draw = () => {
    frameId = 0;
    const d = doc();
    if (!d || !layer) return;
    layer.replaceChildren();
    const sx = d.defaultView?.scrollX ?? 0;
    const sy = d.defaultView?.scrollY ?? 0;
    for (const card of cards) {
      if (card.held == null || card.element.style.display === 'none') continue;
      const r = card.element.getBoundingClientRect();
      if (!r.width) continue;
      const box = make(d, 'div', `${STYLE.placeholder}left:${r.left + sx}px;top:${r.top + sy}px;width:${r.width}px;height:${r.height}px;`);
      const text = make(d, 'span', 'flex:1;overflow:hidden;text-overflow:ellipsis;', `${card.title} — in ${card.held || 'another window'}`);
      const show = make(d, 'button', STYLE.button, 'Show');
      const back = make(d, 'button', STYLE.button, 'Bring back');
      show.type = back.type = 'button';
      show.addEventListener('click', () => post({ type: 'window.show', card: card.id }));
      back.addEventListener('click', () => post({ type: 'card.move', card: card.id, to: 'here' }));
      box.append(text, show, back);
      layer.append(box);
    }
  };
  // On this page's clock, not the frame's: a frame's document is replaced
  // when it loads, and a callback asked of the old one never runs.
  const redraw = () => {
    if (frameId) return;
    frameId = target.requestAnimationFrame ? target.requestAnimationFrame(draw) : setTimeout(draw, 16);
  };

  const apply = () => {
    const d = doc();
    if (!d) return;
    [cards, placed] = state ? applyCards(d, state) : [cardsOf(d), arrangement(cardsOf(d).map((c) => c.id), null)];
    for (const card of cards) {
      card.element.contextMenu = partOf(card);
      sizes?.observe(card.element);
    }
    shownCount(cards.filter((c) => c.element.style.display !== 'none').length);
    redraw();
    if (state?.view === 'panel') {
      // The content's height, for a window that fits it.
      const view = d.defaultView;
      const shown = cards.filter((c) => c.element.style.display !== 'none');
      const bottom = Math.max(0, ...shown.map((c) => c.element.getBoundingClientRect().bottom + (view?.scrollY ?? 0)));
      if (shown.length) post({ type: 'window.fit', height: Math.ceil(bottom + 12) });
    }
  };

  // Without a page menu (a panel window): the card's menu opens here.
  const right = (event) => {
    const d = doc();
    if (!d || menusClaimed(d) || !menuHost) return;
    for (const node of event.composedPath?.() ?? []) {
      if (typeof node?.contextMenu !== 'function') continue;
      const part = node.contextMenu(event);
      if (!part) return;
      event.preventDefault();
      menu ??= new ContextMenu({ label: 'Card', doc: menuHost.ownerDocument ?? menuHost });
      if (!menu.list.isConnected) menuHost.append(menu.list);
      const box = frame.getBoundingClientRect();
      menu.show(part.items, { x: event.clientX + box.left, y: event.clientY + box.top, pick: (id) => part.act(id) });
      return;
    }
  };
  const closeMenu = (event) => {
    if (menu?.isOpen && !event?.composedPath?.().includes(menu.list)) menu.close();
  };

  const loaded = () => {
    for (const stop of stops) stop();
    stops = [];
    const d = doc();
    if (!d?.body) return;
    frameId = 0;
    layer = make(d, 'div', STYLE.layer);
    layer.dataset.signalApp = 'cards';
    d.body.append(layer);
    const hide = d.createElement('style');
    hide.dataset.signalApp = 'held';
    hide.textContent = HELD_CSS;
    d.head?.append(hide);
    stops.push(() => hide.remove());
    const listen = (on, type, fn, capture = false) => {
      on.addEventListener(type, fn, capture);
      stops.push(() => on.removeEventListener(type, fn, capture));
    };
    // As it bubbles, so the panel editor's edit mode keeps its own.
    listen(d, 'contextmenu', right);
    listen(d, 'pointerdown', closeMenu, true);
    listen(d, 'keydown', (event) => event.key === 'Escape' && menu?.close());
    listen(d.defaultView, 'resize', redraw);
    const View = frame.contentWindow;
    // A generated panel builds its cards once described, a written panel is
    // shown in place, and cards change size: shown again as told.
    if (View?.MutationObserver) {
      let observer;
      const observe = () => {
        observer.observe(d.querySelector('signal-panel') ?? d.body, { childList: true, subtree: true, attributes: true, attributeFilter: ['editing'] });
        for (const p of d.querySelectorAll('signal-panel')) if (p.shadowRoot) observer.observe(p.shadowRoot, { childList: true, subtree: true });
      };
      observer = new View.MutationObserver(() => {
        observer.disconnect();
        apply();
        observe();
      });
      observe();
      stops.push(() => observer.disconnect());
    }
    if (View?.ResizeObserver) {
      sizes = new View.ResizeObserver(redraw);
      sizes.observe(d.body);
      stops.push(() => {
        sizes?.disconnect();
        sizes = null;
      });
    }
    stops.push(() => layer?.remove());
    apply();
  };

  const told = (event) => {
    state = event.detail ?? null;
    apply();
  };
  const outside = (event) => closeMenu(event);
  const blur = () => menu?.close();

  frame.addEventListener('load', loaded);
  target.addEventListener('signal-window', told);
  target.addEventListener('pointerdown', outside, true);
  target.addEventListener('blur', blur);
  if (doc()?.readyState === 'complete' && doc()?.body) loaded();
  post({ type: 'window.ready' });
  return () => {
    for (const stop of stops) stop();
    menu?.close();
    frame.removeEventListener('load', loaded);
    target.removeEventListener('signal-window', told);
    target.removeEventListener('pointerdown', outside, true);
    target.removeEventListener('blur', blur);
  };
}
