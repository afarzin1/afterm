import type { Transferred } from "../types";

/** Placeholder for a value afterm could not measure. */
export const UNKNOWN = "—";

export function formatBytes(value: number | null | undefined): string {
  if (value === null || value === undefined || !Number.isFinite(value)) return UNKNOWN;
  const units = ["B", "KB", "MB", "GB", "TB", "PB"];
  let size = value;
  let unit = 0;
  while (size >= 1000 && unit < units.length - 1) {
    size /= 1000;
    unit += 1;
  }
  // Sub-10 values keep a decimal so a status bar reading does not jump between
  // "1 GB" and "2 GB" with nothing in between.
  const digits = unit === 0 || size >= 10 ? 0 : 1;
  return `${size.toFixed(digits)} ${units[unit]}`;
}

export function formatRate(bytesPerSecond: number | null | undefined): string {
  if (bytesPerSecond === null || bytesPerSecond === undefined) return UNKNOWN;
  return `${formatBytes(bytesPerSecond)}/s`;
}

export function formatPercent(value: number | null | undefined): string {
  if (value === null || value === undefined || !Number.isFinite(value)) return UNKNOWN;
  return `${Math.round(value)}%`;
}

export function formatLatency(milliseconds: number | null | undefined): string {
  if (milliseconds === null || milliseconds === undefined) return UNKNOWN;
  return `${Math.round(milliseconds)} ms`;
}

/** Compact uptime, e.g. `up 18d 4h`, `up 47m`. */
export function formatUptime(seconds: number | null | undefined): string {
  if (seconds === null || seconds === undefined || seconds < 0) return UNKNOWN;
  const days = Math.floor(seconds / 86400);
  const hours = Math.floor((seconds % 86400) / 3600);
  const minutes = Math.floor((seconds % 3600) / 60);
  if (days > 0) return `up ${days}d ${hours}h`;
  if (hours > 0) return `up ${hours}h ${minutes}m`;
  return `up ${minutes}m`;
}

export function formatRatio(used: number | null, total: number | null): string {
  if (used === null || total === null || total === 0) return UNKNOWN;
  return formatPercent((used / total) * 100);
}

/** Join a directory and a child name for display, for either path flavour. */
export function joinPath(directory: string, name: string): string {
  const separator = directory.includes("\\") && !directory.includes("/") ? "\\" : "/";
  const trimmed = directory.endsWith(separator) ? directory.slice(0, -1) : directory;
  return `${trimmed}${separator}${name}`;
}

/**
 * What is wrong with a name typed into the file pane, or null when nothing is.
 *
 * The new-folder and rename editors ask for a name inside the directory on
 * screen, not a path: a separator would quietly put the result somewhere other
 * than where it appears to be going, and `..` would put it above.
 */
export function nameProblem(name: string): string | null {
  if (!name) return "A name cannot be empty.";
  if (/[\\/]/.test(name)) return "A name cannot contain a path separator.";
  if (name === "." || name === "..") return `"${name}" is not a name.`;
  return null;
}

/** Last path segment, used to label the current directory. */
export function baseName(path: string): string {
  const parts = path.split(/[\\/]/).filter(Boolean);
  return parts[parts.length - 1] ?? path;
}

/**
 * A path split where it is allowed to wrap: after a separator, and nowhere
 * else.
 *
 * To a browser a path is one unbroken word, so a line holding one either
 * overflows or is broken mid-name by `overflow-wrap` — `/home/ar` / `ya`. The
 * pieces here are rendered with a `<wbr>` between them, which gives the line
 * the only break opportunities it should have. The separator stays on the end
 * of the piece it closes rather than opening the next line, so a wrapped path
 * reads as `/home/` then `arya/`, the way it is written.
 *
 * Both flavours are handled at once, deliberately: the separator is whatever
 * the host returned, and a Windows machine browsing a Linux host is looking at
 * forward slashes. A run of them stays together, so a UNC prefix `\\server` is
 * one piece and not an empty first line.
 */
export function pathBreaks(path: string): string[] {
  return path.match(/[^\\/]*[\\/]+|[^\\/]+/g) ?? [];
}

/**
 * The pieces to show when the whole path will not fit, with `..` standing in
 * for the head that was dropped.
 *
 * The tail is the part worth keeping: the directory you are in, and the one
 * above it, say where you are far better than the root does. So a path too
 * long to show is cut from the front — `/srv/deployments/releases/config`
 * becomes `../releases/config` — rather than trailed off at the end, where an
 * ellipsis would hide the only part being read.
 *
 * The separator after the `..` is taken from the last piece dropped, because
 * that piece ends with the separator this path is written in, and a Windows
 * path elided with a forward slash would be a small lie about the host.
 *
 * `drop` is clamped so the last piece always survives: a path is allowed to
 * overflow its line, and a pane that showed only `..` would say nothing.
 */
export function elidePath(pieces: string[], drop: number): string[] {
  if (drop <= 0 || pieces.length === 0) return pieces;
  const cut = Math.min(drop, pieces.length - 1);
  const separator = pieces[cut - 1].endsWith("\\") ? "\\" : "/";
  return [`..${separator}`, ...pieces.slice(cut)];
}

/**
 * A transfer report with what did not make it added to the end of it.
 *
 * A folder is not a transfer that either worked or did not. Links are never
 * followed, and one file that will not go costs that file rather than the
 * tree, so the honest report is what went and what did not. A line that said
 * only where the folder ended up would be claiming a whole copy of something
 * that might be missing half of it.
 */
export function withLosses(head: string, skipped: number, failed: number): string {
  const notes: string[] = [];
  if (skipped > 0) notes.push(`${skipped} ${skipped === 1 ? "entry" : "entries"} skipped`);
  if (failed > 0) notes.push(`${failed} could not be copied`);
  return notes.length > 0 ? `${head} — ${notes.join(", ")}` : head;
}

/**
 * What to say about a folder that has just been transferred.
 *
 * Both names appear — the one it had and the path it went to — because they
 * are not always the same: a second copy of `reports` lands in `reports (1)`,
 * and a name the other side cannot spell arrives spelled differently.
 */
export function transferSummary(
  verb: "Downloaded" | "Uploaded", name: string, result: Transferred
): string {
  const head = result.files === 0
    ? `${verb} ${name} to ${result.path}, which had no files in it`
    : `${verb} ${result.files === 1 ? "1 file" : `${result.files} files`}`
      + ` from ${name} to ${result.path}`;
  return withLosses(head, result.skipped, result.failed);
}
