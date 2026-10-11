import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import {
  CirclePlus, ClipboardCheck, ClipboardCopy, FileDown, KeyRound, Maximize, Minus, Palette,
  PanelLeftClose, Pencil, Plus, Server, Settings, Trash2, X
} from "lucide-react";
import { ConnectDialog, openingTab, type ConnectTab } from "./components/ConnectDialog";
import { FileBrowser, forgetSession } from "./components/FileBrowser";
import { StatusBar } from "./components/StatusBar";
import { WorkspaceGrid } from "./components/WorkspaceGrid";
import type { PaneActions } from "./components/TerminalPane";
import { CommandPalette, type Command } from "./components/CommandPalette";
import { KeyDialog } from "./components/KeyDialog";
import { AuthDialog } from "./components/AuthDialog";
import { MissingClient } from "./components/MissingClient";
import {
  checkOpenssh, closeSession, failureOf, forgetAllPasswords, forgetPassword,
  installShellIntegration, isDesktop, onAuthNotice, onTerminalExit, passwordSupport,
  rememberedPasswords, requestWindowAttention, saveProfiles, setCaptionColour, toggleFullscreen
} from "./lib/backend";
import {
  DEFAULT_FONT_SIZE, MAX_FONT_SIZE, MIN_FONT_SIZE, THEMES, applyTheme, clampFontSize, rememberTheme,
  stepFontSize, storedTheme, type Theme
} from "./theme";
import { typingIntoField } from "./lib/keys";
import { copiesOnSelect } from "./lib/selection";
import { trackDisplayScale } from "./lib/scale";
import { SHELL_INTEGRATION } from "./lib/integration";
import { renameProfile } from "./lib/profile";
import { isLocalShell, localSession, type LocalShellKind } from "./lib/shells";
import * as reconnect from "./lib/reconnect";
import { AGENT_SETUP, type Notice } from "./lib/attention";
import {
  activateWorkspaceTab,
  activeSessionId,
  addWorkspaceTab,
  createWorkspaceLayout,
  cycleWorkspaceTab,
  canZoom,
  isZoomed,
  focusAdjacentWorkspace,
  focusWorkspace,
  moveWorkspaceTab,
  removeWorkspaceTab,
  resizeWorkspaceSplit,
  splitWorkspace,
  workspacePanes,
  type SplitDirection,
  type WorkspaceLayout
} from "./lib/workspaces";
import type { BackendFailure, OpensshPresence, PasswordSupport, Session, SessionProfile } from "./types";

const FONT_SIZE_KEY = "afterm.fontSize";
const COPY_ON_SELECT_KEY = "afterm.copyOnSelect";
/// Whether a saved host may keep its shell in tmux at all. Off unless turned
/// on in Settings: it trades afterm's scrollback and command tracking for
/// tmux's, which is a choice to make knowingly rather than stumble into.
const KEEP_SHELLS_KEY = "afterm.keepShells";
/**
 * How long "Copied to clipboard" stays on screen.
 *
 * Short, because it is confirming something the user watched happen rather
 * than telling them anything they do not know. Long enough to be read out of
 * the corner of an eye and gone before it is in the way of the terminal it
 * sits over. `.copied-flash` fades out on this same duration, so the two
 * numbers move together.
 */
const COPIED_FLASH_MS = 1400;

const SHORTCUTS = [
  { keys: "Ctrl Shift Space", action: "Command palette" },
  { keys: "Ctrl Shift N", action: "New session" },
  { keys: "1 – 9", action: "Open a saved host, on the Ctrl Shift N list" },
  { keys: "Ctrl Shift W", action: "Close session" },
  { keys: "Ctrl Shift B", action: "Toggle sidebar" },
  { keys: "Ctrl Shift R", action: "Rename session" },
  { keys: "Ctrl + / Ctrl -", action: "Terminal font size" },
  { keys: "Ctrl Tab / Ctrl Shift Tab", action: "Next or previous tab in this workspace" },
  { keys: "Ctrl Shift + Arrow", action: "Split active tab" },
  { keys: "Alt Shift + Arrow", action: "Focus adjacent workspace" },
  { keys: "Ctrl Shift Z", action: "Zoom the workspace to fill the window, and back" },
  { keys: "Ctrl Shift F", action: "Find in scrollback" },
  { keys: "Ctrl Shift PgUp / PgDn", action: "Scroll to the previous or next command" },
  { keys: "Ctrl Shift O", action: "Copy a command's output: the last one, or the one scrolled to" },
  { keys: "Ctrl Shift S", action: "Quick select: label addresses, hashes and paths on screen to copy" },
  { keys: "PgUp / PgDn", action: "Scroll the scrollback a page at a time" },
  { keys: "F11", action: "Full screen" },
  { keys: "Ctrl Shift V", action: "Paste" },
  { keys: "Right-click", action: "Paste into the terminal" },
  { keys: "Ctrl Shift C", action: "Copy selection" },
  { keys: "R / S / Q", action: "Reconnect now, stop retrying, or close, on a lost connection" }
];

/**
 * The tabs to open with: none.
 *
 * afterm starts empty every time. It used to reopen whatever was open when it
 * last closed, and that was the wrong default for a program whose tabs are
 * connections: a launch would fire off however many handshakes the last run
 * happened to end with, ask for however many passwords those wanted, and do it
 * before the window had been looked at. Closing afterm is how you end a
 * session, so a launch is a blank window and the connect dialog.
 */
function openingSessions(): Session[] {
  if (!isDesktop()) {
    // The browser preview has no backend and so no host. It is an SSH session
    // rather than a local shell, because the chrome it exists to show — the
    // status bar, the file pane's waiting state — is the chrome an SSH session
    // gets, and a local shell is defined by showing none of it.
    return [{
      id: crypto.randomUUID(),
      name: "Preview",
      kind: "ssh",
      host: "preview",
      user: "afterm",
      port: 22,
      state: "connected"
    }];
  }
  return [];
}

interface Props {
  initialProfiles?: SessionProfile[];
  /// Something to say the moment the window opens, which so far is only ever
  /// that the saved hosts could not be read. Passed in rather than fetched
  /// here because the read happens before React mounts.
  initialNotice?: string;
}

export function App({ initialProfiles = [], initialNotice = "" }: Props) {
  const [sessions, setSessions] = useState<Session[]>(openingSessions);
  const [layout, setLayout] = useState<WorkspaceLayout>(
    () => createWorkspaceLayout(sessions.map(session => session.id))
  );
  const [sidebar, setSidebar] = useState(true);
  // The window opens empty and connects to nothing that was not asked for, so
  // it asks where to go rather than leaving the question hanging. Null is
  // closed; the tab it opens on is the saved hosts whenever there are any.
  const [dialog, setDialog] = useState<ConnectTab | null>(
    () => isDesktop() && sessions.length === 0 ? openingTab(initialProfiles) : null
  );
  const [keyDialog, setKeyDialog] = useState(false);
  const [settings, setSettings] = useState(false);
  const [palette, setPalette] = useState(false);
  const [profiles, setProfiles] = useState<SessionProfile[]>(initialProfiles);
  const [notice, setNotice] = useState(initialNotice);
  // Null until the probe answers, so nothing is claimed about this machine
  // before it has been asked. The banner only ever appears on a real answer.
  const [openssh, setOpenssh] = useState<OpensshPresence | null>(null);
  /// Id of the tab whose name is being edited inline, if any.
  const [renaming, setRenaming] = useState<string | null>(null);
  const [draftName, setDraftName] = useState("");
  const [fontSize, setFontSize] = useState(() =>
    clampFontSize(Number(localStorage.getItem(FONT_SIZE_KEY)) || DEFAULT_FONT_SIZE)
  );
  // On unless it has been turned off. Copy-on-select writes to the clipboard
  // without being asked to, so whatever was on it is gone every time a drag
  // ends — which is why the drag now says so, in the corner, for as long as it
  // takes to read. A copy the user can see happen is no longer the silent
  // surprise that kept this off, and it is what the audience arriving from
  // PuTTY and from X11 expects to find already working.
  const [copyOnSelect, setCopyOnSelect] = useState(
    () => copiesOnSelect(localStorage.getItem(COPY_ON_SELECT_KEY))
  );
  const [keepShells, setKeepShells] = useState(() => localStorage.getItem(KEEP_SHELLS_KEY) === "true");
  /// For the callbacks that open sessions, which are made once. With the
  /// feature off, a host that has it switched on opens an ordinary shell —
  /// the host keeps the switch, so turning the feature back on restores it.
  const keepShellsRef = useRef(keepShells);
  keepShellsRef.current = keepShells;
  /// Counts copy-on-select writes rather than holding a flag, because the
  /// interesting case is the second copy inside the first one's lifetime:
  /// a new number is a new state, so the effect below runs again and restarts
  /// the clock. A flag already true would change nothing and the second copy
  /// would go unremarked. Zero is "nothing to show".
  const [copies, setCopies] = useState(0);
  const [theme, setTheme] = useState<Theme>(storedTheme);
  /// Whether this machine can remember passwords, and why not when it cannot.
  /// Null until the backend has answered, so nothing is claimed about the
  /// machine before it has been asked.
  const [passwords, setPasswords] = useState<PasswordSupport | null>(null);
  /// Ids of the saved hosts that have a remembered password. Asked for by id
  /// rather than enumerated from the store: the list of saved hosts is already
  /// here, and a credential store that cannot be searched is one fewer thing
  /// to depend on.
  const [remembered, setRemembered] = useState<string[]>([]);
  const [integrated, setIntegrated] = useState<ReadonlySet<string>>(() => new Set());
  /// Where each session's shell says it is, for the file pane to follow. Only
  /// sessions whose shell sends OSC 7 are in here, and only once they have.
  const [directories, setDirectories] = useState<ReadonlyMap<string, string>>(() => new Map());

  /// Drop what a session's shell last said about itself. A session that has
  /// gone has no directory, and a reconnect is a new shell that has not spoken
  /// yet — in both cases the pane must not follow a stale one.
  const forgetDirectory = useCallback((id: string) => {
    setDirectories(old => {
      if (!old.has(id)) return old;
      const next = new Map(old);
      next.delete(id);
      return next;
    });
  }, []);

  /// Sessions being reconnected on their own, and where each one has got to.
  /// See `lib/reconnect` for the rules. A ref beside the state because the
  /// callbacks that report an attempt's end have to decide the tab's state
  /// from it, and a stale closure would decide from an old one.
  const [retrying, setRetrying] = useState<ReadonlyMap<string, reconnect.Retrying>>(() => new Map());
  const retryingRef = useRef(retrying);
  retryingRef.current = retrying;
  const updateRetry = useCallback((
    id: string,
    change: (old: reconnect.Retrying | undefined) => reconnect.Retrying | undefined
  ) => {
    setRetrying(old => {
      const value = change(old.get(id));
      if (value === old.get(id)) return old;
      const next = new Map(old);
      if (value) next.set(id, value);
      else next.delete(id);
      return next;
    });
  }, []);

  /// Tabs something has asked for the user in since they were last looked at:
  /// a coding agent done with its turn, a bell. See `lib/attention`.
  const [attention, setAttention] = useState<ReadonlyMap<string, Notice>>(() => new Map());
  const dropAttention = useCallback((id: string) => {
    setAttention(old => {
      if (!old.has(id)) return old;
      const next = new Map(old);
      next.delete(id);
      return next;
    });
  }, []);
  /// Whether the window has the keyboard. A tab in front of a window that is
  /// behind something else is not being looked at either.
  const [windowFocused, setWindowFocused] = useState(() => document.hasFocus());
  useEffect(() => {
    const focus = () => setWindowFocused(true);
    const blur = () => setWindowFocused(false);
    window.addEventListener("focus", focus);
    window.addEventListener("blur", blur);
    return () => {
      window.removeEventListener("focus", focus);
      window.removeEventListener("blur", blur);
    };
  }, []);

  /// Close a tab. `ending` is the user closing it, as opposed to its shell
  /// ending on its own, and is what ends a kept shell on the host.
  const removeSession = useCallback((id: string, ending = false) => {
    updateRetry(id, () => undefined);
    dropAttention(id);
    forgetSession(id);
    forgetDirectory(id);
    void closeSession(id, ending);
    setSessions(old => old.filter(item => item.id !== id));
    setLayout(old => removeWorkspaceTab(old, id));
  }, [forgetDirectory, updateRetry, dropAttention]);

  const activeId = activeSessionId(layout) ?? "";
  const active = useMemo(() => sessions.find(item => item.id === activeId), [sessions, activeId]);
  // Keeps the key handler from re-subscribing on every session state change.
  const stateRef = useRef({ activeId, sessions, profiles, layout });
  stateRef.current = { activeId, sessions, profiles, layout };

  useEffect(() => { localStorage.setItem(FONT_SIZE_KEY, String(fontSize)); }, [fontSize]);
  useEffect(() => {
    localStorage.setItem(COPY_ON_SELECT_KEY, String(copyOnSelect));
  }, [copyOnSelect]);
  useEffect(() => {
    try { localStorage.setItem(KEEP_SHELLS_KEY, String(keepShells)); } catch { /* not kept */ }
  }, [keepShells]);

  useEffect(() => { void passwordSupport().then(setPasswords); }, []);

  // Re-asked whenever the saved hosts change, which covers a host being added,
  // deleted or having its password forgotten. Cheap: one call, and the store
  // is answering about entries it already has open.
  useEffect(() => {
    if (!passwords?.available) return;
    void rememberedPasswords(profiles.map(profile => profile.id)).then(setRemembered);
  }, [profiles, passwords?.available]);

  // A connection that worked and a password that did not get saved. Said in
  // the notice line rather than swallowed: someone who ticked the box is owed
  // the news that it did not take.
  useEffect(() => {
    let stop: (() => void) | undefined;
    void onAuthNotice(({ message }) => setNotice(message)).then(fn => { stop = fn; });
    return () => stop?.();
  }, []);

  // The chrome is sized against a 1080p panel; a denser one draws the same CSS
  // pixel smaller. Set before the theme so the first paint is already at the
  // right size rather than growing into it.
  useEffect(trackDisplayScale, []);

  useEffect(() => {
    applyTheme(theme);
    rememberTheme(theme);
    // The file pane's ground, because the title bar sits directly above the
    // sidebar and reads as the top of it.
    void setCaptionColour(theme.chrome["--menu"], theme.chrome["--text"]);
  }, [theme]);

  // A shell that ended on its own takes its tab with it — that is what Ctrl-D
  // and `exit` mean, and leaving a dead tab behind was the bug. A connection
  // that dropped keeps the tab, because the session it carried is still what
  // the user wanted; the pane offers to reconnect.
  useEffect(() => {
    let stop: (() => void) | undefined;
    void onTerminalExit(({ id, reason }) => {
      if (reason === "exited") removeSession(id);
      else {
        setSessions(old => old.map(item => item.id === id ? { ...item, state: "lost" } : item));
        updateRetry(id, old => reconnect.lost(old, Date.now()));
      }
    }).then(fn => { stop = fn; });
    return () => stop?.();
  }, [removeSession, updateRetry]);

  useEffect(() => {
    if (!notice) return;
    const timer = window.setTimeout(() => setNotice(""), 6000);
    return () => window.clearTimeout(timer);
  }, [notice]);

  // The same shape as the notice's timer, on a much shorter clock. A copy
  // during the flash bumps the count, which tears this effect down and sets it
  // up again, so the second copy gets its own full moment on screen instead of
  // inheriting what was left of the first one's.
  useEffect(() => {
    if (!copies) return;
    const timer = window.setTimeout(() => setCopies(0), COPIED_FLASH_MS);
    return () => window.clearTimeout(timer);
  }, [copies]);

  /// Stable, because the pane wires this into its terminal once per session
  /// and never looks at the prop again.
  const noteCopy = useCallback(() => setCopies(count => count + 1), []);

  /// The workspace filling the window, by the id it had when zoomed. Only in
  /// effect while it still has focus; see `zoomGeometry`.
  const [zoomed, setZoomed] = useState<string | null>(null);
  const toggleZoom = useCallback(() => {
    const { layout: current } = stateRef.current;
    if (isZoomed(current, zoomedRef.current)) setZoomed(null);
    else if (canZoom(current)) setZoomed(current.focusedPaneId);
    else setNotice("Zoom fills the window with one workspace, so it needs a split first.");
  }, []);
  const zoomedRef = useRef(zoomed);
  zoomedRef.current = zoomed;

  /// What each open terminal can be asked to do, for the palette. A ref: the
  /// actions are closures over a terminal, and nothing renders from them.
  const paneActions = useRef(new Map<string, PaneActions>());
  const noteActions = useCallback((id: string, actions: PaneActions | null) => {
    if (actions) paneActions.current.set(id, actions);
    else paneActions.current.delete(id);
  }, []);
  const onActive = (run: (actions: PaneActions) => void) => () => {
    const actions = paneActions.current.get(stateRef.current.activeId);
    if (actions) run(actions);
  };

  /// Something in a tab asked for the user. From the tab they are looking at
  /// it is not news — which is also what keeps a shell's tab-completion bell
  /// from marking the tab it was typed in. From anywhere else it marks the
  /// tab, and with the window behind something, flashes the taskbar as well.
  /// A tab going quiet is a guess, so it never replaces what a program said.
  const noteAttention = useCallback((id: string, notice: Notice) => {
    const looking = document.hasFocus();
    if (looking && id === stateRef.current.activeId) return;
    setAttention(old => notice.quiet && old.has(id) ? old : new Map(old).set(id, notice));
    if (!looking) void requestWindowAttention().catch(() => {});
  }, []);

  // Looking at a tab is what answers it: focusing its workspace, switching to
  // it, or coming back to the window it is in front of.
  useEffect(() => {
    if (windowFocused && activeId) dropAttention(activeId);
  }, [activeId, windowFocused, dropAttention]);

  const copyAgentSetup = useCallback(async () => {
    try {
      await navigator.clipboard.writeText(AGENT_SETUP);
      setNotice("Setup copied. Each setting goes in the file it names, on the machine the agent runs on.");
    } catch {
      setNotice("Could not reach the clipboard.");
    }
  }, []);

  // Once, at startup. The answer is about this machine rather than any host,
  // so asking again per connection would be the same question re-run against
  // every failure; the banner's own button is the way to ask again.
  useEffect(() => { void checkOpenssh().then(setOpenssh).catch(() => {}); }, []);

  /**
   * Put the shell-integration snippet on the clipboard.
   *
   * Written through the browser rather than the native side, unlike the read
   * the terminal does: WebView2 puts a permission prompt in front of reading
   * someone's clipboard and none in front of writing to it.
   */
  const copyIntegration = useCallback(async () => {
    try {
      await navigator.clipboard.writeText(SHELL_INTEGRATION);
      setNotice("Snippet copied. Append it to ~/.bashrc or ~/.zshrc on the host, then open a new session.");
    } catch {
      setNotice("Could not reach the clipboard.");
    }
  }, []);

  /**
   * Put the snippet into the active session's startup file.
   *
   * A button and never automatic. It edits a file in someone's home directory
   * on a machine that is not afterm's, which is a thing to be asked for — but
   * asked for once, by someone who wants it, it is a great deal better than
   * making them paste a shell script into a file by hand. `install_public_key`
   * has always written to `~/.ssh/authorized_keys` on exactly these terms.
   *
   * The file pane no longer waits on this. It follows the shell without any
   * snippet, so what this buys is the command marks and an instant pane rather
   * than one a poll behind.
   */
  const installIntegration = useCallback(async () => {
    const session = stateRef.current.sessions.find(
      item => item.id === stateRef.current.activeId
    );
    if (!session) return;
    try {
      const done = await installShellIntegration(session.id, SHELL_INTEGRATION);
      const where = done.alreadyPresent
        ? `The snippet was already in ${done.path}.`
        : `Snippet added to ${done.path}.`;
      // A snippet in a file the login shell never reads is installed and inert,
      // and the user would have no way of telling that from a host that simply
      // ignored it. Saying so is the whole value of asking the host.
      const caveat = done.needsProfile
        ? " Your login shell reads ~/.bash_profile, which does not source ~/.bashrc — add `. ~/.bashrc` to it or the snippet will not run."
        : " Open a new session.";
      setNotice(where + caveat);
    } catch (problem) {
      setNotice(failureOf(problem).message);
    }
  }, []);

  const addSession = useCallback((profile: SessionProfile, remember: boolean) => {
    // `profileId` only where the host is being saved. An address typed in and
    // not kept has no identity that outlives the tab, so there is nothing for
    // a remembered password to belong to — and the dialog will not offer one.
    setSessions(old => [
      ...old,
      {
        ...profile,
        persistentShell: keepShellsRef.current && profile.persistentShell ? true : undefined,
        state: "connecting",
        profileId: remember ? profile.id : undefined
      }
    ]);
    setLayout(old => addWorkspaceTab(old, profile.id));
    setDialog(null);
    if (remember) {
      setProfiles(old => {
        const next = [...old.filter(item => !(item.host === profile.host && item.user === profile.user)), profile];
        void saveProfiles(next);
        return next;
      });
    }
  }, []);

  /// Reconnect a dropped session without disturbing its tab.
  ///
  /// The backend refuses to spawn over a session it already knows, so the dead
  /// one has to be dropped first — and awaited, or the respawn races it and is
  /// silently ignored. Bumping the attempt is what rebuilds the terminal.
  const retrySession = useCallback((id: string) => {
    // A reconnect is a new shell on a host that has been out of contact, so
    // nothing cached about its filesystem is worth trusting.
    forgetSession(id);
    forgetDirectory(id);
    void closeSession(id).then(() => {
      setSessions(old => old.map(item => item.id === id
        ? { ...item, state: "connecting", attempt: (item.attempt ?? 0) + 1 }
        : item));
    });
  }, [forgetDirectory]);

  /// Reconnect because the user said so: now, and with the five minutes of
  /// retrying started over.
  const retryByHand = useCallback((id: string) => {
    updateRetry(id, old => reconnect.manual(old, Date.now()));
    retrySession(id);
  }, [retrySession, updateRetry]);

  const stopRetrying = useCallback((id: string) => {
    updateRetry(id, old => old && reconnect.stopped(old));
  }, [updateRetry]);

  // The one timer behind every automatic attempt: it waits for whichever
  // session is due soonest, and is set again whenever any of them moves.
  useEffect(() => {
    const due = [...retrying.values()].flatMap(retry => retry.next === null ? [] : [retry.next]);
    if (due.length === 0) return;
    const timer = window.setTimeout(() => {
      const now = Date.now();
      for (const [id, retry] of retryingRef.current) {
        if (retry.next === null || retry.next > now) continue;
        updateRetry(id, old => old && reconnect.attempting(old));
        retrySession(id);
      }
    }, Math.max(0, Math.min(...due) - Date.now()));
    return () => window.clearTimeout(timer);
  }, [retrying, retrySession, updateRetry]);

  /// Open a session on a saved host, from the dialog or the palette. The
  /// dialog closes behind it: it asked where we were going and has its answer.
  const openProfile = useCallback((profile: SessionProfile) => {
    // The tab gets a new id every time; the saved host's own id travels
    // alongside it, which is what a remembered password is keyed to.
    const session: Session = {
      ...profile,
      persistentShell: keepShellsRef.current && profile.persistentShell ? true : undefined,
      id: crypto.randomUUID(),
      profileId: profile.id,
      state: "connecting"
    };
    setSessions(old => [...old, session]);
    setLayout(old => addWorkspaceTab(old, session.id));
    setDialog(null);
  }, []);

  /// Open a shell on this machine, from one of the two buttons on the dialog.
  ///
  /// Nothing is asked and nothing is kept: a local shell has no address to
  /// validate, no credential to remember and no saved entry to become. The
  /// dialog closes behind it the same way opening a host does.
  const openShell = useCallback((kind: LocalShellKind) => {
    const session = localSession(kind);
    setSessions(old => [...old, session]);
    setLayout(old => addWorkspaceTab(old, session.id));
    setDialog(null);
  }, []);

  /// Throw away every remembered password afterm can name, which is every
  /// saved host. Reported rather than assumed: a password someone believes is
  /// gone and is not is the one outcome this feature must never produce.
  const forgetEveryPassword = useCallback(async () => {
    const ids = remembered.slice();
    try {
      await forgetAllPasswords(ids);
      setRemembered([]);
      setNotice(ids.length === 1
        ? "The saved password was forgotten."
        : `${ids.length} saved passwords were forgotten.`);
    } catch (error: unknown) {
      setNotice(`Some passwords could not be forgotten: ${String(error)}`);
      void rememberedPasswords(profiles.map(profile => profile.id)).then(setRemembered);
    }
  }, [profiles, remembered]);

  /// Forget one host's password, leaving the host itself alone.
  const forgetHostPassword = useCallback((id: string) => {
    void forgetPassword(id)
      .then(() => {
        setRemembered(old => old.filter(item => item !== id));
        setNotice("The saved password for that host was forgotten.");
      })
      .catch((error: unknown) =>
        setNotice(`That password could not be forgotten: ${String(error)}`));
  }, []);

  const renameHost = useCallback((id: string, name: string) => {
    setProfiles(old => {
      const next = renameProfile(old, id, name);
      void saveProfiles(next);
      return next;
    });
  }, []);

  const keepShell = useCallback((id: string, keep: boolean) => {
    setProfiles(old => {
      const next = old.map(profile => profile.id === id ? { ...profile, persistentShell: keep } : profile);
      void saveProfiles(next);
      return next;
    });
  }, []);

  const forgetProfile = useCallback((id: string) => {
    // The password goes with the host. A saved credential outliving the host
    // it belonged to is an entry nobody can see and nobody can remove from
    // afterm, which is the one lifecycle failure worth ruling out here.
    void forgetPassword(id).catch((error: unknown) =>
      setNotice(`The saved host is gone, but its password could not be removed: ${String(error)}`));
    setRemembered(old => old.filter(item => item !== id));
    setProfiles(old => {
      const next = old.filter(item => item.id !== id);
      void saveProfiles(next);
      return next;
    });
  }, []);

  const markReady = useCallback((id: string) => {
    setSessions(old => old.map(item => item.id === id ? { ...item, state: "connected" } : item));
    updateRetry(id, old => old && reconnect.ready(old, Date.now()));
  }, [updateRetry]);

  /// An attempt to open a session failed. Inside an outage being retried, a
  /// host that is still unreachable leaves the tab lost, with the next attempt
  /// on the clock; anything else ends the retrying and the tab shows why.
  const markFailed = useCallback((id: string, failure: BackendFailure) => {
    const current = retryingRef.current.get(id);
    const after = current && reconnect.failed(current, failure, Date.now());
    updateRetry(id, () => after);
    const state = after ? "lost" : "error";
    setSessions(old => old.map(item => item.id === id ? { ...item, state } : item));
  }, [updateRetry]);

  // Sessions whose shell is currently sending OSC 133 marks, which is afterm
  // being told its snippet is in place rather than asked to guess.
  //
  // A set that both grows and shrinks. It used to only grow, on the first mark
  // ever seen, which made Settings claim integration for a tab that had since
  // been through `sudo -i` or a nested `ssh` into a shell with no snippet — and
  // kept claiming it after a reconnect. The pane reports what it can observe of
  // the shell in front of it now, and this follows.
  //
  // Held apart from `sessions` because it is not part of what a session is: a
  // reconnected tab has to hear a mark again before this claims one.
  const markIntegrated = useCallback((id: string, present: boolean) => {
    setIntegrated(old => {
      if (old.has(id) === present) return old;
      const next = new Set(old);
      if (present) next.add(id);
      else next.delete(id);
      return next;
    });
  }, []);

  const noteDirectory = useCallback((id: string, path: string) => {
    setDirectories(old => (old.get(id) === path ? old : new Map(old).set(id, path)));
  }, []);

  /// Sessions whose shell has announced a directory itself, through OSC 7.
  ///
  /// A ref rather than state: nothing renders differently for being in here,
  /// and a shell that starts announcing mid-session must take effect on the
  /// next report rather than on the next render.
  const announcing = useRef<Set<string>>(new Set());

  /// A directory the shell announced. Instant — it arrives with the prompt —
  /// and so it wins: a session heard from here is one the poll below stops
  /// answering for.
  const noteAnnouncedDirectory = useCallback((id: string, path: string) => {
    announcing.current.add(id);
    noteDirectory(id, path);
  }, [noteDirectory]);

  /// A directory read off the host by the telemetry poll, for the shells that
  /// announce nothing — which is every shell without the snippet, and so most
  /// of them. Up to one poll behind, where OSC 7 is immediate, and the two
  /// disagree on a nested `ssh`: the announcement carries a path on the far
  /// machine, this carries the client's own directory, which is the one this
  /// session can actually list. The announcement still wins where there is
  /// one, because latency is what the user feels and the nested case is rare.
  const noteProbedDirectory = useCallback((id: string, path: string) => {
    if (announcing.current.has(id)) return;
    noteDirectory(id, path);
  }, [noteDirectory]);

  const startRename = useCallback((id: string) => {
    const session = stateRef.current.sessions.find(item => item.id === id);
    if (!session) return;
    setDraftName(session.name);
    setRenaming(id);
  }, []);

  const commitRename = useCallback(() => {
    setRenaming(current => {
      if (current) {
        setSessions(old => old.map(item => {
          if (item.id !== current) return item;
          const trimmed = draftName.trim();
          // An empty name would leave an unclickable tab; keep the old one.
          return trimmed ? { ...item, name: trimmed } : item;
        }));
      }
      return null;
    });
  }, [draftName]);

  // Whether the session in front of you has a far end at all. A local shell
  // does not, and everything downstream of this — installing a public key, the
  // host on the file pane's top row — is about one.
  const remote = Boolean(active && !isLocalShell(active.kind));
  // Only the session in front of you can be spoken for: the snippet lives on a
  // host, and the other tabs are other hosts.
  const shellIntegrated = Boolean(active) && integrated.has(activeId);
  const canSplit = (workspacePanes(layout).find(pane => pane.id === layout.focusedPaneId)?.tabs.length ?? 0) > 1;

  const commands = useMemo<Command[]>(() => [
    { id: "new", label: "New session…", hint: "Ctrl Shift N", icon: <CirclePlus size={14}/>, run: () => setDialog(openingTab(profiles)) },
    { id: "rename", label: "Rename current session", hint: "Ctrl Shift R", icon: <Pencil size={14}/>, run: () => activeId && startRename(activeId) },
    { id: "close", label: "Close current session", hint: "Ctrl Shift W", icon: <X size={14}/>, run: () => activeId && removeSession(activeId, true) },
    ...(canSplit ? ([
      { id: "split-right", label: "Split tab right", hint: "Ctrl Shift Right", run: () => setLayout(old => splitWorkspace(old, "right")) },
      { id: "split-left", label: "Split tab left", hint: "Ctrl Shift Left", run: () => setLayout(old => splitWorkspace(old, "left")) },
      { id: "split-down", label: "Split tab down", hint: "Ctrl Shift Down", run: () => setLayout(old => splitWorkspace(old, "down")) },
      { id: "split-up", label: "Split tab up", hint: "Ctrl Shift Up", run: () => setLayout(old => splitWorkspace(old, "up")) }
    ] as Command[]) : []),
    { id: "sidebar", label: sidebar ? "Hide sidebar" : "Show sidebar", hint: "Ctrl Shift B", icon: <PanelLeftClose size={14}/>, run: () => setSidebar(value => !value) },
    { id: "fullscreen", label: "Toggle full screen", hint: "F11", icon: <Maximize size={14}/>, run: () => void toggleFullscreen() },
    { id: "files", label: "Show files", hint: "Ctrl Shift B", run: () => setSidebar(true) },
    { id: "hosts", label: "Saved hosts…", hint: "Ctrl Shift N", icon: <Server size={14}/>, run: () => setDialog("hosts") },
    { id: "bigger", label: "Increase terminal font size", hint: "Ctrl +", icon: <Plus size={14}/>, run: () => setFontSize(size => stepFontSize(size, 1)) },
    { id: "smaller", label: "Decrease terminal font size", hint: "Ctrl -", icon: <Minus size={14}/>, run: () => setFontSize(size => stepFontSize(size, -1)) },
    { id: "copy-on-select", label: copyOnSelect ? "Stop copying on select" : "Copy on select", hint: copyOnSelect ? "on" : "off", icon: <ClipboardCopy size={14}/>, run: () => setCopyOnSelect(value => !value) },
    ...THEMES.map(item => ({
      id: `theme-${item.id}`,
      label: `Colour scheme: ${item.label}`,
      hint: theme.id === item.id ? "current" : undefined,
      icon: <Palette size={14}/>,
      run: () => setTheme(item)
    })),
    { id: "previous-command", label: "Scroll to previous command", hint: "Ctrl Shift PgUp", run: onActive(actions => actions.previousCommand()) },
    { id: "next-command", label: "Scroll to next command", hint: "Ctrl Shift PgDn", run: onActive(actions => actions.nextCommand()) },
    { id: "copy-output", label: "Copy a command's output", hint: "Ctrl Shift O", icon: <ClipboardCopy size={14}/>, run: onActive(actions => actions.copyOutput()) },
    { id: "zoom", label: isZoomed(layout, zoomed) ? "Show every workspace" : "Zoom this workspace to fill the window", hint: "Ctrl Shift Z", icon: <Maximize size={14}/>, run: toggleZoom },
    { id: "quick-select", label: "Quick select: copy an address, hash or path", hint: "Ctrl Shift S", run: onActive(actions => actions.quickSelect()) },
    { id: "settings", label: "Settings", icon: <Settings size={14}/>, run: () => setSettings(true) },
    ...(remote
      ? [{ id: "key", label: "Install public key on this host", icon: <KeyRound size={14}/>, run: () => setKeyDialog(true) }]
      : []),
    ...profiles.map(profile => ({
      id: `profile-${profile.id}`,
      label: `Connect to ${profile.name}`,
      hint: `${profile.user}@${profile.host}`,
      icon: <Server size={14}/>,
      run: () => openProfile(profile)
    }))
  ], [
    activeId, canSplit, copyOnSelect, layout, openProfile, profiles, remote, removeSession, sidebar,
    startRename, theme, toggleZoom, zoomed
  ]);

  useEffect(() => {
    const handler = (event: KeyboardEvent) => {
      // Chords are claimed here in the capture phase. Without stopping
      // propagation the terminal would also receive them and send the control
      // byte to the shell: `Ctrl Shift N` as ^N, `Ctrl Shift Space` as NUL,
      // `Ctrl Shift W` as ^W — xterm drops the Shift when it encodes, which is
      // exactly why the shifted half was free to claim and exactly why it
      // still has to be stopped here.
      const claim = () => { event.preventDefault(); event.stopPropagation(); };
      const key = event.key.toLowerCase();
      const direction: SplitDirection | null = event.key === "ArrowLeft" ? "left"
        : event.key === "ArrowRight" ? "right"
          : event.key === "ArrowUp" ? "up"
            : event.key === "ArrowDown" ? "down" : null;
      // A focused terminal is not a field, whatever its hidden textarea looks
      // like. Reading it as one is what sent these chords to the shell.
      const editing = typingIntoField(event.target);

      // Ctrl+Shift rather than plain Shift, so `Shift + Arrow` stays the
      // terminal's: readline, editors and pagers all use it to extend a
      // selection, and a chord afterm claims is one no remote program can see.
      if (direction && event.shiftKey && event.ctrlKey && !event.altKey && !event.metaKey) {
        if (editing) return;
        claim();
        setLayout(old => {
          const next = splitWorkspace(old, direction);
          if (next === old) setNotice("A workspace needs at least two tabs before it can split.");
          return next;
        });
        return;
      }

      // Alt+Shift rather than plain Alt, for the same reason: readline binds
      // `Alt+Left`/`Alt+Right` to word movement as a built-in default, nano
      // binds `Alt+Up`/`Alt+Down` to scrolling, and fish spends all four.
      // `\e[1;4` is bound by none of them.
      if (direction && event.altKey && event.shiftKey && !event.ctrlKey && !event.metaKey) {
        if (editing) return;
        claim();
        setLayout(old => focusAdjacentWorkspace(old, direction));
        return;
      }

      // F11, the way a browser does it, and claimed for the same reason the
      // chords above are: xterm would otherwise send the key to the shell.
      // Unmodified only — Shift+F11 and the rest stay the terminal's.
      if (event.key === "F11" && !event.ctrlKey && !event.altKey && !event.shiftKey
        && !event.metaKey) {
        // In the browser preview the browser's own F11 already is this, so it
        // is left alone rather than fought over.
        if (!isDesktop()) return;
        claim();
        void toggleFullscreen();
        return;
      }

      if (!event.ctrlKey || event.altKey || event.metaKey) return;
      const { activeId: current } = stateRef.current;

      // Font size is the exception to the rule below, and can afford to be:
      // xterm's control-byte table has no entry for `-` or `=`, so neither
      // reaches the shell as anything whether Shift is down or not, and
      // `Ctrl -`/`Ctrl +` is the zoom chord every browser and editor already
      // uses. Both spellings, because `+` and `_` need Shift to type.
      if (key === "=" || key === "+") {
        claim();
        setFontSize(size => stepFontSize(size, 1));
        return;
      }
      if (key === "-" || key === "_") {
        claim();
        setFontSize(size => stepFontSize(size, -1));
        return;
      }

      // Tab is its own case too. No shell binds `Ctrl Tab` — a terminal has no
      // way to encode it — so Shift is free to be the direction here rather
      // than the modifier that makes the chord safe to take.
      if (event.key === "Tab") {
        claim();
        setLayout(old => cycleWorkspaceTab(old, event.shiftKey ? -1 : 1));
        return;
      }

      // Everything past here is `Ctrl Shift`, which costs the far end nothing.
      // xterm encodes `Ctrl`+letter as a single control byte and ignores Shift
      // doing it, and it speaks neither `modifyOtherKeys` nor the kitty
      // protocol, so `Ctrl Shift B` and `Ctrl B` arrive as the same `^B` and
      // no remote program can bind them apart. Taking the shifted half is
      // therefore free, and it leaves the unshifted half where it belongs:
      // tmux keeps its prefix, readline its reverse history search, vim its
      // window commands. It is also where every Linux terminal — GNOME
      // Terminal, Konsole, xfce4-terminal — already puts its own chords.
      if (!event.shiftKey) return;

      if (event.code === "Space" || key === " ") { claim(); setPalette(value => !value); }
      else if (key === "n") { claim(); setDialog(openingTab(stateRef.current.profiles)); }
      else if (key === "r") { claim(); if (current) startRename(current); }
      else if (key === "w") { claim(); if (current) removeSession(current, true); }
      else if (key === "b") { claim(); setSidebar(value => !value); }
      else if (key === "z") { claim(); toggleZoom(); }
    };
    window.addEventListener("keydown", handler, { capture: true });
    return () => window.removeEventListener("keydown", handler, { capture: true });
  }, [removeSession, startRename, toggleZoom]);


  // The banner is a row of the shell rather than an overlay: it is not an
  // event that passes, it is the state the machine is in until it is fixed.
  const missing = openssh !== null && openssh.missing.length > 0;

  return <main className={`app-shell ${missing ? "with-banner" : ""}`}>
    {missing && <MissingClient presence={openssh} onRecheck={setOpenssh}/>}
    <div className="workspace">
      <aside className={`sidebar ${sidebar ? "open" : "closed"}`}>
        {/* The host sits on the same row as the collapse button. It was a
            title saying FILES over a pane that is plainly a file pane, above a
            second row saying which host — two rows spent on one fact. */}
        <div className="sidebar-top">
          {remote && active && <span className="path" title={active.host}>
            <Server size={13}/><span>{active.host}</span>
          </span>}
          <button className="icon-button collapse" title="Hide sidebar (Ctrl Shift B)" onClick={() => setSidebar(false)}><PanelLeftClose size={16}/></button>
        </div>

        {/* The whole pane, rather than half a tab strip. Saved hosts were the
            other half and moved into the connect dialog, which is where a
            session is chosen; files are read for as long as one lasts. */}
        <FileBrowser
          session={active}
          directory={active ? directories.get(active.id) ?? null : null}
          onStatus={setNotice}
        />

        {/* Everything that is not a session lives behind this one button: the
            colour scheme, the font, the shortcut list and key installation. */}
        <div className="sidebar-footer">
          <button onClick={() => setSettings(true)}><Settings size={14}/> Settings</button>
        </div>
      </aside>

      <section className="terminal-region">
        <WorkspaceGrid
          layout={layout}
          zoomed={zoomed}
          onUnzoom={() => setZoomed(null)}
          sessions={sessions}
          sidebar={sidebar}
          fontSize={fontSize}
          theme={theme}
          copyOnSelect={copyOnSelect}
          renaming={renaming}
          draftName={draftName}
          onDraftName={setDraftName}
          onCommitRename={commitRename}
          onCancelRename={() => setRenaming(null)}
          onStartRename={startRename}
          onActivateTab={(paneId, sessionId) =>
            setLayout(old => activateWorkspaceTab(old, paneId, sessionId))}
          onMoveTab={(paneId, sessionId, index) =>
            setLayout(old => moveWorkspaceTab(old, paneId, sessionId, index))}
          onFocusPane={paneId => setLayout(old => focusWorkspace(old, paneId))}
          onCloseSession={id => removeSession(id, true)}
          onNewSession={paneId => {
            setLayout(old => focusWorkspace(old, paneId));
            setDialog(openingTab(profiles));
          }}
          onOpenSidebar={() => setSidebar(true)}
          onResizeSplit={(id, ratio) =>
            setLayout(old => resizeWorkspaceSplit(old, id, ratio))}
          onReady={markReady}
          onExit={markFailed}
          retrying={retrying}
          onRetry={retryByHand}
          onStopRetry={stopRetrying}
          onQuit={id => removeSession(id, true)}
          onIntegration={markIntegrated}
          onDirectory={noteAnnouncedDirectory}
          attention={attention}
          onAttention={noteAttention}
          onActions={noteActions}
          onCopy={noteCopy}
          onNotice={setNotice}
        />

        <StatusBar session={active} onDirectory={noteProbedDirectory}/>
      </section>
    </div>

    {/* Both of the things afterm says in the corner, in one column, because
        they can be on screen at once: a copy-on-select flash lands over
        whatever the file pane or a connection last reported. Stacked they
        read as two messages; overlaid, the short one would sit on top of the
        long one and hide the half of it that mattered. The flash goes
        underneath, nearest the corner, so an arriving one never pushes a
        notice the user is halfway through reading. */}
    {(notice || copies > 0) && <div className="corner-messages">
      {notice && <div className="toast">{notice}</div>}
      {/* Keyed on the count so a copy during the flash restarts the fade
          rather than leaving a faded element in place: a new key is a new
          element, and the animation starts from the top. */}
      {copies > 0 && <div key={copies} className="copied-flash" role="status">
        <ClipboardCheck size={12}/> Copied to clipboard
      </div>}
    </div>}

    <AuthDialog/>

    {palette && <CommandPalette commands={commands} onClose={() => setPalette(false)}/>}
    {dialog && <ConnectDialog
      profiles={profiles}
      tab={dialog}
      onClose={() => setDialog(null)}
      onConnect={addSession}
      onOpen={openProfile}
      onRename={renameHost}
      onKeepShell={keepShell}
      keepShells={keepShells}
      onForget={forgetProfile}
      remembered={remembered}
      onForgetPassword={forgetHostPassword}
      onLocalShell={openShell}
    />}

    {settings && <div className="dialog-backdrop" onMouseDown={event => event.target === event.currentTarget && setSettings(false)}>
      <div className="connect-dialog settings-dialog">
        <div className="dialog-title">
          <div><span className="eyebrow">PREFERENCES</span><h2>Settings</h2></div>
          <button type="button" className="icon-button" onClick={() => setSettings(false)}><X size={17}/></button>
        </div>
        <div className="settings-body">
          {/* Two columns rather than one long scroll: what there is to set is
              short, the shortcut list is long, and stacking them made a panel
              taller than the window it opens in. */}
          <div className="settings-column">
            <span className="section-label">COLOUR SCHEME</span>
            <div className="theme-picker">
              {THEMES.map(item => <button
                key={item.id}
                type="button"
                className={theme.id === item.id ? "selected" : ""}
                onClick={() => setTheme(item)}
              >
                {/* The only place in the interface where a colour is written
                    inline, because here the colour is the thing being chosen. */}
                <span className="swatch" style={{ background: item.chrome["--bg"], borderColor: item.chrome["--raise-2"] }}>
                  <i style={{ background: item.chrome["--text"] }}/>
                  <i style={{ background: item.chrome["--accent"] }}/>
                  <i style={{ background: item.chrome["--blue"] }}/>
                  <i style={{ background: item.chrome["--band"] }}/>
                </span>
                <span><strong>{item.label}</strong></span>
              </button>)}
            </div>

            <span className="section-label">TERMINAL</span>
            <div className="settings-row">
              <div>
                <label>Font size</label>
                <p>JetBrains Mono, {MIN_FONT_SIZE}–{MAX_FONT_SIZE} px.</p>
              </div>
              <div className="stepper">
                <button disabled={fontSize === MIN_FONT_SIZE} onClick={() => setFontSize(size => stepFontSize(size, -1))}><Minus size={13}/></button>
                <b>{fontSize} px</b>
                <button disabled={fontSize === MAX_FONT_SIZE} onClick={() => setFontSize(size => stepFontSize(size, 1))}><Plus size={13}/></button>
              </div>
            </div>

            {/* On by default, so this row is mostly read by someone who has
                come to turn it off. What it costs is named for them: every
                drag spends the clipboard, whatever the corner says. */}
            <div className="settings-row">
              <div>
                <label>Copy on select</label>
                <p>Highlighting with the mouse copies it, replacing the clipboard.</p>
              </div>
              <button
                type="button"
                role="switch"
                aria-checked={copyOnSelect}
                aria-label="Copy on select"
                className={`toggle ${copyOnSelect ? "on" : ""}`}
                onClick={() => setCopyOnSelect(value => !value)}
              ><span/></button>
            </div>

            {/* Off by default, and the cost is in the row, because this is the
                one setting that takes something away from every host it is
                used on. */}
            <div className="settings-row">
              <div>
                <label>Keep shells running in tmux</label>
                <p>{keepShells
                  ? "A saved host can keep its shell in tmux, so a dropped connection does not end it. Switch it on per host in the saved hosts list. tmux takes over the scrollback there, and afterm stops seeing commands."
                  : "Off. When on, a saved host can keep its shell running in tmux across dropped connections, at the cost of afterm's own scrollback and command tracking on that host."}</p>
              </div>
              <button
                type="button"
                role="switch"
                aria-checked={keepShells}
                aria-label="Keep shells running in tmux"
                className={`toggle ${keepShells ? "on" : ""}`}
                onClick={() => setKeepShells(value => !value)}
              ><span/></button>
            </div>

            {/* Says which of the two afterm is doing, because that is the only
                question anyone comes here with. */}
            <div className="settings-row">
              <div>
                <label>Shell integration</label>
                <p>{shellIntegrated
                  ? "Your shell marks where each command begins."
                  : "Commands are guessed from your keystrokes, which misses a pasted one or one recalled from history. The file pane follows this shell either way."}</p>
              </div>
              {shellIntegrated
                ? <span className="settings-state"><i/> Active</span>
                : <div className="settings-actions">
                  {remote && <button
                    type="button"
                    className="settings-action"
                    onClick={() => void installIntegration()}
                  ><FileDown size={13}/> Add to this host</button>}
                  <button
                    type="button"
                    className="settings-action"
                    onClick={() => void copyIntegration()}
                  ><ClipboardCopy size={13}/> Copy snippet</button>
                </div>}
            </div>

            {/* afterm cannot tell an agent is running on a host, only hear it
                ask; so this row is the instructions for making it ask. */}
            <div className="settings-row">
              <div>
                <label>Coding agents</label>
                <p>A tab you are not looking at is marked when the agent in it goes quiet after working, with nothing to set up. One setting on the host has Claude Code or Codex say why, and mark a tab waiting on an approval.</p>
              </div>
              <button
                type="button"
                className="settings-action"
                onClick={() => void copyAgentSetup()}
              ><ClipboardCopy size={13}/> Copy setup</button>
            </div>

            {/* Says what is saved before offering the button that clears it. */}
            <div className="settings-row">
              <div>
                <label>Saved passwords</label>
                <p>{!passwords
                  ? "Checking this machine…"
                  : !passwords.available
                    ? `Unavailable here: ${passwords.reason}.`
                    : remembered.length === 0
                      ? "None saved. A saved host offers to remember its password."
                      : remembered.length === 1
                        ? "One saved host has a password in this computer's credential store."
                        : `${remembered.length} saved hosts have passwords in this computer's credential store.`}</p>
              </div>
              {passwords?.available && remembered.length > 0 && <button
                type="button"
                className="settings-action"
                onClick={() => void forgetEveryPassword()}
              ><Trash2 size={13}/> Forget all</button>}
            </div>

            <span className="section-label">THIS SESSION</span>
            <div className="settings-row">
              <div>
                <label>Install public key</label>
                <p>{remote
                  ? `Append a key to ~/.ssh/authorized_keys on ${active?.host}.`
                  : "Open an SSH session first."}</p>
              </div>
              <button
                type="button"
                className="settings-action"
                disabled={!remote}
                onClick={() => setKeyDialog(true)}
              ><KeyRound size={13}/> Install…</button>
            </div>
          </div>

          <div className="settings-column">
            <span className="section-label">KEYBOARD SHORTCUTS</span>
            <ul className="shortcut-list">
              {SHORTCUTS.map(item => <li key={item.keys}><span>{item.action}</span><kbd>{item.keys}</kbd></li>)}
            </ul>
          </div>
        </div>
        <div className="dialog-actions"><button className="primary" onClick={() => setSettings(false)}>Done</button></div>
      </div>
    </div>}

    {keyDialog && active && <KeyDialog session={active} onClose={() => setKeyDialog(false)}/>}
  </main>;
}
