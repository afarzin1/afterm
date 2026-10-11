import { describe, expect, it } from "vitest";
import { carriedLines, type BufferLike } from "./carry";

/** Rows as xterm holds them: text padded to the width, and whether each wraps. */
function buffer(rows: Array<[string, boolean]>, width = 10): BufferLike {
  return {
    length: rows.length,
    getLine: index => {
      const row = rows[index];
      if (!row) return undefined;
      const [text, isWrapped] = row;
      const padded = text.padEnd(width);
      return { isWrapped, translateToString: trim => trim ? padded.trimEnd() : padded };
    }
  };
}

describe("carrying a terminal's text into its replacement", () => {
  it("keeps each line and drops the empty rows under the last one", () => {
    expect(carriedLines(buffer([["$ ls", false], ["a  b", false], ["", false], ["", false]])))
      .toEqual(["$ ls", "a  b"]);
  });

  /// Joined, so the new terminal wraps it at its own width rather than
  /// keeping a hard break at the old one.
  it("joins a wrapped line back together, keeping a space at the wrap", () => {
    expect(carriedLines(buffer([["error: con", false], ["nection re", true], ["fused", true]])))
      .toEqual(["error: connection refused"]);
    expect(carriedLines(buffer([["one two  ", false], ["three", true]])))
      .toEqual(["one two   three"]);
  });

  it("keeps blank lines between output", () => {
    expect(carriedLines(buffer([["a", false], ["", false], ["b", false]]))).toEqual(["a", "", "b"]);
  });

  it("carries nothing from an empty terminal", () => {
    expect(carriedLines(buffer([["", false], ["", false]]))).toEqual([]);
  });
});
