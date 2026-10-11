/**
 * A program in a tab asking for the user: a coding agent that has finished its
 * turn or wants an approval, or anything else that rings.
 *
 * afterm does not try to recognise the agent. Over SSH the agent runs on the
 * host, where afterm cannot see its process, and a guess from its output would
 * break with every release of it. What does cross the connection is the
 * signal the agents already send to ask a terminal for attention, so that is
 * what is read:
 *
 * - **OSC 9** — `ESC ] 9 ; message BEL`, iTerm2's notification. Claude Code
 *   sends it with `preferredNotifChannel` set to `iterm2`, Codex with
 *   `notification_method = "osc9"`.
 * - **OSC 777** — `ESC ] 777 ; notify ; title ; body BEL`, the rxvt form that
 *   Ghostty and others read. Claude Code's `ghostty` channel.
 * - **OSC 99** — kitty's notification protocol. Claude Code's `kitty` channel.
 * - **BEL** on its own — Claude Code's `terminal_bell`, Codex's `bel`.
 *
 * Every one of those also has a use that is not a request for attention, and
 * reading them as one is the failure to design against. ConEmu put a family
 * of numbered commands under OSC 9, and `OSC 9;4` is the progress report
 * Windows Terminal draws on the taskbar — a build running in a background tab
 * sends it many times a second. A shell rings BEL when tab completion has
 * nothing to offer. The first is filtered here; the second is left to the
 * rule the interface applies to all of them, which is that a signal from the
 * tab being looked at is not news.
 *
 * None of those arrives unless the agent was set up to send it, and an agent
 * left alone sends nothing to a terminal it does not recognise by name. So
 * there is a fourth reading, which needs nothing on the host: a tab that was
 * busy and went quiet. See `QuietWatch`.
 */

/** Something that asked for the user, and what it said, if anything. */
export interface Notice {
  message: string | null;
  /** Inferred from the tab going quiet rather than sent. */
  quiet?: true;
}

const text = (value: string) => {
  const trimmed = value.trim();
  return trimmed === "" ? null : trimmed;
};

/**
 * OSC 9. A notification unless it is one of ConEmu's numbered commands —
 * `9;1` a sleep, `9;2` a message box, `9;4` progress, up to `9;12` — which
 * begin with a number and a separator, or are a bare number.
 */
export function fromOsc9(data: string): Notice | null {
  if (/^\d+(;|$)/.test(data)) return null;
  return { message: text(data) };
}

/** OSC 777. Only `notify`; the other subcommands are not requests for anyone. */
export function fromOsc777(data: string): Notice | null {
  const [command, title = "", ...body] = data.split(";");
  if (command !== "notify") return null;
  return { message: text([title, body.join(";")].filter(part => part.trim()).join(": ")) };
}

/**
 * OSC 99, kitty's protocol: `metadata;payload`, the metadata `key=value`
 * pairs joined by colons. A notification may arrive in pieces, each marked
 * `d=0` until the last, so the pieces are gathered per notification id and the
 * notice is raised by the last one. `p` says what a piece is; only a title or a
 * body is text to show, and a close or a capability query is not a
 * notification at all. `e=1` means the payload is base64.
 */
export class KittyNotifications {
  private pending = new Map<string, string[]>();

  read(data: string): Notice | null {
    const split = data.indexOf(";");
    const metadata = split < 0 ? data : data.slice(0, split);
    let payload = split < 0 ? "" : data.slice(split + 1);
    const fields = new Map(metadata.split(":").filter(Boolean).map(pair => {
      const at = pair.indexOf("=");
      return (at < 0 ? [pair, ""] : [pair.slice(0, at), pair.slice(at + 1)]) as [string, string];
    }));
    const kind = fields.get("p") ?? "title";
    if (kind !== "title" && kind !== "body") return null;
    if (fields.get("e") === "1") {
      try { payload = atob(payload); } catch { payload = ""; }
    }
    const id = fields.get("i") ?? "";
    const parts = this.pending.get(id) ?? [];
    if (payload.trim()) parts.push(payload.trim());
    if (fields.get("d") === "0") {
      // Bounded: a program that opens pieces and never finishes them must not
      // grow this for as long as the tab is open.
      if (this.pending.size < 16 || this.pending.has(id)) this.pending.set(id, parts.slice(-8));
      return null;
    }
    this.pending.delete(id);
    return { message: text(parts.join(": ")) };
  }
}

/** Silence that ends a stretch of work: well clear of the gaps inside one. */
export const QUIET_AFTER = 3000;
/** How long the output must have kept coming for there to have been work. */
export const WORKED_FOR = 2000;
/** Output this soon after a keystroke is its echo, not work. */
export const ECHO_WITHIN = 500;

/**
 * A tab that was busy and went quiet, which from an agent means its turn is
 * over. Measured, not assumed: Claude Code's spinner and title redraw at
 * least every 0.35 s for the whole of a turn, Codex's status line every 0.5 s,
 * and both then write nothing at all until they are given something to do.
 * iTerm2's idle alert, Konsole's monitor for silence and tmux's
 * `monitor-silence` are the same idea, and none of it depends on the agent.
 *
 * Designed against:
 * - **Typing.** The echo is output. Output within `ECHO_WITHIN` of a
 *   keystroke does not count, and a keystroke starts the stretch over, so
 *   typing in a tab and leaving it marks nothing.
 * - **A tab nobody has asked anything of.** A login banner, a slow `.bashrc`,
 *   a tmux status line, a log that bursts now and then. The watch is armed by
 *   typing, or by the tab being looked at, and fires once per arming: one
 *   mark per thing asked for, not one per burst.
 * - **A quick answer.** `ls` finishes before `WORKED_FOR`, and marks nothing.
 *
 * What it cannot tell apart is an agent done with its turn and a long
 * command done with its run, and it does not try: a build that has just
 * finished in a tab you are not looking at is worth a mark too. A program
 * that redraws on a timer, `top` or `watch`, never goes quiet and is never
 * marked.
 */
export class QuietWatch {
  private armed = false;
  private typedAt = -Infinity;
  /** When the current stretch of output began; null between stretches. */
  private since: number | null = null;
  private lastOutput = 0;
  private timer: ReturnType<typeof setTimeout> | undefined;

  constructor(private readonly quiet: () => void, private readonly now = () => Date.now()) {}

  /** The tab is being looked at, so what it does next is worth reporting. */
  arm() {
    this.armed = true;
  }

  /** The user typed into the tab. */
  typed() {
    this.armed = true;
    this.typedAt = this.now();
    this.since = null;
    clearTimeout(this.timer);
  }

  /** Output arrived. */
  wrote() {
    const now = this.now();
    if (!this.armed || now - this.typedAt < ECHO_WITHIN) return;
    this.since ??= now;
    this.lastOutput = now;
    clearTimeout(this.timer);
    this.timer = setTimeout(() => this.settle(), QUIET_AFTER);
  }

  dispose() {
    clearTimeout(this.timer);
  }

  private settle() {
    const since = this.since;
    this.since = null;
    if (since === null || this.lastOutput - since < WORKED_FOR) return;
    this.armed = false;
    this.quiet();
  }
}

/**
 * The lines that tell each agent to signal the terminal. A copy rather than
 * an install, for the same reason as the shell snippet: these are files in
 * someone's home directory on a machine that is not afterm's, and two of them
 * are structured files that a blind append would break.
 */
export const AGENT_SETUP = `# afterm: let a coding agent say what it is waiting for.
# Without these a tab is still marked when its agent goes quiet; with them the
# mark carries the agent's own message. Set on the machine the agent runs on,
# which over SSH is the host.

# Claude Code: a top-level key in ~/.claude/settings.json
#   "preferredNotifChannel": "iterm2"

# Codex: in ~/.codex/config.toml
#   [tui]
#   notifications = ["agent-turn-complete", "approval-requested"]
#   notification_method = "osc9"

# Anything with a hook that runs when it stops (Grok Build, for one): have the
# hook write the same signal to the terminal.
#   printf '\\033]9;%s\\007' "Waiting for you" > /dev/tty
`;
