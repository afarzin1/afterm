import { execFileSync } from "node:child_process";
import { describe, expect, it } from "vitest";
import { directoryFromOsc, SHELL_INTEGRATION } from "./integration";

/** Whether a shell is on this machine, so its half can be checked for real. */
function available(shell: string): boolean {
  try {
    execFileSync("which", [shell], { stdio: "ignore" });
    return true;
  } catch {
    return false;
  }
}

describe("shell integration snippet", () => {
  it("sends the three marks the terminal reads", () => {
    // A opens a prompt, B ends it and opens the command, C says the command
    // has been submitted. The terminal bands what lies between B and C.
    for (const kind of ["A", "B", "C"]) {
      expect(SHELL_INTEGRATION, `mark ${kind}`).toContain(`133;${kind}`);
    }
  });

  it("says which directory the shell is in", () => {
    expect(SHELL_INTEGRATION).toContain("7;file://");
  });

  it("covers both shells a Linux host is likely to be running", () => {
    expect(SHELL_INTEGRATION).toContain("BASH_VERSION");
    expect(SHELL_INTEGRATION).toContain("ZSH_VERSION");
  });

  it("keeps the escapes as escapes rather than as control characters", () => {
    // It is pasted into a file through a clipboard and an editor, both of
    // which would eat a real escape byte. Everything here has to survive being
    // handled as plain text.
    // eslint-disable-next-line no-control-regex
    expect(/[\x00-\x08\x0b-\x1f]/.test(SHELL_INTEGRATION)).toBe(false);
  });

  it.runIf(available("bash"))("is something bash will parse", () => {
    // The real risk in a snippet full of nested quoting is that it does not
    // parse, and the person pasting it finds out at their next login.
    expect(() => execFileSync("bash", ["-n"], { input: SHELL_INTEGRATION })).not.toThrow();
  });

  it.runIf(available("zsh"))("is something zsh will parse", () => {
    expect(() => execFileSync("zsh", ["-n"], { input: SHELL_INTEGRATION })).not.toThrow();
  });

  it.runIf(available("bash"))("makes bash emit the marks around what is typed", () => {
    // The snippet, then a prompt expansion, printed with the escapes made
    // visible. `@P` is bash's own prompt expansion, which is what turns the
    // \\[ \\] wrappers and the escapes in PS1 into what a terminal receives.
    const script = `PS1='$ '\n${SHELL_INTEGRATION}\nprintf '%s' "\${PS1@P}" | cat -v\n`;
    const rendered = execFileSync("bash", [], { input: script, encoding: "utf8" });
    // A before the prompt text, B after it: that is what puts the band on the
    // command and leaves the prompt outside it.
    expect(rendered).toMatch(/133;A.*\$.*133;B/s);
  });

  it.runIf(available("bash"))("makes bash name the directory it is in", () => {
    // `cd` first, so the path in the prompt is one the test chose rather than
    // wherever the runner happens to be.
    const script = `PS1='$ '\n${SHELL_INTEGRATION}\ncd /etc\nprintf '%s' "\${PS1@P}" | cat -v\n`;
    const rendered = execFileSync("bash", [], { input: script, encoding: "utf8" });
    expect(rendered).toMatch(/7;file:\/\/[^/]*\/etc/);
  });

  it.runIf(available("zsh"))("makes zsh name the directory it is in", () => {
    const script = `${SHELL_INTEGRATION}\ncd /etc\n__afterm_precmd | cat -v\n`;
    const rendered = execFileSync("zsh", [], { input: script, encoding: "utf8" });
    expect(rendered).toMatch(/7;file:\/\/[^/]*\/etc/);
  });
});

describe("the directory an OSC 7 names", () => {
  it("reads the path out of a file URL", () => {
    expect(directoryFromOsc("file://holly/home/arya/src")).toBe("/home/arya/src");
    expect(directoryFromOsc("file:///home/arya")).toBe("/home/arya");
  });

  it("takes a bare path as written", () => {
    expect(directoryFromOsc("/var/log")).toBe("/var/log");
  });

  it("undoes percent-encoding, and leaves alone what is not encoded", () => {
    expect(directoryFromOsc("file://holly/home/arya/my%20files")).toBe("/home/arya/my files");
    expect(directoryFromOsc("file://holly/home/arya/100%")).toBe("/home/arya/100%");
  });

  /// `/etc/` and `/etc` are one directory, and following to both would be two
  /// moves for one `cd`.
  it("drops a trailing slash, except at the root", () => {
    expect(directoryFromOsc("file://holly/etc/")).toBe("/etc");
    expect(directoryFromOsc("file://holly/")).toBe("/");
  });

  it("says nothing when there is no path in it", () => {
    expect(directoryFromOsc("file://holly")).toBeNull();
    expect(directoryFromOsc("")).toBeNull();
    expect(directoryFromOsc("nonsense")).toBeNull();
  });
});
