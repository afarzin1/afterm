/**
 * What carries a session.
 *
 * Two of these are SSH and differ only in what carries the terminal: afterm's
 * own connection, or the OpenSSH client for a host whose configuration afterm
 * will not pretend to reproduce.
 *
 * The other two are this machine's own shells, and they are deliberately not
 * equals. They were dropped once — Windows Terminal is a better local terminal
 * than afterm was ever going to be — and they are back on the terms that made
 * dropping them attractive in the first place: nothing to configure, nothing
 * saved, nothing measured. Two buttons on the new-session dialog open one, and
 * every part of afterm that is about a far end says nothing about them rather
 * than inventing something to say.
 */
export type SessionKind = "ssh" | "ssh-cli" | "powershell" | "cmd";
/// `lost` is the one that is recoverable: the connection went away, but the
/// session it carried is still what the user asked for. There is no `closed`:
/// a session that ends is removed, tab and all, which is what closing means.
export type SessionState = "connecting" | "connected" | "error" | "lost";

/**
 * A session, saved or open.
 *
 * The address is not optional, and is empty exactly when the session is a
 * local shell. It used to be optional, and the cost was every screen having to
 * decide what to show in its place — the status bar read `undefined@host` the
 * first time one of them got it wrong. `isLocalShell` is the question to ask
 * before showing an address, rather than whether one happens to be there.
 */
export interface SessionProfile {
  id: string;
  name: string;
  kind: SessionKind;
  host: string;
  user: string;
  port: number;
  /// Keep the shell running on the host between connections, in tmux, so a
  /// reconnect lands back in it. SSH only, off unless a saved host asks.
  persistentShell?: boolean;
}

export interface Session extends SessionProfile {
  state: SessionState;
  /// The saved host this tab was opened from, when it was opened from one.
  ///
  /// Not `id`, which is made fresh for every tab: a remembered password is
  /// bound to the saved host, so it has to travel on something that survives
  /// closing the tab and opening it again. Absent for an address typed in and
  /// not saved, which has nothing durable to remember anything against.
  profileId?: string;
  /// Bumped on every reconnect, so the terminal is rebuilt for the new one
  /// without the tab moving or being renamed.
  attempt?: number;
}

export interface TerminalOutput {
  id: string;
  data: string;
}

/// Why a terminal stopped. `exited` is a shell that ended on its own — `exit`,
/// Ctrl-D, a logout — and has nothing to reconnect to.
export type ExitReason = "exited" | "lost";

export interface TerminalExit {
  id: string;
  reason: ExitReason;
}

/**
 * Host telemetry. Every field is nullable and a null renders as an em dash:
 * afterm shows a number only when it measured one.
 */
export interface Metrics {
  cpu: number | null;
  usedMemory: number | null;
  totalMemory: number | null;
  diskUsed: number | null;
  diskTotal: number | null;
  downloadRate: number | null;
  uploadRate: number | null;
  uptime: number | null;
  latency: number | null;
  /**
   * Where the session's shell is, read off the host rather than announced by
   * it. Not rendered in the bar — the file pane follows it. Null on a host
   * without /proc, and on a shell afterm may not look at.
   */
  directory: string | null;
}

/**
 * Why something failed, in a shape the interface can act on.
 *
 * `cause` decides what to offer and `canPrompt` decides whether offering a
 * credential dialog would achieve anything — the backend works that out once
 * rather than leaving every caller to re-derive it from the method list.
 */
export type FailureCause =
  | "auth"
  | "hostKey"
  | "unreachable"
  | "unsupported"
  | "missingClient"
  | "operation"
  | "exists"
  | "internal";

/**
 * Which OpenSSH command-line tools this machine is missing, if any.
 *
 * afterm carries SSH itself, but hands two jobs to the user's own client —
 * `ssh -G` for configuration and `ssh-keygen -F` for host-key trust — so an
 * empty `missing` is the only state in which anything can connect.
 */
export interface OpensshPresence {
  /** Binary names, as the user would install them. Empty means all present. */
  missing: string[];
  /** What `ssh -V` reported, or null when there was no client to ask. */
  version: string | null;
}

/**
 * What to do when a transfer's destination is already taken. Chosen here,
 * enforced by the backend, which never replaces anything unless told to.
 */
export type Collision = "fail" | "replace" | "rename";

export interface BackendFailure {
  cause: FailureCause;
  message: string;
  methods: string[];
  canPrompt: boolean;
}

/** Telemetry plus, when there is none, the reason why. */
export interface TelemetryReport {
  metrics: Metrics | null;
  note: string | null;
  cause: FailureCause | null;
  methods: string[];
  canPrompt: boolean;
  /** What is carrying this session's telemetry, for the status bar. */
  transport: string | null;
}

/** Which transport a session ended up on, and why. */
export interface TransportReport {
  label: string | null;
  connecting: boolean;
  fallbackReason: string | null;
}

/** One question a host asked during authentication. */
export interface AuthPromptField {
  prompt: string;
  echo: boolean;
}

/// Which method a request belongs to. Only a password can be remembered: a
/// keyboard-interactive round may be asking for a one-time code, and a hidden
/// field is no way to tell the two apart.
export type AuthMethod = "password" | "keyboard-interactive" | "key-passphrase";

export interface AuthRequest {
  session: string;
  request: string;
  host: string;
  user: string;
  name: string;
  instructions: string;
  prompts: AuthPromptField[];
  method: AuthMethod;
  /// True when this answer could be remembered: a password, on a saved host,
  /// with a credential store to put it in. The dialog offers the choice on
  /// this alone and never works out why the answer was no.
  canRemember: boolean;
}

/// Something about a session worth saying, on a connection that worked anyway.
export interface AuthNotice {
  session: string;
  message: string;
}

/// Whether this machine can remember passwords, and why not when it cannot.
export interface PasswordSupport {
  available: boolean;
  reason: string | null;
}

/** Progress on one file transfer. */
export interface TransferProgress {
  id: string;
  name: string;
  done: number;
  total: number | null;
}

/** A public key found on this machine, offered as a one-click install. */
export interface DiscoveredKey {
  path: string;
  name: string;
  kind: string;
  comment: string;
  content: string;
}

/** What the host confirmed after a public key was installed on it. */
export interface KeyInstalled {
  /** The account the key went to, as the host named it. */
  user: string;
  /** The absolute path of the file it went into. */
  path: string;
  /** True when the key was already there and nothing was appended. */
  alreadyPresent: boolean;
}

/** What the host confirmed after the shell snippet was installed on it. */
export interface IntegrationInstalled {
  /** The absolute path of the startup file it went into. */
  path: string;
  /** True when the snippet was already there and nothing was appended. */
  alreadyPresent: boolean;
  /** The login shell the host reported. */
  shell: string;
  /**
   * True when the login shell is bash and the file it reads at login does not
   * source `~/.bashrc` — so the snippet is in place but will never run.
   */
  needsProfile: boolean;
}

export type EntryKind = "directory" | "file" | "link";

export interface FileEntry {
  name: string;
  path: string;
  kind: EntryKind;
  size: number | null;
  modified: string | null;
}

export interface Listing {
  path: string;
  parent: string | null;
  entries: FileEntry[];
}

/**
 * What a folder transfer carried, in either direction.
 *
 * Counts rather than a path alone, because a folder can arrive partly: a link
 * is never followed, and one file that cannot be read does not lose the rest
 * of the tree. The pane says so rather than reporting a plain success.
 */
export interface Transferred {
  /// The folder that was made for it, at the end it was going to. Never one
  /// that was already there, unless the user asked to merge into it.
  path: string;
  /// Regular files that arrived.
  files: number;
  /// Entries passed over, which is every link in the tree.
  skipped: number;
  /// Entries that could not be copied: one end or the other refused them.
  failed: number;
}
