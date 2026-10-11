import { describe, expect, it } from "vitest";
import { makeChannel, type Listen } from "./channel";

/** A backend that only starts delivering once the test lets it. */
function backend<T>() {
  let deliver: ((value: T) => void) | null = null;
  let start: (() => void) | null = null;
  const registered = new Promise<void>(resolve => { start = resolve; });
  const listen: Listen<T> = (_name, handler) => {
    deliver = value => handler({ payload: value });
    return registered.then(() => () => { deliver = null; });
  };
  return {
    listen,
    /** Finish the registration round trip. */
    connect: () => { start?.(); },
    emit: (value: T) => deliver?.(value)
  };
}

describe("a backend event channel", () => {
  /// The bug: `listen` is an IPC round trip, and the session was started before
  /// it finished. Waiting on `ready` is what a caller does so that nothing it
  /// starts can emit into a void.
  it("is not ready until the backend has registered the handler", async () => {
    const host = backend<string>();
    const channel = makeChannel("terminal-output", host.listen);
    let ready = false;
    void channel.ready.then(() => { ready = true; });

    await Promise.resolve();
    expect(ready).toBe(false);

    host.connect();
    await channel.ready;
    expect(ready).toBe(true);
  });

  /// And for a caller that did not wait — the very first pane, whose events can
  /// arrive between registration and its own subscription — nothing is lost.
  it("holds what arrives before the first subscriber and hands it over", async () => {
    const host = backend<string>();
    const channel = makeChannel("terminal-output", host.listen);
    host.connect();
    await channel.ready;

    host.emit("banner");
    host.emit("prompt");

    const seen: string[] = [];
    channel.subscribe(value => seen.push(value));
    expect(seen).toEqual(["banner", "prompt"]);

    host.emit("later");
    expect(seen).toEqual(["banner", "prompt", "later"]);
  });

  it("delivers to every subscriber, and stops when one unsubscribes", async () => {
    const host = backend<string>();
    const channel = makeChannel("terminal-output", host.listen);
    host.connect();
    await channel.ready;

    const one: string[] = [];
    const two: string[] = [];
    const stop = channel.subscribe(value => one.push(value));
    channel.subscribe(value => two.push(value));

    host.emit("a");
    stop();
    host.emit("b");

    expect(one).toEqual(["a"]);
    expect(two).toEqual(["a", "b"]);
  });

  /// A pane that closes while its subscription is still being set up must not
  /// leave one behind, and the queue must not grow without end while nobody is
  /// listening at all.
  it("does not grow without end while nothing is listening", async () => {
    const host = backend<number>();
    const channel = makeChannel("terminal-output", host.listen);
    host.connect();
    await channel.ready;

    for (let index = 0; index < 2000; index += 1) host.emit(index);

    const seen: number[] = [];
    channel.subscribe(value => seen.push(value));
    expect(seen.length).toBeLessThanOrEqual(512);
    // What is kept is the most recent, not the oldest.
    expect(seen.at(-1)).toBe(1999);
  });

  it("is inert with no backend, which is the browser preview", async () => {
    const channel = makeChannel<string>("terminal-output", null);
    await channel.ready;
    const seen: string[] = [];
    const stop = channel.subscribe(value => seen.push(value));
    stop();
    expect(seen).toEqual([]);
  });
});
