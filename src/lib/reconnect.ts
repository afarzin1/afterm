import type { BackendFailure } from "../types";

/**
 * Reconnecting a dropped session on its own.
 *
 * A connection that goes is usually a network that went — a VPN renegotiating,
 * a laptop changing Wi-Fi, a host rebooting — and the fix is to try again once
 * it is back. So a lost session is retried every five seconds for five
 * minutes, counted from when it went, and the popup says when the next attempt
 * is and why the last one failed.
 *
 * The next attempt is scheduled when the previous one has *finished*, never on
 * a clock of its own. An attempt against a host that is not answering can take
 * longer than five seconds to time out, and a native one can be sitting on a
 * password dialog; a fixed interval would stack a second attempt on top of the
 * first, and the second would cancel the first.
 *
 * Only an unreachable host is retried. A refused credential or a changed host
 * key is not something waiting fixes, and trying either again every five
 * seconds is at best noise and at worst a lockout on the far end — and a
 * cancelled password dialog is an answer, not a failure to try again.
 *
 * Plain values and pure functions, so the rules can be tested without a clock.
 */

/** How long after one attempt fails the next one starts. */
export const RETRY_EVERY = 5_000;

/** How long after the connection went the pane stops trying by itself. */
export const RETRY_FOR = 5 * 60_000;

/**
 * How long a reconnected session has to stay up to count as back.
 *
 * Through the OpenSSH client, an attempt "succeeds" as soon as `ssh.exe` is
 * running — before it has reached the host — and then drops again when it
 * finds nothing there. A loss that soon after is the same outage, not a new
 * one, and must not restart the five minutes; otherwise a host that is down
 * would be retried forever.
 */
export const STEADY_AFTER = 10_000;

export interface Retrying {
  /** When the outage began, which the five minutes are counted from. */
  since: number;
  /**
   * When the next attempt starts. Null while one is in flight, and once the
   * pane has stopped trying.
   */
  next: number | null;
  /** Attempts made so far, the user's own included. */
  attempts: number;
  /** What the last failed attempt said, when it said anything. */
  reason: string | null;
  /** When the last attempt reached a shell, if it did. */
  readyAt: number | null;
  /** Why the pane is no longer trying by itself, if it is not. */
  halted: "timeout" | "user" | null;
}

/** Whether waiting could plausibly change the answer. */
export function retryable(failure: BackendFailure): boolean {
  return failure.cause === "unreachable";
}

/** Put the next attempt five seconds out, or stop if that is past the window. */
function scheduled(retry: Retrying, now: number): Retrying {
  const next = now + RETRY_EVERY;
  return next > retry.since + RETRY_FOR
    ? { ...retry, next: null, halted: "timeout" }
    : { ...retry, next, halted: null };
}

/**
 * The connection went. The start of an outage, or — when the last attempt
 * came up and went straight back down — more of the same one.
 */
export function lost(previous: Retrying | undefined, now: number): Retrying {
  const continuing = previous !== undefined
    && previous.halted === null
    && (previous.readyAt === null || now - previous.readyAt < STEADY_AFTER);
  const retry: Retrying = continuing
    ? { ...previous, readyAt: null }
    : { since: now, next: null, attempts: 0, reason: null, readyAt: null, halted: null };
  return scheduled(retry, now);
}

/** An attempt is starting. Nothing else is scheduled until it has finished. */
export function attempting(retry: Retrying): Retrying {
  return { ...retry, attempts: retry.attempts + 1, next: null };
}

/**
 * The user asked for an attempt. It is theirs, so it starts now, and it
 * restarts the five minutes: someone who pressed Reconnect after the pane gave
 * up has said the host is worth waiting for again.
 */
export function manual(previous: Retrying | undefined, now: number): Retrying {
  return {
    since: now,
    next: null,
    attempts: (previous?.attempts ?? 0) + 1,
    reason: previous?.reason ?? null,
    readyAt: null,
    halted: null
  };
}

/**
 * An attempt failed. The next one is scheduled if waiting could help, and
 * `undefined` comes back if it could not, which ends the retrying: the tab
 * shows why, and trying again is the user's call.
 */
export function failed(retry: Retrying, failure: BackendFailure, now: number): Retrying | undefined {
  if (!retryable(failure)) return undefined;
  // Stopped while it was running: the answer still arrives, and it is shown,
  // but it does not start the clock again.
  if (retry.halted === "user") return { ...retry, reason: failure.message || null };
  return scheduled({ ...retry, reason: failure.message || null }, now);
}

/** An attempt reached a shell. Whether it stays is for `lost` to judge. */
export function ready(retry: Retrying, now: number): Retrying {
  return { ...retry, next: null, readyAt: now };
}

/** The user would rather look at what is on screen than have it retried. */
export function stopped(retry: Retrying): Retrying {
  return { ...retry, next: null, halted: "user" };
}

/** Whether an attempt is running now, as opposed to waiting or stopped. */
export function inFlight(retry: Retrying): boolean {
  return retry.next === null && retry.halted === null && retry.readyAt === null;
}
