import { describe, expect, it } from "vitest";
import { typingIntoField } from "./keys";

/** An element, to the extent this rule looks at one. */
const element = (tagName: string, over: Record<string, unknown> = {}) => ({
  tagName,
  closest: () => null,
  ...over
}) as unknown as EventTarget;

describe("what counts as typing", () => {
  it("says yes to the fields a chord must not steal from", () => {
    expect(typingIntoField(element("INPUT"))).toBe(true);
    expect(typingIntoField(element("TEXTAREA"))).toBe(true);
    expect(typingIntoField(element("DIV", { isContentEditable: true }))).toBe(true);
  });

  it("says no to a focused terminal, textarea and all", () => {
    // The bug this rule was written for: xterm's helper textarea looks like a
    // field, so every arrow chord was handed to the shell instead of claimed.
    const helper = element("TEXTAREA", {
      closest: (selector: string) => selector === ".xterm" ? {} : null
    });
    expect(typingIntoField(helper)).toBe(false);
  });

  it("says no to everything that is not a field", () => {
    expect(typingIntoField(element("BUTTON"))).toBe(false);
    expect(typingIntoField(element("DIV"))).toBe(false);
    expect(typingIntoField(null)).toBe(false);
    // A target that is not an element at all — window, or a document.
    expect(typingIntoField({} as EventTarget)).toBe(false);
  });
});
