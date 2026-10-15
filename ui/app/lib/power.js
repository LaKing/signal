// The power switch in Signal.app's toolbar (M2.8-11): the state the page
// shows, from the server's `app` and `power` messages and the switch's
// clicks. Plain ES module, no DOM. Off, the server runs nothing and the
// connection streams nothing (signal-ui.js); this only drives the toolbar.

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

/** The toolbar's switch, Play and Stop for `state`, playing or not. */
export function controls(state, playing) {
  const on = state.power === 'on';
  return {
    checked: on,
    label: on ? 'on' : 'off',
    switchDisabled: state.pending,
    playDisabled: !on || state.pending || Boolean(playing),
    stopDisabled: !on || state.pending || !playing,
  };
}
