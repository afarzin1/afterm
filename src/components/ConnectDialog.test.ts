import { describe, expect, it } from "vitest";
import { hostAddress, hostForKey, hostNumber, openingTab } from "./ConnectDialog";
import type { SessionProfile } from "../types";

const host = (id: string, extra: Partial<SessionProfile> = {}): SessionProfile => ({
  id, name: id, kind: "ssh", host: `${id}.example.com`, user: "deploy", port: 22, ...extra
});

const many = Array.from({ length: 12 }, (_, index) => host(`h${index + 1}`));

describe("which tab the connect dialog opens on", () => {
  it("opens on the saved hosts, because that is where sessions come from", () => {
    expect(openingTab([host("prod")])).toBe("hosts");
  });

  it("opens on the form when there is no list to open on", () => {
    expect(openingTab([])).toBe("new");
  });
});

describe("the numbers beside saved hosts", () => {
  it("numbers from one, the way the keys are labelled", () => {
    expect(hostNumber(0)).toBe(1);
    expect(hostNumber(8)).toBe(9);
  });

  it("stops at nine rather than inventing a key nobody can press", () => {
    expect(hostNumber(9)).toBeNull();
    expect(hostNumber(40)).toBeNull();
  });

  it("opens the host its number is written beside", () => {
    expect(hostForKey(many, "1")).toBe(many[0]);
    expect(hostForKey(many, "9")).toBe(many[8]);
  });

  it("does nothing for a key past the end of the list", () => {
    expect(hostForKey([host("prod")], "7")).toBeUndefined();
  });

  it("leaves every other key alone, including the zero and the letters", () => {
    // `0` has no row, and a letter typed at the list is not a shortcut for the
    // host that happens to sit tenth.
    expect(hostForKey(many, "0")).toBeUndefined();
    expect(hostForKey(many, "a")).toBeUndefined();
    expect(hostForKey(many, "Enter")).toBeUndefined();
  });

  it("never reaches past the ninth, even when the list is longer", () => {
    const reachable = many.filter((_, index) => hostNumber(index) !== null);
    expect(reachable).toHaveLength(9);
    expect(reachable.every((profile, index) => hostForKey(many, String(index + 1)) === profile))
      .toBe(true);
  });
});

describe("how a saved host is addressed", () => {
  it("leaves the default port off, because writing it says nothing", () => {
    expect(hostAddress(host("prod"))).toBe("deploy@prod.example.com");
  });

  it("carries a port that is not the default", () => {
    expect(hostAddress(host("prod", { port: 2222 }))).toBe("deploy@prod.example.com:2222");
  });
});
