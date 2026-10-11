import { useCallback, useEffect, useRef, useState, type CSSProperties } from "react";
import { Terminal, type IDecoration, type IDisposable, type IMarker } from "@xterm/xterm";
import { FitAddon } from "@xterm/addon-fit";
import { WebLinksAddon } from "@xterm/addon-web-links";
import { WebglAddon } from "@xterm/addon-webgl";
import { SearchAddon } from "@xterm/addon-search";
import { Unicode11Addon } from "@xterm/addon-unicode11";
import { ArrowUp, ChevronDown, ChevronUp, PlugZap, X } from "lucide-react";
import {
  failureOf, isDesktop, onTerminalOutput, openLink, pasteClipboardImage, readClipboardText, resizeTerminal,
  setTerminalFlow, spawnSession, writeTerminal
} from "../lib/backend";
import { typingIntoField } from "../lib/keys";
import { carriedLines } from "../lib/carry";
import { fromOsc777, fromOsc9, KittyNotifications, QuietWatch, type Notice } from "../lib/attention";
import { findMatches, labelMatches, type ScreenLine } from "../lib/quickselect";
import { classify, commandEnd, jumpTarget, outputSpan, pinnedAbove, ShellMarks } from "../lib/commands";
import { directoryFromOsc } from "../lib/integration";
import { copiesOnRelease, PRIMARY_BUTTON } from "../lib/selection";
import { clipboardWrite } from "../lib/osc52";
import { FONT_FAMILY, terminalPalette, type Theme } from "../theme";
import { startPreviewShell } from "../lib/preview-shell";
import type { BackendFailure, Session } from "../types";
import { inFlight, RETRY_FOR, type Retrying } from "../lib/reconnect";

interface Props {
  session: Session;
  visible: boolean;
  focused: boolean;
  style: CSSProperties;
  fontSize: number;
  theme: Theme;
  /// True when a mouse selection should go to the clipboard on its own, with
  /// no `Ctrl Shift C`. Off unless the setting is on.
  copyOnSelect: boolean;
  /// Which of the two grounds this pane stands on.
  shade: number;
  onFocus(): void;
  onReady(id: string): void;
  onExit(id: string, failure: BackendFailure): void;
  /// Where the automatic reconnect has got to, when this session is in one.
  retry?: Retrying;
  onRetry(id: string): void;
  onStopRetry(id: string): void;
  onQuit(id: string): void;
  /// Called whenever what afterm can observe about this shell changes: true
  /// when its prompts carry OSC 133 marks, false when they stop. Settings asks
  /// so it can say whether the snippet is in place on the shell that is in
  /// front of the terminal *now* — which `sudo -i` and a nested `ssh` change,
  /// and which a reconnect starts over.
  onIntegration(id: string, present: boolean): void;
  onDirectory(id: string, path: string): void;
  /// Called when something in the terminal asks for the user — a coding agent
  /// done with its turn, a bell. Whether that is news is the caller's call:
  /// it knows which tab is being looked at. See `lib/attention`.
  onAttention(id: string, notice: Notice): void;
  /// Hands the parent this pane's `PaneActions`, and null when it goes.
  onActions(id: string, actions: PaneActions | null): void;
  /// Called when a drag, or the host through OSC 52, has put something on the
  /// clipboard, so the interface can say so. Not for `Ctrl Shift C`: that is a
  /// key the user pressed on purpose and needs no confirmation, while these
  /// are copies nobody asked for out loud.
  onCopy(): void;
  /// Something to say in the corner: how an image paste is going.
  onNotice(message: string): void;
}

/** A line range the user typed on, kept so it can stay highlighted and pinned. */
interface CommandMark {
  marker: IMarker;
  /** The command's last row: a wrapped command has several, and its output
   *  starts after the last. */
  last: IMarker;
  text: string;
}

/**
 * What a pane can be asked to do from outside it — the command palette — for
 * the things that live on its terminal. Registered while the pane is mounted.
 */
export interface PaneActions {
  previousCommand(): void;
  nextCommand(): void;
  copyOutput(): void;
  quickSelect(): void;
}

/** One thing quick select offers: its label, its text, and where it is drawn. */
interface QuickTarget {
  label: string;
  text: string;
  /** One box per row the text covers, in pixels from the pane's corner. */
  boxes: Array<{ left: number; top: number; width: number; height: number }>;
}

/**
 * One highlighted row.
 *
 * `used` is where the row's text ends, which the band needs and cannot always
 * recover later: a row trimmed out of the scrollback is gone. Held so the band
 * can be painted again when the theme changes.
 */
interface Band {
  marker: IMarker;
  used: number;
  decoration?: IDecoration;
}

/**
 * Terminal bytes, out of the base64 the backend sends them in.
 *
 * The obvious spelling — `Uint8Array.from(atob(data), c => c.charCodeAt(0))` —
 * runs the iterable protocol with a callback per byte, and measured 3.4 ms for
 * one 32 KiB chunk against 0.11 ms for this loop: 9 MB/s against 274 MB/s. This
 * is the terminal's throughput ceiling, so it is worth the six lines. Anything
 * that scrolls — a build log, `cat` of a large file — spends its time here.
 */
function decodeChunk(data: string): Uint8Array {
  const binary = atob(data);
  const bytes = new Uint8Array(binary.length);
  for (let index = 0; index < binary.length; index++) {
    bytes[index] = binary.charCodeAt(index);
  }
  return bytes;
}

/**
 * Put the terminal's selection on the clipboard, and say whether it landed.
 *
 * Written through the browser rather than the native side, the way the
 * settings panel writes: WebView2 puts a permission prompt in front of reading
 * someone's clipboard and none in front of writing to it.
 *
 * The answer is what the copy-on-select flash hangs off. A write that threw
 * has to be distinguishable from one that worked, or the interface would
 * announce a copy that never happened and the user would paste the last thing
 * they actually copied without knowing why.
 */
async function copySelection(term: Terminal): Promise<boolean> {
  const text = term.getSelection();
  if (!text) return false;
  try {
    await navigator.clipboard.writeText(text);
    return true;
  } catch {
    // No clipboard. The selection stays where it is, and can be copied again.
    return false;
  }
}

/** How long an image paste may take before the corner says it is under way. */
const SENDING_NOTICE_AFTER = 400;

/**
 * Put the clipboard into the terminal.
 *
 * `paste` rather than `write`: it is what wraps the text for bracketed paste
 * mode, which is how a shell knows not to run a pasted newline.
 *
 * Text wins whenever there is any. Excel, a browser and Word put text and a
 * picture of it on the clipboard together, and what someone pasting those
 * into a terminal wants is the text. With no text, an SSH session sends the
 * clipboard's image to the host and pastes its path there, which is how a
 * coding agent on the host takes a screenshot: it reads the host's clipboard,
 * never this one. A local shell is left alone — an agent running here reads
 * this clipboard itself.
 */
async function pasteClipboard(
  term: Terminal,
  session: Session,
  onNotice: (message: string) => void
): Promise<void> {
  let text = "";
  try {
    text = await readClipboardText();
  } catch {
    // No clipboard, or nothing readable in it. Nothing to paste.
  }
  if (text) {
    term.paste(text);
    return;
  }
  if (session.kind !== "ssh" && session.kind !== "ssh-cli") return;

  // Said only once the wait is long enough to notice, so an empty clipboard —
  // which answers at once — never flashes a message about sending something.
  let announced = false;
  const timer = window.setTimeout(() => {
    announced = true;
    onNotice(`Sending the image to ${session.host}…`);
  }, SENDING_NOTICE_AFTER);
  try {
    const path = await pasteClipboardImage(session.id);
    window.clearTimeout(timer);
    if (path) term.paste(path);
    if (announced) onNotice("");
  } catch (problem) {
    window.clearTimeout(timer);
    onNotice(`The image could not be sent to ${session.host}: ${failureOf(problem).message}`);
  }
}

export function TerminalPane({
  session, visible, focused, style, fontSize, theme, copyOnSelect, shade, onFocus, onReady,
  onExit, retry, onRetry, onStopRetry, onQuit, onIntegration, onDirectory, onAttention, onActions, onCopy,
  onNotice
}: Props) {
  // An automatic attempt keeps the popup up while it runs. Without it the tab
  // flickers between the popup and a blank terminal every five seconds, and a
  // host that takes its time to refuse looks, for that time, like one that let
  // the tab in.
  const reconnecting = session.state === "connecting" && retry !== undefined && inFlight(retry);
  const lost = session.state === "lost" || reconnecting;
  /// The countdown's clock, ticked only while there is a countdown to show.
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    if (!lost || retry?.next == null) return;
    setNow(Date.now());
    const timer = window.setInterval(() => setNow(Date.now()), 250);
    return () => window.clearInterval(timer);
  }, [lost, retry?.next]);
  const host = useRef<HTMLDivElement>(null);
  const terminal = useRef<Terminal>();
  const fit = useRef<FitAddon>();
  const search = useRef<SearchAddon>();
  const [finding, setFinding] = useState(false);
  /// Quick select, while it is up: what is on offer, and the letters typed.
  const [quick, setQuick] = useState<{ targets: QuickTarget[]; typed: string } | null>(null);
  const paneRoot = useRef<HTMLDivElement>(null);
  /// The link under the pointer: where it really goes, and where to say so.
  const [hovered, setHovered] = useState<{ uri: string; x: number; y: number; refused?: string } | null>(null);
  const [query, setQuery] = useState("");
  const [hits, setHits] = useState({ index: -1, count: 0 });
  const commands = useRef<CommandMark[]>([]);
  const pinnedMark = useRef<CommandMark | null>(null);
  const [pinned, setPinned] = useState<string | null>(null);
  /// The current theme, for the handlers below: they live as long as the
  /// terminal does, which is longer than any one theme.
  const current = useRef(theme);
  current.current = theme;
  /// The palette this pane's terminal is painted with, for those same
  /// handlers: the ground moves when the layout puts the pane on the other
  /// shade, not only when the scheme changes.
  const palette = useRef(terminalPalette(theme, shade));
  palette.current = terminalPalette(theme, shade);
  const currentlyVisible = useRef(visible);
  currentlyVisible.current = visible;
  /// Copy-on-select as it stands now, for the same reason: the handlers are
  /// set up once per session and the setting is turned on and off under them.
  const copying = useRef(copyOnSelect);
  copying.current = copyOnSelect;
  /// Paints every band again in the given colour. Owned by the effect that
  /// owns the terminal, called by the one that watches the theme.
  const repaintBands = useRef<((color: string) => void) | null>(null);
  /// The previous terminal's text, handed from the one a reconnect is
  /// replacing to the one it builds. See `lib/carry`.
  const carried = useRef<string[]>([]);
  /// The tab going quiet after work, for the effect that arms it on focus.
  const quietWatch = useRef<QuietWatch | null>(null);

  useEffect(() => {
    if (!host.current) return;

    /**
     * Links, whether a program printed a URL or sent an OSC 8 hyperlink. Both
     * come from the far end, and an OSC 8 target need not resemble the text
     * it is drawn on — so the pointer shows the real address before anything
     * is opened, and opening takes Ctrl, the way Windows Terminal and VS Code
     * do it: a plain click in a terminal is a click to select or to focus.
     */
    const showLink = (event: MouseEvent, uri: string) => {
      const origin = paneRoot.current?.getBoundingClientRect();
      if (!origin) return;
      setHovered({ uri, x: event.clientX - origin.left, y: event.clientY - origin.top });
    };
    const hideLink = () => setHovered(null);
    const followLink = (event: MouseEvent, uri: string) => {
      if (!event.ctrlKey) {
        showLink(event, uri);
        return;
      }
      setHovered(null);
      void openLink(uri).catch(failure => {
        showLink(event, uri);
        setHovered(old => old && { ...old, refused: failureOf(failure).message });
      });
    };

    const term = new Terminal({
      linkHandler: { activate: followLink, hover: showLink, leave: hideLink },
      // Decorations (the highlight behind typed lines) are proposed API in
      // xterm 5.x and throw without this.
      allowProposedApi: true,
      cursorBlink: true,
      cursorStyle: "bar",
      cursorWidth: 2,
      fontFamily: FONT_FAMILY,
      fontSize,
      fontWeight: "normal",
      fontWeightBold: "bold",
      letterSpacing: 0,
      // JetBrains Mono is drawn around a 1.2 leading; anything tighter crowds
      // its tall x-height.
      lineHeight: 1.2,
      scrollback: 20000,
      theme: palette.current
    });
    const fitAddon = new FitAddon();
    const searchAddon = new SearchAddon();
    term.loadAddon(fitAddon);
    term.loadAddon(new WebLinksAddon(followLink, { hover: showLink, leave: hideLink }));
    term.loadAddon(searchAddon);
    // Without this xterm measures wide characters against Unicode 6 tables,
    // which is two versions before emoji were given widths at all. Every
    // emoji and a good part of the CJK range then occupies one cell here and
    // two on the host, the cursor walks out of step with the shell by one
    // column per character, and anything drawn out of box characters — htop,
    // tmux, a progress bar — comes apart down the middle.
    term.loadAddon(new Unicode11Addon());
    term.unicode.activeVersion = "11";
    term.open(host.current);
    terminal.current = term;
    fit.current = fitAddon;
    search.current = searchAddon;
    // `resultIndex` is -1 until a search has run, and the bar reads that as
    // "nothing to show" rather than as a position.
    const results = searchAddon.onDidChangeResults(({ resultIndex, resultCount }) =>
      setHits({ index: resultIndex, count: resultCount }));

    // Split panes are resized and composited even when their terminal does not
    // have keyboard focus. Preserve the last GPU frame so those panes never
    // flash blank between xterm redraws. Unsupported or lost contexts still
    // fall back to xterm's DOM renderer.
    let webgl: WebglAddon | undefined;
    try {
      webgl = new WebglAddon(true);
      webgl.onContextLoss(() => { webgl?.dispose(); webgl = undefined; });
      term.loadAddon(webgl);
    } catch {
      // No WebGL2 here. The DOM renderer is slower, not broken.
    }

    const buffer = () => term.buffer.active;
    const cursorLine = () => buffer().baseY + buffer().cursorY;
    /** Full-screen programs (vim, htop, less) repaint freely; tracking input
     *  lines there would highlight unrelated rows, so it is suspended. */
    const isFullScreenApp = () => buffer().type === "alternate";

    /**
     * The text of a run of rows, joined.
     *
     * `column` is where the command starts on the first row. A shell that
     * marks the end of its prompt says exactly where that is; without a mark
     * it is zero, and the prompt comes along with the command the way it
     * always has.
     */
    const readLines = (from: number, to: number, column = 0) => {
      const parts: string[] = [];
      for (let line = from; line <= to; line += 1) {
        const row = buffer().getLine(line)?.translateToString(true) ?? "";
        parts.push(line === from ? row.slice(column) : row);
      }
      return parts.join(" ").trim();
    };

    // The clipboard lives on `Ctrl Shift`, where every Linux terminal puts it.
    // What that buys is the unmodified pair back: `Ctrl C` now interrupts in
    // every case, so a selection the mouse left lying around can no longer
    // swallow it, and `Ctrl V` reaches the shell as `^V` — readline's
    // `quoted-insert`, vim's visual block.
    //
    // Both are done here rather than handed back to the webview by returning
    // false. Chromium spends these two itself: `Ctrl Shift C` is the element
    // picker and `Ctrl Shift V` is paste-and-match-style. So the event is
    // taken from both sides — `preventDefault` for the webview, `false` for
    // xterm — and the clipboard is worked directly.
    term.attachCustomKeyEventHandler(event => {
      if (event.type !== "keydown") return true;
      // Page keys move through the scrollback, which is what they do in every
      // Windows terminal. Only on the normal buffer: a full-screen program has
      // its own idea of a page and has to be given the key, or `less` and
      // `htop` stop paging the moment afterm starts scrolling behind them.
      if (!event.ctrlKey && !event.altKey && !event.shiftKey && !isFullScreenApp()) {
        if (event.key === "PageUp") { term.scrollPages(-1); return false; }
        if (event.key === "PageDown") { term.scrollPages(1); return false; }
      }
      if (!event.ctrlKey || event.altKey || !event.shiftKey) return true;
      const key = event.key.toLowerCase();
      // Between commands, and the output of one. Normal buffer only, like the
      // page keys: a full-screen program has no commands in it to go between.
      if (!isFullScreenApp()) {
        if (key === "pageup" || key === "pagedown") {
          event.preventDefault();
          jump(key === "pageup" ? -1 : 1);
          return false;
        }
        if (key === "o") {
          event.preventDefault();
          copyOutput();
          return false;
        }
        if (key === "s") {
          event.preventDefault();
          quickSelect();
          return false;
        }
      }
      // With nothing selected there is nothing to copy, and the key is left to
      // the terminal rather than swallowed.
      if (key === "c" && term.hasSelection()) {
        event.preventDefault();
        void copySelection(term);
        return false;
      }
      if (key === "v") {
        event.preventDefault();
        void pasteClipboard(term, session, onNotice);
        return false;
      }
      return true;
    });

    /** Absolute buffer line where the current keystroke run began. */
    let inputAnchor: number | null = null;

    const safeFit = () => {
      try {
        fitAddon.fit();
        term.options.theme = palette.current;
        term.refresh(0, term.rows - 1);
      } catch { /* not laid out yet */ }
    };

    const bands: Band[] = [];

    /** Draw one band, in the colour the theme asks for. */
    const paint = (band: Band, color: string) => {
      try {
        band.decoration = term.registerDecoration({
          marker: band.marker,
          x: 0,
          width: term.cols,
          height: 1,
          // Bottom layer, so xterm paints the band into the canvas beneath the
          // glyphs and the text keeps its own colours.
          layer: "bottom",
          backgroundColor: color
        });
        band.decoration?.onRender(element => {
          element.classList.add("typed-line");
          // xterm only paints cells that hold something, so the band stops at
          // the end of the text. The decoration's own element sits above the
          // glyphs, which means it cannot be filled opaquely without hiding
          // them — but it can fill just the empty remainder of the row. This
          // is where the text ends, as a percentage of the row.
          const filled = term.cols > 0 ? Math.min(100, (band.used / term.cols) * 100) : 100;
          element.style.setProperty("--band-from", `${filled}%`);
        });
      } catch {
        // Highlighting is decorative; losing it must not break the session.
      }
    };

    /**
     * Highlight one buffer row, and return its marker.
     *
     * The record is removed when the row is trimmed out of the scrollback
     * rather than at the next theme change, which is when it used to be: a
     * session that never changed theme accumulated a dead `Band` for every row
     * it had ever highlighted, each holding a disposed marker and a
     * decoration, and every repaint walked the lot.
     */
    const highlightRow = (line: number): IMarker | undefined => {
      const marker = term.registerMarker(line - cursorLine());
      if (!marker) return undefined;
      const band: Band = {
        marker,
        used: (buffer().getLine(line)?.translateToString(true) ?? "").length
      };
      bands.push(band);
      paint(band, current.current.band);
      marker.onDispose(() => {
        band.decoration?.dispose();
        band.decoration = undefined;
        const at = bands.indexOf(band);
        if (at >= 0) bands.splice(at, 1);
      });
      return marker;
    };

    /// Absolute rows that already carry a band, so the same command cannot be
    /// banded twice. Both the keystroke fallback and a `C` mark can arrive for
    /// one command — a shell whose marks are late, or a `DEBUG` trap that fires
    /// more than once — and two decorations on a row show as a double-strength
    /// band that never goes away.
    const banded = new Set<number>();

    // The colour a decoration was registered with is baked in, so a theme
    // change means every band is drawn again. Without this, switching theme
    // leaves each old row half in the previous colour: xterm painted that part
    // under the glyphs, where the CSS half of the band cannot reach it.
    repaintBands.current = color => {
      let live = 0;
      for (const band of bands) {
        band.decoration?.dispose();
        band.decoration = undefined;
        // Trimmed out of the scrollback. Its marker is dead and so is the row.
        if (band.marker.line < 0) continue;
        paint(band, color);
        bands[live] = band;
        live += 1;
      }
      bands.length = live;
    };

    /**
     * Band a submitted command, and keep it for the pin.
     *
     * `column` is where the command begins on its first row: past the prompt
     * when the shell has marked it, zero when it has not.
     */
    const bandCommand = (start: number, end: number, column: number) => {
      if (end < start || banded.has(start)) return;
      const text = readLines(start, end, column);
      // A bare Enter has no command on it to band or to pin.
      if (!text) return;

      // One decoration per row rather than one tall one: each row needs its own
      // fill point, and a wrapped command's earlier rows are full of text.
      let first: IMarker | undefined;
      let last: IMarker | undefined;
      for (let line = start; line <= end; line += 1) {
        if (banded.has(line)) continue;
        banded.add(line);
        const marker = highlightRow(line);
        marker?.onDispose(() => banded.delete(line));
        if (line === start) first = marker;
        if (marker) last = marker;
      }
      if (!first) return;
      const record: CommandMark = { marker: first, last: last ?? first, text };
      commands.current.push(record);
      // The pin's list is bounded by the scrollback, not by the session: a
      // record goes when its row does. Without this the array only ever grew,
      // and it is scanned on every scroll and every render.
      first.onDispose(() => {
        const at = commands.current.indexOf(record);
        if (at >= 0) commands.current.splice(at, 1);
        if (pinnedMark.current === record) {
          pinnedMark.current = null;
          setPinned(null);
        }
      });
    };

    /// Rows where a command's output must have ended: a prompt starting, or the
    /// shell saying the output is done. Each goes when its row is trimmed.
    const boundaries: IMarker[] = [];
    const markBoundary = () => {
      const marker = term.registerMarker(0);
      if (!marker) return;
      boundaries.push(marker);
      marker.onDispose(() => {
        const at = boundaries.indexOf(marker);
        if (at >= 0) boundaries.splice(at, 1);
      });
    };

    /// Where the shell said its prompt ended and typing began, when it says so.
    let promptEnd: { line: number; column: number } | null = null;
    /// Whether the shell in front of the terminal marks its prompts, asked
    /// afresh of every prompt rather than latched by the first mark ever seen.
    const marks = new ShellMarks();
    const tellSettings = (state: boolean | null) => {
      if (state !== null) onIntegration(session.id, state);
    };
    // A reconnect is a new shell, so the session starts claiming nothing.
    tellSettings(marks.reset());

    /**
     * A command whose Enter has been sent and whose band is waiting for the
     * echo to land.
     *
     * Not banded on the spot, because at that moment the row may be short of
     * its last characters: over SSH the echo of what was typed arrives after
     * the keystroke that sent it, and a command that wrapped may not have
     * reached its second row yet. The band is drawn on the next output instead,
     * which is the echo itself, or on a timer if none comes.
     */
    let awaiting: { start: number; column: number; timer: number } | null = null;

    const drawAwaited = () => {
      // A write callback can outlive the pane; the terminal it would read is
      // gone by then.
      if (!awaiting || disposed) return;
      const { start, column, timer } = awaiting;
      awaiting = null;
      window.clearTimeout(timer);
      // The wrapped rows of the command, and no further: output always begins
      // on a row of its own.
      const end = commandEnd(start, buffer().baseY + term.rows - 1, line =>
        buffer().getLine(line)?.isWrapped === true);
      bandCommand(start, end, column);
    };

    /// How long to wait for an echo before banding what is on the row already.
    /// Long enough for a round trip on a slow link, short enough that the band
    /// still appears as part of pressing Enter.
    const ECHO_GRACE = 250;

    /// And how long to leave a marked prompt to send its own `C` before
    /// banding it anyway. A shell with only half the snippet — a `PS1` with the
    /// prompt marks and no `preexec` hook to send `C` — used to get no band at
    /// all, because the marks were trusted to finish what they started.
    const MARK_GRACE = 1500;

    /**
     * OSC 133 — the shell saying where its prompt ends and where a command
     * begins, instead of afterm inferring it from keystrokes.
     *
     * The heuristic reads a pasted or recalled command now, but the marks are
     * still better: they put the band exactly on the command, and the pinned
     * copy carries it without the prompt in front of it. Where both have
     * something to say, `banded` keeps the row from being decorated twice.
     */
    const oscMarks = term.parser.registerOscHandler(133, data => {
      const kind = data.charAt(0);
      // Where a prompt starts and where output ends are both where the output
      // before them stops, which is what copying a command's output needs.
      if (kind === "A" || kind === "D") markBoundary();
      if (kind === "A" || kind === "B" || kind === "C") {
        tellSettings(marks.saw());
      }
      if (kind === "A") {
        // A fresh prompt. Anything left over belongs to a command that was
        // abandoned rather than run — Ctrl-C, or a shell that redrew.
        promptEnd = null;
        inputAnchor = null;
      } else if (kind === "B") {
        promptEnd = { line: cursorLine(), column: buffer().cursorX };
        inputAnchor = null;
      } else if (kind === "C" && promptEnd) {
        // The mark for "output starts here" lands after the newline the shell
        // echoed for Enter — but whether the cursor has moved on to the next
        // row by now is the shell's business, so the command ends at the last
        // row with anything on it.
        let end = cursorLine();
        while (end > promptEnd.line && !(buffer().getLine(end)?.translateToString(true) ?? "")) {
          end -= 1;
        }
        // The mark is the better answer, so it goes first and the pending
        // fallback finds the row already banded.
        bandCommand(promptEnd.line, end, promptEnd.column);
        if (awaiting) window.clearTimeout(awaiting.timer);
        awaiting = null;
        promptEnd = null;
      }
      // Swallowed whatever it was, this one included: an OSC nobody handles is
      // printed to the screen.
      return true;
    });

    /**
     * OSC 7 — the shell saying which directory it is in, which the file pane
     * follows.
     *
     * Sent at every prompt, so all but a few of these repeat the last one.
     * They are dropped here rather than upstairs: unchanged, this is a React
     * state write per prompt, and the interesting event is the change rather
     * than the report. What survives is one call per `cd`, which is exactly
     * what the pane wants — it moves once per directory change, and a pane
     * the user has browsed elsewhere is left where they put it until the
     * shell moves again.
     */
    let reportedDirectory: string | null = null;
    const oscDirectory = term.parser.registerOscHandler(7, data => {
      const path = directoryFromOsc(data);
      if (path && path !== reportedDirectory) {
        reportedDirectory = path;
        onDirectory(session.id, path);
      }
      return true;
    });

    // Requests for attention. Claimed whether or not they are one, so that
    // nothing else reads a progress report as text.
    const attend = (notice: Notice | null) => { if (notice) onAttention(session.id, notice); };
    const kitty = new KittyNotifications();
    const oscNotices = [
      term.parser.registerOscHandler(9, data => { attend(fromOsc9(data)); return true; }),
      term.parser.registerOscHandler(777, data => { attend(fromOsc777(data)); return true; }),
      term.parser.registerOscHandler(99, data => { attend(kitty.read(data)); return true; }),
      term.onBell(() => attend({ message: null }))
    ];
    // OSC 52 — the host copying something here: `/copy` in a coding agent,
    // a yank in vim. Announced with the same flash as copy-on-select, because
    // it is a copy the user did not make with this machine's mouse or keys.
    // See `lib/osc52`.
    const oscClipboard = term.parser.registerOscHandler(52, data => {
      const text = clipboardWrite(data);
      if (text) {
        void navigator.clipboard.writeText(text).then(onCopy, () => {
          // No clipboard. Nothing landed, so nothing is announced.
        });
      }
      return true;
    });
    const quiet = new QuietWatch(() => attend({ message: null, quiet: true }));
    quietWatch.current = quiet;

    /// Whether a bracketed paste is open, carried between chunks.
    let pasting = false;

    const input = term.onData(data => {
      const reading = classify(data, pasting);
      pasting = reading.pasting;
      // A focus report or an answer to a query is the terminal talking, and
      // one goes out the moment the tab is left.
      if (reading.intent !== "ignore") quiet.typed();
      // A full-screen program repaints freely, so a row it drew is not a
      // command and tracking one there would band something unrelated.
      if (!isFullScreenApp()) {
        switch (reading.intent) {
          case "submit": {
            const typedFrom = inputAnchor;
            inputAnchor = null;
            // A marked prompt is the shell's to describe, and its `C` is on
            // its way. An unmarked one is afterm's, whatever this shell did
            // last time — which is how a `sudo -i` or a nested `ssh` into an
            // unintegrated shell gets its highlighting back.
            const { fallback, report } = marks.submitted();
            tellSettings(report);
            // Where the band starts: the shell's own answer when it gave one,
            // which is past the prompt, and the row typing began on when it
            // did not. Both routes end up here, so a `C` that arrives and one
            // that never does produce the same band.
            const from = promptEnd ? promptEnd.line : typedFrom;
            const column = promptEnd ? promptEnd.column : 0;
            if (from !== null) {
              awaiting = {
                start: from,
                column,
                timer: window.setTimeout(drawAwaited, fallback ? ECHO_GRACE : MARK_GRACE)
              };
            }
            break;
          }
          case "start":
            if (inputAnchor === null) inputAnchor = cursorLine();
            break;
          case "clear":
            inputAnchor = null;
            break;
          case "keep":
          case "ignore":
            break;
        }
      }
      if (isDesktop()) void writeTerminal(session.id, data);
    });

    // Right-click pastes, the way it does in PuTTY and Windows Terminal. The
    // webview's own menu is suppressed application-wide — it offers Reload and
    // Print, which belong to a web page — so this is what the button is for.
    // It matters more than it used to: with paste on `Ctrl Shift V` this is
    // the one-handed way to do it, and the one PuTTY already taught.
    const contextMenu = (event: MouseEvent) => {
      event.preventDefault();
      void pasteClipboard(term, session, onNotice);
    };
    host.current.addEventListener("contextmenu", contextMenu);

    // Copy on select, when the setting is on: what a drag highlights is on the
    // clipboard when the button comes up, the way PuTTY and every X11
    // terminal have always done it. `Ctrl Shift C` still works and is still
    // the documented way; this saves the reach for the people who never
    // learned to make it.
    //
    // Bound to the end of the drag rather than to xterm's `onSelectionChange`,
    // which is the obvious hook and the wrong one twice over. It fires for
    // every cell the mouse crosses, so one drag down a screen of output is a
    // hundred clipboard writes for ninety-nine selections nobody meant to
    // keep. And it fires for selections the mouse never made: the search
    // addon selects each hit it lands on, so `Ctrl Shift F` and a walk through
    // the results would leave whatever the last hit was on the clipboard
    // instead of what the user had put there.
    //
    // Only the left button starts a drag: right-click is paste, and letting it
    // through here would have every paste answer itself by putting the
    // selection back on the clipboard it had just read. `copiesOnRelease`
    // holds the rest of the rule, including why the release is heard on the
    // window rather than on the pane.
    let dragging = false;
    const pressed = (event: MouseEvent) => {
      if (event.button === PRIMARY_BUTTON) dragging = true;
    };
    const released = () => {
      const fromHere = dragging;
      dragging = false;
      if (!copiesOnRelease(copying.current, fromHere, term.hasSelection())) return;
      void copySelection(term).then(written => { if (written) onCopy(); });
    };
    host.current.addEventListener("mousedown", pressed);
    window.addEventListener("mouseup", released);

    // Set by the cleanup below. A tab closed while its session is still being
    // opened ends the attempt on the backend, whose refusal arrives here after
    // the terminal it would be written to has gone.
    let disposed = false;
    let stopListen: (() => void) | undefined;
    const start = async () => {
      safeFit();
      // Dimmed, so it reads as the connection before this one; and first, so
      // the new shell's output lands after it rather than under it.
      const previous = carried.current;
      carried.current = [];
      if (previous.length > 0) term.write(`\x1b[2m${previous.join("\r\n")}\x1b[0m\r\n`);
      if (!isDesktop()) {
        startPreviewShell(term);
        return;
      }
      // Listening first, and waiting for it. `onTerminalOutput` is an IPC
      // round trip; starting the session before it finished meant the login
      // banner, the first prompt and the integration marks in it could all be
      // written before anything was there to receive them — and a mark lost at
      // the start is a tab that mis-bands its first command for as long as it
      // is open.
      stopListen = await onTerminalOutput(payload => {
        if (payload.id !== session.id) return;
        quiet.wrote();
        draw(decodeChunk(payload.data));
      });
      // The pane went while that was in flight. Undo the subscription now
      // rather than leaving it for a cleanup that has already run, and do not
      // start a session nobody is showing.
      if (disposed) {
        stopListen();
        stopListen = undefined;
        return;
      }
      try {
        // For an `ssh.exe` session this resolves once the process is
        // spawned, which is before it has authenticated; for a native one it
        // resolves once the connection is authenticated, because there is no
        // session until then. Nothing downstream may assume the stronger of
        // the two — the file pane waits for a listing, not for this.
        await spawnSession(session, term.cols, term.rows);
        onReady(session.id);
      } catch (error) {
        if (disposed) return;
        // The palette's own red, so the message reads as the terminal's
        // rather than carrying one scheme's colour into another's.
        const failure = failureOf(error);
        term.writeln(`\x1b[91mafterm: ${failure.message}\x1b[0m`);
        onExit(session.id, failure);
      }
    };
    void start();

    /**
     * How far behind the interface may fall before the reader is held.
     *
     * Bytes handed to xterm are not bytes drawn: it parses on its own
     * schedule, and says so through the callback below. Without a ceiling the
     * only queue between a fast producer and a slower renderer is the
     * webview's own, which has none — so `cat` of something large is a memory
     * spike and a window that stops answering rather than a terminal that
     * scrolls for a while. Held at a megabyte and let go again at a quarter of
     * it: a burst that fits is never interrupted, and a flood is throttled
     * once rather than stuttered at the mark.
     */
    const HOLD_ABOVE = 1024 * 1024;
    const RESUME_BELOW = 256 * 1024;
    let queued = 0;
    let held = false;

    const draw = (bytes: Uint8Array) => {
      queued += bytes.length;
      if (!held && queued > HOLD_ABOVE) {
        held = true;
        void setTerminalFlow(session.id, true);
      }
      term.write(bytes, () => {
        // The first output after an Enter is the echo of what was typed, so
        // this is the moment the row is whole and the band can be drawn on it.
        drawAwaited();
        queued -= bytes.length;
        if (held && queued <= RESUME_BELOW) {
          held = false;
          void setTerminalFlow(session.id, false);
        }
      });
    };

    /**
     * Scroll to the previous or next command.
     *
     * A command lands on the second row, not the first: the pinned command
     * sits over the first row, and a jump that put its target under the pin
     * would show you the command you just left instead. So "here" is the
     * second row as well, and jumping again from a command goes to the one
     * beyond it rather than to itself.
     */
    const jump = (step: -1 | 1) => {
      if (isFullScreenApp()) return;
      const anchor = buffer().viewportY + 1;
      const target = jumpTarget(commands.current.map(mark => mark.marker.line), anchor, step);
      if (target === null) {
        if (step > 0) term.scrollToBottom();
        return;
      }
      term.scrollToLine(Math.max(0, target - 1));
    };

    /**
     * Select what one command printed, and copy it.
     *
     * Which command: the one at the top of the view after a jump, and the most
     * recent one that printed anything when the view is at the bottom — the
     * common case is "copy what I just ran", with no jump first. Selected as
     * well as copied, so what went to the clipboard is on the screen to see.
     */
    const copyOutput = () => {
      if (isFullScreenApp()) return;
      const atBottom = buffer().viewportY >= buffer().baseY;
      const starts = commands.current.map(mark => mark.marker.line);
      const stops = [...boundaries.map(marker => marker.line), ...starts];
      const spanOf = (mark: CommandMark) => {
        const end = mark.last.line;
        if (end < 0) return null;
        const span = outputSpan(end, stops, cursorLine());
        if (!span) return null;
        // Blank rows at the end are the gap before the next prompt, not output.
        let to = span.to;
        while (to > span.from && !(buffer().getLine(to)?.translateToString(true) ?? "").trim()) to -= 1;
        return { from: span.from, to };
      };
      let span: { from: number; to: number } | null = null;
      if (atBottom) {
        for (let index = commands.current.length - 1; index >= 0 && !span; index -= 1) {
          span = spanOf(commands.current[index]);
        }
      } else {
        const anchor = buffer().viewportY + 1;
        const mark = pinnedAbove(commands.current, anchor + 1, record => record.marker.line);
        span = mark ? spanOf(mark) : null;
      }
      if (!span) return;
      term.selectLines(span.from, span.to);
      void copySelection(term).then(written => { if (written) onCopy(); });
    };
    /**
     * Read the screen as logical lines, a wrapped row joined onto the row it
     * continues, with the row and column each character is drawn at. The
     * second half of a wide character has width 0 and no text of its own.
     */
    const readScreen = (): ScreenLine[] => {
      const lines: ScreenLine[] = [];
      const top = buffer().viewportY;
      let current: ScreenLine | null = null;
      for (let row = 0; row < term.rows; row += 1) {
        const line = buffer().getLine(top + row);
        if (!line) continue;
        if (!line.isWrapped || !current) {
          current = { text: "", cells: [] };
          lines.push(current);
        }
        for (let column = 0; column < term.cols; column += 1) {
          const cell = line.getCell(column);
          if (!cell || cell.getWidth() === 0) continue;
          for (const character of cell.getChars() || " ") {
            current.text += character;
            current.cells.push({ row, column });
          }
        }
      }
      return lines;
    };

    /** Label everything copyable on screen, and wait for a label. */
    const quickSelect = () => {
      const screen = host.current?.querySelector(".xterm-screen");
      const root = paneRoot.current;
      if (!screen || !root || term.rows === 0 || term.cols === 0) return;
      const lines = readScreen();
      // The pinned command covers the top row; a label there would sit on
      // the pin, over text that cannot be seen.
      const covered = pinnedMark.current !== null;
      const matches = labelMatches(findMatches(lines).filter(match =>
        !covered || lines[match.line].cells[match.start]?.row !== 0));
      if (matches.length === 0) return;
      const area = screen.getBoundingClientRect();
      const origin = root.getBoundingClientRect();
      const cellWidth = area.width / term.cols;
      const cellHeight = area.height / term.rows;
      const targets = matches.map(match => {
        const rows = new Map<number, { from: number; to: number }>();
        for (const { row, column } of lines[match.line].cells.slice(match.start, match.end)) {
          const span = rows.get(row);
          if (span) span.to = Math.max(span.to, column);
          else rows.set(row, { from: column, to: column });
        }
        return {
          label: match.label,
          text: match.text,
          boxes: [...rows].map(([row, { from, to }]) => ({
            left: area.left - origin.left + from * cellWidth,
            top: area.top - origin.top + row * cellHeight,
            width: (to - from + 1) * cellWidth,
            height: cellHeight
          }))
        };
      });
      setQuick({ targets, typed: "" });
    };
    // The labels are drawn where the text was when they were made; once the
    // screen moves they point at something else.
    const endQuick = term.onScroll(() => setQuick(null));
    const endQuickOnResize = term.onResize(() => setQuick(null));

    onActions(session.id, {
      previousCommand: () => jump(-1),
      nextCommand: () => jump(1),
      copyOutput,
      quickSelect
    });

    // Pin the most recent typed line that has scrolled off the top, and drop
    // the pin as soon as the user scrolls back far enough to see it again.
    const refreshPin = () => {
      const top = buffer().viewportY;
      // No copy of the list: this runs on every scroll and every render, and
      // the allocation would cost more than the search it feeds.
      const candidate = pinnedAbove(commands.current, top, mark => mark.marker.line);
      pinnedMark.current = candidate;
      setPinned(candidate?.text ?? null);
    };
    const listeners: IDisposable[] = [
      term.onScroll(refreshPin), term.onRender(refreshPin), results, oscMarks, oscDirectory,
      oscClipboard, ...oscNotices, endQuick, endQuickOnResize
    ];

    const element = host.current;
    // One fit per frame, and one window change per size the far end has not
    // already been told about. Dragging a divider fires this observer for
    // every frame of the drag, and every one of those was a `SIGWINCH`: a
    // full-screen program repaints itself for each, which is what made vim
    // and htop thrash while a split was being resized.
    let frame = 0;
    let sentCols = 0;
    let sentRows = 0;
    const observer = new ResizeObserver(() => {
      if (!currentlyVisible.current || !host.current || frame) return;
      frame = requestAnimationFrame(() => {
        frame = 0;
        safeFit();
        if (term.cols === sentCols && term.rows === sentRows) return;
        sentCols = term.cols;
        sentRows = term.rows;
        void resizeTerminal(session.id, term.cols, term.rows);
      });
    });
    observer.observe(element);

    return () => {
      disposed = true;
      // The normal buffer only: a full-screen program's screen is the
      // alternate one, and is a picture of a program that is no longer there.
      try { carried.current = carriedLines(term.buffer.normal); } catch { carried.current = []; }
      observer.disconnect();
      if (frame) cancelAnimationFrame(frame);
      // A pane that goes while it is holding the reader back must let go, or
      // the session it was showing is left with a shut gate and no one to open
      // it.
      if (held) void setTerminalFlow(session.id, false);
      element.removeEventListener("contextmenu", contextMenu);
      element.removeEventListener("mousedown", pressed);
      window.removeEventListener("mouseup", released);
      if (awaiting) window.clearTimeout(awaiting.timer);
      input.dispose();
      quiet.dispose();
      if (quietWatch.current === quiet) quietWatch.current = null;
      listeners.forEach(listener => listener.dispose());
      stopListen?.();
      repaintBands.current = null;
      onActions(session.id, null);
      try { webgl?.dispose(); } catch { /* the context was already gone */ }
      try { term.dispose(); } catch { /* nothing left to dispose */ }
      commands.current = [];
    };
  }, [session.id, session.attempt]);

  useEffect(() => {
    const term = terminal.current;
    if (!term) return;
    term.options.theme = terminalPalette(theme, shade);
    repaintBands.current?.(theme.band);
  }, [theme, shade]);

  // A font size change is a resize. The pane keeps its pixels and gains or
  // loses rows and columns inside them, so the far end has to be told, exactly
  // as it is when the window or a divider moves — otherwise the shell goes on
  // wrapping to the old width and the screen tears. Nothing here changes the
  // element's size, so the ResizeObserver never fires and cannot cover for it:
  // the damage used to stand until some unrelated resize told the host by
  // accident, which is why leaving full screen appeared to be the fix.
  useEffect(() => {
    const term = terminal.current;
    if (!term) return;
    term.options.fontSize = fontSize;
    requestAnimationFrame(() => {
      try {
        fit.current?.fit();
        term.refresh(0, term.rows - 1);
      } catch { /* not laid out yet */ }
      void resizeTerminal(session.id, term.cols, term.rows);
    });
  }, [fontSize, session.id]);

  /**
   * Run the search, or move through what it found.
   *
   * `incremental` while typing, so the view follows the first hit ahead of the
   * cursor instead of jumping to the next one on every keystroke.
   */
  const find = useCallback((term: string, direction: 1 | -1, incremental = false) => {
    const options = {
      incremental,
      decorations: {
        matchBackground: theme.searchMatch,
        activeMatchBackground: theme.searchActive,
        matchOverviewRuler: theme.searchMatch,
        activeMatchColorOverviewRuler: theme.searchActive
      }
    };
    if (!term) {
      search.current?.clearDecorations();
      setHits({ index: -1, count: 0 });
      return;
    }
    if (direction === 1) search.current?.findNext(term, options);
    else search.current?.findPrevious(term, options);
  }, [theme]);

  const closeFind = useCallback(() => {
    setFinding(false);
    setQuery("");
    search.current?.clearDecorations();
    setHits({ index: -1, count: 0 });
    terminal.current?.focus();
  }, []);

  // `Ctrl Shift F` is claimed here rather than left to xterm. Capture, so it
  // never reaches the terminal at all. Shifted, so `Ctrl F` still goes to the
  // shell as `^F` — readline's `forward-char`, and a page forward in vim and
  // less. xterm encodes both to the same `^F` regardless, so the shifted half
  // is afterm's to take at no cost to the far end.
  useEffect(() => {
    if (!focused || lost) return;
    const handler = (event: KeyboardEvent) => {
      if (!event.ctrlKey || event.altKey || event.metaKey || !event.shiftKey) return;
      if (event.key.toLowerCase() !== "f") return;
      event.preventDefault();
      event.stopPropagation();
      setFinding(true);
    };
    window.addEventListener("keydown", handler, { capture: true });
    return () => window.removeEventListener("keydown", handler, { capture: true });
  }, [focused, lost]);

  // One keystroke each, claimed in the capture phase so the dead terminal does
  // not swallow them — it still has its scrollback and its handlers, it just
  // has nowhere to send anything. Modified keys are left alone, or
  // `Ctrl Shift R` would rename the tab and reconnect it at the same time.
  useEffect(() => {
    if (!focused || !lost) return;
    const handler = (event: KeyboardEvent) => {
      if (event.ctrlKey || event.altKey || event.metaKey) return;
      // An attempt that is running can have a password dialog open over the
      // popup, and a password has letters in it.
      if (typingIntoField(event.target)) return;
      const key = event.key.toLowerCase();
      if (key !== "r" && key !== "q" && key !== "s") return;
      event.preventDefault();
      event.stopPropagation();
      // Closing a session tears down an attempt still opening, so Reconnect
      // waits for the running one to answer rather than cancelling it.
      if (key === "r" && !reconnecting) onRetry(session.id);
      else if (key === "s" && retry?.next != null) onStopRetry(session.id);
      else if (key === "q") onQuit(session.id);
    };
    window.addEventListener("keydown", handler, { capture: true });
    return () => window.removeEventListener("keydown", handler, { capture: true });
  }, [focused, lost, reconnecting, retry?.next, session.id, onRetry, onStopRetry, onQuit]);

  // Quick select owns the keyboard while it is up, ahead of the terminal: a
  // label is letters, and letters would otherwise go to the shell. Lowercase
  // copies; Shift copies and pastes it at the prompt, which is the usual next
  // step. Anything that is not the next letter of some label closes it.
  useEffect(() => {
    if (!quick) return;
    const handler = (event: KeyboardEvent) => {
      if (["Shift", "Control", "Alt", "Meta", "CapsLock"].includes(event.key)) return;
      event.preventDefault();
      event.stopPropagation();
      const close = () => { setQuick(null); terminal.current?.focus(); };
      if (event.ctrlKey || event.altKey || event.metaKey || !/^[a-z]$/i.test(event.key)) return close();
      const typed = quick.typed + event.key.toLowerCase();
      const chosen = quick.targets.find(target => target.label === typed);
      if (chosen) {
        close();
        void navigator.clipboard.writeText(chosen.text).then(() => {
          onCopy();
          if (event.shiftKey) terminal.current?.paste(chosen.text);
        }).catch(() => {});
        return;
      }
      if (quick.targets.some(target => target.label.startsWith(typed))) setQuick({ ...quick, typed });
      else close();
    };
    window.addEventListener("keydown", handler, { capture: true });
    return () => window.removeEventListener("keydown", handler, { capture: true });
  }, [quick, onCopy]);

  useEffect(() => {
    if (!visible) return;
    requestAnimationFrame(() => {
      const term = terminal.current;
      if (!term) return;
      try {
        fit.current?.fit();
        term.refresh(0, term.rows - 1);
      } catch { /* not laid out yet */ }
      void resizeTerminal(session.id, term.cols, term.rows);
    });
  }, [visible, session.id]);

  // Declared after the effect that makes the watch, so a tab that opens
  // focused is armed from the start.
  useEffect(() => {
    if (focused) quietWatch.current?.arm();
  }, [focused]);

  useEffect(() => {
    if (!focused) return;
    requestAnimationFrame(() => terminal.current?.focus());
  }, [focused]);

  return <div
    ref={paneRoot}
    className={`terminal-pane ${visible ? "active" : ""} shade-${shade}`}
    style={style}
    aria-hidden={!visible}
    onPointerDownCapture={onFocus}
  >
    {finding && <div className="find-bar">
      <input
        autoFocus
        value={query}
        placeholder="Find in scrollback"
        onChange={event => { setQuery(event.target.value); find(event.target.value, 1, true); }}
        onKeyDown={event => {
          // The terminal is behind this, and the app's chords are above it.
          event.stopPropagation();
          if (event.key === "Escape") closeFind();
          else if (event.key === "Enter") find(query, event.shiftKey ? -1 : 1);
        }}
      />
      <span className={hits.count === 0 && query ? "none" : ""}>
        {query === "" ? "" : hits.count === 0 ? "no matches" : `${hits.index + 1} of ${hits.count}`}
      </span>
      <button title="Previous (Shift Enter)" disabled={!hits.count} onClick={() => find(query, -1)}><ChevronUp size={13}/></button>
      <button title="Next (Enter)" disabled={!hits.count} onClick={() => find(query, 1)}><ChevronDown size={13}/></button>
      <button title="Close (Escape)" onClick={closeFind}><X size={13}/></button>
    </div>}
    {pinned && <button
      className="pinned-command"
      title="Scroll back to this command"
      onClick={() => {
        const mark = pinnedMark.current;
        if (mark && mark.marker.line >= 0) terminal.current?.scrollToLine(mark.marker.line);
      }}
    ><ArrowUp size={12}/><span>{pinned}</span></button>}
    <div className="terminal-surface" ref={host} aria-label={`${session.name} terminal`}/>
    {hovered && <div className="link-hint" style={{ left: hovered.x, top: hovered.y }}>
      {hovered.refused
        ? <span className="refused">{hovered.refused}</span>
        : <span><kbd>Ctrl</kbd> click to open</span>}
      <span className="uri">{hovered.uri}</span>
    </div>}
    {quick && <div className="quick-select" role="dialog" aria-label="Quick select: type a label to copy, Shift to paste as well, Escape to close">
      {quick.targets.flatMap(target => target.boxes.map((box, index) =>
        <span key={`${target.label}-${box.top}-${box.left}-${index}`} className="quick-hit" style={box}/>))}
      {quick.targets.map(target => <span
        key={`${target.label}-${target.boxes[0].top}-${target.boxes[0].left}`}
        className={`quick-label ${target.label.startsWith(quick.typed) ? "" : "out"}`}
        style={{ left: target.boxes[0].left, top: target.boxes[0].top, height: target.boxes[0].height }}
      >{target.label.slice(quick.typed.length)}</span>)}
      <div className="quick-hint">Type a label to copy it · Shift to paste it too · Esc to close</div>
    </div>}
    {/* Over the terminal rather than replacing it: what was on screen when the
        connection went is usually the reason it went. */}
    {lost && <div className="session-lost" role="alertdialog" aria-label="Connection lost">
      <div>
        <strong><PlugZap size={15}/> Connection lost</strong>
        <p>{session.user ? `${session.user}@${session.host}` : session.name} stopped responding.</p>
        {retry && <p className="session-lost-retry" aria-live="polite">{retryStatus(retry, reconnecting, now)}</p>}
        {retry?.reason && <p className="session-lost-reason">{retry.reason}</p>}
        <div className="session-lost-actions">
          <button className="primary" disabled={reconnecting} onClick={() => onRetry(session.id)}>
            <kbd>R</kbd> {retry?.next != null ? "Reconnect now" : "Reconnect"}
          </button>
          {retry?.next != null &&
            <button className="secondary" onClick={() => onStopRetry(session.id)}><kbd>S</kbd> Stop retrying</button>}
          <button className="secondary" onClick={() => onQuit(session.id)}><kbd>Q</kbd> Close tab</button>
        </div>
      </div>
    </div>}
  </div>;
}

/** What the popup says about the automatic reconnect. */
function retryStatus(retry: Retrying, running: boolean, now: number): string {
  if (running) return `Reconnecting… attempt ${retry.attempts}`;
  if (retry.next !== null) {
    const seconds = Math.max(1, Math.ceil((retry.next - now) / 1000));
    return `Reconnecting in ${seconds}s${retry.attempts ? ` · ${retry.attempts} tried so far` : ""}`;
  }
  if (retry.halted === "timeout") return `Stopped trying after ${RETRY_FOR / 60_000} minutes.`;
  if (retry.halted === "user") return "Not reconnecting on its own.";
  return "";
}
