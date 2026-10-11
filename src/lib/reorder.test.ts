import { describe, expect, it } from "vitest";
import { dropIndex } from "./reorder";

/// Four tabs, 100 wide, so the others sit at 50, 150, 250 and 350 depending on
/// which one is in hand.
describe("where a dragged tab lands", () => {
  it("stays put until the pointer passes a neighbour's midpoint", () => {
    expect(dropIndex([150, 250, 350], 140)).toBe(0);
    expect(dropIndex([150, 250, 350], 160)).toBe(1);
  });

  it("moves one place per neighbour crossed, never two", () => {
    expect(dropIndex([150, 250, 350], 260)).toBe(2);
    expect(dropIndex([150, 250, 350], 900)).toBe(3);
  });

  it("reads a drag to the left the same way", () => {
    expect(dropIndex([50, 150, 250], 240)).toBe(2);
    expect(dropIndex([50, 150, 250], 40)).toBe(0);
  });

  /// One tab is the whole strip once it is in hand, and there is nowhere for
  /// it to go.
  it("has one answer when nothing else is in the strip", () => {
    expect(dropIndex([], 1000)).toBe(0);
  });
});
