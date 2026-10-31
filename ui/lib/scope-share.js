// One scope capture per source per page (review 2026-10-31): the engine has
// 8 scope slots and every `subscribe` claims one, even for a source already
// captured, so the views of one source (a panel's scope and its plate's in
// the schematic, two faces of one part) share one subscription of their
// connection. Each view gets every frame and its own `onSubscribed` /
// `onError`; the subscription is let go when its last view stops. A refused
// subscription (`onError`) is let go at once: a view that asks again (its
// retry) subscribes anew. No DOM.

const shared = new WeakMap();

/**
 * `connection.subscribe(request, onFrame, {onSubscribed, onError})`, shared
 * with every other view of the same `request` on `connection`; returns the
 * view's stop.
 */
export function shareSubscription(connection, request, onFrame, { onSubscribed, onError } = {}) {
  let entries = shared.get(connection);
  if (!entries) shared.set(connection, (entries = new Map()));
  const key = JSON.stringify(Object.keys(request).sort().map((k) => [k, request[k]]));
  const view = { onFrame, onSubscribed, onError };
  let entry = entries.get(key);
  if (entry) {
    entry.views.add(view);
    if (entry.reply !== undefined) {
      const reply = entry.reply;
      queueMicrotask(() => {
        if (entry.views.has(view)) view.onSubscribed?.(reply);
      });
    }
  } else {
    entry = { views: new Set([view]), reply: undefined, stop: null, dead: false };
    entries.set(key, entry);
    const own = entry;
    const letGo = () => {
      if (entries.get(key) === own) entries.delete(key);
      own.dead = true;
      own.stop?.();
    };
    own.stop = connection.subscribe(
      request,
      (frame) => {
        for (const v of [...own.views]) v.onFrame(frame);
      },
      {
        onSubscribed: (reply) => {
          own.reply = reply;
          for (const v of [...own.views]) v.onSubscribed?.(reply);
        },
        onError: (error) => {
          const views = [...own.views];
          own.views.clear();
          letGo();
          for (const v of views) v.onError?.(error);
        },
      },
    );
    // Refused before `subscribe` returned: its stop is only known now.
    if (own.dead) own.stop?.();
  }
  const mine = entry;
  return () => {
    if (!mine.views.delete(view) || mine.views.size) return;
    if (entries.get(key) === mine) entries.delete(key);
    mine.dead = true;
    mine.stop?.();
  };
}
