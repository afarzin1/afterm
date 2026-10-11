import type { Session, SessionKind } from "../types";

/**
 * The shells afterm can open on this machine, and everything there is to know
 * about one.
 *
 * Two entries, in the order the dialog offers them. They are second-class on
 * purpose: there is no form, no saved entry, no address, and nothing to pick —
 * the button *is* the session. Everything afterm shows about a far end (the
 * host on the file pane, the readings in the status bar, installing a public
 * key) asks `isLocalShell` first and says nothing rather than inventing an
 * answer, which is the whole of what keeps them cheap to carry.
 */
export const LOCAL_SHELLS = [
  { kind: "cmd", label: "Command Prompt" },
  { kind: "powershell", label: "PowerShell" }
] as const;

export type LocalShellKind = (typeof LOCAL_SHELLS)[number]["kind"];

/** Whether this session runs here rather than somewhere else. */
export function isLocalShell(kind: SessionKind): kind is LocalShellKind {
  return kind === "cmd" || kind === "powershell";
}

/**
 * A session on one of them, ready to open.
 *
 * The address is empty rather than absent — see `SessionProfile` — and the
 * port is zero, because there is no port. Nothing reads either: `isLocalShell`
 * is what every display site is gated on.
 */
export function localSession(kind: LocalShellKind): Session {
  const shell = LOCAL_SHELLS.find(item => item.kind === kind);
  return {
    id: crypto.randomUUID(),
    name: shell?.label ?? kind,
    kind,
    host: "",
    user: "",
    port: 0,
    state: "connecting"
  };
}
