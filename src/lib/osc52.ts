/**
 * OSC 52 — a program on the host putting text on this machine's clipboard.
 *
 * How a coding agent hands over a long command intact. What it prints is
 * wrapped by the agent itself, to the width it was told, with an indent on
 * every continuation row: those are real newlines and real spaces by the time
 * they reach the terminal, and a drag across them copies four lines where the
 * agent meant one. Claude Code's `/copy` sends the text this way instead when
 * it is running over SSH, as do tmux, vim and Neovim when asked to.
 *
 * Writes only. A query — `?` in place of the text — asks the terminal to type
 * the clipboard back to the host, which would hand any server a session is
 * open to whatever was last copied on this machine, so it goes unanswered, as
 * it does in Windows Terminal.
 */

/**
 * The text an OSC 52 sequence asks to copy, or null when it asks for nothing
 * afterm will do.
 *
 * `data` is what follows `52;`: a list of selections, then the text in base64.
 * The selections — `c` for the clipboard, `p` and `s` for X11's primary and
 * selection buffers, digits for cut buffers, nothing for the default — all
 * mean the one clipboard Windows has. Null for a query, for an empty payload
 * (xterm reads that as "clear", and clearing someone's clipboard from a
 * server is not worth supporting) and for anything that is not base64 of
 * UTF-8 text, so a garbled sequence never replaces a good clipboard.
 */
export function clipboardWrite(data: string): string | null {
  const split = data.indexOf(";");
  if (split < 0) return null;
  if (!/^[cpqs0-7]*$/.test(data.slice(0, split))) return null;
  const payload = data.slice(split + 1);
  if (!payload || payload === "?") return null;
  let binary: string;
  try {
    binary = atob(payload);
  } catch {
    return null;
  }
  const bytes = new Uint8Array(binary.length);
  for (let index = 0; index < binary.length; index++) {
    bytes[index] = binary.charCodeAt(index);
  }
  try {
    return new TextDecoder("utf-8", { fatal: true }).decode(bytes) || null;
  } catch {
    return null;
  }
}
