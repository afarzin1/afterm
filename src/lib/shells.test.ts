import { describe, expect, it } from "vitest";
import { LOCAL_SHELLS, isLocalShell, localSession } from "./shells";

describe("what counts as a shell on this machine", () => {
  it("names the two, and nothing else", () => {
    expect(isLocalShell("cmd")).toBe(true);
    expect(isLocalShell("powershell")).toBe(true);
    expect(isLocalShell("ssh")).toBe(false);
    expect(isLocalShell("ssh-cli")).toBe(false);
  });

  it("offers exactly the two the dialog has buttons for", () => {
    expect(LOCAL_SHELLS.map(shell => shell.kind)).toEqual(["cmd", "powershell"]);
    expect(LOCAL_SHELLS.every(shell => isLocalShell(shell.kind))).toBe(true);
  });
});

describe("opening one", () => {
  it("is a session with nowhere to go, named after the shell", () => {
    const session = localSession("powershell");
    expect(session.name).toBe("PowerShell");
    expect(session.kind).toBe("powershell");
    expect(session.state).toBe("connecting");
    // Empty rather than absent, so nothing downstream has to cope with a
    // missing field — and empty rather than plausible, so a screen that
    // forgets to ask `isLocalShell` shows nothing rather than a fiction.
    expect(session.host).toBe("");
    expect(session.user).toBe("");
  });

  it("is never the same session twice", () => {
    expect(localSession("cmd").id).not.toBe(localSession("cmd").id);
  });

  it("is not a saved host: it carries no profile to remember it by", () => {
    expect("profileId" in localSession("cmd")).toBe(false);
  });
});
