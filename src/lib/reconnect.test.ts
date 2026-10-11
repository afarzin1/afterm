import { describe, expect, it } from "vitest";
import {
  attempting, failed, inFlight, lost, manual, ready, RETRY_EVERY, RETRY_FOR, STEADY_AFTER, stopped
} from "./reconnect";
import type { BackendFailure, FailureCause } from "../types";

const failure = (cause: FailureCause, message = "no route to host"): BackendFailure =>
  ({ cause, message, methods: [], canPrompt: false });

describe("reconnecting a lost session", () => {
  it("tries again five seconds after the connection goes", () => {
    const retry = lost(undefined, 1_000);
    expect(retry.next).toBe(1_000 + RETRY_EVERY);
    expect(retry.since).toBe(1_000);
    expect(retry.halted).toBeNull();
  });

  /// A fixed interval would start a second attempt on top of one still
  /// waiting for a slow host, or for a password.
  it("schedules nothing while an attempt is running", () => {
    const retry = attempting(lost(undefined, 0));
    expect(retry.next).toBeNull();
    expect(retry.attempts).toBe(1);
    expect(inFlight(retry)).toBe(true);
  });

  it("counts the five seconds from when the last attempt failed", () => {
    const retry = failed(attempting(lost(undefined, 0)), failure("unreachable"), 30_000);
    expect(retry?.next).toBe(30_000 + RETRY_EVERY);
    expect(retry?.reason).toBe("no route to host");
  });

  it("stops after five minutes, counted from the loss rather than the last attempt", () => {
    let retry = lost(undefined, 0);
    let now = 0;
    while (retry.next !== null) {
      now = retry.next;
      retry = failed(attempting(retry), failure("unreachable"), now)!;
    }
    expect(retry.halted).toBe("timeout");
    expect(now).toBeLessThanOrEqual(RETRY_FOR);
    expect(retry.attempts).toBe(RETRY_FOR / RETRY_EVERY);
  });

  it.each<FailureCause>(["auth", "hostKey", "unsupported", "missingClient", "internal"])(
    "does not retry a %s failure, which waiting would not fix",
    cause => {
      expect(failed(attempting(lost(undefined, 0)), failure(cause), 6_000)).toBeUndefined();
    }
  );

  /// Through `ssh.exe` an attempt is "ready" once the process runs, and
  /// drops again as soon as it finds the host still down. That must not
  /// start the five minutes over, or a host that is down is retried forever.
  it("treats a drop straight after reconnecting as the same outage", () => {
    const up = ready(attempting(lost(undefined, 0)), 5_000);
    const again = lost(up, 5_000 + STEADY_AFTER - 1);
    expect(again.since).toBe(0);
    expect(again.attempts).toBe(1);
  });

  it("treats a drop after the session stayed up as a new outage", () => {
    const up = ready(attempting(lost(undefined, 0)), 5_000);
    const later = 5_000 + STEADY_AFTER;
    const again = lost(up, later);
    expect(again.since).toBe(later);
    expect(again.attempts).toBe(0);
  });

  it("restarts the five minutes when the user reconnects after it gave up", () => {
    const gaveUp = { ...lost(undefined, 0), next: null, halted: "timeout" as const, attempts: 60 };
    const retry = manual(gaveUp, RETRY_FOR + 1_000);
    expect(retry.since).toBe(RETRY_FOR + 1_000);
    expect(retry.halted).toBeNull();
    expect(retry.attempts).toBe(61);
    expect(inFlight(retry)).toBe(true);
    // And a failure of that attempt goes back to trying on its own.
    expect(failed(retry, failure("unreachable"), RETRY_FOR + 2_000)?.next)
      .toBe(RETRY_FOR + 2_000 + RETRY_EVERY);
  });

  it("stays stopped when the user stops it", () => {
    const retry = stopped(lost(undefined, 0));
    expect(retry.next).toBeNull();
    expect(retry.halted).toBe("user");
    expect(inFlight(retry)).toBe(false);
    // A later loss, after the user reconnected by hand and it came up, is
    // a fresh outage with retrying back on.
    expect(lost(retry, 60_000).halted).toBeNull();
  });
});
