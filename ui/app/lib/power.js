// The power switch in Signal.app's toolbar (M2.8-11): the state the page
// shows, from the server's `app` and `power` messages and the switch's
// clicks. Plain ES module, no DOM. Off, the server runs nothing and the
// connection streams nothing (signal-ui.js); this only drives the toolbar.
// On, the open project plays (ADR 0033): the toolbar has no Play or Stop,
// a project starts and stops from its own transport, and Project ▸ Restart
// from Files plays it again from frame 0.

/** Before the server said: on (Signal.app starts switched on), nothing pending. */
export const INITIAL = Object.freeze({ power: 'on', pending: false });

/**
 * The next state after `event`: `{type: 'app', app}` (the server's app
 * state), `{type: 'toggle'}` (a click, sent), `{type: 'reply', reply}` (the
 * `power` reply), `{type: 'error'}` (refused or not sent).
 */
export function reduce(state, event) {
  switch (event?.type) {
    case 'app': {
      const power = event.app?.power;
      return power === 'on' || power === 'off' ? { ...state, power } : state;
    }
    case 'toggle':
      return state.pending ? state : { ...state, pending: true };
    case 'reply':
      return { power: event.reply?.on ? 'on' : 'off', pending: false };
    case 'error':
      return { ...state, pending: false };
    default:
      return state;
  }
}

/** What a click on the switch sends. */
export function powerRequest(state) {
  return { type: 'app.power', on: state.power !== 'on' };
}

/** The toolbar's switch for `state`. */
export function controls(state) {
  const on = state.power === 'on';
  return {
    checked: on,
    label: on ? 'on' : 'off',
    switchDisabled: state.pending,
  };
}

/**
 * What Project ▸ Restart from Files sends: `transport.play`, which plays the
 * open project again from frame 0 from its files (playing or not).
 */
export function restartRequest() {
  return { type: 'transport.play' };
}
