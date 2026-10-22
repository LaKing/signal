// The interface setup page's draft (the M2.5 dialog's, crates/signal-app/
// src/dialog.rs): what is chosen until Apply. A chosen device is kept by its
// identity (name, id, signature), never by its place in the list; the server
// finds it again after a refresh (`setup.devices`' `choices`, the dialog's
// own lookup). A saved device that is not found is unresolved: shown as
// such, kept by Apply, replaced only by an explicit choice. No DOM.

import { parseNumber } from '../../lib/entry.js';

/** What the page shows for a saved device that is not found (one string). */
export const NOT_FOUND = '— not found: choose a device —';

/** A listed device's identity, as the Setup keeps it. */
export function identity(device) {
  return { name: device.name, id: device.id ?? null, signature: device.signature ?? '' };
}

/**
 * A choice from the server's resolution of it: `{found: index}`,
 * `{unresolved: reason}`, or null (none: the default output, no input).
 */
export function choiceOf(resolution, devices) {
  if (resolution && Number.isInteger(resolution.found) && devices[resolution.found]) {
    return { kind: 'device', device: identity(devices[resolution.found]), index: resolution.found };
  }
  if (resolution && typeof resolution.unresolved === 'string') {
    return { kind: 'unresolved', reason: resolution.unresolved };
  }
  return { kind: 'unset' };
}

/** The draft of the Setup, from `setup.get`'s and `setup.devices`' replies. */
export function draftOf(setup, listed) {
  const s = setup?.setup ?? {};
  const devices = listed?.devices ?? [];
  return {
    output: choiceOf(listed?.output, devices),
    input: choiceOf(listed?.input, devices),
    buffer: s.buffer ?? 128,
    cpuWorkers: s.cpuWorkers ?? 'auto',
    channels: (s.outputChannels ?? []).join(', '),
    rtPolicy: s.rtPolicy ?? 'park50',
  };
}

/**
 * The draft with the fields as typed (buffer, CPU workers, channel map,
 * wake policy): taken before anything redraws the page from the draft, so
 * choosing a device or refreshing the list keeps unsaved edits.
 */
export function withFields(draft, fields) {
  const kept = {};
  for (const key of ['buffer', 'cpuWorkers', 'channels', 'rtPolicy']) {
    if (fields?.[key] !== undefined && fields[key] !== null) kept[key] = fields[key];
  }
  return { ...draft, ...kept };
}

/** The `choices` a refresh asks the server to find again. */
export function choicesOf(draft) {
  const wanted = (choice) => (choice.kind === 'device' ? choice.device : null);
  return { output: wanted(draft.output), input: wanted(draft.input) };
}

/**
 * The draft after a refresh: a chosen device found again where it is now,
 * or unresolved when it is gone; unset and unresolved choices stay.
 */
export function refreshed(draft, listed) {
  const devices = listed?.devices ?? [];
  const again = (choice, key) => (choice.kind === 'device' ? choiceOf(listed?.choices?.[key], devices) : choice);
  return { ...draft, output: again(draft.output, 'output'), input: again(draft.input, 'input') };
}

/** Choosing the device at `index` (null: none). */
export function choose(index, devices) {
  return index === null || index === undefined || !devices[index]
    ? { kind: 'unset' }
    : { kind: 'device', device: identity(devices[index]), index };
}

/** A choice as `setup.apply` takes it: keep, none (null) or the device. */
export function selection(choice) {
  if (choice.kind === 'unresolved') return 'keep';
  if (choice.kind === 'unset') return null;
  return choice.device;
}

/** "CPU workers" as typed: auto (empty or `auto`), or 1 to 64. */
export function cpuWorkers(text) {
  const t = String(text ?? '').trim().toLowerCase();
  if (t === '' || t === 'auto') return 'auto';
  const n = parseNumber(t);
  if (!Number.isInteger(n) || n < 1 || n > 64) throw new Error('CPU workers is auto or 1 to 64');
  return n;
}

/**
 * The buffer as typed, in frames: a number (a decimal comma or dot, as every
 * typed number, ui/lib/entry.js `parseNumber`), rounded, at least 1.
 */
export function bufferFrames(text) {
  const n = parseNumber(String(text ?? ''));
  if (n === null) throw new Error('Buffer is a number of frames, 1 or more');
  return Math.max(1, Math.round(n));
}

/** The `setup.apply` message of a draft. */
export function applyMessage(draft) {
  return {
    type: 'setup.apply',
    output: selection(draft.output),
    input: selection(draft.input),
    buffer: bufferFrames(draft.buffer),
    cpuWorkers: cpuWorkers(draft.cpuWorkers),
    outputChannels: String(draft.channels ?? '').trim() === '' ? null : String(draft.channels),
    rtPolicy: draft.rtPolicy,
  };
}
