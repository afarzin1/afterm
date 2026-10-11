import { describe, expect, it } from "vitest";
import { classify, commandEnd, jumpTarget, outputSpan, pinnedAbove, ShellMarks } from "./commands";

/** Feed a run of chunks through `classify`, carrying the paste state along. */
function read(chunks: string[]): string[] {
  let pasting = false;
  return chunks.map(chunk => {
    const reading = classify(chunk, pasting);
    pasting = reading.pasting;
    return reading.intent;
  });
}

describe("reading terminal input", () => {
  it("treats ordinary typing as typing", () => {
    expect(read(["l", "s", "\r"])).toEqual(["start", "start", "submit"]);
  });

  /// The first half of the finding: everything beginning with Escape was
  /// ignored, so a pasted command never started a run and Enter had nothing to
  /// highlight.
  it("starts a run on a bracketed paste, and does not submit inside one", () => {
    // One chunk, as xterm delivers a paste.
    expect(read(["\x1b[200~echo hello\x1b[201~"])).toEqual(["start"]);
    // The Enter that follows is separate, and is the submission.
    expect(read(["\x1b[200~echo hello\x1b[201~", "\r"])).toEqual(["start", "submit"]);
    // A newline *inside* the paste is text. This is the whole point of the
    // mode, and reading it as Enter would band and pin the wrong thing.
    expect(read(["\x1b[200~one\rtwo\x1b[201~"])).toEqual(["start"]);
  });

  it("follows a paste split across chunks", () => {
    expect(read(["\x1b[200~echo ", "hello\rthere", "\x1b[201~", "\r"]))
      .toEqual(["start", "start", "start", "submit"]);
  });

  /// The other half: a command recalled with the arrow keys arrives as an
  /// escape sequence and nothing else. Both cursor modes, because a shell may
  /// put the terminal in either.
  it("starts a run on a command recalled from history", () => {
    for (const key of ["\x1b[A", "\x1b[B", "\x1bOA", "\x1bOB", "\x12"]) {
      expect(read([key, "\r"]), key).toEqual(["start", "submit"]);
    }
  });

  it("keeps a run through editing without starting one", () => {
    for (const key of ["\x1b[D", "\x1b[C", "\x7f", "\x15", "\x1b\x7f", "\x1b[3~"]) {
      expect(read([key]), key).toEqual(["keep"]);
    }
  });

  it("ignores what is not input at all", () => {
    // A function key, a mouse report, and a focus event.
    for (const noise of ["\x1bOP", "\x1b[<0;10;5M", "\x1b[I", "\x1b[O"]) {
      expect(read([noise]), noise).toEqual(["ignore"]);
    }
  });

  it("abandons the line on interrupt and on clear-screen", () => {
    expect(read(["l", "s", "\x03"])).toEqual(["start", "start", "clear"]);
    expect(read(["\x0c"])).toEqual(["clear"]);
  });
});

describe("finding where a command ends", () => {
  /// A command longer than the terminal is wide occupies several rows, and the
  /// band has to cover all of them. Reading the cursor instead gets this wrong
  /// whenever the echo has not caught up — which over SSH is most of the time
  /// for the last characters before Enter.
  it("follows a command onto the rows it wrapped onto", () => {
    const wrapped = new Set([11, 12]);
    expect(commandEnd(10, 40, line => wrapped.has(line))).toBe(12);
  });

  it("stops at a row that begins something new", () => {
    expect(commandEnd(10, 40, () => false)).toBe(10);
  });

  it("never runs past the last row it was given", () => {
    expect(commandEnd(10, 11, () => true)).toBe(11);
  });
});

describe("choosing the command to pin", () => {
  const commands = [{ line: 5 }, { line: 12 }, { line: 30 }, { line: 44 }];
  const at = (command: { line: number }) => command.line;

  it("picks the most recent one above the viewport", () => {
    expect(pinnedAbove(commands, 31, at)).toEqual({ line: 30 });
    expect(pinnedAbove(commands, 30, at)).toEqual({ line: 12 });
    expect(pinnedAbove(commands, 100, at)).toEqual({ line: 44 });
  });

  it("has nothing to pin when the first command is still on screen", () => {
    expect(pinnedAbove(commands, 5, at)).toBeNull();
    expect(pinnedAbove([], 100, at)).toBeNull();
  });

  /// The scan runs on every scroll and every render, so it has to be cheap
  /// whatever the scrollback holds. A walk over twenty thousand records is the
  /// shape of the problem this replaced.
  it("costs a handful of comparisons rather than a walk", () => {
    const many = Array.from({ length: 20_000 }, (_, index) => ({ line: index * 2 }));
    let looked = 0;
    const counted = new Proxy(many, {
      get(target, key) {
        if (typeof key === "string" && /^\d+$/.test(key)) looked += 1;
        return Reflect.get(target, key);
      }
    });
    expect(pinnedAbove(counted, 39_000, at)?.line).toBe(38_998);
    expect(looked).toBeLessThan(40);
  });
});

describe("tracking whether the shell marks its prompts", () => {
  /// A shell with no snippet is banded from keystrokes, every time. This is
  /// what a nested `ssh` or `sudo -i` looks like, and what used to stop
  /// working after a single mark from the login shell.
  it("bands from keystrokes when nothing marks the prompt", () => {
    const marks = new ShellMarks();
    for (let command = 0; command < 3; command += 1) {
      expect(marks.submitted().fallback).toBe(true);
    }
  });

  it("leaves a marked prompt to the shell", () => {
    const marks = new ShellMarks();
    marks.saw();
    expect(marks.submitted().fallback).toBe(false);
  });

  /// The recovery the finding asked for: marks stop arriving, and the fallback
  /// comes back for the very next command rather than never.
  it("takes the fallback back the moment the marks stop", () => {
    const marks = new ShellMarks();
    marks.saw();
    expect(marks.submitted().fallback).toBe(false);
    // `sudo -i`: no marks from here on.
    expect(marks.submitted().fallback).toBe(true);
    expect(marks.submitted().fallback).toBe(true);
    // ...and back out again.
    marks.saw();
    expect(marks.submitted().fallback).toBe(false);
  });

  /// Settings reports what it can see now, and is told only when that changes.
  it("reports the observed state, and only when it changes", () => {
    const marks = new ShellMarks();
    expect(marks.reset()).toBe(false);
    expect(marks.saw()).toBe(true);
    expect(marks.saw()).toBeNull();
    expect(marks.submitted().report).toBeNull();
    // The next command has no mark, so the session stops claiming integration.
    expect(marks.submitted().report).toBe(false);
    expect(marks.submitted().report).toBeNull();
    // A reconnect says nothing new when nothing was being claimed.
    expect(marks.reset()).toBeNull();
  });
});

describe("the output of a command", () => {
  it("runs from the row after the command to the row before the next prompt", () => {
    // Command on 10, output 11–14, the next prompt starts on 15.
    expect(outputSpan(10, [15], 16)).toEqual({ from: 11, to: 14 });
  });

  it("stops at whichever boundary comes first, in any order", () => {
    expect(outputSpan(10, [30, 15, 12, 4], 40)).toEqual({ from: 11, to: 11 });
  });

  /// The last command, with no prompt marked after it yet: the cursor's row
  /// is the new prompt, or the line a running command has not finished.
  it("falls back to the cursor when nothing is marked after it", () => {
    expect(outputSpan(10, [2, 5], 14)).toEqual({ from: 11, to: 13 });
  });

  it("is nothing when the command printed nothing", () => {
    expect(outputSpan(10, [11], 12)).toBeNull();
    expect(outputSpan(10, [], 11)).toBeNull();
  });
});

describe("jumping between commands", () => {
  const lines = [3, 20, 45, 90];

  it("goes to the nearest command above or below", () => {
    expect(jumpTarget(lines, 46, -1)).toBe(45);
    expect(jumpTarget(lines, 45, -1)).toBe(20);
    expect(jumpTarget(lines, 45, 1)).toBe(90);
    expect(jumpTarget(lines, 21, 1)).toBe(45);
  });

  it("says so when there is nowhere further to go", () => {
    expect(jumpTarget(lines, 3, -1)).toBeNull();
    expect(jumpTarget(lines, 90, 1)).toBeNull();
    expect(jumpTarget([], 10, -1)).toBeNull();
  });

  it("never lands on a command trimmed out of the scrollback", () => {
    expect(jumpTarget([-1, -1, 12], 10, -1)).toBeNull();
  });
});
