import { describe, expect, it } from "vitest";
import { clipboardWrite } from "./osc52";

/** Base64 of `text` as UTF-8, the way a program on the host encodes it. */
function encode(text: string): string {
  return btoa(String.fromCharCode(...new TextEncoder().encode(text)));
}

describe("reading an OSC 52 clipboard write", () => {
  /// The whole point: a command far wider than the pane, copied as the one
  /// line it is, with no wrapping and no indent.
  it("decodes the text a program asked to copy", () => {
    const command = "az pipelines run --name afterm --branch main --parameters " + "x=1 ".repeat(60);
    expect(clipboardWrite(`c;${encode(command)}`)).toBe(command);
  });

  it("keeps text that is not ASCII", () => {
    expect(clipboardWrite(`c;${encode("échec — 失败 ✓")}`)).toBe("échec — 失败 ✓");
  });

  /// tmux sends an empty list, vim sends `+`'s `c` or `*`'s `p`; there is one
  /// clipboard here and they all mean it.
  it("takes every selection name as the clipboard", () => {
    for (const targets of ["", "p", "s", "cs", "0"]) {
      expect(clipboardWrite(`${targets};${encode("hello")}`)).toBe("hello");
    }
  });

  /// Answering would type this machine's clipboard into whatever host asked.
  it("never answers a query", () => {
    expect(clipboardWrite("c;?")).toBeNull();
  });

  it("leaves the clipboard alone for an empty write", () => {
    expect(clipboardWrite("c;")).toBeNull();
  });

  /// A sequence cut short or garbled in transit must not replace a good
  /// clipboard with junk.
  it("ignores what is not base64 of text", () => {
    expect(clipboardWrite("c;not base64!")).toBeNull();
    expect(clipboardWrite(`c;${btoa("\xff\xfe")}`)).toBeNull();
    expect(clipboardWrite("hello")).toBeNull();
    expect(clipboardWrite(`x;${encode("hello")}`)).toBeNull();
  });
});
