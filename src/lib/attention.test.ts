import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { ECHO_WITHIN, fromOsc777, fromOsc9, KittyNotifications, QUIET_AFTER, QuietWatch, WORKED_FOR } from "./attention";

describe("reading a request for attention", () => {
  it("reads an OSC 9 notification, with or without a message", () => {
    expect(fromOsc9("Claude is waiting for your input")).toEqual({ message: "Claude is waiting for your input" });
    // Whitespace around the message is not part of it.
    expect(fromOsc9("\n\nAgent turn complete")).toEqual({ message: "Agent turn complete" });
    expect(fromOsc9("")).toEqual({ message: null });
  });

  /// The one that would make every background build light its tab.
  it("ignores ConEmu's numbered OSC 9 commands, progress above all", () => {
    expect(fromOsc9("4;1;42")).toBeNull();
    expect(fromOsc9("4;0")).toBeNull();
    expect(fromOsc9("9;C:\\Users\\arya")).toBeNull();
    expect(fromOsc9("12")).toBeNull();
  });

  it("does not mistake a message that starts with a number for a command", () => {
    expect(fromOsc9("3 tests failed")).toEqual({ message: "3 tests failed" });
  });

  it("reads an OSC 777 notify and nothing else under 777", () => {
    expect(fromOsc777("notify;Claude Code;Waiting for input")).toEqual({ message: "Claude Code: Waiting for input" });
    expect(fromOsc777("notify;;Done")).toEqual({ message: "Done" });
    expect(fromOsc777("notify;Title;body;with;semicolons")).toEqual({ message: "Title: body;with;semicolons" });
    expect(fromOsc777("preexec")).toBeNull();
  });

  it("reads a kitty notification, whole or in pieces", () => {
    const kitty = new KittyNotifications();
    expect(kitty.read(";Build finished")).toEqual({ message: "Build finished" });
    expect(kitty.read("i=1:d=0;Claude Code")).toBeNull();
    expect(kitty.read("i=1:p=body;Needs approval")).toEqual({ message: "Claude Code: Needs approval" });
    // Base64, as the protocol allows.
    expect(kitty.read(`e=1;${btoa("encoded")}`)).toEqual({ message: "encoded" });
  });

  it("does not treat a kitty close or query as a notification", () => {
    const kitty = new KittyNotifications();
    expect(kitty.read("i=1:p=close;")).toBeNull();
    expect(kitty.read("i=1:p=?;")).toBeNull();
  });
});

describe("a tab going quiet", () => {
  let clock = 0;
  let marks = 0;
  let watch: QuietWatch;
  /** Output every `every` ms for `lasting` ms, then the clock run on. */
  const work = (lasting: number, every = 300) => {
    for (let at = 0; at <= lasting; at += every) {
      watch.wrote();
      vi.advanceTimersByTime(every);
      clock += every;
    }
  };
  const wait = (ms: number) => { vi.advanceTimersByTime(ms); clock += ms; };

  beforeEach(() => {
    vi.useFakeTimers();
    clock = 0;
    marks = 0;
    watch = new QuietWatch(() => marks++, () => clock);
  });
  afterEach(() => {
    watch.dispose();
    vi.useRealTimers();
  });

  it("marks a turn that worked and then stopped", () => {
    watch.typed();
    wait(ECHO_WITHIN);
    work(10_000);
    expect(marks).toBe(0);
    wait(QUIET_AFTER);
    expect(marks).toBe(1);
  });

  it("does not mark a quick answer", () => {
    watch.typed();
    wait(ECHO_WITHIN);
    work(WORKED_FOR - 600);
    wait(QUIET_AFTER * 2);
    expect(marks).toBe(0);
  });

  /// Typing a long prompt and leaving the tab before pressing Enter.
  it("does not count the echo of typing as work", () => {
    for (let key = 0; key < 40; key++) {
      watch.typed();
      wait(50);
      watch.wrote();
      wait(150);
    }
    wait(QUIET_AFTER * 2);
    expect(marks).toBe(0);
  });

  /// A login banner, a slow .bashrc, a reconnect.
  it("does not mark a tab nobody asked anything of", () => {
    work(10_000);
    wait(QUIET_AFTER);
    expect(marks).toBe(0);
  });

  it("marks once per thing asked for, and again after being looked at", () => {
    watch.typed();
    wait(ECHO_WITHIN);
    work(5_000);
    wait(QUIET_AFTER);
    work(5_000);
    wait(QUIET_AFTER);
    expect(marks).toBe(1);
    watch.arm();
    work(5_000);
    wait(QUIET_AFTER);
    expect(marks).toBe(2);
  });

  it("does not mark a program that redraws on a timer", () => {
    watch.typed();
    wait(ECHO_WITHIN);
    work(60_000, 1_000);
    expect(marks).toBe(0);
  });
});
