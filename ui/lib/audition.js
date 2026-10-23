// The piano roll's keys play (ADR 0039): which key a press is on, how loud,
// and which sequencer input the note goes to. Pure (no DOM), so `node
// --test ui/` checks it; ui/controls/piano-roll.js draws and sends.

import { MAX_KEY, MIN_KEY, clamp, isBlack, keyToY } from './piano-roll-model.js';

/** A black key's share of the keyboard strip's width (as drawn). */
export const BLACK_SHARE = 0.6;
/** The softest a press plays (at the key's far end); the tip plays at 1. */
export const MIN_VELOCITY = 0.3;

/**
 * The key a press at `x`, `y` of the keyboard strip `width` wide is on, for
 * `view` (the roll's rows: `rowPx`, `scrollY`), and its velocity: `{key,
 * velocity}`, or null outside the keys. Black keys cover the left
 * `BLACK_SHARE` of their row; right of them, a black key's row is the white
 * keys' (its upper half the key above, its lower half the key below), as
 * drawn. The velocity grows with the distance along the key towards its
 * tip (the right end: a black key's is shorter), from `MIN_VELOCITY` to 1.
 */
export function keyAt(x, y, view, width) {
  if (!(width > 0) || x < 0 || x > width) return null;
  const row = MAX_KEY - Math.floor((y + view.scrollY) / view.rowPx);
  if (row < MIN_KEY || row > MAX_KEY) return null;
  const blackW = Math.round(width * BLACK_SHARE);
  let key = row;
  if (isBlack(row) && x >= blackW) key = y - keyToY(row, view) >= view.rowPx / 2 ? row - 1 : row + 1;
  if (key < MIN_KEY || key > MAX_KEY) return null;
  const length = isBlack(key) ? blackW : width;
  return { key, velocity: clamp(MIN_VELOCITY + ((1 - MIN_VELOCITY) * x) / length, MIN_VELOCITY, 1) };
}

/** Whether `instance` (a `describe` instance) has an event input `name`. */
function eventInput(instance, name) {
  return (instance?.ports ?? []).some((p) => p.name === name && p.direction === 'in' && (p.shape === 'events' || p.shape === 'event'));
}

/**
 * Where the roll's live notes go: the `audition` attribute's event input
 * (`none`: nowhere); else the `audition` input of the sequencer whose
 * playhead `position-bind` follows (`/melody.position` → `/melody.audition`)
 * when it has one; else, when the project has exactly one `std.note_seq`,
 * its `audition`.
 * The automatic ones only when `description` (`describe`) has that input.
 * Null for none.
 */
export function auditionTarget({ audition, positionBind } = {}, description) {
  const given = audition?.trim();
  if (given === 'none') return null;
  if (given) return given;
  const instances = description?.instances ?? [];
  const followed = /^(\/.*)\.[^./]+$/.exec(positionBind?.trim() ?? '')?.[1];
  const named = followed && instances.find((i) => i.address === followed);
  if (named && eventInput(named, 'audition')) return `${named.address}.audition`;
  // Else (no `position-bind`, or it follows something without `audition`): the only sequencer.
  const sequencers = instances.filter((i) => i.module === 'std.note_seq' && eventInput(i, 'audition'));
  return sequencers.length === 1 ? `${sequencers[0].address}.audition` : null;
}
