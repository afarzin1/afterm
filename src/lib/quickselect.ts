/**
 * Quick select: every copyable thing on screen gets a short label, and typing
 * the label copies it.
 *
 * What counts is what gets copied off an SSH session all day — addresses,
 * hashes and container ids, UUIDs, paths, URLs, and long numbers such as
 * process ids. Each is a pattern here rather than a setting, because a
 * terminal that asks its users to write regular expressions has stopped being
 * the tool this one is.
 *
 * The screen is read as logical lines — a wrapped row joined onto the row it
 * continues — so a long URL that wraps is still one thing to copy. Positions
 * go back to (row, column) through a map built cell by cell, because a wide
 * character is one character of text and two columns of screen.
 */

/** One logical line of the screen: its text, and where each character sits. */
export interface ScreenLine {
  text: string;
  /** For each character of `text`, the viewport row and column it starts at. */
  cells: Array<{ row: number; column: number }>;
}

export interface Match {
  text: string;
  /** Start and end (exclusive) in the logical line's text. */
  start: number;
  end: number;
  line: number;
}

/**
 * The patterns, most specific first: where two overlap, the earlier one in
 * this list keeps the text, so a UUID is not read as five hex runs and an
 * address with a port is not read as a number.
 */
const PATTERNS: RegExp[] = [
  // URLs. Trailing punctuation is trimmed after matching; see `tidy`.
  /\bhttps?:\/\/[^\s<>"'`]+/g,
  // UUIDs.
  /\b[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}\b/gi,
  // IPv4, with a port or a prefix length when there is one.
  /\b(?:(?:25[0-5]|2[0-4]\d|1?\d?\d)\.){3}(?:25[0-5]|2[0-4]\d|1?\d?\d)(?::\d{1,5}|\/\d{1,2})?\b/g,
  // IPv6: eight groups, or a compressed form. Needs a `::` or all eight groups,
  // so a time of day is not an address.
  /(?<![\w:])(?:[0-9a-f]{1,4}:){7}[0-9a-f]{1,4}(?![\w:])|(?<![\w:])(?:[0-9a-f]{1,4}:)*:(?::?[0-9a-f]{1,4})+(?![\w:])/gi,
  // Hex of seven or more — git's short hash, a container id, a digest — with
  // at least one digit in it, so a word spelt from a–f is left alone.
  /\b(?=[0-9a-f]*\d)(?=[0-9a-f]*[a-f])[0-9a-f]{7,}\b/gi,
  // Paths: absolute, home-relative or dot-relative, starting a word.
  /(?<=^|[\s'"=:(\[,])(?:~|\.{1,2})?\/[\w.@+\-/]*[\w@+\-/]/g,
  // Numbers of four digits or more: process ids, ports, sizes.
  /\b\d{4,}\b/g
];

/** Punctuation a match cannot end in: the sentence around it, not the thing. */
function tidy(text: string): string {
  let end = text.length;
  while (end > 0 && ".,;:!?)]}'\"".includes(text[end - 1])) {
    // A closing bracket that closes an opening one inside the text is part of
    // it — `https://en.wikipedia.org/wiki/Foo_(bar)`.
    const close = text[end - 1];
    const open = close === ")" ? "(" : close === "]" ? "[" : close === "}" ? "{" : null;
    if (open && [...text.slice(0, end - 1)].filter(c => c === open).length
      > [...text.slice(0, end - 1)].filter(c => c === close).length) break;
    end -= 1;
  }
  return text.slice(0, end);
}

/** Everything worth copying on these lines, with no two overlapping. */
export function findMatches(lines: readonly ScreenLine[]): Match[] {
  const found: Match[] = [];
  lines.forEach((line, index) => {
    const taken: Array<[number, number]> = [];
    const free = (start: number, end: number) =>
      taken.every(([from, to]) => end <= from || start >= to);
    for (const pattern of PATTERNS) {
      pattern.lastIndex = 0;
      for (const hit of line.text.matchAll(pattern)) {
        const text = tidy(hit[0]);
        if (text.length < 2) continue;
        const start = hit.index ?? 0;
        const end = start + text.length;
        if (!free(start, end)) continue;
        taken.push([start, end]);
        found.push({ text, start, end, line: index });
      }
    }
  });
  // Screen order, so labels read top to bottom, left to right.
  return found.sort((a, b) => a.line - b.line || a.start - b.start);
}

/**
 * The home row first, then the rows around it, the way WezTerm and Vimium
 * order theirs: the labels that are easiest to type go to the first matches.
 */
export const ALPHABET = "asdfjklghqweruiopzxcvbnmty";

/**
 * A label for each of `count` distinct things. One letter each while the
 * alphabet lasts, two letters each past that — all two, never a mix, so no
 * label is the start of another and a typed letter is never ambiguous.
 */
export function makeLabels(count: number, alphabet = ALPHABET): string[] {
  if (count <= alphabet.length) return [...alphabet.slice(0, count)];
  const labels: string[] = [];
  for (const first of alphabet) {
    for (const second of alphabet) {
      if (labels.length === count) return labels;
      labels.push(first + second);
    }
  }
  return labels;
}

/**
 * Labels for these matches. The same text twice gets the same label — one
 * container id listed in two places is still one thing to copy, and a second
 * label for it is a letter spent on nothing.
 */
export function labelMatches(matches: readonly Match[]): Array<Match & { label: string }> {
  const distinct = [...new Set(matches.map(match => match.text))];
  const labels = makeLabels(distinct.length);
  const byText = new Map(distinct.map((text, index) => [text, labels[index]]));
  return matches.map(match => ({ ...match, label: byText.get(match.text)! }));
}
