// ADR 0047 phase 3: deep zoom's pure parts (ui/lib/board-zoom.js): a die's
// fade by its text on screen, the surroundings' fade, the view's travel into
// a chip; and the open items fixed with it: the maze's bend estimate
// (`minTurns`), the labels kept off one another (`spreadLabels`), the wheel
// left to the schematic by a control that does not own it (`wheelOwner`).
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { DIE_MAX_K, DIE_TEXT, FADE_FROM_PX, FADE_FULL_PX, FULL_K, SURROUND, belowThreshold, dieOpacity, ease, surroundOpacity, tween, zoomInView } from './board-zoom.js';
import { screenRect, toScreen } from './schematic-view.js';
import { minTurns } from './board-maze.js';
import { LABEL_CHAR, spreadLabels } from './board-route.js';
import { wheelOwner } from './wheel.js';

test('a die fades in from 4 px of text on screen, whole from 7 px; around it the level fades to SURROUND', () => {
  const at = (px) => px / DIE_TEXT;
  assert.equal(dieOpacity(at(FADE_FROM_PX)), 0);
  assert.equal(dieOpacity(at(3)), 0);
  assert.ok(Math.abs(dieOpacity(at(5.5)) - 0.5) < 1e-9);
  assert.equal(dieOpacity(at(FADE_FULL_PX)), 1);
  assert.equal(dieOpacity(at(70)), 1);
  assert.equal(dieOpacity(FULL_K), 1);
  assert.ok(belowThreshold(at(3.9)) && !belowThreshold(at(4.1)));
  // Monotonic in the scale.
  let last = -1;
  for (let px = 0; px < 10; px += 0.25) {
    const o = dieOpacity(at(px));
    assert.ok(o >= last);
    last = o;
  }
  assert.equal(surroundOpacity(0), 1);
  assert.ok(Math.abs(surroundOpacity(1) - SURROUND) < 1e-12);
  assert.ok(SURROUND >= 0.15 && SURROUND <= 0.25, 'the owner: about 15–25 %');
});

test('the travel into a chip: from the view to the chip framed, a zoom about the point both keep, eased', () => {
  const viewport = { w: 1000, h: 600 };
  const view = { k: 0.5, x: 10, y: 20 };
  const chip = { x: 300, y: 100, w: 100, h: 160 };
  const rect = screenRect(view, chip);
  const to = zoomInView(view, rect, viewport, 0.5 * 0.2, 24);
  const r = screenRect(to, chip);
  assert.ok(Math.abs(r.x + r.w / 2 - 500) < 1e-6 && Math.abs(r.y + r.h / 2 - 300) < 1e-6, 'centred');
  assert.ok(Math.abs(r.h - 552) < 1e-6, `fitted to the stage's height less its margins: ${r.h}`);
  // The package framed whatever its die's scale (the person zooms on to read); never past DIE_MAX_K.
  const small = zoomInView(view, rect, viewport, 0.5 * 0.01, 24);
  assert.ok(Math.abs(screenRect(small, chip).h - 552) < 1e-6);
  const big = zoomInView(view, rect, viewport, 0.5 * 2, 24);
  assert.ok(Math.abs((big.k / view.k) * 0.5 * 2 - DIE_MAX_K) < 1e-9);
  // The travel: its ends, the scale geometric between, one screen point kept throughout.
  assert.equal(tween(view, to, 0), view);
  assert.equal(tween(view, to, 1), to);
  const mid = tween(view, to, 0.5);
  assert.ok(Math.abs(mid.k - Math.sqrt(view.k * to.k)) < 1e-9);
  const p = { x: 123, y: 45 };
  const f = to.k / view.k;
  const a = toScreen(view, p);
  const b = toScreen(to, p);
  // The fixed point c of the screen map s ↦ f s + t: where the chip grows from.
  const c = { x: (b.x - f * a.x) / (1 - f), y: (b.y - f * a.y) / (1 - f) };
  for (const t of [0.25, 0.5, 0.75]) {
    const v = tween(view, to, t);
    const local = { x: (c.x - view.x) / view.k, y: (c.y - view.y) / view.k };
    const s = toScreen(v, local);
    assert.ok(Math.abs(s.x - c.x) < 1e-6 && Math.abs(s.y - c.y) < 1e-6, `kept at ${t}`);
  }
  // A pan alone (no zoom) is linear.
  const moved = { ...view, x: 110, y: -80 };
  const half = tween(view, moved, 0.5);
  assert.ok(Math.abs(half.x - 60) < 1e-9 && Math.abs(half.y + 30) < 1e-9 && half.k === view.k);
  assert.equal(ease(0), 0);
  assert.equal(ease(1), 1);
  assert.equal(ease(0.5), 0.5);
});

test('the maze’s bend estimate: the fewest turns left to arrive heading the goal’s way', () => {
  // Heading +x (0), arriving +x: straight ahead none; aside two; behind four.
  assert.equal(minTurns(100, 0, 0, 0), 0);
  assert.equal(minTurns(100, 50, 0, 0), 2);
  assert.equal(minTurns(-100, 0, 0, 0), 4);
  // Heading +y (2) to arrive +x: one turn when the goal is ahead on both, else three.
  assert.equal(minTurns(100, 50, 2, 0), 1);
  assert.equal(minTurns(-100, 50, 2, 0), 3);
  assert.equal(minTurns(100, -50, 2, 0), 3);
  // Heading −x (1) to arrive +x: a U, two turns with room aside.
  assert.equal(minTurns(100, 50, 1, 0), 2);
  assert.equal(minTurns(0, 0, 0, 0), 0);
  // Already on the goal's line heading across it: the one last turn.
  assert.equal(minTurns(100, 0, 2, 0), 1);
  assert.equal(minTurns(0, 50, 0, 2), 1);
});

test('net labels are kept off one another: each text moved the fewest lines, its stub at its pin', () => {
  const at = (x, y, side = 'in', text = '◂ transport.position') => ({ net: 'n', text, side, at: { x, y }, corners: [], wire: 0 });
  const labels = [at(500, 100), at(500, 104), at(500, 108), at(900, 100, 'out')];
  spreadLabels(labels);
  const rects = labels.map((l) => {
    const w = l.text.length * LABEL_CHAR;
    const x = l.side === 'out' ? l.textAt.x : l.textAt.x - w;
    return { x, y: l.textAt.y - 9, w, h: 10 };
  });
  for (let i = 0; i < rects.length; i++) {
    for (let j = i + 1; j < rects.length; j++) {
      const [a, b] = [rects[i], rects[j]];
      assert.ok(!(a.x < b.x + b.w && b.x < a.x + a.w && a.y < b.y + b.h && b.y < a.y + a.h), `labels ${i} and ${j} overlap`);
    }
  }
  assert.equal(labels[0].textAt.y, 103, 'the first stays');
  assert.equal(labels[3].textAt.y, 103, 'one apart from the others stays');
  assert.deepEqual(labels.map((l) => l.at.y), [100, 104, 108, 100], 'the stubs stay');
  // Deterministic.
  const again = [at(500, 100), at(500, 104), at(500, 108), at(900, 100, 'out')];
  spreadLabels(again);
  assert.deepEqual(again.map((l) => l.textAt), labels.map((l) => l.textAt));
});

test('the wheel goes to the innermost control that owns it: a knob not focused on the plate leaves it to the schematic', () => {
  const knob = { name: 'knob' };
  const schematic = { name: 'schematic' };
  // The knob neither focused nor clicked; the schematic fills the window.
  assert.deepEqual(wheelOwner([{ element: knob, uses: true, owns: false }, { element: schematic, uses: true, owns: true }]), { owner: schematic, decider: schematic });
  // Focused, the knob takes it.
  assert.deepEqual(wheelOwner([{ element: knob, uses: true, owns: true }, { element: schematic, uses: true, owns: true }]), { owner: knob, decider: knob });
  // No owner: the page scrolls, the innermost that uses the wheel decides (its hint).
  assert.deepEqual(wheelOwner([{ element: knob, uses: true, owns: false }]), { owner: null, decider: knob });
  // Over a part without a wheel use (the step grid's cells): the page's, without that control's hint.
  assert.deepEqual(wheelOwner([{ element: knob, uses: false, owns: false }]), { owner: null, decider: knob });
});
