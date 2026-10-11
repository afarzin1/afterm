/**
 * The element xterm keeps focused to receive keys, and the container it sits
 * in. The terminal is a destination for keystrokes, not a field somebody is
 * typing into, and the difference is what this module exists to draw.
 */
const TERMINAL = ".xterm";

/** The parts of an event target this rule reads, and nothing more. */
interface KeyTarget {
  tagName?: unknown;
  isContentEditable?: unknown;
  closest?(selector: string): unknown;
}

/**
 * Is this keystroke on its way to something the user is typing into?
 *
 * Chords that would steal a key ask first, so `Ctrl Shift Left` splits a
 * workspace from the terminal but still selects a word in a rename box.
 *
 * The terminal answers no. xterm holds a hidden `<textarea>` focused to take
 * keys and drive IME, so a focused terminal used to look exactly like a text
 * field — which is how the arrow chords came to be waved through to the
 * shell as escape sequences instead of being claimed. The
 * find bar and the rename inputs are real fields and still answer yes: they
 * sit beside the terminal surface, not inside it.
 *
 * Duck-typed rather than `instanceof HTMLElement`, so the rule is testable in
 * node without pulling in a DOM.
 */
export function typingIntoField(target: EventTarget | null): boolean {
  const element = target as KeyTarget | null;
  if (!element || typeof element.tagName !== "string") return false;
  if (element.closest?.(TERMINAL)) return false;
  const tag = element.tagName.toLowerCase();
  return tag === "input" || tag === "textarea" || element.isContentEditable === true;
}
