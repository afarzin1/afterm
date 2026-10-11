import { describe, expect, it } from "vitest";
import { imageType, previewable, PREVIEW_LIMIT_BYTES } from "./preview";

describe("which files can be previewed", () => {
  it("knows images by extension, whatever the case", () => {
    expect(imageType("chart.PNG")).toBe("image/png");
    expect(imageType("photo.jpeg")).toBe("image/jpeg");
    expect(imageType("logo.svg")).toBe("image/svg+xml");
  });

  it("does not take a dotfile, a bare dot or another type for an image", () => {
    expect(imageType(".png")).toBeNull();
    expect(imageType("image.")).toBeNull();
    expect(imageType("notes.txt")).toBeNull();
    expect(imageType("archive.png.gz")).toBeNull();
  });

  it("previews files only, and not ones past the limit", () => {
    expect(previewable({ kind: "file", name: "a.png", size: 1024 })).toBe(true);
    expect(previewable({ kind: "file", name: "a.png", size: null })).toBe(true);
    expect(previewable({ kind: "file", name: "a.png", size: PREVIEW_LIMIT_BYTES + 1 })).toBe(false);
    expect(previewable({ kind: "directory", name: "shots.png", size: null })).toBe(false);
    expect(previewable({ kind: "link", name: "a.png", size: null })).toBe(false);
  });
});
