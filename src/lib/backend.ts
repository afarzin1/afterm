import { invoke } from "@tauri-apps/api/core";
import { getCurrentWindow, UserAttentionType } from "@tauri-apps/api/window";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { makeChannel } from "./channel";
import type {
  AuthNotice,
  AuthRequest,
  BackendFailure,
  Collision,
  DiscoveredKey,
  KeyInstalled,
  Listing,
  OpensshPresence,
  PasswordSupport,
  SessionProfile,
  IntegrationInstalled,
  TelemetryReport,
  TerminalExit,
  TerminalOutput,
  Transferred,
  TransferProgress,
  TransportReport
} from "../types";

/**
 * Whether there is a Tauri backend behind this page.
 *
 * The `typeof` guard is not decoration: the event channels below are set up as
 * this module is imported, and a test importing anything that reaches
 * `backend.ts` has no `window` at all.
 */
export const isDesktop = () =>
  typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;

/** Raised when a feature genuinely needs the native backend. */
export class PreviewUnavailable extends Error {
  constructor(feature: string) {
    super(`${feature} needs the desktop app. Run \`npm run tauri dev\`.`);
  }
}

/**
 * Every backend error arrives as a cause plus a message. Anything else — a
 * thrown Error from the preview shell, a string from somewhere unexpected — is
 * given the same shape so callers have one thing to reason about.
 */
export function failureOf(problem: unknown): BackendFailure {
  if (problem && typeof problem === "object" && "cause" in problem && "message" in problem) {
    const value = problem as Partial<BackendFailure>;
    return {
      cause: value.cause ?? "internal",
      message: String(value.message ?? ""),
      methods: value.methods ?? [],
      canPrompt: value.canPrompt ?? false
    };
  }
  const message = problem instanceof Error ? problem.message : String(problem);
  return { cause: "internal", message, methods: [], canPrompt: false };
}

/**
 * Start a session, once there is somewhere for everything it emits to land.
 *
 * The wait is what makes the ordering a property of the call rather than of
 * whichever component happened to mount first. A native session can ask for a
 * password inside this call, and an `ssh.exe` one can exit inside it; both of
 * those used to be able to happen before their listeners existed.
 */
/**
 * Open a session and put a terminal on it.
 *
 * `profileId` is the saved host this tab came from, where it came from one. It
 * is deliberately not the session's own id: that is made fresh for every tab,
 * and a password remembered against it would belong to one tab and then to
 * nothing.
 */
export async function spawnSession(
  profile: SessionProfile & { profileId?: string },
  cols: number,
  rows: number
) {
  if (!isDesktop()) return;
  await eventsReady();
  await invoke("spawn_session", { profile, profileId: profile.profileId ?? null, cols, rows });
}

export async function writeTerminal(id: string, data: string) {
  if (!isDesktop()) return;
  await invoke("terminal_write", { id, data });
}

export async function resizeTerminal(id: string, cols: number, rows: number) {
  if (!isDesktop()) return;
  await invoke("resize_session", { id, cols, rows });
}

/**
 * Hold this session's reader, or let it go again.
 *
 * The interface is the only side that knows how far behind it is, because it
 * is the side xterm tells when a chunk has finished being parsed. A held
 * reader leaves the bytes where they came from — in the pseudo-terminal, or
 * unacknowledged on the SSH channel — which is where a terminal that has
 * stopped listening is supposed to leave them.
 */
export async function setTerminalFlow(id: string, paused: boolean) {
  if (!isDesktop()) return;
  await invoke("set_terminal_flow", { id, paused });
}

/**
 * Close a session. `endShell` is true only when the user closed the tab: it
 * ends a kept shell's tmux session, which a reconnect — also a close — must
 * not do, since the reconnect is on its way to attaching to it.
 */
export async function closeSession(id: string, endShell = false) {
  if (!isDesktop()) return;
  await invoke("close_session", { id, endShell });
}

/**
 * Append a public key to the host's `authorized_keys`, and say what the host
 * confirmed.
 *
 * The answer comes back from the host rather than from afterm: which account
 * the key went to, which file it went into, and whether it was already there.
 * The command runs on afterm's own connection, so none of that depends on what
 * the terminal is currently showing.
 */
export async function installPublicKey(id: string, publicKey: string): Promise<KeyInstalled> {
  if (!isDesktop()) throw new PreviewUnavailable("Public-key installation");
  return invoke<KeyInstalled>("install_public_key", { id, publicKey });
}

/**
 * Append the shell-integration snippet to the host's startup file.
 *
 * The snippet is passed in rather than rebuilt on the other side, so
 * `integration.ts` stays the one place it is written.
 */
export async function installShellIntegration(
  id: string,
  snippet: string
): Promise<IntegrationInstalled> {
  if (!isDesktop()) throw new PreviewUnavailable("Shell-integration installation");
  return invoke<IntegrationInstalled>("install_shell_integration", { id, snippet });
}

/**
 * Paint the native title bar to match the active colour scheme.
 *
 * Windows draws that bar, not afterm, so it is the one surface the stylesheet
 * cannot reach. Nothing anywhere else needs this, and on every other platform
 * it is a no-op.
 */
export async function setCaptionColour(caption: string, text: string) {
  if (!isDesktop()) return;
  await invoke("set_caption_colour", { caption, text });
}

/**
 * Full screen on and off, the way a browser's F11 does it.
 *
 * Asked of the window rather than tracked here: the window is the one that
 * knows, and it can leave full screen without afterm being told — the window
 * manager has its own ways in and out of it on every platform.
 *
 * `setFullscreen` is not in Tauri's default window permissions; reading the
 * state is. Both are in `capabilities/default.json`.
 */
export async function toggleFullscreen(): Promise<void> {
  if (!isDesktop()) return;
  const window = getCurrentWindow();
  await window.setFullscreen(!(await window.isFullscreen()));
}

/**
 * Open a link from the terminal in the default browser. The backend decides
 * what may be opened — web links only — because the page is showing text the
 * far end wrote.
 */
export async function openLink(url: string): Promise<void> {
  if (!isDesktop()) {
    window.open(url, "_blank", "noopener");
    return;
  }
  await invoke("open_link", { url });
}

/**
 * Flash the taskbar button until the window is looked at, for a tab that has
 * asked for the user while afterm was behind something else. Informational
 * rather than critical: Windows flashes the button and stops, where critical
 * would keep flashing it and take the foreground on some shells.
 *
 * Needs `core:window:allow-request-user-attention` in the capabilities.
 */
export async function requestWindowAttention(): Promise<void> {
  if (!isDesktop()) return;
  await getCurrentWindow().requestUserAttention(UserAttentionType.Informational);
}

/**
 * The clipboard's text, for right-click paste.
 *
 * Read on the native side rather than through `navigator.clipboard`, which
 * WebView2 answers with a permission prompt. In the browser preview there is
 * no native side, and the browser's own read is good enough there.
 */
export async function readClipboardText(): Promise<string> {
  if (!isDesktop()) {
    try {
      return await navigator.clipboard.readText();
    } catch {
      // Denied, or no clipboard at all. Pasting nothing is the right answer.
      return "";
    }
  }
  return invoke<string>("clipboard_text");
}

/**
 * Send the clipboard's image to session `id`'s host, and return the path it
 * landed at — or null when the clipboard holds no image.
 *
 * What the paste key does over SSH when there is no text: a coding agent on
 * the host reads the host's clipboard, never this one, but takes an image's
 * path in place of a paste. The browser preview has no host to send it to.
 */
export async function pasteClipboardImage(id: string): Promise<string | null> {
  if (!isDesktop()) return null;
  return invoke<string | null>("paste_clipboard_image", { id });
}

// ----------------------------------------------------------- credentials

/** Answer a credential dialog. `null` means the user dismissed it. */
/**
 * Answer a credential dialog.
 *
 * `remember` is the checkbox, and it is the only say the interface has in the
 * matter. Where the password then goes is decided in Rust — after the host has
 * accepted it, into the operating system's credential store, and only for a
 * request that said it could be. Nothing here ever reads a saved password
 * back: there is no command that returns one.
 */
export async function submitAuthResponse(
  request: string,
  answers: string[] | null,
  remember = false
) {
  if (!isDesktop()) return;
  await invoke("submit_auth_response", { request, answers, remember });
}

/** Whether this machine can remember passwords, and why not when it cannot. */
export async function passwordSupport(): Promise<PasswordSupport> {
  if (!isDesktop()) {
    return { available: false, reason: "the browser preview has no credential store" };
  }
  return invoke<PasswordSupport>("password_support");
}

/** Which of these saved hosts have a remembered password. */
export async function rememberedPasswords(profiles: string[]): Promise<string[]> {
  if (!isDesktop() || profiles.length === 0) return [];
  return invoke<string[]>("remembered_passwords", { profiles });
}

export async function forgetPassword(profile: string) {
  if (!isDesktop()) return;
  await invoke("forget_password", { profile });
}

export async function forgetAllPasswords(profiles: string[]) {
  if (!isDesktop()) return;
  await invoke("forget_all_passwords", { profiles });
}

/**
 * Establish the session's transport, prompting for credentials if the host
 * wants them. The only path that may raise a dialog, and only ever reached from
 * something the user did.
 */
export async function authenticateSession(id: string): Promise<string> {
  if (!isDesktop()) throw new PreviewUnavailable("Authenticating");
  return invoke<string>("authenticate_session", { id });
}

export async function sessionTransport(id: string): Promise<TransportReport> {
  if (!isDesktop()) return { label: null, connecting: false, fallbackReason: null };
  return invoke<TransportReport>("session_transport", { id });
}

// ------------------------------------------------------------------ files

export async function localDownloads(): Promise<string> {
  if (!isDesktop()) throw new PreviewUnavailable("Downloading files");
  return invoke<string>("local_downloads");
}

export async function remoteHome(id: string): Promise<string> {
  if (!isDesktop()) throw new PreviewUnavailable("Browsing remote files");
  return invoke<string>("remote_home", { id });
}

export async function listRemoteDirectory(id: string, path: string): Promise<Listing> {
  if (!isDesktop()) throw new PreviewUnavailable("Browsing remote files");
  return invoke<Listing>("list_remote_directory", { id, path });
}

/**
 * Copy a remote file into the Downloads folder. Resolves to where it landed.
 *
 * The interface names the remote file and nothing else. The backend decides
 * the local path — never outside Downloads, never over a file already there —
 * because a remote name is not something this side can make safe for a
 * Windows filesystem, and a check made here could be bypassed here.
 */
export async function downloadFile(id: string, remote: string): Promise<string> {
  if (!isDesktop()) throw new PreviewUnavailable("Downloading files");
  return invoke<string>("download_file", { id, remote });
}

/**
 * Copy a remote folder into the Downloads folder, whole. Resolves to where it
 * landed and what arrived.
 *
 * The same rule as a file: the backend decides every local path, and a folder
 * already in Downloads is never written into. Links inside the tree are
 * skipped rather than followed, so a download cannot leave the folder that was
 * asked for.
 */
export async function downloadFolder(id: string, remote: string): Promise<Transferred> {
  if (!isDesktop()) throw new PreviewUnavailable("Downloading folders");
  return invoke<Transferred>("download_folder", { id, remote });
}

/**
 * Send a local file into a remote directory under its own name. Resolves to
 * where it landed. `onExists` is what to do if the name is taken; `fail` gets
 * an `exists` failure back, which is the pane's cue to ask.
 */
export async function uploadFile(
  id: string,
  local: string,
  directory: string,
  onExists: Collision
): Promise<string> {
  if (!isDesktop()) throw new PreviewUnavailable("Uploading files");
  return invoke<string>("upload_file", { id, local, directory, onExists });
}

/**
 * Send a local folder into a remote directory, whole. Resolves to where it
 * landed and what went.
 *
 * `onExists` is what to do about a folder already at that name on the host.
 * `fail` gets an `exists` failure back, which is the pane's cue to ask;
 * `replace` is the pane's *Merge*, and is the only answer under which a file
 * on the host is written over.
 */
export async function uploadFolder(
  id: string,
  local: string,
  directory: string,
  onExists: Collision
): Promise<Transferred> {
  if (!isDesktop()) throw new PreviewUnavailable("Uploading folders");
  return invoke<Transferred>("upload_folder", { id, local, directory, onExists });
}

/**
 * Whether a local path is a directory.
 *
 * Asked of the backend rather than worked out here: a drop carries paths and
 * nothing else, and the page has no filesystem to look at.
 */
export async function localIsDirectory(path: string): Promise<boolean> {
  if (!isDesktop()) return false;
  return invoke<boolean>("local_is_directory", { path });
}

export async function createRemoteDirectory(id: string, path: string) {
  if (!isDesktop()) throw new PreviewUnavailable("Creating directories");
  await invoke("create_remote_directory", { id, path });
}

export async function renameRemoteEntry(id: string, from: string, to: string) {
  if (!isDesktop()) throw new PreviewUnavailable("Renaming");
  await invoke("rename_remote_entry", { id, from, to });
}

/** A remote file's bytes, fetched for showing and not kept anywhere. */
export async function previewRemoteFile(id: string, path: string): Promise<ArrayBuffer> {
  if (!isDesktop()) throw new PreviewUnavailable("Previewing files");
  return invoke<ArrayBuffer>("preview_remote_file", { id, remote: path });
}

export async function deleteRemoteEntry(id: string, path: string, directory: boolean) {
  if (!isDesktop()) throw new PreviewUnavailable("Deleting");
  await invoke("delete_remote_entry", { id, path, directory });
}

// -------------------------------------------------------------- telemetry

export async function sessionMetrics(id: string): Promise<TelemetryReport> {
  if (!isDesktop()) {
    return { metrics: null, note: "preview", cause: null, methods: [], canPrompt: false, transport: null };
  }
  return invoke<TelemetryReport>("session_metrics", { id });
}

export async function discoverPublicKeys(): Promise<DiscoveredKey[]> {
  if (!isDesktop()) return [];
  return invoke<DiscoveredKey[]>("discover_public_keys");
}

/**
 * Whether this machine has the OpenSSH tools afterm shells out to.
 *
 * The browser preview has no machine to ask and no connection to make, so it
 * answers with everything present rather than warning about a Windows feature
 * on a page that could not use it either way.
 */
export async function checkOpenssh(): Promise<OpensshPresence> {
  if (!isDesktop()) return { missing: [], version: null };
  return invoke<OpensshPresence>("check_openssh");
}

// --------------------------------------------------------------- profiles

export async function loadProfiles(): Promise<SessionProfile[]> {
  if (!isDesktop()) return [];
  return invoke<SessionProfile[]>("load_profiles");
}

export async function saveProfiles(profiles: SessionProfile[]) {
  if (!isDesktop()) return;
  await invoke("save_profiles", { profiles });
}

// ----------------------------------------------------------------- events

/**
 * Every backend stream, subscribed to as this module is imported — which is
 * before any component has rendered and long before a session can be spawned.
 *
 * `spawnSession` waits on all four, so nothing a new session emits can arrive
 * before there is somewhere for it to go. See `channel.ts` for what went wrong
 * without it.
 */
const channels = {
  output: makeChannel<TerminalOutput>("terminal-output", desktopListen()),
  exit: makeChannel<TerminalExit>("terminal-exit", desktopListen()),
  authRequest: makeChannel<AuthRequest>("ssh-auth-request", desktopListen()),
  authResolved: makeChannel<string>("ssh-auth-resolved", desktopListen()),
  authNotice: makeChannel<AuthNotice>("ssh-auth-notice", desktopListen()),
  progress: makeChannel<TransferProgress>("transfer-progress", desktopListen())
};

/** `listen`, or nothing at all in the browser preview, which emits none of this. */
function desktopListen<T>() {
  if (!isDesktop()) return null;
  return (name: string, handler: (event: { payload: T }) => void) =>
    listen<T>(name, event => handler({ payload: event.payload })) as Promise<() => void>;
}

/** Resolves once every stream a new session could emit on is being delivered. */
export function eventsReady(): Promise<unknown> {
  return Promise.all([
    channels.output.ready,
    channels.exit.ready,
    channels.authRequest.ready,
    channels.authResolved.ready,
    channels.authNotice.ready
  ]);
}

export async function onTerminalOutput(
  handler: (value: TerminalOutput) => void
): Promise<UnlistenFn> {
  await channels.output.ready;
  return channels.output.subscribe(handler);
}

export async function onTerminalExit(handler: (value: TerminalExit) => void): Promise<UnlistenFn> {
  await channels.exit.ready;
  return channels.exit.subscribe(handler);
}

export async function onAuthRequest(handler: (value: AuthRequest) => void): Promise<UnlistenFn> {
  await channels.authRequest.ready;
  return channels.authRequest.subscribe(handler);
}

/** A request that timed out or was abandoned, so its dialog can be dismissed. */
export async function onAuthResolved(handler: (request: string) => void): Promise<UnlistenFn> {
  await channels.authResolved.ready;
  return channels.authResolved.subscribe(handler);
}

/**
 * Something that did not happen on a connection that worked anyway.
 *
 * Saving a password is what this exists for: the handshake succeeded and the
 * credential store refused the write, and letting that pass in silence would
 * leave someone believing a password was saved when it was not.
 */
export async function onAuthNotice(handler: (value: AuthNotice) => void): Promise<UnlistenFn> {
  await channels.authNotice.ready;
  return channels.authNotice.subscribe(handler);
}

export async function onTransferProgress(
  handler: (value: TransferProgress) => void
): Promise<UnlistenFn> {
  await channels.progress.ready;
  return channels.progress.subscribe(handler);
}
