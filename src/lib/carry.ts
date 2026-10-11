/**
 * What a terminal had on it, for the terminal that replaces it.
 *
 * A reconnect builds a new terminal for the new shell. Its predecessor's
 * screen is usually the reason for the reconnect — the error, the last thing
 * a service logged — and the automatic reconnect replaces the terminal five
 * seconds after the connection goes, which is before anyone has read it. So
 * the text comes along, as text: colours and highlights belong to a shell
 * that is gone, and redrawing them would pass them off as the new one's.
 */

/** The part of xterm's buffer this reads, so it can be tested without one. */
export interface BufferLike {
  length: number;
  getLine(index: number): { isWrapped: boolean; translateToString(trimRight?: boolean): string } | undefined;
}

/**
 * The buffer as lines, with each wrapped row joined back onto the line it
 * continues so that the new terminal wraps it at its own width, and the empty
 * rows below the last output left off.
 */
export function carriedLines(buffer: BufferLike): string[] {
  const lines: string[] = [];
  for (let index = 0; index < buffer.length; index++) {
    const line = buffer.getLine(index);
    if (!line) continue;
    // A row that wraps onto the next is full to its edge, and a space at that
    // edge is part of the line; only a row that ends the line is trimmed.
    const continues = buffer.getLine(index + 1)?.isWrapped === true;
    const text = line.translateToString(!continues);
    if (line.isWrapped && lines.length > 0) lines[lines.length - 1] += text;
    else lines.push(text);
  }
  while (lines.length > 0 && lines[lines.length - 1].trim() === "") lines.pop();
  return lines;
}
