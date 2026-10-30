// The front plate in the page (ADR 0047 §2, ui/lib/plate.js has the
// numbers): a grid of cards (`div.signal-cards`, a generated panel's
// `.generated`, a slot's part) laid out on its plate, each card at its
// `place`. Only custom properties and data attributes are set (the theme's
// `[data-plate]` rules place the cards), so the panel file, its editor and
// the app's card styles (ui/app/lib/cards.js) see the panel as written.

import { GUTTER, PU, parsePlace, parsePlate } from '../lib/plate.js';

/** The plate's CSS, for a shadow root (the theme has the same rules for the page). */
export const PLATE_CSS = `
[data-plate] { display: block !important; position: relative; box-sizing: border-box; width: calc(var(--signal-plate-w) * ${PU}px); height: calc(var(--signal-plate-h) * ${PU}px); }
[data-plate] > [data-placed] { position: absolute; box-sizing: border-box; margin: 0; min-width: 0; min-height: 0; left: calc(var(--signal-place-x) * ${PU}px + ${GUTTER}px); top: calc(var(--signal-place-y) * ${PU}px + ${GUTTER}px); width: calc(var(--signal-place-w) * ${PU}px - ${2 * GUTTER}px); height: calc(var(--signal-place-h) * ${PU}px - ${2 * GUTTER}px); }
`;

const PROPS = ['--signal-place-x', '--signal-place-y', '--signal-place-w', '--signal-place-h'];

function unplace(card) {
  for (const p of PROPS) card.style.removeProperty(p);
  delete card.dataset.placed;
  delete card.dataset.unplaced;
}

/**
 * Lays out the cards of `grid` (its element children) on the plate
 * `plate` (`"W H"`, or null: none, the grid as it flows). A card's place is
 * its `place` attribute, or `places.get(card)` (a generated panel's); a
 * card without one goes below the placed ones, the plate's width, as tall
 * as its content (and the plate grows to hold it). Returns the plate's
 * size in px (`{w, h}`) and the cards without a place, or null without a
 * plate.
 */
export function layOutCards(grid, plate, places = null) {
  if (!grid) return null;
  const size = parsePlate(plate);
  const cards = [...grid.children];
  if (!size) {
    delete grid.dataset.plate;
    grid.style.removeProperty('--signal-plate-w');
    grid.style.removeProperty('--signal-plate-h');
    for (const card of cards) unplace(card);
    return null;
  }
  grid.dataset.plate = '';
  grid.style.setProperty('--signal-plate-w', String(size.w));
  const set = (card, p) => {
    card.style.setProperty('--signal-place-x', String(p.x));
    card.style.setProperty('--signal-place-y', String(p.y));
    card.style.setProperty('--signal-place-w', String(p.w));
    if (p.h === null) card.style.removeProperty('--signal-place-h');
    else card.style.setProperty('--signal-place-h', String(p.h));
    card.dataset.placed = '';
  };
  let bottom = 0;
  const unplaced = [];
  for (const card of cards) {
    const place = places?.get(card) ?? parsePlace(card.getAttribute('place'));
    if (!place) {
      unplaced.push(card);
      continue;
    }
    delete card.dataset.unplaced;
    set(card, place);
    bottom = Math.max(bottom, place.y + place.h);
  }
  let h = Math.max(size.h, bottom);
  grid.style.setProperty('--signal-plate-h', String(h));
  // Below the placed cards, each as tall as its content (its height left to it, so it is measured
  // again whenever it changes: the panel observes these cards).
  for (const card of unplaced) {
    card.dataset.unplaced = '';
    set(card, { x: 0, y: h, w: size.w, h: null });
    h += Math.ceil((card.offsetHeight + 2 * GUTTER) / PU) || 1;
  }
  grid.style.setProperty('--signal-plate-h', String(h));
  return { w: size.w * PU, h: h * PU, unplaced };
}

/** The placed cards of `grid` whose content is taller than their card: `[{card, need}]`, `need` the height in pu it would take. */
export function overflowing(grid) {
  if (!grid?.hasAttribute?.('data-plate')) return [];
  const found = [];
  for (const card of grid.children) {
    if (!card.hasAttribute('data-placed') || card.hasAttribute('data-unplaced')) continue;
    // Its content's extent (an open slot card's: its header and its part).
    const content = card.scrollHeight;
    if (content > card.clientHeight + 1) found.push({ card, need: Math.ceil((content + 2 * GUTTER) / PU) });
  }
  return found;
}
