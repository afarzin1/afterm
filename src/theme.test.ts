import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";
import { CARBON, STOCKHOLM, THEMES, terminalPalette, type Theme } from "./theme";
import { SHADE_COUNT } from "./lib/workspaces";

const css = readFileSync(new URL("./styles.css", import.meta.url), "utf8");

/**
 * Custom properties the interface sets on one element rather than the theme
 * setting them on the document. A theme must not be expected to supply these.
 */
const PER_ELEMENT = new Set(["--band-from"]);

/**
 * Metrics rather than colours: sizes the stylesheet declares in its own
 * `:root` and `scale.ts` adjusts for the display. A theme has no opinion on
 * how large the file pane is, so it must not be asked for one — and the prefix
 * is the rule, so a new metric needs no edit here.
 */
const METRIC = /^--ui-/;

/** Every `var(--x)` in the stylesheet that has no fallback to fall back on. */
function required(): Set<string> {
  const names = new Set<string>();
  for (const match of css.matchAll(/var\((--[a-z0-9-]+)\s*([,)])/g)) {
    const answered = PER_ELEMENT.has(match[1]) || METRIC.test(match[1]);
    if (match[2] === ")" && !answered) names.add(match[1]);
  }
  return names;
}

const channels = (colour: string) => {
  const match = /^#([0-9a-f]{2})([0-9a-f]{2})([0-9a-f]{2})/.exec(colour);
  expect(match, `${colour} is not a #rrggbb colour`).not.toBeNull();
  return [match![1], match![2], match![3]].map(part => parseInt(part, 16));
};


/** WCAG relative luminance, which is what a contrast ratio is built out of. */
function luminance(colour: string): number {
  const [red, green, blue] = channels(colour).map(value => {
    const part = value / 255;
    return part <= 0.03928 ? part / 12.92 : ((part + 0.055) / 1.055) ** 2.4;
  });
  return 0.2126 * red + 0.7152 * green + 0.0722 * blue;
}

function contrast(a: string, b: string): number {
  const [high, low] = [luminance(a), luminance(b)].sort((x, y) => y - x);
  return (high + 0.05) / (low + 0.05);
}

/** The eight slots, each with the bright that has to sit above it. */
const ANSI_PAIRS = [
  "black", "red", "green", "yellow", "blue", "magenta", "cyan", "white"
] as const;

const brightOf = (name: string) => `bright${name[0].toUpperCase()}${name.slice(1)}`;

describe("themes", () => {
  it.each(THEMES.map(theme => [theme.id, theme] as const))(
    "%s answers every custom property the stylesheet reads",
    (_id, theme: Theme) => {
      for (const token of required()) {
        expect(Object.keys(theme.chrome)).toContain(token);
      }
    }
  );

  it("has no token the stylesheet never reads", () => {
    // Read from TypeScript rather than CSS: the settings panel paints a swatch
    // of each scheme in its own colours.
    const inSwatch = ["--bg", "--raise-2", "--text", "--accent", "--blue", "--band"];
    const wanted = required();
    for (const token of Object.keys(THEMES[0].chrome)) {
      expect(wanted.has(token) || inSwatch.includes(token), `${token} is unused`).toBe(true);
    }
  });

  it("matches the terminal ground to the chrome, so the window is one surface", () => {
    for (const theme of THEMES) {
      expect(theme.terminal.background).toBe(theme.chrome["--bg"]);
    }
  });

  it("matches the other grounds too, so a shaded pane is one surface as well", () => {
    for (const theme of THEMES) {
      expect(theme.grounds).toEqual([
        theme.chrome["--bg"], theme.chrome["--bg-alt"], theme.chrome["--bg-alt-2"]
      ]);
    }
  });

  it("has one ground for every shade the layout hands out", () => {
    // The two are counted in different files and have to agree: a shade with
    // no ground behind it is a pane painted on the plain one, beside a
    // neighbour that already has it.
    for (const theme of THEMES) {
      expect(theme.grounds, theme.id).toHaveLength(SHADE_COUNT);
    }
  });

  it("steps the grounds down evenly, and only just", () => {
    for (const theme of THEMES) {
      const steps = theme.grounds.map(channels);
      for (let index = 1; index < steps.length; index += 1) {
        for (const channel of [0, 1, 2]) {
          expect(
            steps[index][channel],
            `${theme.id} ground ${index} channel ${channel}`
          ).toBeLessThan(steps[index - 1][channel]);
        }
        // Far enough apart that two panes read as two, close enough that the
        // window still reads as one slab rather than a set of boxes. Held to
        // the same drop at every step so the darkest ground is as far from the
        // theme as the scheme can afford and no further.
        const drop = steps[index - 1][1] - steps[index][1];
        expect(drop, theme.id).toBeGreaterThanOrEqual(4);
        expect(drop, theme.id).toBeLessThanOrEqual(8);
      }
    }
  });

  it("keeps every ground off the ANSI black programs paint with", () => {
    // Black is the slot a program reaches for as a background. A ground equal
    // to it would swallow every black cell painted on that pane.
    for (const theme of THEMES) {
      for (const ground of theme.grounds) {
        expect([theme.id, ground]).not.toEqual([theme.id, theme.terminal.black]);
      }
    }
  });

  it("paints a shaded terminal on its own ground, cursor included", () => {
    for (const theme of THEMES) {
      expect(terminalPalette(theme, 0)).toBe(theme.terminal);
      for (const shade of [1, 2]) {
        const shaded = terminalPalette(theme, shade);
        expect(shaded.background).toBe(theme.grounds[shade]);
        // The ink inside the cursor block is the ground it stands on, or the
        // cursor reads as a smudge instead of a hole in the text.
        expect(shaded.cursorAccent).toBe(theme.grounds[shade]);
        expect(shaded.foreground).toBe(theme.terminal.foreground);
      }
      // A shade with no ground behind it is the plain one rather than a
      // window that fails to draw.
      expect(terminalPalette(theme, SHADE_COUNT)).toBe(theme.terminal);
    }
  });
});

describe("carbon", () => {
  /**
   * The chrome, and only the chrome.
   *
   * This used to be the chrome and the terminal together, when the scheme was
   * grey to the last value. The sixteen ANSI slots carry hue now — six of them
   * are warm — because their job is semantic and grey cannot do it: brightness
   * can say two things differ, not which of them is the error. The rule is
   * given up there on purpose and kept everywhere else, which is where this
   * scheme's identity lives.
   */
  const greys = Object.values(CARBON.chrome);

  /** Pure white and pure black have no room for a tint and anchor the ramp. */
  const anchored = (colour: string) => {
    const [red, green, blue] = channels(colour);
    return red === green && green === blue && (red === 0 || red === 255);
  };

  it("has chrome that is never warm, which is what neutral grey looked like", () => {
    // R = G = B is grey by the numbers and brown to the eye. Every value here
    // leans the other way instead: red lowest, blue highest, never reversed.
    for (const colour of greys) {
      const [red, green, blue] = channels(colour);
      expect([colour, red <= green, green <= blue]).toEqual([colour, true, true]);
    }
  });

  it("keeps the chrome's blue far too faint to name a colour", () => {
    for (const colour of greys) {
      if (anchored(colour)) continue;
      const [red, green, blue] = channels(colour);
      // Stockholm's ground spreads 18 between red and blue and reads as blue.
      // A quarter of that reads as grey, which is the whole point.
      expect(blue - red, colour).toBeGreaterThanOrEqual(4);
      expect(blue - red, colour).toBeLessThanOrEqual(10);
      // Green sits under half way up, which is what holds the hue steady
      // instead of letting it drift towards cyan.
      expect(green - red, colour).toBeGreaterThan(0);
      expect(green - red, colour).toBeLessThan(blue - green + 2);
    }
  });

  it("lets the terminal carry the hue the chrome may not", () => {
    // Stated as the thing that would be lost if someone flattened the palette
    // back to grey: an error and a success have to differ by something other
    // than one of them being lighter.
    // Bright white is the exception and always was: it is #ffffff, the cap on
    // the ramp, and pure white has nowhere to put a hue.
    const slots = ANSI_PAIRS
      .flatMap(name => [name, brightOf(name)])
      .filter(name => name !== "brightWhite");
    for (const name of slots) {
      const [red, green, blue] = channels(String(CARBON.terminal[name as "red"]));
      expect([name, red !== green || green !== blue]).toEqual([name, true]);
    }

    // And still held well back: none of them is Stockholm's value outright.
    for (const name of ANSI_PAIRS) {
      expect([name, CARBON.terminal[name as "red"]])
        .not.toEqual([name, STOCKHOLM.terminal[name as "red"]]);
    }
  });
});

describe("the sixteen ANSI slots", () => {
  it("keeps every bright above its own normal, in every scheme", () => {
    // A bright is the same colour, lighter. Not a second, louder one — which
    // is what a full-saturation palette turns it into.
    for (const theme of THEMES) {
      for (const name of ANSI_PAIRS) {
        const normal = String(theme.terminal[name as "red"]);
        const bright = String(theme.terminal[brightOf(name) as "brightRed"]);
        expect(
          [theme.id, name, luminance(bright) > luminance(normal)]
        ).toEqual([theme.id, name, true]);
      }
    }
  });

  it("keeps every slot readable on its own ground, in every scheme", () => {
    // The job of the sixteen is semantic, and a slot that cannot be read
    // cannot do it. The Terminus values these replaced put Stockholm's error
    // red at 2.79 to one — the least legible colour in the terminal — and
    // Carbon's own blue sat at 3.64, with its comment grey at 2.62.
    //
    // Black is left out on purpose: it is darker than the ground because that
    // is what it is for, and it is the one slot programs reach for as a
    // background rather than as ink.
    const painted = ["background", "cursorAccent", "selectionBackground", "black"];
    for (const theme of THEMES) {
      const ground = theme.terminal.background!;
      for (const [name, colour] of Object.entries(theme.terminal)) {
        if (painted.includes(name) || typeof colour !== "string") continue;
        expect([theme.id, name, contrast(colour, ground) >= 4.5])
          .toEqual([theme.id, name, true]);
      }
    }
  });

  it("keeps ANSI green off the chrome's accent", () => {
    // A prompt the colour of the tab underline and the primary button reads as
    // selected rather than as a prompt.
    expect(STOCKHOLM.terminal.green).not.toBe(STOCKHOLM.chrome["--accent"]);
    expect(STOCKHOLM.terminal.green).not.toBe(STOCKHOLM.chrome["--accent-bright"]);
  });
});
