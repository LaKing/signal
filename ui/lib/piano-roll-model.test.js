// <signal-piano-roll>'s model (ADR 0028 §2): geometry, zoom, snap,
// hit-testing, the edits and their undo, the keys; no DOM.

import test from 'node:test';
import assert from 'node:assert/strict';
import { validatePattern } from './pattern-text.js';
import {
  PianoRollModel,
  ZOOM,
  beatToX,
  centreKey,
  clampScroll,
  clean,
  gridLines,
  hitNote,
  hitVelocity,
  isBlack,
  keyName,
  keyToY,
  limitMove,
  makeView,
  noteRect,
  prevFloat,
  trimmedLength,
  notesInBox,
  resolveOverlaps,
  rollKey,
  snap,
  snapFloor,
  velocityAt,
  xToBeat,
  yToKey,
  zoomX,
  zoomY,
} from './piano-roll-model.js';

const note = (key, start, length = 1, velocity = 0.8) => ({ key, start, length, velocity });
const pattern = (notes, length = 16) => ({ schema_version: 1, kind: 'notes', length, notes });
const view = makeView({ beatPx: 40, rowPx: 10, scrollX: 0, scrollY: 0, width: 800, height: 400 });

test('keys: names (60 = C4) and black keys', () => {
  assert.equal(keyName(60), 'C4');
  assert.equal(keyName(61), 'C♯4');
  assert.equal(keyName(0), 'C-1');
  assert.equal(keyName(127), 'G9');
  assert.deepEqual(
    [60, 61, 62, 63, 64, 65, 66, 67, 68, 69, 70, 71].map(isBlack),
    [false, true, false, true, false, false, true, false, true, false, true, false],
  );
});

test('snap rounds and floors to the grid, triplets included, without float noise', () => {
  assert.equal(snap(0.37, 0.25), 0.25);
  assert.equal(snap(0.38, 0.25), 0.5);
  assert.equal(snapFloor(0.49, 0.25), 0.25);
  assert.equal(snapFloor(0.5, 0.25), 0.5);
  assert.equal(snap(0.7, 1 / 3), 2 / 3);
  assert.equal(snap(0.37 + 0.38, 0.125), 0.75);
  assert.equal(clean(0.1 + 0.2), 0.30000000000000004, 'not on a 1/192 multiple: kept');
  assert.equal(clean(0.75 + 1e-12), 0.75);
  assert.equal(snapFloor(2.9999999999, 1), 3, 'a hair under is on the line');
});

test('beats and keys map to pixels and back', () => {
  const v = { ...view, scrollX: 20, scrollY: 300 };
  assert.equal(beatToX(2, v), 60);
  assert.equal(xToBeat(60, v), 2);
  assert.equal(keyToY(127, v), -300);
  assert.equal(yToKey(keyToY(60, v), v), 60);
  assert.equal(yToKey(keyToY(60, v) + 9.9, v), 60);
  assert.equal(yToKey(-1e6, v), 127, 'clamped');
  assert.equal(yToKey(1e6, v), 0);
  assert.deepEqual(noteRect(note(60, 1, 2), v), { x: 20, y: (127 - 60) * 10 - 300, w: 80, h: 10 });
});

test('zoom keeps the point under the pointer, within limits; scroll stays inside', () => {
  const v = { ...view, scrollX: 100 };
  const before = xToBeat(300, v);
  const z = zoomX(v, 2, 300, 64);
  assert.equal(z.beatPx, 80);
  assert.ok(Math.abs(xToBeat(300, z) - before) < 1e-9);
  assert.equal(zoomX(v, 1000, 0, 64).beatPx, ZOOM.maxBeat);
  assert.equal(zoomX(v, 0.001, 0, 64).beatPx, ZOOM.minBeat);
  const y = zoomY({ ...view, scrollY: 500 }, 1.5, 200, 64);
  assert.equal(y.rowPx, 15);
  assert.ok(Math.abs((200 + y.scrollY) / 15 - (200 + 500) / 10) < 1e-9);
  assert.deepEqual(clampScroll({ ...view, scrollX: -5, scrollY: 1e9 }, 64), { ...view, scrollX: 0, scrollY: 128 * 10 - 400 });
  const c = centreKey(view, 60, 64);
  assert.equal(yToKey(200, c), 60);
});

test('grid lines: bars, beats and the snap, thinned when too close', () => {
  const lines = gridLines(0, 4, view, { grid: 0.25 });
  assert.equal(lines.length, 17);
  assert.deepEqual(lines.slice(0, 5).map((l) => l.level), [0, 2, 2, 2, 1]);
  assert.equal(lines[16].level, 0);
  // 40 px per beat: 1/32 lines (5 px) are dropped, beats remain.
  assert.deepEqual(gridLines(0, 2, view, { grid: 0.125 }).map((l) => l.beat), [0, 1, 2]);
  // Very far out: every few bars only.
  const far = gridLines(0, 64, { ...view, beatPx: 0.5 }, { minPx: 7 });
  assert.ok(far.every((l) => l.level === 0));
  assert.ok(far[1].beat >= 8);
});

test('hit-testing: body, right edge, topmost, nothing', () => {
  const notes = [{ ...note(60, 0, 2), id: 1 }, { ...note(60, 1, 0.25), id: 2 }];
  const y = keyToY(60, view) + 5;
  assert.deepEqual(hitNote(notes, 10, y, view), { id: 1, part: 'body' });
  assert.deepEqual(hitNote(notes, 78, y, view), { id: 1, part: 'end' });
  assert.deepEqual(hitNote(notes, 42, y, view), { id: 2, part: 'body' }, 'the later drawn wins');
  assert.equal(hitNote(notes, 10, y + 10, view), null);
  assert.equal(hitNote(notes, 81, y, view), null);
  // A short note's edge is at most a third of it.
  assert.deepEqual(hitNote([{ ...note(60, 0, 0.25), id: 3 }], 2, y, view), { id: 3, part: 'body' });
});

test('the rubber band takes notes touching it', () => {
  const notes = [note(60, 0, 1), note(62, 2, 1), note(70, 1, 1)].map((n, i) => ({ ...n, id: i + 1 }));
  assert.deepEqual(notesInBox(notes, 0.5, 2.5, 59, 63), [1, 2]);
  assert.deepEqual(notesInBox(notes, 2.5, 0.5, 63, 59), [1, 2], 'any corner order');
  assert.deepEqual(notesInBox(notes, 1, 2, 59, 75), [3], 'touching edges do not count');
});

test('velocity bars: nearest start, selected first among a chord; y to velocity', () => {
  const notes = [note(60, 1), note(64, 1), note(67, 3)].map((n, i) => ({ ...n, id: i + 1 }));
  assert.equal(hitVelocity(notes, 41, view), 2, 'the higher key');
  assert.equal(hitVelocity(notes, 41, view, new Set([1])), 1, 'the selected one');
  assert.equal(hitVelocity(notes, 100, view), null);
  assert.equal(velocityAt(4, 84), 1);
  assert.equal(velocityAt(80, 84), 0);
  assert.equal(velocityAt(42, 84), Math.round(0.5 * 127) / 127);
  assert.equal(velocityAt(-50, 84), 1);
});

test('overlaps are resolved: the edited note wins, else the later trims the earlier', () => {
  const ids = (list) => list.map((n) => [n.id, n.start, n.length]);
  const a = { ...note(60, 0, 4), id: 1 };
  const b = { ...note(60, 2, 1), id: 2 };
  assert.deepEqual(ids(resolveOverlaps([a, b], new Set([2]))), [
    [1, 0, 2],
    [2, 2, 1],
  ]);
  assert.deepEqual(ids(resolveOverlaps([a, b], new Set([1]))), [[1, 0, 4]], 'the covered note goes');
  assert.deepEqual(ids(resolveOverlaps([a, { ...b, start: 0 }], new Set([2]))), [[2, 0, 1]], 'same start: the edited one stays');
  assert.deepEqual(ids(resolveOverlaps([a, b])), [
    [1, 0, 2],
    [2, 2, 1],
  ]);
  const other = { ...note(61, 1, 4), id: 3 };
  assert.equal(resolveOverlaps([a, other], new Set([1])).length, 2, 'other keys are not touched');
});

test('the model reads and writes a pattern, sorted, without ids', () => {
  const m = new PianoRollModel(pattern([note(64, 1), note(60, 0)]));
  assert.deepEqual(m.pattern(), pattern([note(60, 0), note(64, 1)]));
  assert.ok(m.notes.every((n) => n.id > 0));
});

test('add: snapped down, the last length, selected, undoable', () => {
  const m = new PianoRollModel(pattern([]));
  const id = m.add(1.3, 60.4, { grid: 0.25 });
  assert.deepEqual(m.pattern().notes, [note(60, 1.25, 0.25)]);
  assert.deepEqual([...m.selected], [id]);
  m.lastLength = 1;
  m.add(2.9, 62, { grid: 0.5, velocity: 0.5 });
  assert.deepEqual(m.pattern().notes[1], note(62, 2.5, 1, 0.5));
  assert.ok(m.undo());
  assert.equal(m.notes.length, 1);
  assert.ok(m.redo());
  assert.equal(m.notes.length, 2);
  assert.ok(m.undo() && m.undo());
  assert.equal(m.notes.length, 0);
  assert.equal(m.undo(), false);
});

test('add over an existing note on the key trims it', () => {
  const m = new PianoRollModel(pattern([note(60, 0, 4)]));
  m.add(2, 60, { grid: 1 });
  assert.deepEqual(m.pattern().notes, [note(60, 0, 2), note(60, 2, 1)]);
  assert.deepEqual(validatePattern(m.pattern()), []);
});

test('a drag moves the selection on the grid, both axes, as one undo step', () => {
  const m = new PianoRollModel(pattern([note(60, 1, 1), note(64, 1.5, 0.5), note(70, 0, 1)]));
  const [a, b] = [60, 64].map((key) => m.notes.find((n) => n.key === key));
  m.select([a.id, b.id]);
  m.beginDrag('move', a.id, 1.2, 60);
  m.dragTo(2.1, 62, { grid: 0.25 });
  m.dragTo(2.33, 63, { grid: 0.25 });
  assert.ok(m.endDrag());
  assert.deepEqual(m.pattern().notes, [note(70, 0, 1), note(63, 2.25, 1), note(67, 2.75, 0.5)]);
  assert.equal(m.undoStack.length, 1);
  m.undo();
  assert.deepEqual(m.pattern().notes, [note(70, 0, 1), note(60, 1, 1), note(64, 1.5, 0.5)]);
});

test('a drag is clamped at beat 0 and the key range; a drag back to the start changes nothing', () => {
  const m = new PianoRollModel(pattern([note(120, 1, 1), note(100, 2, 1)]));
  m.selectAll();
  m.beginDrag('move', m.notes[0].id, 1, 120);
  m.dragTo(-5, 140, { grid: 0.25 });
  assert.deepEqual(m.pattern().notes, [note(127, 0, 1), note(107, 1, 1)]);
  m.dragTo(1, 120, { grid: 0.25 });
  assert.equal(m.endDrag(), false);
  assert.equal(m.undoStack.length, 0);
  assert.deepEqual(limitMove([note(5, 0.5)], -2, -10), [-0.5, -5]);
});

test('Alt-drag moves copies and keeps the originals', () => {
  const m = new PianoRollModel(pattern([note(60, 0, 1)]));
  const id = m.notes[0].id;
  m.beginDrag('move', id, 0.5, 60, { copy: true });
  m.dragTo(2.5, 60, { grid: 1 });
  m.endDrag();
  assert.deepEqual(m.pattern().notes, [note(60, 0, 1), note(60, 2, 1)]);
  assert.ok(!m.selected.has(id), 'the copy is selected');
  m.undo();
  assert.deepEqual(m.pattern().notes, [note(60, 0, 1)]);
});

test('a moved note overlapping a still one trims it', () => {
  const m = new PianoRollModel(pattern([note(60, 0, 4), note(62, 2, 1)]));
  m.select([m.notes[1].id]);
  m.beginDrag('move', m.notes[1].id, 2, 62);
  m.dragTo(2, 60, { grid: 1 });
  m.endDrag();
  assert.deepEqual(m.pattern().notes, [note(60, 0, 2), note(60, 2, 1)]);
});

test('resize: snapped end, at least one grid step, sets the last length', () => {
  const m = new PianoRollModel(pattern([note(60, 0, 1), note(64, 0, 0.5)]));
  m.selectAll();
  m.beginDrag('resize', m.notes[0].id, 1, 60);
  m.dragTo(1.6, 60, { grid: 0.25 });
  m.endDrag();
  assert.deepEqual(m.pattern().notes, [note(60, 0, 1.5), note(64, 0, 1)]);
  assert.equal(m.lastLength, 1.5);
  m.beginDrag('resize', m.notes[0].id, 1.5, 60);
  m.dragTo(-3, 60, { grid: 0.25 });
  m.endDrag();
  assert.deepEqual(m.pattern().notes, [note(60, 0, 0.25), note(64, 0, 0.25)]);
  // Free (⌘): no snap, at least 1/64.
  m.beginDrag('resize', m.notes[0].id, 0.25, 60);
  m.dragTo(0.3, 60, { free: true });
  m.endDrag();
  assert.equal(m.pattern().notes[0].length, 0.3);
});

test('resizing into the next note on the key trims the edited one to keep it valid', () => {
  const m = new PianoRollModel(pattern([note(60, 0, 1), note(60, 2, 1)]));
  m.select([m.notes[0].id]);
  m.beginDrag('resize', m.notes[0].id, 1, 60);
  m.dragTo(3.5, 60, { grid: 0.5 });
  m.endDrag();
  assert.deepEqual(m.pattern().notes, [note(60, 0, 3.5)], 'the edited note wins');
  assert.deepEqual(validatePattern(m.pattern()), []);
});

test('Escape cancels a drag', () => {
  const m = new PianoRollModel(pattern([note(60, 0, 1)]));
  m.beginDrag('move', m.notes[0].id, 0, 60);
  m.dragTo(3, 70, { grid: 1 });
  assert.equal(rollKey(m, { key: 'Escape' }, 0.25), 'selection');
  assert.deepEqual(m.pattern().notes, [note(60, 0, 1)]);
  assert.equal(m.drag, null);
});

test('velocity drags move every selected note by the same amount, clamped', () => {
  const m = new PianoRollModel(pattern([note(60, 0, 1, 0.5), note(64, 0, 1, 0.9)]));
  m.selectAll();
  m.beginVelocity(m.notes[0].id);
  m.velocityTo(0.7);
  assert.ok(m.endDrag());
  const [a, b] = m.pattern().notes;
  assert.ok(Math.abs(a.velocity - 0.7) < 0.01);
  assert.equal(b.velocity, 1);
  m.undo();
  assert.deepEqual(m.pattern().notes.map((n) => n.velocity), [0.5, 0.9]);
});

test('remove, select, duplicate, loop length', () => {
  const m = new PianoRollModel(pattern([note(60, 0, 1), note(62, 1, 1), note(64, 2, 1)]));
  const [a, b, c] = m.notes.map((n) => n.id);
  m.select([a]);
  m.select([c], { add: true });
  assert.deepEqual([...m.selected].sort(), [a, c].sort());
  m.select([a], { toggle: true });
  assert.deepEqual([...m.selected], [c]);
  assert.ok(m.remove());
  assert.equal(m.notes.length, 2);
  assert.equal(m.remove([999]), false);
  m.select([a, b]);
  assert.ok(m.duplicate(0.25));
  assert.deepEqual(m.pattern().notes, [note(60, 0, 1), note(62, 1, 1), note(60, 2, 1), note(62, 3, 1)]);
  assert.ok(m.setLength(7.6, 1));
  assert.equal(m.length, 8);
  assert.equal(m.setLength(8, 1), false);
  m.undo();
  assert.equal(m.length, 16);
});

test('the keys: arrows move (octave with Shift, a snap sideways), Delete, ⌘A, ⌘D, ⌘Z / ⇧⌘Z', () => {
  const m = new PianoRollModel(pattern([note(60, 1, 1)]));
  const key = (k, extra = {}) => rollKey(m, { key: k, shiftKey: false, metaKey: false, ctrlKey: false, ...extra }, 0.25);
  assert.equal(key('a', { metaKey: true }), 'selection');
  assert.equal(key('ArrowUp'), true);
  assert.equal(key('ArrowUp', { shiftKey: true }), true);
  assert.equal(key('ArrowRight'), true);
  assert.deepEqual(m.pattern().notes, [note(73, 1.25, 1)]);
  key('ArrowLeft');
  key('ArrowLeft');
  key('ArrowLeft');
  key('ArrowLeft');
  key('ArrowLeft');
  key('ArrowLeft');
  assert.equal(m.pattern().notes[0].start, 0, 'not before 0');
  assert.equal(key('ArrowLeft'), 'handled');
  assert.equal(key('z', { metaKey: true }), true);
  assert.equal(m.pattern().notes[0].start, 0.25);
  assert.equal(key('z', { metaKey: true, shiftKey: true }), true);
  assert.equal(m.pattern().notes[0].start, 0);
  assert.equal(key('d', { metaKey: true }), true);
  assert.equal(m.notes.length, 2);
  assert.equal(key('Delete'), true);
  assert.equal(m.notes.length, 1);
  assert.equal(key('q'), null);
  assert.equal(key('Escape'), 'selection');
  assert.equal(m.selected.size, 0);
  assert.equal(key('ArrowUp'), 'handled', 'nothing selected: nothing moves');
});

test('every edit leaves a valid pattern (a random walk)', () => {
  let seed = 3;
  const random = () => (seed = (seed * 16807) % 2147483647) / 2147483647;
  const m = new PianoRollModel(pattern([]));
  for (let i = 0; i < 400; i++) {
    const r = random();
    if (r < 0.35) m.add(random() * 16, 55 + Math.floor(random() * 10), { grid: 0.25 });
    else if (r < 0.55 && m.notes.length) {
      const n = m.notes[Math.floor(random() * m.notes.length)];
      m.beginDrag('move', n.id, n.start, n.key);
      m.dragTo(n.start + (random() - 0.5) * 4, n.key + Math.round((random() - 0.5) * 4), { grid: 0.25 });
      m.endDrag();
    } else if (r < 0.7 && m.notes.length) {
      const n = m.notes[Math.floor(random() * m.notes.length)];
      m.select([n.id]);
      m.beginDrag('resize', n.id, n.start + n.length, n.key);
      m.dragTo(n.start + n.length + (random() - 0.3) * 3, n.key, { grid: 0.25 });
      m.endDrag();
    } else if (r < 0.8) m.undo();
    else if (r < 0.85) m.redo();
    else if (r < 0.9) {
      m.selectAll();
      m.moveSelection(0.25, random() < 0.5 ? 1 : -1);
    } else if (r < 0.95) m.duplicate(0.25);
    else m.remove();
    assert.deepEqual(validatePattern(m.pattern()), [], `step ${i}`);
  }
});

test('trimming never leaves a float overlap: grids of 1/3, 1/6, 1/4, 1/8, 1/32 swept', () => {
  for (const g of [1 / 3, 1 / 6, 1 / 4, 1 / 8, 1 / 32]) {
    for (let p = 0; p < 300; p++) {
      for (let q = p + 1; q < p + 30; q++) {
        const a = clean(p * g);
        const b = clean(q * g);
        const out = resolveOverlaps([{ id: 1, ...note(60, a, 100) }, { id: 2, ...note(60, b, 1) }], new Set([2]));
        const problems = validatePattern(pattern(out.map(({ id, ...n }) => n), 512));
        assert.deepEqual(problems, [], `grid ${g}: ${a} then ${b}`);
        assert.ok(out[0].start + out[0].length <= b);
      }
    }
  }
});

test('free values: a trimmed note ends at or before the next start', () => {
  let seed = 11;
  const random = () => (seed = (seed * 16807) % 2147483647) / 2147483647;
  for (let i = 0; i < 20000; i++) {
    const a = random() * 8;
    const b = a + random() * 2 + 0.001;
    const out = resolveOverlaps([{ id: 1, ...note(60, a, 5) }, { id: 2, ...note(60, b, 1) }], new Set([2]));
    assert.deepEqual(validatePattern(pattern(out.map(({ id, ...n }) => n))), [], `${a} ${b}`);
  }
  assert.ok(prevFloat(1) < 1 && 1 - prevFloat(1) < 1e-15);
  assert.equal(prevFloat(0), 0);
  assert.equal(trimmedLength(2, 2), 0);
  const start = 1 / 3;
  const end = 8 / 3;
  assert.ok(start + trimmedLength(start, end) <= end);
});

test('adding on a triplet grid over a long note keeps the pattern writable', () => {
  const m = new PianoRollModel(pattern([note(60, 1 / 3, 4)]));
  m.add(8 / 3 + 0.01, 60, { grid: 1 / 3 });
  assert.deepEqual(validatePattern(m.pattern()), []);
  assert.equal(m.pattern().notes[1].start, 8 / 3);
});

test('an Alt-click without movement leaves nothing: no copies, no undo step', () => {
  const m = new PianoRollModel(pattern([note(60, 0, 1)]));
  m.select([m.notes[0].id]);
  m.beginDrag('move', m.notes[0].id, 0.5, 60, { copy: true });
  m.dragTo(0.55, 60, { grid: 0.25 });
  assert.equal(m.endDrag(), false);
  assert.equal(m.undoStack.length, 0);
  assert.deepEqual(m.pattern().notes, [note(60, 0, 1)]);
});
