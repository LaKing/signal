// ADR 0051 §3–4, tests 3 and 4 (phase 1's part): equal widths, rails,
// centred and scaled panels, peripherals right of the Stacks (receiving)
// or left (sending only), deterministic layouts that never depend on the
// view.

import test from 'node:test';
import assert from 'node:assert/strict';
import { PU } from './plate.js';
import { RAIL_PU } from './plane-file.js';
import { BOX_PU, CABLE_GAP_PU, HEADER_PU, STACK_GAP_PU, boxPorts, boxSize, cablePath, pinPlaces, planeLayout, stackLayout } from './plane-layout.js';
import { deviceBoxes } from './plane-peripherals.js';

test('every instrument is the Stack’s width; a narrower panel is centred, a wider one scaled down whole; rails between', () => {
  const layout = stackLayout({ width: 160 }, [
    { entry: 'card:a', w: 1280, h: 300 },
    { entry: 'card:b', w: 640, h: 101 },
    { entry: 'card:c', w: 2560, h: 400 },
    { entry: 'card:d', w: 320, h: 80, x: 16 },
  ]);
  assert.equal(layout.w, 1280);
  assert.ok(layout.rects.every((r) => r.w === 1280 && r.x === 0));
  const [a, b, c, d] = layout.rects;
  assert.equal(a.y, HEADER_PU * PU + RAIL_PU * PU);
  assert.equal(b.y, a.y + a.h + RAIL_PU * PU);
  assert.deepEqual(b.panel, { x: 320, y: b.y, w: 640, h: 101, scale: 1 });
  assert.equal(b.h, 104); // whole pu, never under the panel
  assert.equal(c.panel.scale, 0.5);
  assert.equal(c.panel.w, 1280);
  assert.equal(c.wide, true);
  assert.equal(c.h, 200);
  assert.equal(d.panel.x, 16); // a placed card keeps its x where it fits
  assert.equal(layout.h, d.y + d.h + RAIL_PU * PU);
  // Heights vary; the layout is a function of what was measured only.
  assert.deepEqual(stackLayout({ width: 160 }, [{ entry: 'card:a', w: 1280, h: 300 }]), stackLayout({ width: 160 }, [{ entry: 'card:a', w: 1280, h: 300 }]));
});

test('edge pins stand in their instrument’s rectangle, grouped per instrument', () => {
  const { rects } = stackLayout({ width: 160 }, [
    { entry: 'card:a', w: 1280, h: 300 },
    { entry: 'card:b', w: 1280, h: 300 },
  ]);
  const pins = pinPlaces(
    [
      { side: 'out', instrument: 'card:b', address: '/x.l' },
      { side: 'out', instrument: 'card:b', address: '/x.r' },
      { side: 'in', instrument: 'card:a', address: '/y.in' },
    ],
    rects,
  );
  assert.ok(pins[0].y > rects[1].y && pins[0].y < rects[1].y + rects[1].h);
  assert.equal(pins[1].y - pins[0].y, 3 * PU);
  assert.ok(pins[2].y > rects[0].y && pins[2].y < rects[0].y + rects[0].h);
});

test('peripherals: receiving devices right of the Stacks, sending-only ones left, in inventory order; Stacks side by side', () => {
  const boxes = deviceBoxes([
    { index: 0, name: 'Mic', id: 'm', direction: 'input', configs: [{ channels: 1 }] },
    { index: 1, name: 'SSL', id: 's', direction: 'output', configs: [{ channels: 4 }] },
    { index: 2, name: 'SSL', id: 's', direction: 'input', configs: [{ channels: 2 }] },
    { index: 3, name: 'Phones', id: 'p', direction: 'output', configs: [{ channels: 2 }] },
  ]);
  const layout = planeLayout({
    stacks: [
      { name: 'primary', at: null, size: { w: 1280, h: 3000 } },
      { name: 'second', at: null, size: { w: 1280, h: 1000 } },
    ],
    boxes,
    tray: 2,
  });
  const p = layout.stacks.get('primary');
  const s = layout.stacks.get('second');
  assert.deepEqual(p, { x: 0, y: 0, w: 1280, h: 3000 });
  assert.equal(s.x, 1280 + STACK_GAP_PU * PU);
  const ssl = layout.boxes.get('id:s');
  const phones = layout.boxes.get('id:p');
  const mic = layout.boxes.get('id:m');
  assert.equal(ssl.x, s.x + s.w + CABLE_GAP_PU * PU);
  assert.equal(phones.x, ssl.x);
  assert.ok(phones.y > ssl.y);
  assert.equal(mic.x + mic.w, -CABLE_GAP_PU * PU);
  assert.ok(layout.tray.y >= mic.y + mic.h);
  // A Stack with `at` stands there.
  const placed = planeLayout({ stacks: [{ name: 'a', at: [10, 20], size: { w: 1280, h: 10 } }], boxes: [] });
  assert.deepEqual(placed.stacks.get('a'), { x: 80, y: 160, w: 1280, h: 10 });
  assert.deepEqual(placed.bounds, { x: 80, y: 160, w: 1280, h: 10 });
});

test('a box’s ports: playback on its left edge, capture on its right, a pin per channel inside its port’s row', () => {
  const [box] = deviceBoxes([
    { index: 0, name: 'SSL', id: 's', direction: 'output', configs: [{ channels: 4 }] },
    { index: 1, name: 'SSL', id: 's', direction: 'input', configs: [{ channels: 2 }] },
  ]);
  const size = boxSize(box);
  assert.equal(size.w, BOX_PU.w * PU);
  const ports = boxPorts(box);
  assert.ok(ports.inputs.every((p) => p.x === 0));
  assert.ok(ports.outputs.every((p) => p.x === size.w));
  const [out12] = ports.inputs;
  assert.deepEqual(out12.pins.map((p) => p.channel), [1, 2]);
  assert.ok(out12.pins.every((p) => p.y > out12.y && p.y < out12.y + out12.h));
  assert.ok(ports.inputs[1].y > out12.y);
});

test('a cable leaves the Stack and arrives at the device horizontally', () => {
  assert.equal(cablePath({ x: 0, y: 10 }, { x: 400, y: 50 }), 'M0 10 C200 10 200 50 400 50');
});
