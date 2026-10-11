import type { Terminal } from "@xterm/xterm";

/// A tiny shell used only by `npm run dev`, where there is no Rust backend and
/// therefore no PTY.
///
/// It is deliberately labelled as afterm's own preview shell rather than
/// dressed up as a real machine: it has no hostname, no user, and no working
/// directory, so nothing it prints can be mistaken for the state of an actual
/// system. Its purpose is to exercise the terminal chrome — input highlighting,
/// the pinned command line, scrollback — while iterating on the interface.

/// Palette indices, not truecolour: the preview shell is what the terminal
/// half of a colour scheme is looked at through, so it has to follow the
/// scheme rather than hold Stockholm's values in escape sequences.
const PROMPT = "\x1b[32mafterm-preview\x1b[0m$ ";

const BACKSPACE = "\u007f";

const HELP = [
  "Commands in this preview shell:",
  "  lines <n>   print n numbered lines, for testing scrollback and pinning",
  "  colors      print the 16 ANSI colours",
  "  ls          a coloured listing, the way `ls --color` paints one",
  "  git         a `git status`, the way git paints one",
  "  marks       shell integration on or off, for the input band",
  "  clear       clear the screen",
  "  help        this list"
];

/**
 * A colour scheme is judged on output, not on a swatch row.
 *
 * `colors` says whether sixteen slots are distinguishable; it says nothing
 * about whether they do their job. These two are the job: a listing has to let
 * directories recede while archives and executables stay apart, and a status
 * has to keep staged and unstaged from reading as the same thing. Both use
 * palette indices only, so they show whatever scheme is loaded rather than
 * carrying one of them in an escape sequence.
 */
const LISTING = [
  ["34;1", "deployments"], ["34;1", "releases"], ["34;1", "node_modules"],
  ["36;1", "current -> releases/2026-09-04"],
  ["32;1", "deploy.sh"], ["32;1", "rollback.sh"],
  ["31;1", "backup-2026-08-30.tar.gz"], ["35;1", "banner.png"],
  ["0", "README.md"], ["0", "package.json"], ["0", "afterm.log"]
];

const STATUS = [
  "On branch \x1b[32mmain\x1b[0m",
  "Your branch is ahead of \x1b[31morigin/main\x1b[0m by 2 commits.",
  "",
  "Changes to be committed:",
  "  \x1b[90m(use \"git restore --staged <file>...\" to unstage)\x1b[0m",
  "        \x1b[32mnew file:   src/lib/integration.ts\x1b[0m",
  "        \x1b[32mmodified:   src/theme.ts\x1b[0m",
  "",
  "Changes not staged for commit:",
  "  \x1b[90m(use \"git add <file>...\" to update what will be committed)\x1b[0m",
  "        \x1b[31mmodified:   src/styles.css\x1b[0m",
  "        \x1b[31mdeleted:    src/lib/localfs.ts\x1b[0m",
  "",
  "Untracked files:",
  "        \x1b[31mgrok_color_review.md\x1b[0m"
];

/**
 * OSC 133, the way a shell with integration installed sends it.
 *
 * Off by default, so the preview exercises the keystroke heuristic — which is
 * what a host without the snippet gets, and the path most sessions are on.
 * `marks` turns it on, and then the band should sit on the command alone with
 * the prompt outside it. It is the only way to see either path without a real
 * shell on the other end.
 */
const mark = (kind: "A" | "B" | "C", on: boolean) => (on ? `\x1b]133;${kind}\x07` : "");

export function startPreviewShell(term: Terminal) {
  term.writeln("\x1b[32mafterm preview shell\x1b[0m");
  term.writeln(
    "\x1b[90mNo PTY is attached. Run `npm run tauri dev` for real shells.\x1b[0m"
  );
  term.writeln("");

  let marks = false;
  const prompt = () => term.write(`${mark("A", marks)}${PROMPT}${mark("B", marks)}`);
  prompt();

  let line = "";
  term.onData(data => {
    if (data === "\r") {
      const input = line.trim();
      line = "";
      term.write("\r\n");
      // After the newline, which is where a `preexec` hook runs.
      term.write(mark("C", marks));
      if (input === "marks") {
        marks = !marks;
        term.writeln(`shell integration ${marks ? "on" : "off"}`);
      } else {
        run(term, input);
      }
      prompt();
      return;
    }
    if (data === BACKSPACE) {
      if (line.length > 0) {
        line = line.slice(0, -1);
        term.write("\b \b");
      }
      return;
    }
    if (data < " " && data !== "\t") return;
    line += data;
    term.write(data);
  });
}

function run(term: Terminal, input: string) {
  const [command, argument] = input.split(/\s+/);
  switch (command) {
    case "":
      return;
    case "help":
      HELP.forEach(text => term.writeln(text));
      return;
    case "clear":
      term.clear();
      return;
    case "ls":
      for (const [colour, name] of LISTING) {
        term.write(`\x1b[${colour}m${name}\x1b[0m  `);
      }
      term.writeln("");
      return;
    case "git":
      STATUS.forEach(text => term.writeln(text));
      return;
    case "colors":
      for (let color = 0; color < 16; color += 1) {
        term.write(`\x1b[${color < 8 ? 30 + color : 90 + color - 8}mcolor ${color}\x1b[0m  `);
      }
      term.writeln("");
      return;
    case "lines": {
      const count = Math.min(2000, Math.max(1, Number(argument) || 40));
      for (let index = 1; index <= count; index += 1) {
        term.writeln(`  line ${String(index).padStart(4, " ")} of ${count}`);
      }
      return;
    }
    default:
      term.writeln(`\x1b[91mpreview shell: unknown command: ${command}\x1b[0m`);
  }
}
