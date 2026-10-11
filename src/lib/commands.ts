/**
 * Working out which rows of the scrollback hold a command, when the shell does
 * not say.
 *
 * A shell with OSC 133 in its prompt says exactly where a prompt ends and a
 * command begins, and afterm uses that whenever it arrives. Most hosts do not
 * have the snippet, so the fallback is what nearly everyone sees, and it has
 * to be better than "the first keystroke starts a run".
 *
 * What the first version got wrong, both halves reproduced against a real bash
 * over SSH:
 *
 * * **Anything beginning with Escape was ignored outright.** Bracketed paste
 *   arrives as `ESC [ 200 ~ … ESC [ 201 ~`, and a command recalled with Up
 *   arrives as `ESC [ A`. Neither started a run, so Enter had nothing to
 *   highlight and nothing to pin — a pasted command and a recalled one were
 *   invisible to the feature, which is exactly the two cases people repeat
 *   most.
 * * **A single mark switched the fallback off for the life of the tab.** One
 *   OSC 133 from the login shell was enough; `sudo -i` or a nested `ssh` into
 *   an unintegrated shell then had no marks *and* no fallback, so highlighting
 *   stopped for everything, including plainly typed commands.
 *
 * Both are decided here rather than in the pane, because both are rules about
 * bytes and rows and neither needs a terminal to be true.
 */

/** What one chunk of terminal input means for the line being typed. */
export type Intent =
  /** Enter: whatever is on the line has been submitted. */
  | "submit"
  /** Something arrived that puts a command on the line. */
  | "start"
  /** Editing what is already there. Keeps a run, never starts one. */
  | "keep"
  /** The line was abandoned — Ctrl-C, or the screen cleared. */
  | "clear"
  /** Nothing to do with the line at all. */
  | "ignore";

export interface Reading {
  intent: Intent;
  /** Whether a bracketed paste is still open after this chunk. */
  pasting: boolean;
}

const PASTE_START = "\x1b[200~";
const PASTE_END = "\x1b[201~";

/**
 * Escape sequences that recall or complete a whole command: the history keys,
 * in both the normal and application cursor modes a shell may put the terminal
 * in, and reverse search. What arrives after one of these is a command the
 * user did not type a character of.
 */
const RECALLS = ["\x1b[A", "\x1b[B", "\x1bOA", "\x1bOB"];

/**
 * Escape sequences that move or delete within the line. They cannot put a
 * command on an empty line, so they never start a run — but they are part of
 * editing one, so they must not end it either. The old code lumped these in
 * with everything else beginning with Escape and ignored them, which was
 * harmless; lumping the history keys in with them was not.
 */
const EDITS = [
  "\x1b[C", "\x1b[D", "\x1bOC", "\x1bOD", // left, right
  "\x1b[H", "\x1b[F", "\x1bOH", "\x1bOF", // home, end
  "\x1b[1~", "\x1b[4~", "\x1b[3~",        // home, end, delete
  "\x1bb", "\x1bf", "\x1bd", "\x1b\x7f"   // word motion, word delete
];

/**
 * Read one chunk of input.
 *
 * `pasting` is the state the previous chunk left behind: a paste can be split
 * across chunks, and its contents must not be read as keystrokes — a newline
 * inside a bracketed paste is text, not a submission, which is the whole reason
 * the mode exists.
 */
export function classify(data: string, pasting: boolean): Reading {
  if (pasting) {
    const end = data.indexOf(PASTE_END);
    if (end === -1) return { intent: "start", pasting: true };
    // What follows the end marker in the same chunk is ordinary input again —
    // most often the Enter that runs what was pasted.
    return { ...classify(data.slice(end + PASTE_END.length), false), pasting: false };
  }

  if (data.startsWith(PASTE_START)) {
    return { ...classify(data.slice(PASTE_START.length), true), intent: "start" };
  }

  if (data === "\r" || data === "\n" || data === "\r\n") return { intent: "submit", pasting };

  if (data.startsWith("\x1b")) {
    if (RECALLS.includes(data)) return { intent: "start", pasting };
    if (EDITS.includes(data)) return { intent: "keep", pasting };
    // A function key, a mouse report, a focus event, an answer to a device
    // query. None of them is typing.
    return { intent: "ignore", pasting };
  }

  switch (data) {
    // Interrupt, and clear-screen: the line is gone either way.
    case "\x03":
    case "\x0c":
      return { intent: "clear", pasting };
    // End of file. Nothing was typed, so there is nothing to submit.
    case "\x04":
      return { intent: "ignore", pasting };
    // Backspace, kill-line, kill-word, transpose: editing.
    case "\x7f":
    case "\x08":
    case "\x0b":
    case "\x15":
    case "\x17":
    case "\x14":
      return { intent: "keep", pasting };
    // Reverse search puts somebody else's command on the line.
    case "\x12":
      return { intent: "start", pasting };
    default:
      break;
  }

  // Tab completion extends what is there and can also complete an empty line
  // into a listing; either way something is on the line afterwards.
  return { intent: "start", pasting };
}

/**
 * The last row of a command that begins on `start`.
 *
 * A command longer than the terminal is wide occupies several rows, and xterm
 * marks each continuation as wrapped. That is a better answer than the cursor's
 * position for two reasons: it is exact for a wrapped command, and it does not
 * depend on the echo having caught up — over SSH the last characters of a
 * command and the newline after them arrive after the Enter that sent them.
 */
export function commandEnd(
  start: number,
  last: number,
  isWrapped: (line: number) => boolean
): number {
  let end = start;
  while (end < last && isWrapped(end + 1)) end += 1;
  return end;
}

/**
 * The most recent command that has scrolled above `top`, which is the one to
 * pin.
 *
 * A binary search rather than a walk. The pin is refreshed on every scroll and
 * every render, and the list is as long as the scrollback allows — twenty
 * thousand rows of it — so a walk is work proportional to the session's whole
 * history, done on the frame that is trying to scroll.
 *
 * Lines are non-decreasing because commands are appended in the order they are
 * run. A record whose marker has been trimmed out of the scrollback reports a
 * negative line; those are pruned as they die, and skipped here so that one
 * arriving late cannot make the search look at an unsorted list.
 */
export function pinnedAbove<T>(
  commands: readonly T[],
  top: number,
  lineOf: (command: T) => number
): T | null {
  let low = 0;
  let high = commands.length - 1;
  let found: T | null = null;
  while (low <= high) {
    const middle = (low + high) >> 1;
    const line = lineOf(commands[middle]);
    if (line >= 0 && line < top) {
      found = commands[middle];
      low = middle + 1;
    } else {
      high = middle - 1;
    }
  }
  return found;
}

/**
 * Whether the shell in front of the terminal marks its own prompts.
 *
 * Not a latch, which is what it used to be: one OSC 133 anywhere turned the
 * keystroke fallback off for the life of the tab. The marks are sent per prompt
 * and per command, so this is a question about the shell that is there now —
 * and that changes with `sudo -i`, with a nested `ssh` into a host that has no
 * snippet, and again on the way back out of either.
 *
 * Two things come out of it. Whether a submitted command should be banded from
 * keystrokes or left to the shell's own `C` mark; and what Settings should say
 * about this session, which is the observed state rather than a high-water
 * mark.
 */
export class ShellMarks {
  /** Whether a mark has arrived for the prompt now on screen. */
  private current = false;
  /** What the interface was last told, so it is only told about changes. */
  private reported: boolean | null = null;

  /** A mark arrived. Returns the state to report, or `null` if unchanged. */
  saw(): boolean | null {
    this.current = true;
    return this.settle(true);
  }

  /**
   * A command was submitted.
   *
   * `fallback` is whether to band it from the keystroke anchor. False means
   * this prompt was marked, so the shell is about to say where the command is
   * and saying it twice would band it twice.
   */
  submitted(): { fallback: boolean; report: boolean | null } {
    const marked = this.current;
    this.current = false;
    return { fallback: !marked, report: this.settle(marked) };
  }

  /** A new connection is a new shell, with nothing yet known about it. */
  reset(): boolean | null {
    this.current = false;
    return this.settle(false);
  }

  private settle(state: boolean): boolean | null {
    if (this.reported === state) return null;
    this.reported = state;
    return state;
  }
}

/**
 * The rows a command printed.
 *
 * From the row after the command's last one, up to the row before whichever
 * comes first after it: a boundary — a prompt starting (OSC 133 `A`), the
 * output ending (`D`), the next command — or the cursor. The cursor is the
 * limit of last resort because it is where the next prompt is being drawn, or,
 * for a command still running, the line it has not finished yet. Null when
 * nothing lies between.
 *
 * Boundaries in any order: they come from two lists, and neither the caller
 * nor this needs them merged.
 */
export function outputSpan(
  commandEnd: number,
  boundaries: Iterable<number>,
  cursor: number
): { from: number; to: number } | null {
  let limit = cursor;
  for (const line of boundaries) {
    if (line > commandEnd && line < limit) limit = line;
  }
  const from = commandEnd + 1;
  const to = limit - 1;
  return to >= from ? { from, to } : null;
}

/**
 * Where to jump from `anchor`: the nearest command start above it for a step
 * of -1, below it for +1, or null when there is none that way. `lines` in the
 * order commands were run, which is also buffer order; a trimmed command
 * reports a negative line and is never a target.
 */
export function jumpTarget(lines: readonly number[], anchor: number, step: -1 | 1): number | null {
  if (step < 0) {
    for (let index = lines.length - 1; index >= 0; index -= 1) {
      if (lines[index] >= 0 && lines[index] < anchor) return lines[index];
    }
    return null;
  }
  for (const line of lines) {
    if (line > anchor) return line;
  }
  return null;
}
