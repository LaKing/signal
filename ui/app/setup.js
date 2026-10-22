// The interface setup page (the M2.5-1 dialog, now web): output and input
// devices by stable id (same-named ones told apart), their capabilities and
// whether they play the project's rate, the buffer, the output channel map,
// CPU workers, the workers' wake policy, the compute devices (read-only),
// Refresh (not while playing) and Apply. Plain ES module.

import { el } from './app.js';
import { NOT_FOUND, applyMessage, bufferFrames, choicesOf, choose, draftOf, refreshed, withFields } from './lib/draft.js';
import { getDefaultConnection, onDefaultConnection } from '../signal-ui.js';

const REFRESH_WHILE_PLAYING = 'switch the power off to refresh: listing devices pauses metering and status';

export class SignalAppSetup extends HTMLElement {
  connectedCallback() {
    if (this.firstChild) return;
    this.path = el('p', { className: 'setup-path' });
    this.problems = el('ul', { className: 'messages' });
    this.output = el('select', { name: 'output' });
    this.outputWhy = el('div', { className: 'warn' });
    this.capabilities = el('ul', { className: 'capabilities' });
    this.check = el('div', { className: 'check' });
    this.input = el('select', { name: 'input' });
    this.inputWhy = el('div', { className: 'warn' });
    // Text, not `type="number"` (WebKit's drops a decimal comma): read by `bufferFrames`.
    this.buffer = el('input', { name: 'buffer', type: 'text', inputMode: 'numeric', size: 6, spellcheck: false, autocomplete: 'off' });
    this.workers = el('input', { name: 'cpuWorkers', type: 'text', size: 6, placeholder: 'auto' });
    this.channels = el('input', { name: 'channels', type: 'text', size: 12, placeholder: '1, 2' });
    this.policy = el('select', { name: 'rtPolicy' });
    this.compute = el('ul', { className: 'compute' });
    this.refresh = el('button', { type: 'button', textContent: 'Refresh' });
    this.apply = el('button', { type: 'button', textContent: 'Apply' });
    this.result = el('ul', { className: 'messages' });
    const field = (label, ...control) => el('div', { className: 'field' }, el('label', { textContent: label }), ...control);
    this.append(
      this.path,
      this.problems,
      field('Output', this.output, this.outputWhy, this.capabilities, this.check),
      field('Input (metered)', this.input, this.inputWhy),
      field('Buffer (frames)', this.buffer),
      field('Output channels', this.channels, el('span', { className: 'muted', textContent: ' device channel of each sink channel; empty: 1, 2, …' })),
      field('CPU workers', this.workers, el('span', { className: 'muted', textContent: ' auto, or 1 to 64 threads per block' })),
      field('Wake policy', this.policy, el('span', { className: 'muted', textContent: ' how idle workers wait (never changes the output)' })),
      el('details', {}, el('summary', { textContent: 'Compute devices (read-only)' }), this.compute),
      el('div', { className: 'actions' }, this.refresh, this.apply),
      this.result,
    );
    this.output.addEventListener('change', () => this.chose('output', this.output.value));
    this.input.addEventListener('change', () => this.chose('input', this.input.value));
    this.buffer.addEventListener('change', () => this.checkRate());
    this.refresh.addEventListener('click', () => this.list(true));
    this.apply.addEventListener('click', () => this.applied());
    this.stops = [];
    this.stops.push(onDefaultConnection((connection) => this.attach(connection)));
  }

  disconnectedCallback() {
    for (const stop of this.stops ?? []) stop();
  }

  attach(connection) {
    this.connection = connection;
    this.stops.push(connection.on('status', (s) => s === 'open' && this.load()));
    this.stops.push(connection.on('app', (app) => this.playing(app.playing)));
    if (connection.status === 'open') this.load();
  }

  request(message) {
    const connection = this.connection ?? getDefaultConnection();
    return connection ? connection.request(message) : Promise.reject(new Error('not connected'));
  }

  async load() {
    try {
      this.setup = await this.request({ type: 'setup.get' });
      const app = await this.request({ type: 'app.get' }).catch(() => null);
      this.rate = app?.project?.sampleRate ?? null;
      this.playing(this.setup.playing);
      this.path.textContent = `Setup file: ${this.setup.path}`;
      this.problems.replaceChildren(...(this.setup.problems ?? []).map((p) => el('li', { className: 'warn', textContent: p })));
      this.policy.replaceChildren(...(this.setup.rtPolicies ?? []).map((name) => el('option', { value: name, textContent: name })));
      await this.list(false, true);
    } catch (error) {
      this.say(error.message, 'error');
    }
  }

  playing(playing) {
    this.isPlaying = Boolean(playing);
    this.refresh.disabled = this.isPlaying;
    this.refresh.title = this.isPlaying ? REFRESH_WHILE_PLAYING : '';
  }

  /** The device list (again, with `refresh`); a new draft from the Setup on `fresh`. */
  async list(refresh, fresh = false) {
    try {
      const message = { type: 'setup.devices', refresh };
      if (this.draft && !fresh) message.choices = choicesOf(this.draft);
      this.listed = await this.request(message);
    } catch (error) {
      this.say(error.message, 'warn');
      return;
    }
    this.draft = fresh || !this.draft ? draftOf(this.setup, this.listed) : refreshed(withFields(this.draft, this.fields()), this.listed);
    if (this.listed.error) this.say(this.listed.error, 'error');
    this.compute.replaceChildren(...(this.listed.compute ?? []).map((line) => el('li', { textContent: line })));
    this.render();
  }

  render() {
    const devices = this.listed?.devices ?? [];
    const options = (select, direction, none, choice) => {
      const items = [el('option', { value: '', textContent: none })];
      if (choice.kind === 'unresolved') items.push(el('option', { value: 'keep', textContent: NOT_FOUND }));
      for (const d of devices.filter((d) => d.direction === direction)) {
        items.push(el('option', { value: String(d.index), textContent: d.label ?? d.name }));
      }
      select.replaceChildren(...items);
      select.value = choice.kind === 'unresolved' ? 'keep' : choice.kind === 'device' ? String(choice.index) : '';
    };
    options(this.output, 'output', 'System default', this.draft.output);
    options(this.input, 'input', 'None', this.draft.input);
    this.outputWhy.textContent = this.draft.output.kind === 'unresolved' ? this.draft.output.reason : '';
    this.inputWhy.textContent = this.draft.input.kind === 'unresolved' ? this.draft.input.reason : '';
    this.buffer.value = String(this.draft.buffer);
    this.workers.value = this.draft.cpuWorkers === 'auto' || this.draft.cpuWorkers === null ? 'auto' : String(this.draft.cpuWorkers);
    this.channels.value = this.draft.channels;
    this.policy.value = this.draft.rtPolicy;
    this.checkRate();
  }

  /** The fields as typed now (kept in the draft before it is drawn again). */
  fields() {
    return {
      buffer: this.buffer.value,
      cpuWorkers: this.workers.value,
      channels: this.channels.value,
      rtPolicy: this.policy.value || undefined,
    };
  }

  chose(key, value) {
    if (value === 'keep') return;
    const devices = this.listed?.devices ?? [];
    this.draft = { ...withFields(this.draft, this.fields()), [key]: choose(value === '' ? null : Number(value), devices) };
    this.render();
  }

  /** The chosen output's capabilities, and whether it plays the project's rate. */
  async checkRate() {
    const devices = this.listed?.devices ?? [];
    const output = this.draft.output;
    const device = output.kind === 'device' ? devices[output.index] : output.kind === 'unset' ? devices.find((d) => d.direction === 'output' && d.default) : null;
    this.capabilities.replaceChildren(...(device?.capabilities ?? []).map((c) => el('li', { textContent: c })));
    this.check.textContent = '';
    if (!device || !this.rate) return;
    try {
      // By identity: the host may have listed the devices again since.
      const output = { name: device.name, id: device.id ?? null, signature: device.signature ?? '' };
      const checked = await this.request({ type: 'setup.check', output, buffer: bufferFrames(this.buffer.value) });
      this.check.textContent = checked.message;
      this.check.className = checked.ok ? 'check' : 'check error';
    } catch (error) {
      this.check.textContent = error.message;
    }
  }

  async applied() {
    this.result.replaceChildren();
    let message;
    try {
      message = applyMessage(withFields(this.draft, this.fields()));
    } catch (error) {
      this.say(error.message, 'error');
      return;
    }
    try {
      const reply = await this.request(message);
      if (reply.error) this.say(reply.error, 'error');
      else this.say(reply.restarted ? 'applied; playing again from frame 0' : 'saved', '');
      for (const note of reply.notes ?? []) this.say(note, '');
      this.setup = await this.request({ type: 'setup.get' });
      await this.list(false, true);
    } catch (error) {
      this.say(error.message, 'error');
    }
  }

  say(text, kind) {
    this.result.append(el('li', { className: kind, textContent: text }));
  }
}

customElements.get('signal-app-setup') ?? customElements.define('signal-app-setup', SignalAppSetup);
