import { describe, expect, it } from "vitest";
import { copiesOnRelease, copiesOnSelect } from "./selection";

describe("when letting go of the mouse copies the selection", () => {
  it("copies a selection dragged in this pane, with the setting on", () => {
    expect(copiesOnRelease(true, true, true)).toBe(true);
  });

  it("copies nothing while the setting is off", () => {
    expect(copiesOnRelease(false, true, true)).toBe(false);
  });

  /// A release that no drag here began is a click somewhere else in the
  /// window, and a pane can be holding a selection from minutes ago. Copying
  /// then would overwrite a clipboard the user has filled since.
  it("ignores a release that began outside this pane", () => {
    expect(copiesOnRelease(true, false, true)).toBe(false);
  });

  /// The click that clears a selection is a release like any other, and must
  /// not be followed by a write.
  it("leaves the clipboard alone when nothing is selected", () => {
    expect(copiesOnRelease(true, true, false)).toBe(false);
  });
});

describe("reading copy-on-select back from the settings store", () => {
  /// The default, and the only state the great majority of machines are in:
  /// nothing stored because nobody has opened the settings panel.
  it("is on when nothing has been stored", () => {
    expect(copiesOnSelect(null)).toBe(true);
  });

  it("is on when it was stored on", () => {
    expect(copiesOnSelect("true")).toBe(true);
  });

  /// The one state that has to survive a restart: a user who went looking for
  /// this setting and turned it off meant it.
  it("is off when it was turned off", () => {
    expect(copiesOnSelect("false")).toBe(false);
  });
});
