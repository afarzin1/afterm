import { describe, expect, it } from "vitest";
import { scaleForWidth } from "./scale";

describe("display scale", () => {
  it("leaves the size the chrome was drawn at alone", () => {
    expect(scaleForWidth(1920)).toBe(1);
    expect(scaleForWidth(1366)).toBe(1);
  });

  // The width is in CSS pixels, so Windows' own scaling has already been
  // applied. A 4K panel at 200% reports 1920 and must not be scaled twice.
  it("does not scale a display Windows has already scaled", () => {
    expect(scaleForWidth(1920)).toBe(1);
  });

  it("scales 1440p and 4K by their own steps", () => {
    expect(scaleForWidth(2560)).toBeGreaterThan(1);
    expect(scaleForWidth(3840)).toBeGreaterThan(scaleForWidth(2560));
  });

  it("never scales down, however narrow the display", () => {
    expect(scaleForWidth(1024)).toBe(1);
    expect(scaleForWidth(0)).toBe(1);
  });
});
