import test from 'node:test';
import assert from 'node:assert/strict';

import { Client, HEADER, KIND, PROTOCOL, decodeFrame, helloMessage, meterFraction, parseFragment, socketUrl, toDb } from './protocol.js';

/** A frame as the server writes it (crates/signal-ui/src/frames.rs). */
function frame(kind, sub, first, count, extra, write) {
  const size = { [KIND.METERS]: 8, [KIND.SCOPE]: 4, [KIND.PARAMETERS]: 16 }[kind];
  const buffer = new ArrayBuffer(HEADER + size * count);
  const view = new DataView(buffer);
  view.setUint8(0, kind);
  view.setUint8(1, PROTOCOL);
  view.setUint32(4, sub, true);
  view.setBigUint64(8, BigInt(first), true);
  view.setUint32(16, count, true);
  view.setUint32(20, extra, true);
  for (let i = 0; i < count; i++) write(view, HEADER + i * size, i);
  return buffer;
}

test('the fragment gives the WebSocket port and the token', () => {
  assert.deepEqual(parseFragment('#ws=50123&token=ab12'), { ws: 50123, token: 'ab12' });
  assert.deepEqual(parseFragment(''), { ws: null, token: null });
  assert.deepEqual(parseFragment('#ws=70000&token='), { ws: null, token: null });
});

test('the socket URL is on the page host', () => {
  assert.equal(socketUrl('127.0.0.1', 9000), 'ws://127.0.0.1:9000/');
  assert.equal(socketUrl('::1', 9000), 'ws://[::1]:9000/');
});

test('hello carries the protocol and the token', () => {
  assert.deepEqual(helloMessage('t'), { type: 'hello', protocol: 1, token: 't' });
});

test('meter frames decode per channel', () => {
  const buffer = frame(KIND.METERS, 7, 4800, 2, 480, (v, at, i) => {
    v.setFloat32(at, 0.5 * (i + 1), true);
    v.setFloat32(at + 4, 0.25, true);
  });
  const decoded = decodeFrame(buffer);
  assert.equal(decoded.kind, KIND.METERS);
  assert.equal(decoded.version, 1);
  assert.equal(decoded.sub, 7);
  assert.equal(decoded.firstFrame, 4800);
  assert.equal(decoded.extra, 480);
  assert.deepEqual(decoded.values, [{ peak: 0.5, rms: 0.25 }, { peak: 1, rms: 0.25 }]);
});

test('scope frames decode to points', () => {
  const buffer = frame(KIND.SCOPE, 2, 2 ** 40, 3, 10, (v, at, i) => v.setFloat32(at, i - 1, true));
  const decoded = decodeFrame(buffer);
  assert.equal(decoded.firstFrame, 2 ** 40);
  assert.equal(decoded.extra, 10);
  assert.deepEqual(Array.from(decoded.values), [-1, 0, 1]);
});

test('parameter frames carry base and effective, NaN when unknown', () => {
  const buffer = frame(KIND.PARAMETERS, 3, 0, 2, 0, (v, at, i) => {
    v.setFloat64(at, i === 0 ? 0.75 : NaN, true);
    v.setFloat64(at + 8, i === 0 ? 0.5 : NaN, true);
  });
  const [known, unknown] = decodeFrame(buffer).values;
  assert.deepEqual(known, { base: 0.75, effective: 0.5 });
  assert.ok(Number.isNaN(unknown.base) && Number.isNaN(unknown.effective));
});

test('short, unknown or mis-sized frames are refused', () => {
  assert.throws(() => decodeFrame(new ArrayBuffer(10)), /at least 24/);
  const unknown = frame(KIND.SCOPE, 1, 0, 0, 1, () => {});
  new DataView(unknown).setUint8(0, 9);
  assert.throws(() => decodeFrame(unknown), /unknown frame kind 9/);
  const short = frame(KIND.SCOPE, 1, 0, 2, 1, () => {}).slice(0, HEADER + 4);
  assert.throws(() => decodeFrame(short), /has 32 bytes, not 28/);
});

test('levels in dBFS and on a meter', () => {
  assert.equal(toDb(1), 0);
  assert.equal(toDb(0), -Infinity);
  assert.ok(Math.abs(toDb(0.5) + 6.0206) < 1e-3);
  assert.equal(meterFraction(1), 1);
  assert.equal(meterFraction(0), 0);
  assert.equal(meterFraction(0.001), 0);
  assert.ok(Math.abs(meterFraction(10 ** (-30 / 20)) - 0.5) < 1e-9);
});

test('the client says hello first, then numbers its requests', () => {
  const sent = [];
  class FakeSocket {
    constructor(url) { this.url = url; }
    send(text) { sent.push(JSON.parse(text)); }
  }
  const messages = [];
  const client = new Client('ws://127.0.0.1:1/', 'tok', { WebSocketImpl: FakeSocket, onMessage: (m) => messages.push(m) });
  client.socket.onopen();
  assert.equal(client.request({ type: 'describe' }), 1);
  assert.equal(client.request({ type: 'stats' }), 2);
  assert.deepEqual(sent, [helloMessage('tok'), { type: 'describe', id: 1 }, { type: 'stats', id: 2 }]);
  client.socket.onmessage({ data: '{"type":"welcome","protocol":1}' });
  assert.deepEqual(messages, [{ type: 'welcome', protocol: 1 }]);
});
