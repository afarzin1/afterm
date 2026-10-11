/**
 * The snippet that makes a shell say where its prompt ends and its commands
 * begin.
 *
 * afterm has always highlighted the line you typed on and pinned it once it
 * scrolled away, and until now it worked that out from keystrokes: the first
 * one starts a run, Enter ends it. That reads an ordinary command well and
 * cannot see three common ones at all — a command recalled from history with
 * the arrow keys, a pasted one, and a prompt that spans lines — because none
 * of those arrive as typing.
 *
 * OSC 133 is the shell answering the question instead of afterm guessing at
 * it. `A` is the start of a prompt, `B` the end of it and so the start of what
 * you type, `C` the start of the command's output, `D` its exit. afterm reads
 * A, B and C; the marks also mean the pinned copy carries the command without
 * the prompt in front of it, which no amount of guessing could work out.
 *
 * It is a paste rather than something afterm installs, and deliberately:
 * putting it there automatically would mean either editing a file on a machine
 * that is not afterm's to edit, or typing it into the shell at login and then
 * clearing the screen to hide the echo — which would take the host's banner
 * with it. Neither is worth doing behind someone's back. Without the snippet
 * little is lost: the keystroke reading stays, and stands aside for whichever
 * prompt actually carries marks. It is asked per prompt rather than latched
 * once, so a shell opened inside a marked one — `sudo -i`, a nested `ssh` —
 * goes back to keystrokes instead of losing highlighting altogether.
 *
 * OSC 7 is the other half of what a prompt can say: `file://host/path`, the
 * directory the shell is in, sent at every prompt. The file pane follows it —
 * once per change, so a pane browsed somewhere else stays there until the
 * shell moves again. It is emitted from the prompt rather than from a `cd`
 * hook because the shell is at a prompt exactly when the pane is worth
 * moving, and because one mechanism covers `cd`, `pushd`, a script that
 * returns somewhere else, and a shell that starts wherever it likes.
 *
 * The pane no longer depends on it. It reads the shell's directory off the
 * host through the telemetry stream — see `PROBE` in `remote.rs` — so it
 * follows a shell that has never heard of this snippet. What OSC 7 still buys
 * is latency: it arrives with the prompt, where the read is up to one poll
 * behind, so an announcing session is one the poll stops answering for.
 *
 * The DEBUG trap in the bash half runs before every command, including the
 * ones inside a function or a pipeline, so `C` arrives more than once for a
 * single command line. That is why the terminal treats the second and later
 * ones as nothing to do rather than as a new command.
 */
export const SHELL_INTEGRATION = `# afterm shell integration.
# Tells the terminal where a prompt ends and a command begins (OSC 133), so the
# highlighted line and the pinned command follow the shell instead of being
# guessed from keystrokes, and which directory the shell is in (OSC 7), so the
# file pane follows it. Append to ~/.bashrc or ~/.zshrc, then open a new
# session. Safe to leave in place for any other terminal: it prints nothing
# they will show.
if [ -n "$BASH_VERSION" ]; then
  PS1='\\[\\e]7;file://\\H\${PWD}\\a\\e]133;A\\a\\]'"$PS1"'\\[\\e]133;B\\a\\]'
  trap 'printf "\\033]133;C\\007"' DEBUG
elif [ -n "$ZSH_VERSION" ]; then
  __afterm_precmd() { print -n '\\e]133;A\\a'; print -Pn '\\e]7;file://%m%d\\a'; }
  __afterm_preexec() { print -n '\\e]133;C\\a'; }
  autoload -Uz add-zsh-hook
  add-zsh-hook precmd __afterm_precmd
  add-zsh-hook preexec __afterm_preexec
  PS1="$PS1"'%{'$'\\e]133;B\\a''%}'
fi
`;


/**
 * The directory an OSC 7 payload names, or null if it names none.
 *
 * The sequence is `file://host/path`, and the host half is thrown away. It
 * cannot be checked against anything afterm knows — the session was opened
 * against whatever the user typed, an alias or an address, while the shell
 * sends whatever `hostname` says — so checking it would reject the ordinary
 * case to catch an unusual one. The unusual one is a nested `ssh` or a
 * `docker exec` inside a marked session, which reports a path belonging to a
 * machine this pane cannot list; the pane answers that by leaving the listing
 * it has rather than by replacing it with an error.
 *
 * Lenient about the rest, because these come from other people's shells: a
 * bare path is taken as written, percent-encoding is undone when it decodes
 * and left alone when it does not, and a trailing slash is dropped so that
 * `/etc/` and `/etc` are not two directories to follow to.
 */
export function directoryFromOsc(data: string): string | null {
  let path = data.trim();
  if (path.startsWith("file://")) {
    const rest = path.slice("file://".length);
    const slash = rest.indexOf("/");
    // A host and no path at all says nothing about where the shell is.
    if (slash < 0) return null;
    path = rest.slice(slash);
  }
  if (!path.startsWith("/")) return null;
  try {
    path = decodeURIComponent(path);
  } catch {
    // Not valid encoding, so not encoded. What arrived is the path.
  }
  return path.length > 1 && path.endsWith("/") ? path.slice(0, -1) : path;
}
