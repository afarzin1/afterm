/**
 * One backend event stream, subscribed to before anything can emit on it.
 *
 * The bug this exists for: a terminal was started and *then* its output
 * listener was registered. `listen` is asynchronous — it is an IPC round trip
 * to register the handler — so everything the far end wrote in between went
 * nowhere. On a fast local shell that is the whole login banner and the first
 * prompt; on a shell with integration marks in its prompt it is the `A` and `B`
 * that tell afterm where the prompt ends, so the first command is mis-banded
 * for the life of the tab. Exit and credential events had the same shape:
 * `AuthDialog` renders after `WorkspaceGrid`, and React runs a child's effects
 * before its parent's, so a session could ask for a password before anything
 * was listening for the question.
 *
 * Two things close it. Registration starts when this module is imported, which
 * is before any component has rendered, and `ready` is a promise a caller can
 * wait on before doing anything that could produce an event. And anything that
 * still arrives before the first subscriber is held rather than dropped, so
 * even a caller that did not wait sees it.
 *
 * The queue is bounded and only fills while nobody is listening at all, which
 * is a window measured in milliseconds at startup. Past the bound the oldest
 * are dropped: an event too old to have been claimed by anyone is worth less
 * than the ones behind it.
 */

const QUEUED_LIMIT = 512;

export interface Channel<T> {
  /** Resolves once the backend is delivering to this channel. */
  readonly ready: Promise<void>;
  /** Start receiving. The returned function stops. */
  subscribe(handler: (value: T) => void): () => void;
}

/** What `listen` looks like, so a test can stand in for it. */
export type Listen<T> = (
  name: string,
  handler: (event: { payload: T }) => void
) => Promise<() => void>;

export function makeChannel<T>(name: string, listen: Listen<T> | null): Channel<T> {
  const handlers = new Set<(value: T) => void>();
  const waiting: T[] = [];

  const deliver = (value: T) => {
    if (handlers.size === 0) {
      waiting.push(value);
      if (waiting.length > QUEUED_LIMIT) waiting.shift();
      return;
    }
    // A copy, so a handler that unsubscribes itself does not disturb the walk.
    for (const handler of [...handlers]) handler(value);
  };

  const ready = listen
    ? listen(name, event => deliver(event.payload)).then(() => undefined)
    : Promise.resolve();

  return {
    ready,
    subscribe(handler) {
      handlers.add(handler);
      if (waiting.length > 0) {
        const held = waiting.splice(0, waiting.length);
        for (const value of held) handler(value);
      }
      return () => {
        handlers.delete(handler);
      };
    }
  };
}
