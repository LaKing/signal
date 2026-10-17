// <signal-step-grid>'s model (ADR 0028 §2): the click cycle, painting, the
// accent row, levels, step count and length, undo, layout and hit-testing.

import test from 'node:test';
import assert from 'node:assert/strict';
import { parsePattern, serializePattern, validatePattern } from './pattern-text.js';
import { StepGridModel, accentAt, cycle, gridLayout, groupOf, hitGrid, laneY, resizeHits, stepX } from './step-grid-model.js';

const beat = () => ({
  schema_version: 1,
  kind: 'steps',
  steps: 8,
  step: 0.25,
  lanes: [
    { name: 'kick', hits: 'x...x...', level: 0.9 },
    { name: 'snare', hits: '..X...X.' },
  ],
});

test('a click cycles rest → hit → accent → rest', () => {
  assert.equal(cycle('.'), 'x');
  assert.equal(cycle('x'), 'X');
  assert.equal(cycle('X'), '.');
});

test('steps come in coloured groups of four', () => {
  assert.deepEqual([0, 3, 4, 8, 12, 15, 16].map(groupOf), [0, 0, 1, 2, 3, 3, 0]);
});

test('a press cycles a cell; one undo step', () => {
  const m = new StepGridModel(beat());
  assert.equal(m.press(0, 1), 'x');
  assert.ok(m.release());
  assert.equal(m.pattern().lanes[0].hits, 'xx..x...');
  m.press(0, 0);
  m.release();
  assert.equal(m.cell(0, 0), 'X');
  assert.ok(m.undo());
  assert.ok(m.undo());
  assert.equal(m.pattern().lanes[0].hits, 'x...x...');
  assert.ok(m.redo());
  assert.equal(m.pattern().lanes[0].hits, 'xx..x...');
});

test('a drag paints the state the first cell took, across lanes, as one undo step', () => {
  const m = new StepGridModel(beat());
  m.press(1, 0); // rest → hit
  for (const s of [1, 2, 3]) m.enter(1, s);
  m.enter(0, 3);
  assert.equal(m.enter(1, 3), false, 'already that state');
  assert.ok(m.release());
  assert.equal(m.pattern().lanes[1].hits, 'xxxx..X.');
  assert.equal(m.pattern().lanes[0].hits, 'x..xx...');
  assert.equal(m.undoStack.length, 1);
  m.undo();
  assert.deepEqual(m.pattern(), parsePattern(serializePattern(beat())));
  assert.equal(m.enter(0, 0), false, 'no stroke: nothing paints');
});

test('a press cycled back to where it was is no undo step', () => {
  const m = new StepGridModel({ ...beat(), lanes: [{ name: 'a', hits: '........' }] });
  m.paint = null;
  m.press(0, 0);
  m.put(0, 0, '.');
  assert.equal(m.release(), false);
  assert.equal(m.undoStack.length, 0);
});

test('the accent row: a step with accents loses them, else its hits become accents', () => {
  const m = new StepGridModel(beat());
  assert.ok(!accentAt(m.lanes, 0));
  assert.ok(m.toggleAccent(0));
  assert.equal(m.cell(0, 0), 'X');
  assert.ok(accentAt(m.lanes, 0));
  assert.ok(m.toggleAccent(0));
  assert.equal(m.cell(0, 0), 'x');
  assert.ok(m.toggleAccent(2));
  assert.equal(m.cell(1, 2), 'x');
  assert.equal(m.toggleAccent(1), false, 'no hits there');
  assert.equal(m.undoStack.length, 3);
});

test('levels: a drag is one undo step, clamped and rounded; reset removes the key', () => {
  const m = new StepGridModel(beat());
  assert.equal(m.level(0), 0.9);
  assert.equal(m.level(1), 1, 'the default');
  m.setLevel(1, 0.5);
  m.setLevel(1, 0.333333);
  assert.ok(m.commitLevel());
  assert.equal(m.pattern().lanes[1].level, 0.33);
  assert.equal(m.undoStack.length, 1);
  m.setLevel(1, 7);
  m.commitLevel();
  assert.equal(m.level(1), 1);
  assert.ok(m.resetLevel(1));
  assert.ok(!('level' in m.pattern().lanes[1]));
  assert.equal(m.resetLevel(1), false);
  assert.equal(m.commitLevel(), false);
});

test('step count: lanes repeat when longer, are cut when shorter', () => {
  assert.equal(resizeHits('x..X', 10), 'x..Xx..Xx.');
  assert.equal(resizeHits('x..X', 2), 'x.');
  assert.equal(resizeHits('', 3), '...');
  const m = new StepGridModel(beat());
  assert.ok(m.setSteps(16));
  assert.equal(m.pattern().lanes[0].hits, 'x...x...x...x...');
  assert.deepEqual(validatePattern(m.pattern()), []);
  assert.equal(m.setSteps(16), false);
  m.setSteps(4);
  assert.equal(m.pattern().lanes[1].hits, '..X.');
  m.undo();
  m.undo();
  assert.equal(m.steps, 8);
  assert.ok(m.setStepLength(0.5));
  assert.equal(m.pattern().step, 0.5);
  assert.equal(m.setStepLength(0), false);
});

test('clearing a lane', () => {
  const m = new StepGridModel(beat());
  assert.ok(m.clearLane(0));
  assert.equal(m.pattern().lanes[0].hits, '........');
  assert.equal(m.clearLane(0), false);
});

test('the model writes back a valid, canonical pattern', () => {
  const m = new StepGridModel(beat());
  assert.equal(serializePattern(m.pattern()), serializePattern(beat()));
});

test('layout: groups of four with a wider gap; hit-testing cells, knobs, names, the accent row', () => {
  const L = gridLayout(16, 2);
  assert.equal(stepX(L, 1) - stepX(L, 0), L.cell + L.gap);
  assert.equal(stepX(L, 4) - stepX(L, 3), L.cell + L.groupGap);
  assert.equal(L.width, stepX(L, 15) + L.cell + L.pad);
  const mid = (lane) => laneY(L, lane) + L.row / 2;
  assert.deepEqual(hitGrid(L, stepX(L, 5) + 3, mid(1)), { area: 'cell', lane: 1, step: 5 });
  // In the gap between groups: the nearer cell.
  assert.deepEqual(hitGrid(L, stepX(L, 4) - 2, mid(0)), { area: 'cell', lane: 0, step: 4 });
  assert.deepEqual(hitGrid(L, stepX(L, 3) + L.cell + 2, mid(0)), { area: 'cell', lane: 0, step: 3 });
  assert.deepEqual(hitGrid(L, stepX(L, 0), laneY(L, 2) + 5), { area: 'cell', lane: 2, step: 0 }, 'the accent row');
  assert.deepEqual(hitGrid(L, L.left - 24, mid(1)), { area: 'knob', lane: 1 });
  assert.deepEqual(hitGrid(L, 20, mid(0)), { area: 'name', lane: 0 });
  assert.equal(hitGrid(L, 20, laneY(L, 2) + 5), null, 'no knob on the accent row');
  assert.equal(hitGrid(L, 200, 10), null, 'the header');
  assert.equal(hitGrid(L, 200, L.height + 50), null);
  assert.equal(hitGrid(L, L.width + 100, mid(0)), null);
});

test('a level change not yet committed becomes its own undo step before a press, an accent, undo or redo', () => {
  const m = new StepGridModel(beat());
  m.setLevel(1, 0.4);
  m.press(0, 1);
  m.release();
  assert.equal(m.undoStack.length, 2, 'the level, then the press');
  m.undo();
  assert.equal(m.level(1), 0.4);
  m.undo();
  assert.equal(m.level(1), 1);
  m.setLevel(0, 0.2);
  m.toggleAccent(0);
  m.undo();
  assert.equal(m.level(0), 0.2);
  m.setLevel(0, 0.1);
  m.undo();
  assert.equal(m.level(0), 0.2, 'undo took back the level just set');
  m.setLevel(0, 0.3);
  m.redo();
  assert.equal(m.level(0), 0.3, 'the new level dropped the redo');
});

test('a load drops a paint stroke and a level drag in progress', () => {
  const m = new StepGridModel(beat());
  m.press(0, 1);
  m.setLevel(0, 0.1);
  m.load(beat());
  assert.equal(m.release(), false);
  assert.equal(m.commitLevel(), false);
  assert.equal(m.pattern().lanes[0].hits, 'x...x...');
});

test('the default model is a valid empty grid', () => {
  assert.deepEqual(validatePattern(new StepGridModel().pattern()), []);
});
