/**
 * Which remote files afterm can show without downloading them first.
 *
 * Images the webview draws itself, named by extension because that is all a
 * listing says about a file. What is shown is the file's own bytes in an
 * `<img>`, typed from this table, which is also the safety of it: an SVG drawn
 * as an image runs no script and loads nothing, whatever the host put in it.
 */

const IMAGE_TYPES: Record<string, string> = {
  png: "image/png",
  jpg: "image/jpeg",
  jpeg: "image/jpeg",
  gif: "image/gif",
  webp: "image/webp",
  avif: "image/avif",
  bmp: "image/bmp",
  ico: "image/x-icon",
  svg: "image/svg+xml"
};

/** Matched by the backend's own limit in `preview_remote_file`. */
export const PREVIEW_LIMIT_BYTES = 64 * 1024 * 1024;

/** The media type to show this file as, or null for a file that is not one. */
export function imageType(name: string): string | null {
  const dot = name.lastIndexOf(".");
  if (dot <= 0 || dot === name.length - 1) return null;
  return IMAGE_TYPES[name.slice(dot + 1).toLowerCase()] ?? null;
}

/** Whether a listing entry can be previewed: an image, and not too large. */
export function previewable(entry: { kind: string; name: string; size: number | null }): boolean {
  return entry.kind === "file" && imageType(entry.name) !== null
    && (entry.size === null || entry.size <= PREVIEW_LIMIT_BYTES);
}
