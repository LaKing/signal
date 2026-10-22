import test from 'node:test';
import assert from 'node:assert/strict';
import { COMMIT_DELAY_MS } from './binding.js';
import { ClickPair, DOUBLE_CLICK_MS, DOUBLE_CLICK_PX, EntryField, NOTE_MS, clampEntry, entryText, parseNumber, parseValue } from './entry.js';
import { keyAction } from './gesture.js';
import { toPosition, toValue } from './taper.js';

const HZ = { unit: 'Hz', range: { min: 20, max: 20000 }, taper: 'log' };
const MS = { unit: 'ms', range: { min: 0.1, max: 60000 }, taper: 'log' };
const S = { unit: 's', range: { min: 0, max: 10 } };
const PLAIN = { unit: null, range: { min: -10, max: 1e9 } };
const GAIN_FADER = { unit: null, range: { min: 0, max: 4 }, taper: 'db' };
const MIX = { unit: null, range: { min: 0, max: 1 } };
const DB = { unit: 'dB', range: { min: -60, max: 12 } };
const ST = { unit: 'st', range: { min: -24, max: 24 } };
const CT = { unit: 'ct', range: { min: -100, max: 100 } };

const close = (actual, expected, message) => assert.ok(Math.abs(actual - expected) <= 1e-12 * Math.max(1, Math.abs(expected)), `${message}: ${actual} != ${expected}`);
const value = (text, spec) => {
  const parsed = parseValue(text, spec);
  assert.ok('value' in parsed, `${text}: ${parsed.error}`);
  return parsed.value;
};
const reason = (text, spec) => {
  const parsed = parseValue(text, spec);
  assert.ok('error' in parsed && typeof parsed.error === 'string' && parsed.error.length > 0, `${text} is refused with a message`);
  return parsed.reason;
};

test('numbers: signs, a decimal comma or dot, exponents', () => {
  assert.equal(value('440', PLAIN), 440);
  assert.equal(value('  +2.5 ', PLAIN), 2.5);
  assert.equal(value('-0,25', PLAIN), -0.25);
  assert.equal(value('.5', PLAIN), 0.5);
  assert.equal(value(',5', PLAIN), 0.5);
  assert.equal(value('3.', PLAIN), 3);
  assert.equal(value('1e3', PLAIN), 1000);
  assert.equal(value('1,5E-2', PLAIN), 0.015);
  assert.equal(value('-2e+1', PLAIN), -20);
});

test('SI prefixes k, m and M, case-sensitive', () => {
  assert.equal(value('1.2k', PLAIN), 1200);
  assert.equal(value('500m', PLAIN), 0.5);
  assert.equal(value('2M', PLAIN), 2e6);
  assert.equal(value('2 M', PLAIN), 2e6);
  assert.equal(reason('2K', PLAIN), 'unit', 'K is not a prefix');
  assert.equal(value('1.5kHz', HZ), 1500, 'a prefix before the unit');
  assert.equal(value('1 MHz', HZ), 1e6, 'M before Hz is mega');
  assert.equal(value('1 mHz', HZ), 0.001, 'm before Hz is milli');
  assert.equal(value('2k', HZ), 2000, 'a prefix alone is in the parameter unit');
});

test('units and their forms, case-insensitive', () => {
  assert.equal(value('440 Hz', HZ), 440);
  assert.equal(value('440hz', HZ), 440);
  assert.equal(value('1.5 kHz', HZ), 1500);
  assert.equal(value('1.5KHZ', HZ), 1500);
  assert.equal(value('250 ms', MS), 250);
  assert.equal(value('0.25 s', MS), 250, 'seconds to ms');
  assert.equal(value('2S', MS), 2000);
  assert.equal(value('1MS', MS), 1, 'a unit form is read before a prefix');
  assert.equal(value('250 ms', S), 0.25, 'ms to seconds');
  assert.equal(value('-6 dB', DB), -6);
  assert.equal(value('3 st', ST), 3);
  assert.equal(value('-50 CT', CT), -50);
  assert.equal(value('120 BPM', { unit: 'bpm', range: { min: 20, max: 300 } }), 120);
});

test('dB on a dB-taper fader and on a linear gain', () => {
  close(value('-6 dB', GAIN_FADER), 10 ** (-6 / 20), 'dB taper');
  close(value('0db', GAIN_FADER), 1, '0 dB is unity');
  // auditor-1-005: a gain by its plan kind, whatever the display taper.
  close(value('-6 dB', { unit: null, range: { min: 0, max: 4 }, taper: 'linear', kind: 'gain' }), 10 ** (-6 / 20), 'a linear-gain parameter on a linear taper');
  close(value('-6 dB', { unit: null, range: { min: 0, max: 4 }, kind: 'gain' }), 10 ** (-6 / 20), 'without a taper');
  assert.equal(reason('-6 dB', { unit: null, range: { min: 0, max: 4 }, kind: 'control' }), 'unit', 'another kind takes no dB');
  assert.equal(value('0.5', GAIN_FADER), 0.5, 'a plain number is the linear value');
  assert.equal(value('-inf', GAIN_FADER), 0, '-inf is the minimum of a dB taper');
  assert.equal(value('-∞ dB', GAIN_FADER), 0);
  assert.equal(value('-INF dB', { ...GAIN_FADER, range: { min: 0.001, max: 4 } }), 0.001, 'the minimum, whatever it is');
  assert.equal(reason('-6 dB', MIX), 'unit', 'dB does not fit a plain 0..1 parameter');
  assert.equal(value('-inf', DB), -60, 'a dB parameter: −∞ is its minimum');
});

test('% for a 0..1 range only', () => {
  assert.equal(value('50%', MIX), 0.5);
  assert.equal(value('12.5 %', MIX), 0.125);
  assert.equal(reason('50%', PLAIN), 'unit');
  assert.equal(reason('50%', HZ), 'unit');
});

test('refusals, each with a named reason', () => {
  assert.equal(reason('', PLAIN), 'empty');
  assert.equal(reason('   ', PLAIN), 'empty');
  assert.equal(reason('abc', PLAIN), 'not-a-number');
  assert.equal(reason('1.2.3', PLAIN), 'unit');
  assert.equal(reason('NaN', PLAIN), 'not-a-number');
  assert.equal(reason('e5', PLAIN), 'not-a-number');
  assert.equal(reason('440 ms', HZ), 'unit', 'ms does not fit Hz');
  assert.equal(reason('3 st', CT), 'unit', 'st only where the unit is st');
  assert.equal(reason('3 ct', ST), 'unit');
  assert.equal(reason('3 st', PLAIN), 'unit');
  assert.equal(reason('1 kHz', MS), 'unit');
  assert.equal(reason('1 kk', PLAIN), 'unit', 'one prefix at most');
  assert.equal(reason('1 k%', MIX), 'unit', 'no prefix before %');
  assert.equal(reason('1e400', PLAIN), 'not-finite');
  assert.equal(reason('inf', PLAIN), 'not-finite');
  assert.equal(reason('-∞', PLAIN), 'not-finite');
  assert.equal(reason('+inf dB', GAIN_FADER), 'not-finite', 'only −∞ is the minimum');
  assert.match(parseValue('440 ms', HZ).error, /"ms" does not fit a parameter in Hz/);
});

test('the starting text reads back to the same value, for every unit', () => {
  const values = [0, 1, -1, 0.1, 1 / 3, -2 / 7, 440, 12345.678901234567, 1e-7, 6.02e23, 1e21, Number.MIN_VALUE, Number.MAX_VALUE];
  for (const unit of [null, 'Hz', 'ms', 's', 'dB', '%', 'st', 'ct', 'bpm', 'cycles']) {
    for (const taper of ['linear', 'log', 'db']) {
      const spec = { unit, range: { min: 0, max: 1 }, taper };
      for (const v of values) {
        const text = entryText(v, unit);
        assert.equal(value(text, spec), v, `${unit} ${taper}: ${text}`);
      }
    }
  }
  assert.equal(entryText(440, 'Hz'), '440 Hz', 'full precision, in the parameter unit');
  assert.equal(entryText(1 / 3, null), '0.3333333333333333');
  assert.equal(entryText(NaN, 'Hz'), '');
});

test('clamping names the end it went to', () => {
  assert.deepEqual(clampEntry(5, { min: 0, max: 1 }), { value: 1, clamped: 'Clamped to the maximum, 1.000' });
  assert.deepEqual(clampEntry(5, { min: 20, max: 20000, unit: 'Hz' }), { value: 20, clamped: 'Clamped to the minimum, 20.0 Hz' });
  assert.deepEqual(clampEntry(0.5, { min: 0, max: 1 }), { value: 0.5, clamped: null });
});

// The keys move the position (taper.js) by a fixed share of the travel, so
// the value step follows the control's taper.
const step = (key, v, range, fine = false) => toValue(keyAction(key, toPosition(v, range), { fine }), range);

test('keyboard steps per taper: 1/100, 1/1000 with Shift, 1/10 with Page, Home and End', () => {
  const linear = { min: 0, max: 200, taper: 'linear' };
  close(step('ArrowUp', 100, linear), 102, 'linear arrow');
  close(step('ArrowLeft', 100, linear), 98, 'linear arrow down');
  close(step('ArrowUp', 100, linear, true), 100.2, 'linear fine');
  close(step('PageDown', 100, linear), 80, 'linear page');
  assert.equal(step('Home', 100, linear), 0);
  assert.equal(step('End', 100, linear), 200);

  const log = { min: 20, max: 20000, taper: 'log' };
  close(step('ArrowUp', 200, log), 200 * 1000 ** 0.01, 'log arrow: a constant ratio');
  close(step('ArrowRight', 2000, log, true), 2000 * 1000 ** 0.001, 'log fine');
  close(step('PageUp', 200, log), 200 * 1000 ** 0.1, 'log page');
  assert.equal(step('Home', 200, log), 20);
  assert.equal(step('End', 200, log), 20000);

  const db = { min: 0, max: 1, taper: 'db' };
  close(20 * Math.log10(step('ArrowDown', 10 ** (-30 / 20), db)), -30.6, 'dB arrow: 1/100 of the −60..0 dB span');
  close(20 * Math.log10(step('ArrowUp', 10 ** (-30 / 20), db, true)), -29.94, 'dB fine');
  close(20 * Math.log10(step('PageUp', 10 ** (-30 / 20), db)), -24, 'dB page');
  assert.equal(step('Home', 0.5, db), 0);
  assert.equal(step('End', 0.5, db), 1);
  assert.equal(step('ArrowUp', 1, db), 1, 'clamped at the end');
});

test('the keys commit once, about 500 ms after the last', () => {
  assert.equal(COMMIT_DELAY_MS, 500);
});

test('the field: open, edit, commit', () => {
  const field = new EntryField();
  assert.equal(field.state, 'closed');
  assert.equal(field.open(440, HZ), '440 Hz', 'opens with the base in full precision');
  assert.equal(field.state, 'open');
  field.edit('1.5 kHz');
  assert.deepEqual(field.commit(), { action: 'commit', value: 1500, clamped: null });
  assert.equal(field.state, 'closed');
  assert.deepEqual(field.commit(), { action: 'none' }, 'nothing once closed');
});

test('the field: Escape cancels, a click elsewhere cancels unchanged and commits changed', () => {
  const field = new EntryField();
  field.open(440, HZ);
  field.edit('880');
  assert.deepEqual(field.cancel(), { action: 'cancel' }, 'Escape sends nothing');
  assert.equal(field.state, 'closed');
  field.open(440, HZ);
  assert.deepEqual(field.blur(), { action: 'cancel' }, 'unchanged');
  field.open(440, HZ);
  field.edit('440 Hz');
  assert.deepEqual(field.blur(), { action: 'cancel' }, 'changed back to the same text');
  field.open(440, HZ);
  field.edit('880');
  assert.deepEqual(field.blur(), { action: 'commit', value: 880, clamped: null }, 'changed');
});

test('the field: invalid text stays open, marked, and sends nothing', () => {
  const field = new EntryField();
  field.open(440, HZ);
  field.edit('440 ms');
  const result = field.commit();
  assert.equal(result.action, 'invalid');
  assert.equal(result.reason, 'unit');
  assert.equal(field.state, 'invalid');
  assert.equal(field.error, result.error, 'the tooltip says why');
  field.edit('450');
  assert.equal(field.state, 'open', 'editing clears the mark');
  assert.equal(field.error, null);
  assert.deepEqual(field.commit(), { action: 'commit', value: 450, clamped: null });
});

test('the field: an out-of-range value is clamped, shown, then closes', () => {
  const field = new EntryField();
  field.open(440, HZ);
  field.edit('50k');
  assert.deepEqual(field.commit(), { action: 'commit', value: 20000, clamped: 'Clamped to the maximum, 20.00 kHz' });
  assert.equal(field.state, 'clamped');
  assert.equal(field.text, '20000 Hz', 'the field shows the clamped value');
  assert.ok(NOTE_MS >= 1500 && NOTE_MS <= 2500, 'for about 2 s');
  assert.deepEqual(field.blur(), { action: 'none' }, 'a click elsewhere keeps the clamp shown its time, sending nothing');
  assert.equal(field.state, 'clamped');
  assert.deepEqual(field.cancel(), { action: 'cancel' }, 'its time is up: closed, nothing sent');
  field.open(440, HZ);
  field.edit('-5');
  assert.equal(field.commit().value, 20);
  assert.deepEqual(field.commit(), { action: 'cancel' }, 'Enter on the shown clamp closes it');
  field.open(1, GAIN_FADER);
  field.edit('-inf');
  assert.deepEqual(field.commit(), { action: 'commit', value: 0, clamped: null }, '-inf is the minimum, not a clamp');
});

test('the field: Enter on an unchanged text sends nothing; invalid text is let go on blur', () => {
  const field = new EntryField();
  field.open(440, HZ);
  assert.deepEqual(field.commit(), { action: 'cancel' }, 'Enter only to look: no write');
  field.open(440, HZ);
  field.edit('440 ms');
  assert.equal(field.commit().action, 'invalid');
  assert.deepEqual(field.blur(), { action: 'cancel' }, 'never open and unfocused');
  assert.equal(field.state, 'closed');
});

test('the minus sign U+2212 and −∞ dB on a dB parameter and a linear gain', () => {
  assert.deepEqual(parseValue('−6', { range: { min: -12, max: 12 } }), { value: -6 });
  assert.deepEqual(parseValue('−∞', { unit: 'dB', range: { min: -60, max: 0 } }), { value: -60 });
  assert.deepEqual(parseValue('-inf dB', { kind: 'gain', range: { min: 0, max: 4 } }), { value: 0 });
  assert.equal(parseValue('-inf', { unit: 'Hz', range: { min: 20, max: 20000 } }).reason, 'not-finite');
});

test('ClickPair: a second press of the same button, near in time and place, is a double-click', () => {
  const press = (timeStamp, clientX = 10, clientY = 10, button = 0) => ({ timeStamp, clientX, clientY, button });
  const pair = new ClickPair();
  assert.equal(pair.down(press(1000)), false);
  assert.equal(pair.down(press(1000 + DOUBLE_CLICK_MS, 10 + DOUBLE_CLICK_PX)), true, 'at the limits');
  assert.equal(pair.down(press(1700)), false, 'a third press starts a new pair');
  assert.equal(pair.down(press(1800)), true);
  assert.equal(pair.down(press(5000)), false);
  assert.equal(pair.down(press(5000 + DOUBLE_CLICK_MS + 1)), false, 'too slow');
  assert.equal(pair.down(press(9000)), false);
  assert.equal(pair.down(press(9100, 10 + DOUBLE_CLICK_PX + 1)), false, 'too far');
  assert.equal(pair.down(press(9150, 10 + DOUBLE_CLICK_PX + 1, 10, 2)), false, 'another button');
  pair.reset();
  assert.equal(pair.down(press(9200)), false, 'reset forgets the first press');
});

test('parseNumber: a decimal comma or dot, nothing else after the number', () => {
  assert.equal(parseNumber('8,5'), 8.5);
  assert.equal(parseNumber(' 8.5 '), 8.5);
  assert.equal(parseNumber('-0,25'), -0.25);
  assert.equal(parseNumber(',5'), 0.5);
  assert.equal(parseNumber('1e3'), 1000);
  assert.equal(parseNumber(''), null);
  assert.equal(parseNumber('abc'), null);
  assert.equal(parseNumber('8 Hz'), null);
  assert.equal(parseNumber('256,0', { integer: true }), 256);
  assert.equal(parseNumber('256,5', { integer: true }), null);
});

test('parseValue: a decimal comma and a dot mean the same, with a unit or a prefix', () => {
  for (const [comma, dot, spec] of [
    ['0,5', '0.5', {}],
    ['130,5 bpm', '130.5 bpm', { unit: 'bpm', range: { min: 20, max: 400 } }],
    ['1,2k', '1.2k', { unit: 'Hz', range: { min: 20, max: 20000 } }],
    ['-6,5 dB', '-6.5 dB', { kind: 'gain', range: { min: 0, max: 4 } }],
    ['2,5E-2', '2.5E-2', {}],
  ]) {
    assert.deepEqual(parseValue(comma, spec), parseValue(dot, spec), comma);
    assert.ok('value' in parseValue(comma, spec), comma);
  }
});
