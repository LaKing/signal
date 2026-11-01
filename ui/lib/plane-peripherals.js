// The peripherals of the Signal Project plane (ADR 0051 §2–3): this
// machine's audio devices, one box per device (one even if it plays and
// records), as the Setup and the device inventory give them
// (`setup.get`, `setup.devices`). **Our signal direction:** a device's
// playback channels are inputs on its box (left edge: we send to them), its
// capture channels are outputs (right edge). Ports are named as the device
// names them: playback in stereo pairs ("Out 1–2", "Out 3–4"), capture per
// channel ("In 1", "In 2"); the inventory reads no CoreAudio channel names
// yet (ADR 0038 §10), so these are the names until it does.
// The project names roles, never hardware (ADR 0038): a role endpoint
// (`std.audio_out` is `main`) is drawn on the channels its role is bound to
// on this machine (today the Setup's `[output]` and `output_channels`), and
// a Setup-level cable is two texts drawn as one: the project's connection
// into the endpoint and that binding. Nothing here writes a file. Pure;
// no DOM.

/** The kinds drawn in phase 1. */
export const KINDS = Object.freeze(['audio']);

/** A device's channels in one direction: the most any of its configurations has. */
export function channelsOf(device) {
  return Math.max(0, ...(device?.configs ?? []).map((c) => Number(c.channels) || 0));
}

/** A device's key on the plane: its stable id, else its name (one box per device, both directions). */
export function deviceKey(device) {
  return device?.id ? `id:${device.id}` : `name:${device?.name ?? ''}`;
}

/** The playback ports of a device of `n` channels, in stereo pairs: `[{key, name, channels}]` ("Out 1–2"; an odd last one "Out 5"). */
export function playbackPorts(n) {
  const ports = [];
  for (let c = 1; c <= n; c += 2) {
    const channels = c + 1 <= n ? [c, c + 1] : [c];
    ports.push({ key: `out-${c}`, name: channels.length === 2 ? `Out ${c}–${c + 1}` : `Out ${c}`, channels });
  }
  return ports;
}

/** The capture ports of a device of `n` channels, one per channel ("In 1"). */
export function capturePorts(n) {
  return Array.from({ length: n }, (_, i) => ({ key: `in-${i + 1}`, name: `In ${i + 1}`, channels: [i + 1] }));
}

/**
 * The device boxes of the inventory (`setup.devices`' `devices`, in its
 * order), one per device: `{key, name, present, inputs, outputs, side,
 * note}`. `inputs` are the device's playback ports (we send to them, the
 * box's left edge), `outputs` its capture ports (the right edge). `side`:
 * `right` for a device that receives from Stacks (it has playback
 * channels), `left` for one that only sends (ADR 0051 §3, "Peripheral
 * places"). Inventory order is kept, by each device's first entry.
 */
export function deviceBoxes(devices = []) {
  const boxes = new Map();
  for (const d of devices) {
    const key = deviceKey(d);
    if (!boxes.has(key)) boxes.set(key, { key, name: d.name, present: true, playback: 0, capture: 0, indexes: [], note: null });
    const box = boxes.get(key);
    box.indexes.push(d.index);
    if (d.direction === 'output') box.playback = Math.max(box.playback, channelsOf(d));
    else if (d.direction === 'input') box.capture = Math.max(box.capture, channelsOf(d));
  }
  return [...boxes.values()].map(({ playback, capture, ...box }) => ({
    ...box,
    inputs: playbackPorts(playback),
    outputs: capturePorts(capture),
    side: playback > 0 ? 'right' : 'left',
  }));
}

/**
 * The channels a role of `width` ports plays on (ADR 0038; today the
 * Setup's `output_channels`, project-format.md §9): the map when it is
 * usable (as long as the role is wide, each channel from 1, at most the
 * device's, none twice), else the identity map, as the player does.
 */
export function boundChannels(map, width, deviceChannels = Infinity) {
  const ok = Array.isArray(map) && map.length === width && map.every((c) => Number.isInteger(c) && c >= 1 && c <= deviceChannels) && new Set(map).size === map.length;
  return ok ? [...map] : Array.from({ length: width }, (_, i) => i + 1);
}

/**
 * Where the Setup binds role `main` on this machine: `{key, present,
 * name, reason}`. The Setup's `[output]` found in the inventory is that
 * device; one the inventory does not find is bound but absent (a hollow
 * box with the Setup's name); no `[output]` is the system's default output.
 * No device at all (an empty inventory, headless): `null`.
 */
export function mainBinding(setup, devices) {
  const list = devices?.devices ?? [];
  const wanted = setup?.setup?.output ?? null;
  const found = devices?.output?.found;
  if (wanted) {
    const device = Number.isInteger(found) ? list.find((d) => d.index === found) : null;
    if (device) return { key: deviceKey(device), present: true, name: device.name, reason: null };
    return { key: `absent:${wanted.name ?? ''}`, present: false, name: wanted.name ?? 'the Setup’s output', reason: devices?.output?.unresolved ?? 'not present' };
  }
  const fallback = list.find((d) => d.direction === 'output' && d.default) ?? null;
  return fallback ? { key: deviceKey(fallback), present: true, name: fallback.name, reason: null } : null;
}

/**
 * The peripherals and role bindings drawn at the Setup level: `{boxes,
 * bindings}`. `setup`: `setup.get`'s reply; `devices`: `setup.devices`'
 * (or null: none listed, headless); `endpoints`: `roleEndpoints`'.
 * `bindings`: `[{endpoint, role, layout, ports, device, channels, heard,
 * note}]`, one per role endpoint: `device` the key of the box it plays on
 * (null when not heard on this machine), `channels` the device channel of
 * each endpoint port. A device the Setup binds but the inventory lacks is
 * a box of its own, `present: false`, "not present".
 */
export function peripherals({ setup = null, devices = null, endpoints = [] } = {}) {
  const boxes = deviceBoxes(devices?.devices ?? []);
  const main = mainBinding(setup, devices);
  if (main && !main.present) {
    const width = Math.max(2, ...endpoints.filter((e) => e.role === 'main').map((e) => e.ports.length));
    boxes.push({ key: main.key, name: main.name, present: false, indexes: [], note: `not present${main.reason && main.reason !== 'not present' ? `: ${main.reason}` : ''}`, inputs: playbackPorts(width), outputs: [], side: 'right' });
  }
  const bindings = endpoints.map((e) => {
    if (e.role !== 'main' || !main) return { endpoint: e.name, role: e.role, layout: e.layout, ports: e.ports, device: null, channels: [], heard: false, note: 'not heard on this machine' };
    const box = boxes.find((b) => b.key === main.key);
    const deviceChannels = box ? box.inputs.reduce((n, p) => Math.max(n, ...p.channels), 0) : Infinity;
    const channels = boundChannels(setup?.setup?.outputChannels ?? null, e.ports.length, main.present ? deviceChannels : Infinity);
    return { endpoint: e.name, role: e.role, layout: e.layout, ports: e.ports, device: main.key, channels, heard: main.present, note: main.present ? null : 'not present' };
  });
  return { boxes, bindings };
}

/** The label of a role endpoint as drawn on its device's port ("main · stereo"). */
export function endpointLabel(binding) {
  return `${binding.role} · ${binding.layout}`;
}

/** The port of `box` that carries device channel `channel` on its playback side, or null. */
export function playbackPortOf(box, channel) {
  return box?.inputs.find((p) => p.channels.includes(channel)) ?? null;
}

/**
 * The Setup-level cables (ADR 0051 §3): for each role binding, each
 * endpoint port fed from a Stack's edge pin (`pins`: Map(stack name →
 * `edgePins`')), one cable `{stack, pin, endpoint, port, device, channel,
 * dashed}` from that pin to the device channel the binding gives the port.
 * `dashed` when the device is absent. A role not heard here has no cable
 * to a device (its endpoint is drawn hollow beside the Stack).
 */
export function setupCables(pins, bindings) {
  const cables = [];
  for (const b of bindings) {
    if (!b.device) continue;
    b.ports.forEach((port, i) => {
      for (const [stack, list] of pins) {
        for (const pin of list) {
          if (pin.side !== 'out' || !pin.outside.includes(port)) continue;
          cables.push({ stack, pin: pin.address, endpoint: b.endpoint, port, device: b.device, channel: b.channels[i], dashed: !b.heard });
        }
      }
    });
  }
  return cables;
}
