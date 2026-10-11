import { describe, expect, it } from "vitest";
import { answersFor, blankAnswers, dequeueRequest, enqueueRequest, forget } from "./auth-queue";
import type { AuthRequest } from "../types";

const ask = (session: string, request: string, prompts = 1): AuthRequest => ({
  session,
  request,
  host: `${session}.example.com`,
  user: "deploy",
  name: "Password",
  instructions: "",
  prompts: Array.from({ length: prompts }, () => ({ prompt: "Password:", echo: false })),
  method: "password",
  canRemember: false
});

describe("requests waiting on the user", () => {
  it("keeps the first request in front when a second session asks", () => {
    const queue = enqueueRequest(enqueueRequest([], ask("a", "a/0")), ask("b", "b/0"));
    expect(queue.map(item => item.request)).toEqual(["a/0", "b/0"]);
  });

  it("does not queue the same request twice", () => {
    const once = enqueueRequest([], ask("a", "a/0"));
    const again = enqueueRequest(once, { ...ask("a", "a/0"), instructions: "again" });
    expect(again).toHaveLength(1);
    expect(again[0].instructions).toBe("again");
  });

  it("removes a request by its own id and leaves the rest waiting", () => {
    const queue = enqueueRequest(enqueueRequest([], ask("a", "a/0")), ask("b", "b/0"));
    expect(dequeueRequest(queue, "a/0").map(item => item.request)).toEqual(["b/0"]);
    // Resolving something that is not there — a timeout for a request already
    // answered — changes nothing.
    expect(dequeueRequest(queue, "zzz")).toEqual(queue);
  });
});

describe("what the fields show", () => {
  it("starts blank, one field per prompt the host sent", () => {
    expect(blankAnswers(ask("a", "a/0", 3))).toEqual(["", "", ""]);
    expect(blankAnswers(undefined)).toEqual([]);
  });

  it("shows typing only for the request it was typed into", () => {
    const typed = { id: "a/0", values: ["hunter2"] };
    expect(answersFor(typed, ask("a", "a/0"))).toEqual(["hunter2"]);
    // The next request in the queue must not inherit the last one's secret.
    expect(answersFor(typed, ask("b", "b/0"))).toEqual([""]);
    expect(answersFor(typed, undefined)).toEqual([]);
  });

  it("drops the typing when its request ends, and nothing else's", () => {
    const typed = { id: "a/0", values: ["hunter2"] };
    expect(forget(typed, "a/0")).toBeNull();
    expect(forget(typed, "b/0")).toBe(typed);
    expect(forget(null, "a/0")).toBeNull();
  });
});
