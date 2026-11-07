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

/** A box shows at most this many port rows per edge; the channels past them are one row ("Out 11–64"), so a 64-channel loopback device stays a box. */
export const MAX_PORT_ROWS = 6;

const range = (from, to) => Array.from({ length: to - from + 1 }, (_, i) => from + i);

/** The rest of a device's channels from `from` to `n`, one row: `{key, name, channels, rest: true}`. */
function restPort(prefix, key, from, n) {
  return { key: `${key}-${from}`, name: from === n ? `${prefix} ${from}` : `${prefix} ${from}–${n}`, channels: range(from, n), rest: true };
}

/** The playback ports of a device of `n` channels, in stereo pairs: `[{key, name, channels}]` ("Out 1–2"; an odd last one "Out 5"; past MAX_PORT_ROWS, one row of the rest). */
export function playbackPorts(n) {
  const ports = [];
  const pairs = Math.ceil(n / 2);
  for (let c = 1; c <= n; c += 2) {
    if (pairs > MAX_PORT_ROWS && ports.length === MAX_PORT_ROWS - 1) {
      ports.push(restPort('Out', 'out', c, n));
      break;
    }
    const channels = c + 1 <= n ? [c, c + 1] : [c];
    ports.push({ key: `out-${c}`, name: channels.length === 2 ? `Out ${c}–${c + 1}` : `Out ${c}`, channels });
  }
  return ports;
}

/** The capture ports of a device of `n` channels, one per channel ("In 1"; past MAX_PORT_ROWS, one row of the rest). */
export function capturePorts(n) {
  if (n <= MAX_PORT_ROWS) return range(1, n).map((c) => ({ key: `in-${c}`, name: `In ${c}`, channels: [c] }));
  return [...range(1, MAX_PORT_ROWS - 1).map((c) => ({ key: `in-${c}`, name: `In ${c}`, channels: [c] })), restPort('In', 'in', MAX_PORT_ROWS, n)];
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
  let main = mainBinding(setup, devices);
  // A device named for `main` (devices.local.toml, or the Setup's [play.main]) is the clock, as the player takes it.
  const mainDevice = setup?.local?.play?.main?.device ?? setup?.setup?.play?.main?.device ?? null;
  if (mainDevice) {
    const box = boxes.find((b) => b.key === (mainDevice.id ? `id:${mainDevice.id}` : `name:${mainDevice.name}`));
    main = box ? { key: box.key, present: true, name: box.name, reason: null } : { key: `absent:${mainDevice.name ?? ''}`, present: false, name: mainDevice.name ?? 'the Setup’s output', reason: 'not present' };
  }
  if (main && !main.present) {
    const width = Math.max(2, ...endpoints.filter((e) => e.role === 'main').map((e) => e.ports.length));
    boxes.push({ key: main.key, name: main.name, present: false, indexes: [], note: `not present${main.reason && main.reason !== 'not present' ? `: ${main.reason}` : ''}`, inputs: playbackPorts(width), outputs: [], side: 'right' });
  }
  const roles = playRoles(setup);
  const channelsOfBox = (box) => (box ? box.inputs.reduce((n, p) => Math.max(n, ...p.channels), 0) : Infinity);
  // ADR 0038 §4: canonical order (role, then the endpoint's address); the later of two roles on one channel is refused there.
  const ordered = [...endpoints].sort((a, b) => (a.role < b.role ? -1 : a.role > b.role ? 1 : 0) || String(a.ports[0] ?? a.name).localeCompare(String(b.ports[0] ?? b.name)));
  const hasMain = ordered.some((e) => e.role === 'main');
  const used = new Map();
  const seen = new Set();
  const bound = new Map();
  ordered.forEach((e, k) => {
    const unheard = (note) => bound.set(e, { endpoint: e.name, role: e.role, layout: e.layout, ports: e.ports, device: null, channels: [], heard: false, note });
    if (seen.has(e.role) || !main) return unheard('not heard on this machine');
    seen.add(e.role);
    // A project without `main` plays its first role as main (unless that role is bound itself).
    const as = e.role === 'main' || (!hasMain && k === 0 && !roles[e.role]) ? 'main' : e.role;
    const entry = roles[as] ?? null;
    if (as !== 'main' && !entry) return unheard('not heard on this machine');
    let key = main.key;
    let heard = main.present;
    let note = main.present ? null : 'not present';
    if (entry?.device && as !== 'main') {
      const box = boxes.find((b) => b.key === (entry.device.id ? `id:${entry.device.id}` : `name:${entry.device.name}`));
      if (!box) return unheard(`${entry.device.name}: not present`);
      if (box.key !== main.key) {
        // ADR 0052: another present device plays through a clock adapter.
        key = box.key;
        heard = box.present !== false;
        note = heard ? 'adapted: through a clock adapter' : 'not present';
      }
    }
    const box = boxes.find((b) => b.key === key);
    const width = e.ports.length;
    const limit = box?.present ? channelsOfBox(box) : Infinity;
    const wanted = as === 'main' ? boundChannels(entry?.channels ?? null, width, limit) : (entry?.channels ?? []).slice(0, width);
    const channels = [];
    for (let i = 0; i < width; i++) {
      const c = wanted[i];
      // Channels are taken per device (ADR 0052): channel 1 of two devices is two channels.
      const refused = !Number.isInteger(c) || c < 1 || c > limit || channels.includes(c) || (heard && used.has(`${key}\u0000${c}`));
      channels.push(refused ? null : c);
      if (!refused && heard) used.set(`${key}\u0000${c}`, e.role);
    }
    bound.set(e, { endpoint: e.name, role: e.role, layout: e.layout, ports: e.ports, device: key, channels, heard, note });
  });
  const bindings = endpoints.map((e) => bound.get(e));
  return { boxes, bindings, main };
}

/**
 * The roles this machine binds (ADR 0038 §4, §6): `setup.get`'s
 * `setup.play` (`main`'s short form `outputChannels` when it has no
 * `[play.main]`), then `devices.local.toml`'s overrides (`local.play`),
 * merged key by key. `{role: {channels, device}}`.
 */
export function playRoles(setup) {
  const roles = {};
  for (const [role, e] of Object.entries(setup?.setup?.play ?? {})) roles[role] = { channels: e?.channels ?? [], device: e?.device ?? null };
  if (!roles.main && Array.isArray(setup?.setup?.outputChannels)) roles.main = { channels: setup.setup.outputChannels, device: null };
  for (const [role, e] of Object.entries(setup?.local?.play ?? {})) {
    const base = roles[role] ?? { channels: [], device: null };
    const channels = Array.isArray(e?.channels) ? e.channels : base.channels;
    if (channels.length) roles[role] = { channels, device: e?.device ?? base.device };
  }
  return roles;
}

/** The device channels a role of `width` channels takes when its cable lands on `port` at channel `channel` (from there on, as many as it is wide). */
export function landingChannels(port, width, channel = port?.channels?.[0]) {
  const first = Number.isInteger(channel) ? channel : (port?.channels?.[0] ?? 1);
  return Array.from({ length: Math.max(1, width) }, (_, i) => first + i);
}

/** A new role's name for a cable landing on free channels (ADR 0038 §1: what it is for; the person renames it in the text): `cue`, then `cue_2`, `cue_3`, … */
export function newRoleName(taken = []) {
  const names = new Set(taken);
  if (!names.has('cue')) return 'cue';
  for (let n = 2; ; n++) if (!names.has(`cue_${n}`)) return `cue_${n}`;
}

/** The inventory's entry of box `key` as the Setup names a device (`{name, id, signature}`), its output (or, with `direction` `input`, its input); null without one. */
export function deviceEntry(devices, key, direction = 'output') {
  const d = (devices?.devices ?? []).find((x) => deviceKey(x) === key && x.direction === direction);
  return d ? { name: d.name, id: d.id ?? undefined, signature: '' } : null;
}

/** A new capture role's name (ADR 0038 §1; the person renames it in the text): `input`, then `input_2`, … */
export function newInputRoleName(taken = []) {
  const names = new Set(taken);
  if (!names.has('input')) return 'input';
  for (let n = 2; ; n++) if (!names.has(`input_${n}`)) return `input_${n}`;
}

/**
 * What a cable dragged from a device's capture port writes (ADR 0038 §7,
 * the owner's "connect the AT2020 to the Mac Studio Speakers", 2026-11-06):
 * the signals meet in explicit device slots, never a drawn device. A new
 * mono capture slot of a new role (`device.add` with `capture`) bound to
 * the port's channel (`device.route` with `side: capture`), wired into the
 * role the drop lands on, or into a new stereo output (`device.add`, its
 * `device.route`) on free channels. `{ops, routes, note}`.
 */
function captureDrop(drag, drop, { bindings, instances, main, devices, scope }) {
  const none = { ops: [], route: null, routes: [], note: null };
  if (!drop) return { ...none, note: 'drop an input on a device’s output to play it there' };
  const taken = [...bindings.map((b) => b.role), ...instances];
  const role = newInputRoleName(taken);
  const ops = [{ op: 'device.add', name: role, capture: 'mono', role }];
  // The Setup's [capture.<role>], or this project's devices.local.toml (Shift), as the output it plays on.
  const routes = [{ type: 'device.route', side: 'capture', role, channels: drag.port.channels.slice(0, 1), device: deviceEntry(devices, drag.box, 'input'), scope }];
  const occupant = bindings.find((b) => b.device === drop.box && b.channels.includes(drop.channel));
  const into = (ports) => ports.forEach((port) => ops.push({ op: 'connection.add', from: `${role}.m`, to: strip(port) }));
  if (occupant) {
    into(occupant.ports);
    return { ops, route: null, routes, note: `${role} · ${drag.port.name} into ${occupant.role}` };
  }
  const play = newRoleName([...taken, role]);
  ops.push({ op: 'device.add', name: play, play: 'stereo', role: play });
  into([`/${play}.l`, `/${play}.r`]);
  const deviceOf = (box) => (box === main?.key ? null : deviceEntry(devices, box));
  routes.push({ type: 'device.route', role: play, channels: landingChannels(drop.port, 2, drop.port.channels[0]), device: deviceOf(drop.box), scope });
  return { ops, route: null, routes, note: `${role} · ${drag.port.name} → ${play} on ${drop.port.name}: new slots (rename their roles in project.toml)` };
}

/**
 * What cables from a device's input straight to a device's output
 * (`throughCables`') write when lifted (the owner's, 2026-11-07: "I cannot
 * disconnect the AT2020 from the outputs"): `drag.cables`, one by its
 * output end or every cable of an input jack by that jack. Dropped on the
 * empty plane, their connections are removed; on another output, moved
 * there: into the role playing on that channel, or into a new stereo
 * output on free channels (one input into every port of it, inputs in
 * order onto its ports otherwise). The capture slots stay: their input is
 * still read, and recorded. `{ops, routes, note}`.
 */
function throughDrop(drag, drop, { bindings, instances, main, devices, scope }) {
  const cables = drag.cables ?? [];
  const sources = [...new Set(cables.map((c) => strip(c.source)))];
  const removes = cables.map((c) => ({ op: 'connection.remove', from: strip(c.source), to: strip(c.port) }));
  const names = cables.map((c) => `${strip(c.source)} → ${strip(c.port)}`).join(', ');
  const none = { ops: [], route: null, routes: [], note: null };
  if (!cables.length) return none;
  if (!drop) return { ...none, ops: removes, note: `${names} removed` };
  /** Each source into `ports`: one source into all of them, several in order. */
  const into = (ports) => (sources.length === 1 ? ports.map((port) => [sources[0], port]) : sources.map((source, i) => [source, ports[i % ports.length]]));
  const occupant = bindings.find((b) => b.device === drop.box && b.channels.includes(drop.channel));
  if (occupant) {
    const adds = into(occupant.ports.map(strip));
    const kept = new Set(cables.map((c) => `${strip(c.source)} ${strip(c.port)}`));
    if (adds.every(([from, to]) => kept.has(`${from} ${to}`)) && adds.length === cables.length) return none;
    const ops = [...removes, ...adds.map(([from, to]) => ({ op: 'connection.add', from, to }))];
    return { ...none, ops, note: `${sources.join(', ')} into ${occupant.role}` };
  }
  const play = newRoleName([...bindings.map((b) => b.role), ...instances]);
  const ops = [...removes, { op: 'device.add', name: play, play: 'stereo', role: play }, ...into([`${play}.l`, `${play}.r`]).map(([from, to]) => ({ op: 'connection.add', from, to }))];
  const device = drop.box === main?.key ? null : deviceEntry(devices, drop.box);
  const routes = [{ type: 'device.route', role: play, channels: landingChannels(drop.port, 2, drop.port.channels[0]), device, scope }];
  return { ...none, ops, routes, note: `${sources.join(', ')} → ${play} on ${drop.port.name}: a new output (rename its role in project.toml)` };
}

const strip = (address) => String(address).replace(/^\//, '');
const sideOf = (label) => (/(^|_)(l|left)$/.test(label) ? 'l' : /(^|_)(r|right)$/.test(label) ? 'r' : null);

/**
 * What a Setup-level cable drag writes (ADR 0051 §3, §9: a cable is the
 * project's connection and the machine's binding, drawn as one). `drag`:
 * `{kind: 'create', pin, siblings}` from a Stack's out edge pin (`pin`:
 * `edgePins`' `{address, label, outside}`; `siblings`: the same
 * instrument's other out pins), or `{kind: 'move', cable}` by a cable's
 * device end (`setupCables`'). `drop`: the playback port it lands on,
 * `{box, port, channel}` (`channel` the jack nearest the pointer), or null
 * (the empty plane). Returns `{ops, route, note}`:
 * - a cable moved to another port: `route`, the `device.route` that binds
 *   its role there (`device` null on the clock device); no project text;
 * - a cable dropped on the empty plane: `ops`, its connection removed;
 * - a pin dropped on a port a role plays on: `ops`, the connections into
 *   that role's ports (paired by name, `landingPorts`);
 * - a pin dropped on free channels: a new device slot of a new role
 *   (`device.add`, stereo), the connections into it, and its `route`.
 * `scope`: `machine` (the Setup) or `project` (`devices.local.toml`).
 */
export function cableDrop(drag, drop, { bindings = [], instances = [], main = null, devices = null, scope = 'machine' } = {}) {
  const none = { ops: [], route: null, note: null };
  if (drag?.kind === 'capture') return captureDrop(drag, drop, { bindings, instances, main, devices, scope });
  const deviceOf = (box) => (box === main?.key ? null : deviceEntry(devices, box));
  if (drag?.kind === 'through') return throughDrop(drag, drop, { bindings, instances, main, devices, scope });
  if (drag?.kind === 'move') {
    const cable = drag.cable;
    if (!drop) return { ...none, ops: [{ op: 'connection.remove', from: strip(cable.pin), to: strip(cable.port) }], note: `${strip(cable.pin)} → ${strip(cable.port)} removed` };
    const binding = bindings.find((b) => b.endpoint === cable.endpoint);
    if (!binding) return none;
    const index = Math.max(0, binding.ports.indexOf(cable.port));
    const first = Math.max(1, drop.channel - index);
    const channels = Array.from({ length: binding.ports.length }, (_, i) => first + i);
    const same = drop.box === binding.device && channels.every((c, i) => c === binding.channels[i]);
    if (same) return none;
    return { ...none, route: { type: 'device.route', role: binding.role, channels, device: deviceOf(drop.box), scope }, note: `${binding.role} on ${drop.port.name}` };
  }
  if (drag?.kind !== 'create' || !drop) return none;
  const pin = drag.pin;
  const candidates = [pin, ...(drag.siblings ?? [])];
  /** The pins feeding role ports `ports`: a paired pin (l, r) its side's port and its sibling the other; an unpaired one every port. */
  const feeds = (ports) => {
    const side = sideOf(pin.label);
    if (!side) return ports.map((port) => [pin, port]);
    return ports
      .map((port) => [candidates.find((c) => sideOf(c.label) === String(port).slice(String(port).lastIndexOf('.') + 1)), port])
      .filter(([c]) => c);
  };
  const occupant = bindings.find((b) => b.device === drop.box && b.channels.includes(drop.channel));
  if (occupant) {
    const index = occupant.channels.indexOf(drop.channel);
    const ports = sideOf(pin.label) ? occupant.ports : landingPorts(pin.label, occupant.ports, index);
    const ops = feeds(ports)
      .filter(([c, port]) => !c.outside?.includes(port))
      .map(([c, port]) => ({ op: 'connection.add', from: strip(c.address), to: strip(port) }));
    return { ...none, ops, note: ops.length ? null : `${strip(pin.address)} already plays ${occupant.role}` };
  }
  const role = newRoleName([...bindings.map((b) => b.role), ...instances]);
  const ports = [`/${role}.l`, `/${role}.r`];
  const ops = [{ op: 'device.add', name: role, play: 'stereo', role }, ...feeds(ports).map(([c, port]) => ({ op: 'connection.add', from: strip(c.address), to: strip(port) }))];
  const channels = landingChannels(drop.port, 2, drop.port.channels[0]);
  return { ops, route: { type: 'device.route', role, channels, device: deviceOf(drop.box), scope }, note: `${role} · stereo on ${drop.port.name}: a new output (rename its role in project.toml)` };
}

/**
 * Which endpoint ports a Stack pin's cable feeds when it lands on a role
 * of ports `ports` at port index `index`: the pin's own name pairs it
 * (`l` → `l`, `r` → `r`, `out_l` → `l`); otherwise the port it landed on.
 */
export function landingPorts(pinLabel, ports, index) {
  const side = /(^|_)(l|left)$/.test(pinLabel) ? 'l' : /(^|_)(r|right)$/.test(pinLabel) ? 'r' : null;
  const named = side ? ports.find((p) => String(p).endsWith(`.${side}`)) : null;
  return [named ?? ports[Math.min(Math.max(index, 0), ports.length - 1)]].filter(Boolean);
}

/**
 * The device boxes drawn whole and the rest (the operator's review of
 * phase 1: a machine lists monitors, loopbacks, aggregates and meeting
 * apps' devices beside the one the project plays on). Whole: a device a
 * role the project uses is bound to, the Setup's output device (`main`:
 * `mainBinding`'s, its key or null), a device a Setup cable reaches, and a
 * bound device that is absent. The rest are `others`, one compact list in
 * inventory order. `{full, others}`, each in `boxes`' order.
 */
export function splitBoxes(boxes, { bindings = [], cables = [], through = [], main = null } = {}) {
  const keys = new Set([...bindings.map((b) => b.device), ...cables.map((c) => c.device), ...through.map((t) => t.from.device), main?.key ?? null].filter(Boolean));
  const full = [];
  const others = [];
  for (const box of boxes) (keys.has(box.key) || !box.present ? full : others).push(box);
  return { full, others };
}

/** A device's channels in each direction, for its line in the list of other devices ("2 out · 2 in"; one direction alone, "2 out"). */
export function channelSummary(box) {
  const count = (ports) => ports.reduce((n, p) => n + p.channels.length, 0);
  const out = count(box.inputs);
  const into = count(box.outputs);
  return [out ? `${out} out` : null, into ? `${into} in` : null].filter(Boolean).join(' · ') || 'no channels';
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
 * Where the Setup (`devices.local.toml`'s `capture` over it) binds
 * capture role `role` (ADR 0038 §4, §6, §7): `{key,
 * channels}`, the box of its device (the Setup's input, else the system's
 * default input, when the binding names none) and its device channels, as
 * the player routes it; `main` without a binding takes the default
 * input's channels from 1. A named device not listed (unplugged) is
 * `present: false` with a key of its own. Null when the role is not
 * captured here.
 */
export function captureBinding(setup, devices, role, width) {
  // devices.local.toml over the Setup, key by key (ADR 0038 §6).
  const machine = setup?.setup?.capture?.[role] ?? null;
  const local = setup?.local?.capture?.[role] ?? null;
  const entry = machine || local ? { channels: local?.channels ?? machine?.channels ?? [], device: local?.device ?? machine?.device ?? null } : null;
  if (!entry && role !== 'main') return null;
  const list = devices?.devices ?? [];
  const named = entry?.device ?? setup?.setup?.input ?? null;
  const device = named
    ? list.find((d) => d.direction === 'input' && (named.id ? d.id === named.id : d.name === named.name))
    : list.find((d) => d.direction === 'input' && d.default);
  const channels = entry ? (entry.channels ?? []).slice(0, width) : Array.from({ length: width }, (_, i) => i + 1);
  // A named device not listed now (unplugged): bound but absent, drawn hollow with its cables dashed.
  // Its key is the one it had while listed, so its box keeps its place (`keepOrder`).
  if (!device) return named ? { key: deviceKey(named), channels, present: false, name: named.name ?? 'an input' } : null;
  return { key: deviceKey(device), channels, present: true, name: device.name };
}

/**
 * The cables straight from a device's input to a device's output (ADR
 * 0038 §7): a capture slot's output wired to a play side's input, with no
 * instrument between them, so no Stack pin to draw from. One cable `{from:
 * {device, channel}, to: {device, channel}, label}` per such connection
 * whose capture channel the Setup binds and whose play channel `bindings`
 * gives; `source` and `port` are the connection's two ends, `endpoint`
 * the play role's.
 */
export function throughCables({ instances = [], connections = [], setup = null, devices = null, bindings = [] } = {}) {
  const captures = new Map();
  for (const i of instances) {
    if (!i.device?.capture) continue;
    const ports = (i.ports ?? []).filter((p) => p.direction === 'out' && (p.shape ?? 'signal') === 'signal').map((p) => p.address);
    const bound = captureBinding(setup, devices, i.device.role, ports.length);
    if (bound) ports.forEach((port, n) => captures.set(port, { device: bound.key, channel: bound.channels[n], present: bound.present, name: bound.name }));
  }
  const cables = [];
  for (const c of connections) {
    const from = captures.get(c.from);
    if (!from || !Number.isInteger(from.channel)) continue;
    for (const b of bindings) {
      const n = b.device ? b.ports.indexOf(c.to) : -1;
      if (n < 0 || !Number.isInteger(b.channels[n])) continue;
      cables.push({ from, to: { device: b.device, channel: b.channels[n] }, source: c.from, port: c.to, endpoint: b.endpoint, label: `${c.from} → ${c.to} (${b.endpoint})`, dashed: !b.heard || !from.present });
    }
  }
  return cables;
}

/**
 * `boxes` in the order they were drawn before (`previous`, keys), so a
 * device unplugged keeps its place as a hollow box and comes back to it
 * (the owner's, 2026-11-07: "the not-present box is somewhere else
 * entirely"). A box not drawn before goes after the one before it here.
 * `{boxes, order}`: `order` keeps the keys of boxes gone, for their
 * return.
 */
export function keepOrder(boxes, previous = []) {
  const rank = new Map(previous.map((key, i) => [key, i]));
  let last = -1;
  const ranked = boxes.map((box, i) => {
    const known = rank.get(box.key);
    last = known ?? last + 1e-3;
    return { box, rank: last, i };
  });
  ranked.sort((a, b) => a.rank - b.rank || a.i - b.i);
  const here = new Set(boxes.map((b) => b.key));
  const order = [...ranked.map((r) => ({ key: r.box.key, rank: r.rank })), ...previous.filter((key) => !here.has(key)).map((key) => ({ key, rank: rank.get(key) }))]
    .sort((a, b) => a.rank - b.rank)
    .map((r) => r.key);
  return { boxes: ranked.map((r) => r.box), order };
}

/**
 * The boxes of input devices `through` cables start from that the
 * inventory lacks (unplugged): hollow, "not present", their capture
 * jacks up to the highest channel a cable uses (the owner's, 2026-11-07:
 * an unplugged AT2020 showed nothing).
 */
export function absentInputBoxes(through, boxes) {
  const have = new Set(boxes.map((b) => b.key));
  const missing = new Map();
  for (const t of through) {
    if (t.from.present !== false || have.has(t.from.device)) continue;
    const box = missing.get(t.from.device) ?? { name: t.from.name, channels: 1 };
    box.channels = Math.max(box.channels, t.from.channel);
    missing.set(t.from.device, box);
  }
  return [...missing].map(([key, b]) => ({ key, name: b.name, present: false, indexes: [], note: 'not present', inputs: [], outputs: capturePorts(b.channels), side: 'left' }));
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
      // A channel refused (two roles on it, or none the device has) carries no cable.
      if (!Number.isInteger(b.channels[i])) return;
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

/** `x` Hz as kHz, as a person reads it ("44.1", "48"). */
const khz = (hz) => `${Math.round(hz / 100) / 10}`;

/**
 * A device box's clock badge at the Setup level (ADR 0052 phase 4):
 * `{text, state, title}` or null. The clock device's box reads `clock`; a
 * secondary output (`devices.secondaries`, by name) "adapted · +37 ppm ·
 * 7.3 ms", and "44.1 → 48 kHz" when the rates differ, its `state`
 * (`starting`, `locking`, `locked`, `suspect`, `absent`) for the drawing,
 * its reason in the title; an adapted input (`devices.captures`) "adapted
 * in · −74 ppm". `main`: `peripherals`' clock binding.
 */
export function clockBadge(box, { main = null, devices = null } = {}) {
  if (!box) return null;
  if (main && box.key === main.key && box.present) return { text: 'clock', state: 'clock', title: 'The clock device: the engine runs on its callbacks' };
  const ppm = (x) => `${x >= 0 ? '+' : '−'}${Math.abs(x).toFixed(1)} ppm`;
  const of = (status, inward) => {
    const state = status.state ?? 'starting';
    const parts = [inward ? 'adapted in' : 'adapted'];
    if (state === 'suspect') parts.push('suspect');
    else if (state === 'absent') parts.push('absent');
    else if (state === 'locking' || state === 'starting') parts.push(state === 'locking' ? `locking ${ppm(status.drift_ppm ?? 0)}` : 'starting');
    else parts.push(ppm(status.drift_ppm ?? 0));
    if (!inward && Number.isFinite(status.latency_ms) && (state === 'locked' || state === 'locking')) parts.push(`${status.latency_ms.toFixed(1)} ms`);
    if (!inward && status.rate && status.clock_rate && status.rate !== status.clock_rate) parts.push(`${khz(status.clock_rate)} → ${khz(status.rate)} kHz`);
    const roles = (status.roles ?? []).join(', ');
    const title = [`${inward ? 'Captures' : 'Plays'} ${roles || 'nothing'} through a clock adapter`, status.reason, Number.isFinite(status.slips) ? `${status.slips} slips` : null].filter(Boolean).join(' · ');
    return { text: parts.join(' · '), state, title };
  };
  // An output's status on a box that plays (its playback ports), an input's on one that captures: a device
  // listed in both directions under one name (the AT2020) is two boxes.
  const secondary = (box.inputs ?? []).length ? (devices?.secondaries ?? []).find((s) => s.name === box.name) : null;
  if (secondary) return of(secondary, false);
  const capture = (box.outputs ?? []).length ? (devices?.captures ?? []).find((c) => c.name === box.name && c.clock === 'adapted') : null;
  if (capture) return of(capture, true);
  return null;
}
