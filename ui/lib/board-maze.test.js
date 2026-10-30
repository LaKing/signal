// ADR 0047 §6 (the owner's verdict of 2026-10-30): the maze router of the plate's legs (ui/lib/board-maze.js).
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { Maze } from './board-maze.js';

const bounds = { x0: -64, y0: -64, x1: 640, y1: 400 };
const through = ([p, q], b) => Math.min(p.x, q.x) < b.x + b.w && Math.max(p.x, q.x) > b.x && Math.min(p.y, q.y) < b.y + b.h && Math.max(p.y, q.y) > b.y;

test('a leg goes round a control, never under it, out of its pin to the right and into its pin from the left', () => {
  const own = { id: 'own', x: 0, y: 100, w: 56, h: 80 };
  const wall = { id: 'wall', x: 72, y: 60, w: 56, h: 200 };
  const chip = { id: 'chip', x: 400, y: 100, w: 100, h: 100 };
  const a = { x: 36, y: 140 };
  const b = { x: 400, y: 150 };
  const maze = new Maze([own, wall, chip], [a, b], [], bounds);
  const c = maze.route(a, b, 'n', { own: [own] });
  assert.ok(c);
  assert.deepEqual(c[0], a);
  assert.deepEqual(c.at(-1), b);
  assert.ok(c[1].x > a.x && c[1].y === a.y, 'leaves to the right');
  assert.ok(c.at(-2).x < b.x && c.at(-2).y === b.y, 'enters from the left');
  for (let k = 1; k < c.length; k++) {
    assert.ok(c[k - 1].x === c[k].x || c[k - 1].y === c[k].y, 'orthogonal');
    assert.ok(!through([c[k - 1], c[k]], wall), 'not under the wall');
  }
  // The same again: the same route (deterministic).
  const again = new Maze([own, wall, chip], [a, b], [], bounds).route(a, b, 'n', { own: [own] });
  assert.deepEqual(again, c);
});

test('two nets never share a way; one bus’s legs may, and run together', () => {
  const chip = { id: 'chip', x: 400, y: 100, w: 100, h: 120 };
  const ends = [{ x: 0, y: 40 }, { x: 0, y: 56 }];
  const pins = [{ x: 400, y: 140 }, { x: 400, y: 156 }];
  const run = (bus) => {
    const maze = new Maze([chip], [...ends, ...pins], [], bounds);
    return [maze.route(ends[0], pins[0], 'a', { bus }), maze.route(ends[1], pins[1], 'b', { bus })];
  };
  const segs = (c) => c.slice(1).map((q, i) => [c[i], q]);
  const overlap = ([p, q], [r, s]) =>
    (p.x === q.x && r.x === s.x && p.x === r.x && Math.max(Math.min(p.y, q.y), Math.min(r.y, s.y)) < Math.min(Math.max(p.y, q.y), Math.max(r.y, s.y))) ||
    (p.y === q.y && r.y === s.y && p.y === r.y && Math.max(Math.min(p.x, q.x), Math.min(r.x, s.x)) < Math.min(Math.max(p.x, q.x), Math.max(r.x, s.x)));
  const [a, b] = run(null);
  assert.ok(a && b);
  for (const s of segs(a)) for (const t of segs(b)) assert.ok(!overlap(s, t), 'two nets apart');
  const [c, d] = run('bus:chip');
  assert.ok(c && d);
});
