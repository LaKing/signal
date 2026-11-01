#!/usr/bin/env node
// The Signal Project plane's measurement (ADR 0051 test 9: at most 5 000
// drawn elements, 50 fps; docs/benchmarks.md "The Signal Project plane").
// Headless Chrome driven over the DevTools protocol (no dependency: Node's
// own fetch and WebSocket) on the app page of a running `signal ui`, which
// prints its URL on stderr. Measures <signal-plane> (its dev-only
// `metrics()`: elements and drawn elements per layer, renders) and the
// page: frames (requestAnimationFrame), main-thread time (Performance
// metrics), WebSocket frames received per second by kind, and how many
// parameter frames repeat the last one; idle at Setup, a pinch zoom in
// through Stack, Wiring, Control and Engine, idle there, a pinch zoom out,
// a Stack scrolled with the wheel, idle at Stack. A measure, never a judge
// (AGENTS.md rule 6). Use a scratch copy of the project and a scratch
// Setup, the null driver:
//
//   signal ui --driver null --setup /tmp/x/setup.toml /tmp/x/studio 2> /tmp/x/studio.log
//   node scripts/plane-perf.mjs /tmp/x/studio.log studio [--stopped]
//
// Prints JSON; CHROME names another browser binary.

import { spawn } from 'node:child_process';
import { readFileSync, mkdtempSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';

const [logFile, label, ...flags] = process.argv.slice(2);
const stopped = flags.includes('--stopped');
const log = readFileSync(logFile, 'utf8');
const ui = log.match(/ui at (http:\/\/\S+)/)[1];
const url = ui.replace('/#', '/ui/app/index.html#');
const port = 9300 + Math.floor(Math.random() * 500);
const profile = mkdtempSync(join(tmpdir(), 'signal-plane-perf-'));
const chrome = spawn(process.env.CHROME ?? '/Applications/Google Chrome.app/Contents/MacOS/Google Chrome', [
  '--headless=new', `--remote-debugging-port=${port}`, `--user-data-dir=${profile}`, '--window-size=1440,900',
  '--no-first-run', '--no-default-browser-check', '--disable-extensions', '--mute-audio', 'about:blank',
], { stdio: 'ignore' });
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

async function json(path, method = 'GET') {
  for (let i = 0; i < 100; i++) {
    try {
      const r = await fetch(`http://127.0.0.1:${port}${path}`, { method });
      if (r.ok) return r.json();
    } catch {}
    await sleep(100);
  }
  throw new Error('no CDP');
}

const pages = await json('/json/list');
const page = pages.find((p) => p.type === 'page');
const ws = new WebSocket(page.webSocketDebuggerUrl);
await new Promise((r) => ws.addEventListener('open', r));
let next = 1;
const pending = new Map();
const wsFrames = { count: 0, bytes: 0, kinds: {}, unchangedParams: 0, params: 0, sent: {} };
const lastParams = new Map();
const asked = new Map();
const subInfo = new Map();
const perSub = new Map();
ws.addEventListener('message', (e) => {
  const m = JSON.parse(e.data);
  if (m.id && pending.has(m.id)) {
    const { resolve, reject } = pending.get(m.id);
    pending.delete(m.id);
    m.error ? reject(new Error(JSON.stringify(m.error))) : resolve(m.result);
  } else if (m.method === 'Network.webSocketFrameReceived') {
    wsFrames.count++;
    const r = m.params.response;
    if (r.opcode === 2) {
      const b = Buffer.from(r.payloadData, 'base64');
      wsFrames.bytes += b.length;
      const kind = ['?', 'meters', 'scope', 'parameters', 'vector'][b[0]] ?? '?';
      wsFrames.kinds[kind] = (wsFrames.kinds[kind] ?? 0) + 1;
      if (kind === 'parameters') {
        const sub = b.readUInt32LE(4);
        perSub.set(sub, (perSub.get(sub) ?? 0) + 1);
        const payload = b.subarray(24).toString('hex');
        wsFrames.params++;
        if (lastParams.get(sub) === payload) wsFrames.unchangedParams++;
        lastParams.set(sub, payload);
      }
    } else {
      wsFrames.bytes += r.payloadData.length;
      wsFrames.kinds.text = (wsFrames.kinds.text ?? 0) + 1;
      try {
        const msg = JSON.parse(r.payloadData);
        if (msg.type === 'subscribed' && asked.has(msg.id)) subInfo.set(msg.sub, asked.get(msg.id));
      } catch {}
    }
  } else if (m.method === 'Network.webSocketFrameSent') {
    try {
      const msg = JSON.parse(m.params.response.payloadData);
      const key = msg.type === 'subscribe' ? `subscribe ${msg.stream}` : msg.type;
      if (msg.type === 'subscribe') asked.set(msg.id, `${msg.stream} ${(msg.addresses ?? [msg.port ?? msg.ports ?? '']).join?.(',') ?? msg.port}`);
      wsFrames.sent[key] = (wsFrames.sent[key] ?? 0) + 1;
    } catch {}
  }
});
const send = (method, params = {}) => new Promise((resolve, reject) => {
  const id = next++;
  pending.set(id, { resolve, reject });
  ws.send(JSON.stringify({ id, method, params }));
});
const evaluate = async (expression) => {
  const r = await send('Runtime.evaluate', { expression, awaitPromise: true, returnByValue: true });
  if (r.exceptionDetails) throw new Error(JSON.stringify(r.exceptionDetails));
  return r.result.value;
};

await send('Emulation.setDeviceMetricsOverride', { width: 1440, height: 900, deviceScaleFactor: 2, mobile: false });
await send('Performance.enable');
await send('Network.enable');
await send('Page.enable');
await send('Page.navigate', { url });
// Laid out: the plane's element count steady for 2 s.
let last = -1;
let steady = 0;
for (let i = 0; i < 120 && steady < 4; i++) {
  await sleep(500);
  const n = await evaluate(`(() => { const p = document.querySelector('signal-plane'); return p?.metrics ? p.metrics().elements : -1; })()`);
  steady = n > 50 && n === last ? steady + 1 : 0;
  last = n;
}
await evaluate(`(async () => {
  const m = await import('/ui/signal-ui.js');
  window.__conn = await new Promise((r) => m.onDefaultConnection((c) => c && r(c)));
  window.__frames = [];
  const loop = (t) => { window.__frames.push(t); requestAnimationFrame(loop); };
  requestAnimationFrame(loop);
  return true;
})()`);
const transport = await evaluate(`window.__conn.request({ type: '${stopped ? 'transport.stop' : 'transport.play'}' }).then((r) => r.type, (e) => String(e))`);
await sleep(1500);

async function metrics() {
  const { metrics } = await send('Performance.getMetrics');
  return Object.fromEntries(metrics.map((m) => [m.name, m.value]));
}

/** One phase: `act` run while frames, main-thread time and WebSocket frames are counted. */
async function phase(name, act) {
  await evaluate('window.__frames.length = 0');
  const before = await metrics();
  const r0 = await evaluate(`(() => { const m = document.querySelector('signal-plane').metrics(); return { renders: m.renders, renderMs: m.renderMs }; })()`);
  const f0 = { ...wsFrames, kinds: { ...wsFrames.kinds } };
  const s0 = new Map(perSub);
  const t0 = performance.now();
  const peak = { drawn: 0, elements: 0, level: null };
  const sample = async () => {
    const m = await evaluate(`(() => { const m = document.querySelector('signal-plane').metrics(); return { drawn: m.drawn, elements: m.elements, level: m.level, layers: m.layers }; })()`);
    if (m.drawn > peak.drawn) Object.assign(peak, m);
    return m;
  };
  const levels = [];
  await act(async () => levels.push(await sample()));
  const seconds = (performance.now() - t0) / 1000;
  const after = await metrics();
  const frames = await evaluate('window.__frames.slice()');
  const r1 = await evaluate(`(() => { const m = document.querySelector('signal-plane').metrics(); return { renders: m.renders, renderMs: m.renderMs }; })()`);
  const gaps = frames.slice(1).map((t, i) => t - frames[i]).sort((a, b) => a - b);
  const q = (p) => gaps.length ? Math.round(gaps[Math.min(gaps.length - 1, Math.floor(p * gaps.length))] * 10) / 10 : null;
  const d = (k) => (after[k] - before[k]) * 1000;
  const n = Math.max(1, frames.length);
  return {
    phase: name,
    seconds: Math.round(seconds * 100) / 100,
    fps: Math.round((frames.length / seconds) * 10) / 10,
    frameMs: { p50: q(0.5), p95: q(0.95), max: q(1) },
    over20ms: gaps.filter((g) => g > 20).length,
    mainThreadMsPerFrame: { task: +(d('TaskDuration') / n).toFixed(2), script: +(d('ScriptDuration') / n).toFixed(2), layout: +(d('LayoutDuration') / n).toFixed(2), style: +(d('RecalcStyleDuration') / n).toFixed(2) },
    mainThreadBusyPct: Math.round((d('TaskDuration') / (seconds * 1000)) * 1000) / 10,
    renders: r1.renders - r0.renders,
    renderMsPerRender: r1.renders > r0.renders ? +((r1.renderMs - r0.renderMs) / (r1.renders - r0.renders)).toFixed(3) : null,
    wsFramesPerSec: Math.round(((wsFrames.count - f0.count) / seconds) * 10) / 10,
    wsKBPerSec: Math.round(((wsFrames.bytes - f0.bytes) / seconds / 1024) * 10) / 10,
    wsPerSecByKind: Object.fromEntries(Object.entries(wsFrames.kinds).map(([k, v]) => [k, Math.round(((v - (f0.kinds[k] ?? 0)) / seconds) * 10) / 10])),
    parametersUnchangedPct: wsFrames.params > f0.params ? Math.round(((wsFrames.unchangedParams - f0.unchangedParams) / (wsFrames.params - f0.params)) * 1000) / 10 : null,
    activeParameterSubs: [...perSub].filter(([k, v]) => v > (s0.get(k) ?? 0)).length,
    topParameterSubs: [...perSub].map(([k, v]) => [subInfo.get(k) ?? `sub ${k}`, v - (s0.get(k) ?? 0)]).filter(([, v]) => v > 0).sort((a, b) => b[1] - a[1] || String(a[0]).localeCompare(String(b[0]))).slice(0, 400).map(([k]) => k),
    peakDrawn: peak.drawn,
    peakElements: peak.elements,
    peakLevel: peak.level,
    levels: levels.map((l) => `${l.level}:${l.drawn}/${l.elements}`),
    peakLayers: Object.fromEntries(Object.entries(peak.layers ?? {}).map(([k, v]) => [k, k === 'stacks' ? Object.fromEntries(Object.entries(v).map(([s, c]) => [s, `${c.drawn}/${c.elements}`])) : `${v.drawn}/${v.elements}`])),
  };
}

const centre = { x: 720, y: 450 };
const wheel = (dy, ctrl, at = centre) => send('Input.dispatchMouseEvent', { type: 'mouseWheel', x: at.x, y: at.y, deltaX: 0, deltaY: dy, modifiers: ctrl ? 2 : 0 });
const results = [];
// Back to the whole Setup.
await evaluate(`(() => { const p = document.querySelector('signal-plane'); p.fitSetup?.(); return true; })()`);
await sleep(1500);
results.push(await phase('idle at Setup, 5 s', async (s) => { await s(); await sleep(5000); await s(); }));
// Pan at Setup: a mouse's wheel zooms there, so pan by a trackpad-like horizontal-less... use pinch out/in pairs instead.
results.push(await phase('pinch zoom in, Setup -> Stack -> Wiring -> Control', async (s) => {
  const stack = await evaluate(`(() => { const p = document.querySelector('signal-plane'); const r = p.focusRect?.() ?? [...p.layout.stacks.values()][0]; const v = p.view; return { x: v.x + (r.x + r.w / 2) * v.k, y: v.y + (r.y + r.h * 0.3) * v.k }; })()`);
  const at = { x: Math.round(Math.max(10, Math.min(1430, stack.x))), y: Math.round(Math.max(10, Math.min(890, stack.y))) };
  await s();
  for (let i = 0; i < 160; i++) {
    await wheel(-12, true, at);
    await sleep(16);
    if (i % 20 === 19) await s();
  }
  await sleep(800);
  await s();
}));
results.push(await phase('idle at the deepest level reached, 3 s', async (s) => { await s(); await sleep(3000); await s(); }));
results.push(await phase('pinch zoom out to Setup', async (s) => {
  for (let i = 0; i < 160; i++) {
    await wheel(12, true);
    await sleep(16);
    if (i % 20 === 19) await s();
  }
  await sleep(800);
  await s();
}));
// Focus the primary Stack and scroll it with the wheel (the Stack level).
await evaluate(`(() => { const p = document.querySelector('signal-plane'); p.focusStack(p.focused ?? [...p.stacks.keys()][0]); return true; })()`);
await sleep(1500);
results.push(await phase('Stack level: wheel scroll down and up', async (s) => {
  await s();
  for (let i = 0; i < 120; i++) {
    await wheel(i < 60 ? 40 : -40, false);
    await sleep(16);
    if (i % 30 === 29) await s();
  }
  await sleep(500);
  await s();
}));
results.push(await phase('idle at Stack, 5 s', async (s) => { await s(); await sleep(5000); await s(); }));
const subs = await evaluate(`(async () => { const s = await window.__conn.request({ type: 'stats' }).catch((e) => ({ error: String(e) })); return s; })()`);
console.log(JSON.stringify({ label, stopped, transport, sentTotals: wsFrames.sent, url: url.replace(/token=[0-9a-f]+/, 'token=…'), results, stats: subs }, null, 1));
ws.close();
chrome.kill();
setTimeout(() => {
  rmSync(profile, { recursive: true, force: true });
  process.exit(0);
}, 500);
