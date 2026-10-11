import { describe, expect, it } from "vitest";
import {
  UNKNOWN, baseName, formatBytes, formatLatency, formatPercent, formatRatio,
  formatRate, formatUptime, joinPath, elidePath, nameProblem, pathBreaks, transferSummary
} from "./format";
import { clampFontSize, MAX_FONT_SIZE, MIN_FONT_SIZE, stepFontSize } from "../theme";

describe("metric formatting", () => {
  it("shows an em dash rather than inventing a number", () => {
    // Every metric is nullable on purpose: an unmeasured value must never
    // render as 0, which would read as a real reading.
    expect(formatBytes(null)).toBe(UNKNOWN);
    expect(formatPercent(null)).toBe(UNKNOWN);
    expect(formatRate(null)).toBe(UNKNOWN);
    expect(formatLatency(null)).toBe(UNKNOWN);
    expect(formatUptime(null)).toBe(UNKNOWN);
    expect(formatRatio(null, null)).toBe(UNKNOWN);
    expect(formatBytes(undefined)).toBe(UNKNOWN);
    expect(formatPercent(Number.NaN)).toBe(UNKNOWN);
  });

  it("distinguishes zero from unknown", () => {
    expect(formatPercent(0)).toBe("0%");
    expect(formatBytes(0)).toBe("0 B");
  });

  it("scales byte counts", () => {
    expect(formatBytes(512)).toBe("512 B");
    expect(formatBytes(1500)).toBe("1.5 KB");
    expect(formatBytes(8_000_000_000)).toBe("8.0 GB");
    expect(formatBytes(45_000_000_000)).toBe("45 GB");
  });

  it("formats rates and ratios", () => {
    expect(formatRate(124_000)).toBe("124 KB/s");
    expect(formatRatio(42, 100)).toBe("42%");
    expect(formatRatio(1, 0)).toBe(UNKNOWN);
  });

  it("formats uptime at day, hour and minute scales", () => {
    expect(formatUptime(18 * 86400 + 4 * 3600)).toBe("up 18d 4h");
    expect(formatUptime(5 * 3600 + 30 * 60)).toBe("up 5h 30m");
    expect(formatUptime(47 * 60)).toBe("up 47m");
    expect(formatUptime(-1)).toBe(UNKNOWN);
  });
});

describe("path helpers", () => {
  it("joins posix and windows paths with the right separator", () => {
    expect(joinPath("/var/www", "app")).toBe("/var/www/app");
    expect(joinPath("/var/www/", "app")).toBe("/var/www/app");
    expect(joinPath("C:\\Users\\arya", "notes.txt")).toBe("C:\\Users\\arya\\notes.txt");
  });

  it("takes the last segment of either flavour", () => {
    expect(baseName("/var/www/app")).toBe("app");
    expect(baseName("C:\\Users\\arya\\notes.txt")).toBe("notes.txt");
    expect(baseName("/")).toBe("/");
  });
});

describe("terminal font size", () => {
  it("clamps to the supported range", () => {
    expect(clampFontSize(1)).toBe(MIN_FONT_SIZE);
    expect(clampFontSize(999)).toBe(MAX_FONT_SIZE);
    expect(clampFontSize(14)).toBe(14);
    // A stored value that is not a number must not poison the terminal.
    expect(clampFontSize(Number.NaN)).toBe(14);
  });

  it("rounds fractional sizes rather than passing them through", () => {
    expect(clampFontSize(13.4)).toBe(13);
    expect(clampFontSize(13.6)).toBe(14);
  });

  it("steps by one and stops at both ends", () => {
    expect(stepFontSize(14, 1)).toBe(15);
    expect(stepFontSize(14, -1)).toBe(13);
    expect(stepFontSize(MIN_FONT_SIZE, -1)).toBe(MIN_FONT_SIZE);
    expect(stepFontSize(MAX_FONT_SIZE, 1)).toBe(MAX_FONT_SIZE);
  });
});

describe("where a path may break", () => {
  it("breaks after each separator, never inside a name", () => {
    expect(pathBreaks("/home/arya/_sandbox")).toEqual(["/", "home/", "arya/", "_sandbox"]);
  });

  it("handles the other separator, because the host chooses it", () => {
    expect(pathBreaks("C:\\Users\\arya\\Documents"))
      .toEqual(["C:\\", "Users\\", "arya\\", "Documents"]);
  });

  it("keeps a run of separators together, so a UNC prefix is one piece", () => {
    expect(pathBreaks("\\\\server\\share\\logs"))
      .toEqual(["\\\\", "server\\", "share\\", "logs"]);
  });

  it("leaves a root, a bare name and an empty path alone", () => {
    expect(pathBreaks("/")).toEqual(["/"]);
    expect(pathBreaks("afterm")).toEqual(["afterm"]);
    expect(pathBreaks("")).toEqual([]);
  });

  it("loses nothing: the pieces rejoin into the path they came from", () => {
    for (const path of ["/", "/var/log/nginx/access.log", "C:\\", "relative/thing", "/a//b/"]) {
      expect(pathBreaks(path).join("")).toBe(path);
    }
  });
});

describe("cutting a path down from the front", () => {
  const unix = () => pathBreaks("/srv/deployments/releases/config");

  it("shows the whole path when nothing needs dropping", () => {
    expect(elidePath(unix(), 0)).toEqual(["/", "srv/", "deployments/", "releases/", "config"]);
  });

  it("keeps the tail, which is the part worth reading", () => {
    expect(elidePath(unix(), 3).join("")).toBe("../releases/config");
    expect(elidePath(unix(), 2).join("")).toBe("../deployments/releases/config");
  });

  it("elides a Windows path with a Windows separator", () => {
    const pieces = pathBreaks("C:\\Users\\arya\\AppData\\Roaming");
    expect(elidePath(pieces, 2).join("")).toBe("..\\arya\\AppData\\Roaming");
  });

  it("never drops the last piece, however hard it is pushed", () => {
    // A pane showing only `..` would say nothing at all; a name too wide for
    // the line is allowed to overflow it and scroll instead.
    expect(elidePath(unix(), 99).join("")).toBe("../config");
    expect(elidePath(unix(), 4).join("")).toBe("../config");
  });

  it("has nothing to do with an empty path", () => {
    expect(elidePath([], 3)).toEqual([]);
  });
});

describe("nameProblem", () => {
  it("accepts an ordinary name, in either path flavour", () => {
    expect(nameProblem("reports")).toBeNull();
    expect(nameProblem("customer-integrations")).toBeNull();
    expect(nameProblem("notes 2026.md")).toBeNull();
  });

  it("refuses a name that is really a path", () => {
    // Both editors ask for a name inside the directory on screen. A separator
    // would put the result somewhere other than where it appears to be going,
    // and the pane would show a directory that had not changed.
    expect(nameProblem("logs/today")).not.toBeNull();
    expect(nameProblem("logs\\today")).not.toBeNull();
    expect(nameProblem("/etc")).not.toBeNull();
  });

  it("refuses the two names that mean somewhere else", () => {
    expect(nameProblem(".")).not.toBeNull();
    expect(nameProblem("..")).not.toBeNull();
    // A leading dot is only a hidden file, which is a name like any other.
    expect(nameProblem(".config")).toBeNull();
  });

  it("refuses an empty name", () => {
    expect(nameProblem("")).not.toBeNull();
  });
});

describe("what a folder transfer reports", () => {
  const landed = { path: "/home/arya/Downloads/reports", files: 12, skipped: 0, failed: 0 };

  it("names both ends, because they are not always the same name", () => {
    expect(transferSummary("Downloaded", "reports", { ...landed, path: "/home/arya/Downloads/reports (1)" }))
      .toBe("Downloaded 12 files from reports to /home/arya/Downloads/reports (1)");
  });

  it("counts one file as one file", () => {
    expect(transferSummary("Downloaded", "reports", { ...landed, files: 1 }))
      .toBe("Downloaded 1 file from reports to /home/arya/Downloads/reports");
  });

  it("says what did not arrive rather than reporting a clean success", () => {
    // The case this exists for: a tree with a link in it and a file that will
    // not go still "succeeds", and a line that stopped at the path would be
    // telling the user they have a complete copy.
    expect(transferSummary("Downloaded", "reports", { ...landed, skipped: 2, failed: 1 }))
      .toBe(
        "Downloaded 12 files from reports to /home/arya/Downloads/reports"
        + " — 2 entries skipped, 1 could not be copied"
      );
    expect(transferSummary("Downloaded", "reports", { ...landed, skipped: 1 }))
      .toContain("1 entry skipped");
  });

  it("does not claim files for a folder that had none", () => {
    expect(transferSummary("Downloaded", "empty", { ...landed, files: 0 }))
      .toBe("Downloaded empty to /home/arya/Downloads/reports, which had no files in it");
  });
});
