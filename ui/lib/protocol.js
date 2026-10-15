// Signal's UI protocol, client side (docs/ui-protocol.md). Plain ES module,
// no build step. The pure functions here are tested with `node --test ui/`;
// `Client` is a thin wrapper around a browser WebSocket.

/** The protocol version this client speaks. */
export const PROTOCOL = 1;

/** The binary frame header's size in bytes. */
export const HEADER = 24;

/** Binary frame kinds. */
export const KIND = Object.freeze({ METERS: 1, SCOPE: 2, PARAMETERS: 3, VECTOR: 4 });

/**
 * The WebSocket port and the session token from the page URL's fragment
 * (`#ws=<port>&token=<token>`); the fragment never goes over HTTP.
 */
export function parseFragment(hash) {
  const params = new URLSearchParams(String(hash || '').replace(/^#/, ''));
  const ws = Number.parseInt(params.get('ws') ?? '', 10);
  return {
    ws: Number.isInteger(ws) && ws > 0 && ws < 65536 ? ws : null,
    token: params.get('token') || null,
  };
}

/** The WebSocket URL on the page's host. */
export function socketUrl(hostname, port) {
  const host = hostname.includes(':') && !hostname.startsWith('[') ? `[${hostname}]` : hostname;
  return `ws://${host}:${port}/`;
}

/** The first message every client sends. */
export function helloMessage(token) {
  return { type: 'hello', protocol: PROTOCOL, token };
}

/**
 * A binary frame as an object: its header fields and its values.
 * Meters: `[{peak, rms}]` per channel; scope: a Float32Array of points;
 * vector: a Float32Array of the rows at one frame (`firstFrame`; `extra` is
 * the decimation); parameters: `[{base, effective}]` per address (NaN when not known yet).
 */
export function decodeFrame(buffer) {
  const bytes = buffer instanceof ArrayBuffer ? buffer : buffer.buffer.slice(buffer.byteOffset, buffer.byteOffset + buffer.byteLength);
  if (bytes.byteLength < HEADER) {
    throw new Error(`a frame has at least ${HEADER} bytes, not ${bytes.byteLength}`);
  }
  const view = new DataView(bytes);
  const frame = {
    kind: view.getUint8(0),
    version: view.getUint8(1),
    sub: view.getUint32(4, true),
    firstFrame: Number(view.getBigUint64(8, true)),
    count: view.getUint32(16, true),
    extra: view.getUint32(20, true),
    values: null,
  };
  const size = { [KIND.METERS]: 8, [KIND.SCOPE]: 4, [KIND.PARAMETERS]: 16, [KIND.VECTOR]: 4 }[frame.kind];
  if (size === undefined) {
    throw new Error(`unknown frame kind ${frame.kind}`);
  }
  if (bytes.byteLength !== HEADER + size * frame.count) {
    throw new Error(`a frame of ${frame.count} values has ${HEADER + size * frame.count} bytes, not ${bytes.byteLength}`);
  }
  const values = [];
  for (let i = 0; i < frame.count; i++) {
    const at = HEADER + i * size;
    if (frame.kind === KIND.METERS) {
      values.push({ peak: view.getFloat32(at, true), rms: view.getFloat32(at + 4, true) });
    } else if (frame.kind === KIND.PARAMETERS) {
      values.push({ base: view.getFloat64(at, true), effective: view.getFloat64(at + 8, true) });
    } else {
      values.push(view.getFloat32(at, true));
    }
  }
  frame.values = frame.kind === KIND.SCOPE || frame.kind === KIND.VECTOR ? Float32Array.from(values) : values;
  return frame;
}

/** A linear level in dBFS (−Infinity for silence). */
export function toDb(level) {
  return level > 0 ? 20 * Math.log10(level) : -Infinity;
}

/** A level's place on a meter from `floor` dBFS (0) to 0 dBFS (1). */
export function meterFraction(level, floor = -60) {
  const db = toDb(level);
  if (!Number.isFinite(db)) return 0;
  return Math.min(1, Math.max(0, (db - floor) / -floor));
}

/**
 * A protocol client: `hello` first, then requests with ids; replies and
 * pushed messages go to `onMessage`, binary frames (decoded) to `onFrame`.
 */
export class Client {
  constructor(url, token, { WebSocketImpl = globalThis.WebSocket, onMessage = () => {}, onFrame = () => {}, onClose = () => {} } = {}) {
    this.nextId = 1;
    this.socket = new WebSocketImpl(url);
    this.socket.binaryType = 'arraybuffer';
    this.socket.onopen = () => this.socket.send(JSON.stringify(helloMessage(token)));
    this.socket.onmessage = (event) => {
      if (typeof event.data === 'string') {
        onMessage(JSON.parse(event.data));
      } else {
        onFrame(decodeFrame(event.data));
      }
    };
    this.socket.onclose = (event) => onClose(event);
  }

  /** Sends `message` with a new id, which its reply carries. */
  request(message) {
    const id = this.nextId++;
    this.socket.send(JSON.stringify({ ...message, id }));
    return id;
  }
}
