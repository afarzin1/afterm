import type { ITheme } from "@xterm/xterm";

/// JetBrains Mono is an outline face, so unlike a bitmap font it stays correct
/// at any size and the size control is a plain range.
export const MIN_FONT_SIZE = 9;
export const MAX_FONT_SIZE = 32;
export const DEFAULT_FONT_SIZE = 14;

export function clampFontSize(value: number): number {
  if (!Number.isFinite(value)) return DEFAULT_FONT_SIZE;
  return Math.min(MAX_FONT_SIZE, Math.max(MIN_FONT_SIZE, Math.round(value)));
}

export function stepFontSize(current: number, direction: 1 | -1): number {
  return clampFontSize(clampFontSize(current) + direction);
}

export const FONT_FAMILY = '"JetBrains Mono", Consolas, "Courier New", monospace';

/**
 * Every colour the chrome uses, named by the CSS custom property that carries
 * it.
 *
 * The keys are the CSS names, so there is no translation step and no way for a
 * token to exist under two spellings. A theme has to give a value for every
 * one of them — that is what this union buys over `Record<string, string>`:
 * adding a token is a compile error until both themes answer it.
 */
export type ChromeToken =
  /* Surfaces. Separation comes from spacing and weight rather than rules, so
     the window reads as a single slab; --raise is the first lift and --raise-2
     is both the second lift and the border colour. --bg-alt and --bg-alt-2 are
     the second and third grounds, which is what a workspace stands on when a
     workspace it touches has taken the one before: panes that share an edge
     are told apart by shade instead of by a line between them, and three
     grounds is what a pane lying against both halves of a split needs. They
     step down from --bg by the same amount each time, so no pair of them is
     ever more than a shade or two apart, and every lift in the chrome is still
     a lift wherever it lands. */
  | "--bg" | "--bg-alt" | "--bg-alt-2" | "--menu" | "--raise" | "--raise-2" | "--line"
  /* Text, brightest to faintest. */
  | "--bright" | "--text" | "--text-2" | "--text-3" | "--muted" | "--muted-2" | "--faint"
  /* The accent, and everything derived from it: the ink that reads on top of
     it, its glow behind a live indicator, its focus ring, and the surface,
     border and text of a panel that carries good news. */
  | "--accent" | "--accent-bright" | "--accent-ink" | "--accent-glow" | "--accent-ring"
  | "--accent-surface" | "--accent-line" | "--accent-text"
  /* File-kind marks in the browser: directories and symlinks. */
  | "--blue" | "--blue-fill" | "--cyan"
  /* The band behind a line the user typed, and the pinned copy of it. */
  | "--band" | "--band-deep" | "--band-line"
  /* Trouble. --warn is a refused credential or a rejected host key, which is
     not the same kind of trouble as unreachable. */
  | "--danger" | "--danger-glow" | "--warn"
  /* Over the window: the dimming behind a dialog, the lighter dimming used
     where what is underneath still has to be readable, and the shadows. */
  | "--scrim" | "--scrim-soft" | "--shadow" | "--shadow-soft";

export interface Theme {
  id: string;
  /// What the settings panel calls it.
  label: string;
  /// xterm's own palette, including the ground the chrome matches.
  terminal: ITheme;
  /**
   * The grounds a pane may stand on, in shade order: --bg, --bg-alt,
   * --bg-alt-2.
   *
   * Literals for the same reason `band` is one: xterm paints its background
   * into the canvas, where CSS cannot reach it. Shading a pane in the
   * stylesheet alone would stop at the tab bar and the terminal would paint
   * straight over the rest.
   */
  grounds: [string, string, string];
  /**
   * The band behind a line the user typed.
   *
   * A literal rather than a custom property because xterm paints it into the
   * canvas beneath the glyphs, where CSS cannot reach.
   */
  band: string;
  /**
   * Search hit backgrounds, for the same reason: xterm paints these into the
   * canvas, so they are values rather than custom properties.
   */
  searchMatch: string;
  searchActive: string;
  chrome: Record<ChromeToken, string>;
}

/**
 * A blue-tinted ground, with an ANSI palette chosen for it rather than
 * inherited. The background matches the chrome exactly so the terminal is not
 * a panel inside the window, it is the window.
 *
 * The chrome's accent is not one of those ANSI values, and used to be: it was
 * `#00cd00` and `#00ff00`, the terminal's own two greens, which are fully
 * saturated at both ends. Against a desaturated blue-grey ground they were the
 * one loud thing in the window — a primary button and a selection ring that
 * read as a colour first and as a control second. It is now the same hue held
 * back to around half saturation, which still clears seven to one against the
 * ground and stops competing with the text for attention. The ANSI palette
 * itself is untouched: those greens belong to the host's output, not to
 * afterm's chrome, and a terminal that quietly restyles them is lying about
 * what the program printed.
 *
 * The sixteen used to be Terminus's VGA values, which are the primaries at
 * full saturation. They were the wrong palette for this ground twice over.
 *
 * They failed at the thing they exist for. The job of the sixteen is semantic
 * — red fails, yellow warns, green is fine, blue recedes, dim is comments —
 * and four of them could not be read against this background: red at 2.79 to
 * one, blue at 2.88, magenta at 3.48, and the grey used for comments at 4.08.
 * Error red was the least legible colour in the terminal. They were also the
 * loudest thing in the window now that the chrome's accent is not, and their
 * contrast ran from 2.79 to 9.58, which is what "some slots glow" means with a
 * number on it. These run 5.11 to 10.69, and every one clears 4.5.
 *
 * Hues are taken from palettes proven on a ground like this one — One Dark and
 * Tokyo Night — rather than from a VGA adapter. Two are the chrome's own: ANSI
 * yellow is `--warn` and bright red is `--danger`, because the chrome already
 * had to answer the same question. Green is deliberately not the chrome's
 * accent: a prompt the colour of the tab underline reads as selected.
 *
 * Every bright is its own normal, lighter. Not a second, louder colour, which
 * is what full-saturation palettes make it.
 *
 * ANSI black is the one slot the grounds moved. It was `#12161e`, which is now
 * the third ground: a pane standing on it would have painted every black cell
 * in exactly its own colour, and black is the slot programs reach for as a
 * background. It has gone one further step down the same ramp, which is where
 * it belongs — darker than any ground it can be painted on is the whole of its
 * job.
 *
 * The input band is the one place the tint is deliberately held back. It was a
 * saturated blue, which made the line the user typed read as a coloured
 * highlight rather than as the same surface lifted; it is now the ground's own
 * hue at half the saturation and a step brighter, so it stands further off the
 * ground than the old blue did while the white on top of it keeps a contrast
 * ratio of seven to one.
 */
export const STOCKHOLM: Theme = {
  id: "stockholm",
  label: "Stockholm",
  band: "#414c60",
  grounds: ["#1a202c", "#161b25", "#12161e"],
  searchMatch: "#33405a",
  searchActive: "#4a6bbd",
  terminal: {
    background: "#1a202c",
    foreground: "#e6ebf2",
    cursor: "#e6ebf2",
    cursorAccent: "#1a202c",
    selectionBackground: "#33405a",
    black: "#0d1016",
    red: "#e06c75",
    green: "#98c379",
    yellow: "#d8b070",
    blue: "#7aa2f7",
    magenta: "#c678dd",
    cyan: "#56b6c2",
    white: "#c8d2e0",
    brightBlack: "#8e9bb0",
    brightRed: "#ff5a5a",
    brightGreen: "#b8d99a",
    brightYellow: "#e8c98a",
    brightBlue: "#9db6ff",
    brightMagenta: "#d7a0ea",
    brightCyan: "#7dd0db",
    brightWhite: "#ffffff"
  },
  chrome: {
    "--bg": "#1a202c",
    "--bg-alt": "#161b25",
    "--bg-alt-2": "#12161e",
    "--menu": "#161b25",
    "--raise": "#232b3a",
    "--raise-2": "#2c3547",
    "--line": "#4b566e",
    "--bright": "#ffffff",
    "--text": "#e6ebf2",
    "--text-2": "#c8d2e0",
    "--text-3": "#b8c4d4",
    "--muted": "#8e9bb0",
    "--muted-2": "#7d8a9e",
    "--faint": "#66748a",
    "--accent": "#4cbf7c",
    "--accent-bright": "#6fd99b",
    "--accent-ink": "#0c2a19",
    "--accent-glow": "#4cbf7c88",
    "--accent-ring": "#4cbf7c1f",
    "--accent-surface": "#1c3327",
    "--accent-line": "#357a52",
    "--accent-text": "#a8e6c3",
    "--blue": "#5c5cff",
    "--blue-fill": "#24345a",
    "--cyan": "#00cdcd",
    "--band": "#414c60",
    "--band-deep": "#363f50",
    "--band-line": "#5a677f",
    "--danger": "#ff5a5a",
    "--danger-glow": "#ff000088",
    "--warn": "#d8b070",
    "--scrim": "#1a202cdd",
    "--scrim-soft": "#1a202cb0",
    "--shadow": "#000000cc",
    "--shadow-soft": "#1a202c55"
  }
};

/**
 * Stockholm with the hue taken out: the same single-surface layout, the same
 * lifts and the same weights, in grey.
 *
 * Cool grey, not neutral grey. Every value here was once R = G = B, which is
 * grey by the numbers and brown to the eye: a neutral dark surface reads warm,
 * and the warmth is what made the scheme look muddy rather than sleek. So the
 * whole ramp carries one hue at around 213 degrees, red dropped and blue
 * lifted with green a little under half way between. The spread runs from five
 * steps at the darkest to nine at the brightest — a quarter of Stockholm's
 * tint, far too little to name a colour, just enough to stop the grey going
 * warm. Pure white and pure black are the two exceptions, having nowhere to
 * go; they anchor the ends of the ramp.
 *
 * The ANSI palette is the one place the grey stops, and the reason is that the
 * sixteen are not afterm's voice — they are the host's. Their job is semantic:
 * red fails, yellow warns, blue recedes, dim is comments. Sixteen greys were
 * laid out as one luminance ramp, six steps apart, so that they could be told
 * apart, and they could — but being distinguishable is not the same as meaning
 * something. Brightness can say two things differ; it cannot say which of them
 * is the error. In a `git status`, staged and unstaged were two greys and only
 * the headings said which was which.
 *
 * So the sixteen carry Stockholm's hues at two fifths of its saturation, on
 * Stockholm's lightness. Enough to tell an error from a success, far short of
 * a second coloured scheme — and six of them are warm now, which is the rule
 * this scheme was built on being deliberately given up for the terminal and
 * kept everywhere else.
 *
 * The chrome is untouched and stays cool grey to the last token, because the
 * chrome is where this scheme's identity actually lives: the tabs, the
 * sidebar, the status bar, the accent. afterm is grey; the host is not.
 *
 * Nothing in the chrome is yellow, including the two places it used to be: a
 * refused credential and a rejected host key read as brightness.
 */
export const CARBON: Theme = {
  id: "carbon",
  label: "Carbon",
  band: "#303336",
  grounds: ["#1a1c1f", "#141619", "#0e1013"],
  searchMatch: "#3a3d40",
  searchActive: "#6c6f73",
  terminal: {
    background: "#1a1c1f",
    foreground: "#e0e4e9",
    cursor: "#e0e4e9",
    cursorAccent: "#1a1c1f",
    selectionBackground: "#3a3d40",
    black: "#16171a",
    red: "#bd8f92",
    green: "#9cad8f",
    yellow: "#b9a98f",
    blue: "#9fb0d2",
    magenta: "#b596bf",
    cyan: "#769da2",
    white: "#cfd3d9",
    brightBlack: "#989da6",
    brightRed: "#ce8b8b",
    brightGreen: "#b9c6ad",
    brightYellow: "#ccbfa6",
    brightBlue: "#bac4e2",
    brightMagenta: "#ccb6d4",
    brightCyan: "#99babf",
    brightWhite: "#ffffff"
  },
  chrome: {
    "--bg": "#1a1c1f",
    "--bg-alt": "#141619",
    "--bg-alt-2": "#0e1013",
    "--menu": "#141619",
    "--raise": "#232629",
    "--raise-2": "#2f3235",
    "--line": "#4c4f52",
    "--bright": "#ffffff",
    "--text": "#e0e4e9",
    "--text-2": "#c2c6ca",
    "--text-3": "#b4b8bc",
    "--muted": "#919498",
    "--muted-2": "#808387",
    "--faint": "#6a6d71",
    /* Stockholm's green is a live indicator, a small-caps label and a primary
       button. Grey has one dimension, so the calm end of it carries "live" and
       the loud end is left to --danger: in a scheme with no hue, brightness is
       the only way to say which of the two dots needs attention. */
    "--accent": "#a5a8ac",
    "--accent-bright": "#e4e8ed",
    "--accent-ink": "#101215",
    "--accent-glow": "#ffffff40",
    "--accent-ring": "#ffffff1f",
    "--accent-surface": "#282b2e",
    "--accent-line": "#54575b",
    "--accent-text": "#e4e8ed",
    "--blue": "#979a9e",
    "--blue-fill": "#373a3d",
    "--cyan": "#c0c4c8",
    "--band": "#303336",
    "--band-deep": "#272a2d",
    "--band-line": "#4a4d50",
    "--danger": "#ffffff",
    "--danger-glow": "#ffffff99",
    "--warn": "#c4c8cc",
    "--scrim": "#1a1c1fdd",
    "--scrim-soft": "#1a1c1fb0",
    "--shadow": "#000000cc",
    "--shadow-soft": "#1a1c1f55"
  }
};

export const THEMES: Theme[] = [STOCKHOLM, CARBON];

export const DEFAULT_THEME = STOCKHOLM;

const THEME_KEY = "afterm.theme";

/** The theme with this id, or the default when the id is unknown or stale. */
export function themeById(id: string | null): Theme {
  return THEMES.find(theme => theme.id === id) ?? DEFAULT_THEME;
}

/** The theme chosen last time this machine ran afterm. */
export function storedTheme(): Theme {
  return themeById(localStorage.getItem(THEME_KEY));
}

export function rememberTheme(theme: Theme): void {
  localStorage.setItem(THEME_KEY, theme.id);
}

/**
 * Push a theme's chrome onto the document.
 *
 * Called before the first render rather than from a component, so nothing ever
 * paints against half a palette.
 */
/**
 * The palette for a terminal standing on one of the grounds.
 *
 * `cursorAccent` travels with the background because it is the ink drawn
 * inside the cursor block: it has to be the ground the block sits on, or the
 * cursor reads as a smudge rather than as a hole in the text.
 *
 * Shade 0 is handed back the theme's own palette untouched, so the common case
 * allocates nothing and keeps its identity between renders. An unknown shade
 * lands there too: a pane painted on the plain ground is wrong only in that it
 * might match its neighbour, which is what the shades exist to avoid, and not
 * something worth throwing a window away over.
 */
export function terminalPalette(theme: Theme, shade: number): ITheme {
  const ground = theme.grounds[shade];
  if (!ground || shade === 0) return theme.terminal;
  return { ...theme.terminal, background: ground, cursorAccent: ground };
}

export function applyTheme(theme: Theme): void {
  const root = document.documentElement;
  for (const [token, value] of Object.entries(theme.chrome)) {
    root.style.setProperty(token, value);
  }
  root.dataset.theme = theme.id;
}
