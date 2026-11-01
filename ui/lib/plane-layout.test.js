// ADR 0051 §3–4, tests 3 and 4 (phase 1's part): equal widths, rails,
// centred and scaled panels, peripherals right of the Stacks (receiving)
// or left (sending only), deterministic layouts that never depend on the
// view.

import test from 'node:test';
import assert from 'node:assert/strict';
import { PU } from './plate.js';
import { RAIL_PU } from './plane-file.js';
import { BOX_PU, CABLE_GAP_PU, HEADER_PU, OTHERS_PU, STACK_GAP_PU, TEXT, boxPorts, boxSize, cablePath, capChars, othersBoxes, othersSize, pinPlaces, planeLayout, stackLayout, textSize, textSizes } from './plane-layout.js';
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

test('the list of other devices: below the receiving devices, its head alone collapsed, a small box per device expanded', () => {
  const boxes = deviceBoxes([{ index: 0, name: 'SSL', id: 's', direction: 'output', configs: [{ channels: 4 }] }]);
  const stacks = [{ name: 'primary', at: null, size: { w: 1280, h: 3000 } }];
  const collapsed = planeLayout({ stacks, boxes, others: { count: 9, expanded: false } });
  const ssl = collapsed.boxes.get('id:s');
  assert.equal(collapsed.others.x, ssl.x);
  assert.ok(collapsed.others.y >= ssl.y + ssl.h);
  assert.deepEqual(othersSize({ count: 9, expanded: false }), { w: BOX_PU.w * PU, h: OTHERS_PU.head * PU });
  assert.deepEqual(othersBoxes({ count: 9, expanded: false }), []);
  const expanded = planeLayout({ stacks, boxes, others: { count: 9, expanded: true } });
  const small = othersBoxes({ count: 9, expanded: true });
  assert.equal(small.length, 9);
  // Inside the list, below its head, none overlapping.
  for (const [i, b] of small.entries()) {
    assert.ok(b.x >= 0 && b.x + b.w <= expanded.others.w && b.y >= OTHERS_PU.head * PU && b.y + b.h <= expanded.others.h, `box ${i} inside`);
    if (i) assert.ok(b.y >= small[i - 1].y + small[i - 1].h, `box ${i} below the last`);
  }
  // Nothing else moves when the list opens: the Stacks and the whole boxes stand where they stood.
  assert.deepEqual(expanded.stacks, collapsed.stacks);
  assert.deepEqual(expanded.boxes, collapsed.boxes);
  assert.equal(planeLayout({ stacks, boxes }).others, null);
});

test('text sizes: readable at the Setup’s fit, scaling with the zoom above, never past their room, continuous', () => {
  // The studio's default Setup fit on a 1400 × 860 stage is about k 0.25 (measured): names at 11 px, port labels at 9.5 px or more.
  const k = 0.25;
  for (const [name, rule] of Object.entries(TEXT)) assert.ok(textSize(rule, k) * k >= rule.min - 1e-9, `${name} at the fit`);
  assert.ok(textSize(TEXT.title, k) * k >= 11 && textSize(TEXT.port, k) * k >= 9.5);
  // At the Stack level and above the text is its base size in plane units: it scales with the zoom.
  assert.deepEqual(textSizes(1.05), Object.fromEntries(Object.entries(TEXT).map(([n, r]) => [n, r.base])));
  // On screen, a text never shrinks as the view zooms in, and its plane size never grows: no pop, no jump.
  for (const rule of Object.values(TEXT)) {
    let prev = null;
    for (let kk = 0.01; kk <= 4; kk *= 1.001) {
      const size = textSize(rule, kk);
      assert.ok(size >= rule.base && size <= rule.cap);
      if (prev) {
        assert.ok(size <= prev.size + 1e-9 && size * kk >= prev.size * prev.k - 1e-9);
        assert.ok(Math.abs(size - prev.size) <= prev.size * 0.0011, 'continuous');
      }
      prev = { size, k: kk };
    }
  }
});

test('the layout holds every text at its cap: rows, title, header and names cut to fit', () => {
  // A port row holds its name and its role at their caps, one above the other.
  assert.ok(BOX_PU.port * PU >= TEXT.port.cap + TEXT.role.cap + 8);
  // The title area holds the kind line and the name.
  assert.ok(BOX_PU.title * PU >= TEXT.kind.cap + TEXT.title.cap + 16);
  // The Stack's header holds its name.
  assert.ok(HEADER_PU * PU >= TEXT.head.cap * 1.2);
  // A small box holds its name and its channels.
  assert.ok(OTHERS_PU.box * PU >= TEXT.small.cap + TEXT.kind.cap + 8);
  assert.ok(OTHERS_PU.head * PU >= TEXT.title.cap * 1.3);
  // "main · stereo" fits a port's half of a box at its cap; names are cut to what fits at theirs.
  const half = (BOX_PU.w * PU) / 2 - 9 - 24;
  assert.ok(capChars(TEXT.role, half) >= 'main · stereo'.length);
  assert.ok(capChars(TEXT.title, BOX_PU.w * PU - 48) >= 'Mac Studio Speakers'.length);
});
