// The project panel's cards across Signal.app's windows (ADR 0032,
// docs/ui-controls.md "Cards across windows"). The project's main panel
// (`ui/main.html`) stays the one source of its cards; each window shows a
// subset: a panel window the cards it holds, in its order, and the main
// window's Panel tab every card no panel window holds. Which window holds
// which card is the project's `ui/windows.toml`, kept by the app (the
// native side); a page only shows what it is told (`signal-window`) and
// reports a card dragged by its heading (`card.drag`), and, when a card is
// dropped on it (`signal-card-drop`), where it goes (`card.place`). Cards
// are only hidden and ordered with CSS, never moved in the DOM, so the
// panel file, its editor and `panel.written` see the panel as written.
//
// A card's id is its `id` attribute when it has one, else the slug of its
// first heading (`kick`, `output`; a second card of the same heading
// `kick-2`), else `card` (`card-2`, …), in the panel's order.

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

/**
 * What a window shows of the cards `ids` (in the panel's order), told by
 * the app as `state`: a panel window (`{ view: 'panel', cards }`) its cards
 * in its order; a main window (`{ view: 'main', held, order }`) every card
 * not `held`, those in `order` first. Returns `[{ id, shown, order }]`.
 */
export function arrangement(ids, state) {
  const panel = state?.view === 'panel';
  const listed = panel ? (state.cards ?? []) : (state?.order ?? []);
  const held = new Set(panel ? [] : (state?.held ?? []));
  const rank = new Map(listed.map((id, i) => [id, i]));
  return ids.map((id, i) => {
    const shown = panel ? rank.has(id) : !held.has(id);
    const order = rank.has(id) ? rank.get(id) : listed.length + i;
    return { id, shown, order };
  });
}

/**
 * Where a card dropped at `point` goes, among the shown cards' `boxes`
 * (`[{ id, left, top, right, bottom }]`, in the order shown): before the
 * card it is dropped on (on its first half, reading left to right) or the
 * one after it, at the end below or after them all; `null` is the end.
 */
export function dropBefore(boxes, point) {
  for (let i = 0; i < boxes.length; i += 1) {
    const b = boxes[i];
    if (point.x >= b.left && point.x < b.right && point.y >= b.top && point.y < b.bottom) {
      const firstHalf = point.x < (b.left + b.right) / 2;
      return firstHalf ? b.id : (boxes[i + 1]?.id ?? null);
    }
  }
  // Above a card's row, before the first card that starts below the point.
  const after = boxes.find((b) => b.top > point.y);
  return after ? after.id : null;
}

/** How far the pointer moves before a press on a heading is a drag. */
export const DRAG_DISTANCE = 6;

/** The cards of panel document `doc`: its `.signal-card`s, or a generated panel's groups. */
export function findCards(doc) {
  const own = [...doc.querySelectorAll('.signal-card')].filter((c) => !c.parentElement?.closest('.signal-card'));
  if (own.length) return own;
  const generated = [...doc.querySelectorAll('signal-panel')].flatMap((p) => [...(p.shadowRoot?.querySelectorAll('.generated > section') ?? [])]);
  return generated;
}

function heading(card) {
  return [...card.children].find((c) => /^H[1-6]$/.test(c.tagName)) ?? null;
}

/** `[{ element, id }]` of `doc`'s cards. */
export function cardsOf(doc) {
  const elements = findCards(doc);
  const ids = cardIds(elements.map((e) => ({ id: e.id, title: heading(e)?.textContent ?? '' })));
  return elements.map((element, i) => ({ element, id: ids[i] }));
}

/** Shows `doc`'s cards as `state` says (hidden and ordered with CSS only). */
export function applyCards(doc, state) {
  const cards = cardsOf(doc);
  const placed = arrangement(cards.map((c) => c.id), state);
  cards.forEach(({ element }, i) => {
    element.style.display = placed[i].shown ? '' : 'none';
    element.style.order = state ? String(placed[i].order) : '';
  });
  return cards;
}

/** The card `doc` would put a card dropped at `point` (its own coordinates) before. */
export function dropTarget(doc, point) {
  const boxes = cardsOf(doc)
    .filter(({ element }) => element.style.display !== 'none')
    .map(({ element, id }) => {
      const r = element.getBoundingClientRect();
      return { id, left: r.left, top: r.top, right: r.right, bottom: r.bottom };
    })
    .sort((a, b) => a.top - b.top || a.left - b.left);
  return dropBefore(boxes, point);
}

/**
 * Lets `doc`'s cards be dragged by their heading: a press that moves
 * DRAG_DISTANCE, released anywhere (the pointer is captured, so outside
 * the window too), calls `dropped(id)`; Escape cancels. Not while the
 * panel's layout is edited (`signal-panel[editing]`). Returns the remover.
 */
export function dragCards(doc, dropped) {
  let drag = null;
  const end = (event, drop) => {
    if (!drag) return;
    const { card, id, moved } = drag;
    drag = null;
    card.style.opacity = '';
    doc.documentElement.style.cursor = '';
    if (drop && moved) {
      event?.preventDefault();
      dropped(id);
    }
  };
  const down = (event) => {
    if (event.button !== 0 || drag) return;
    const path = event.composedPath();
    const head = path.find((n) => n.tagName && /^H[1-6]$/.test(n.tagName));
    const card = head && cardsOf(doc).find((c) => c.element === head.parentElement);
    if (!card || card.element.closest('signal-panel[editing]') || path.some((n) => n.localName === 'signal-panel' && n.hasAttribute?.('editing'))) return;
    drag = { card: card.element, id: card.id, x: event.screenX, y: event.screenY, moved: false };
    head.setPointerCapture?.(event.pointerId);
  };
  const move = (event) => {
    if (!drag || drag.moved) return;
    if (Math.hypot(event.screenX - drag.x, event.screenY - drag.y) >= DRAG_DISTANCE) {
      drag.moved = true;
      drag.card.style.opacity = '0.5';
      doc.documentElement.style.cursor = 'grabbing';
    }
  };
  const up = (event) => end(event, true);
  const cancel = () => end(null, false);
  const key = (event) => event.key === 'Escape' && cancel();
  doc.addEventListener('pointerdown', down, true);
  doc.addEventListener('pointermove', move, true);
  doc.addEventListener('pointerup', up, true);
  doc.addEventListener('pointercancel', cancel, true);
  doc.addEventListener('keydown', key, true);
  return () => {
    doc.removeEventListener('pointerdown', down, true);
    doc.removeEventListener('pointermove', move, true);
    doc.removeEventListener('pointerup', up, true);
    doc.removeEventListener('pointercancel', cancel, true);
    doc.removeEventListener('keydown', key, true);
  };
}

/** The app's message channel (wry's `window.ipc`), or null outside Signal.app. */
export function appChannel(target = globalThis) {
  const ipc = target?.ipc;
  return ipc && typeof ipc.postMessage === 'function' ? (message) => ipc.postMessage(JSON.stringify(message)) : null;
}

/**
 * Connects panel frame `frame` (a same-origin iframe of the project's
 * panel) of this window to the app: tells it the window is ready, shows its
 * cards as each `signal-window` says, makes them draggable, and answers a
 * `signal-card-drop` (the point in this page's coordinates) with
 * `card.place`; `shownCount` is told how many cards it shows. Outside
 * Signal.app it does nothing. Returns the remover.
 */
export function connectCards(frame, target = globalThis, shownCount = () => {}) {
  const post = appChannel(target);
  if (!post) return () => {};
  let state = null;
  let stopDrag = null;
  let observer = null;
  const doc = () => frame.contentDocument;
  const apply = () => {
    const d = doc();
    if (!d || !state) return;
    const cards = applyCards(d, state);
    shownCount(cards.filter(({ element }) => element.style.display !== 'none').length);
  };
  const loaded = () => {
    stopDrag?.();
    observer?.disconnect();
    const d = doc();
    if (!d) return;
    stopDrag = dragCards(d, (card) => post({ type: 'card.drag', card }));
    // A generated panel builds its cards once described, and a written
    // panel is shown in place: shown again as told.
    const View = frame.contentWindow?.MutationObserver;
    if (View) {
      observer = new View(() => {
        observer.disconnect();
        apply();
        observe();
      });
      const observe = () => {
        observer.observe(d.body ?? d.documentElement, { childList: true, subtree: true });
        for (const p of d.querySelectorAll('signal-panel')) if (p.shadowRoot) observer.observe(p.shadowRoot, { childList: true, subtree: true });
      };
      observe();
    }
    apply();
  };
  const told = (event) => {
    state = event.detail ?? null;
    apply();
  };
  const drop = (event) => {
    const { card, x, y } = event.detail ?? {};
    if (!isCardId(card)) return;
    const box = frame.getBoundingClientRect();
    const d = doc();
    const before = d ? dropTarget(d, { x: x - box.left, y: y - box.top }) : null;
    post({ type: 'card.place', card, before });
  };
  frame.addEventListener('load', loaded);
  target.addEventListener('signal-window', told);
  target.addEventListener('signal-card-drop', drop);
  if (doc()?.readyState === 'complete') loaded();
  post({ type: 'window.ready' });
  return () => {
    stopDrag?.();
    observer?.disconnect();
    frame.removeEventListener('load', loaded);
    target.removeEventListener('signal-window', told);
    target.removeEventListener('signal-card-drop', drop);
  };
}
