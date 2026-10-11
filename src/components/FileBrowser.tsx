import { Fragment, useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import { open as openFileDialog } from "@tauri-apps/plugin-dialog";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import {
  CornerLeftUp, Download, Ellipsis, Eye, File, Folder, FolderPlus, FolderUp, Link2, Pencil, RefreshCw, Trash2,
  Upload
} from "lucide-react";
import { ImageViewer } from "./ImageViewer";
import { previewable } from "../lib/preview";
import {
  authenticateSession,
  createRemoteDirectory,
  deleteRemoteEntry,
  downloadFile,
  downloadFolder,
  failureOf,
  isDesktop,
  localDownloads,
  listRemoteDirectory,
  localIsDirectory,
  onTransferProgress,
  remoteHome,
  renameRemoteEntry,
  uploadFile,
  uploadFolder
} from "../lib/backend";
import {
  baseName, elidePath, formatBytes, joinPath, nameProblem, pathBreaks, transferSummary,
  withLosses
} from "../lib/format";
import { isLocalShell } from "../lib/shells";
import type {
  BackendFailure, Collision, FileEntry, Listing, Session, SessionState, TransferProgress,
  Transferred
} from "../types";

interface Props {
  session: Session | undefined;
  /// Where this session's shell last said it is, or null if it has not said.
  directory: string | null;
  onStatus(message: string): void;
}

const ICONS = { directory: Folder, file: File, link: Link2 } as const;

/** What went wrong, said in the pane's own words rather than the backend's. */
const TITLES = {
  auth: "This host wants a credential",
  hostKey: "Host key not verified",
  unreachable: "Could not reach this host",
  unsupported: "Not supported for this host",
  missingClient: "The OpenSSH client is not available",
  operation: "Could not list this directory",
  exists: "Already exists",
  internal: "Something went wrong"
} as const;

/**
 * A dropped entry whose name is already taken where it was dropped, waiting
 * for the user to say what happens to it. `choose` is the upload loop's own
 * continuation: the loop is parked on it until one of the buttons is pressed.
 *
 * `folder` is what the question is about, and the two questions are not the
 * same one: replacing a file puts one file over another, where the answer for
 * a folder merges a tree into the one that is there and replaces only the
 * files inside it that collide. A button saying "Replace" over a folder would
 * be promising to remove what is there, which afterm does not do.
 */
interface CollisionPrompt {
  name: string;
  into: string;
  folder: boolean;
  choose(choice: Collision | "skip"): void;
}

/**
 * One line for the status bar about what a drop did.
 *
 * `uploaded` is one label per entry — a folder's carries its own file count —
 * and `files` is how many files went in total, which is not the same number
 * once a folder is among them. A drop of two folders is not "2 files".
 */
function uploadSummary(uploaded: string[], skipped: number, files: number): string {
  const parts: string[] = [];
  if (uploaded.length === 1) parts.push(`Uploaded ${uploaded[0]}`);
  else if (uploaded.length > 1) parts.push(`Uploaded ${files} files`);
  if (skipped > 0) parts.push(`skipped ${skipped}`);
  return parts.join(", ") || "Nothing uploaded";
}

/**
 * Listings already fetched, so walking back up a tree costs nothing.
 *
 * Module level rather than a ref, because the pane unmounts every time the
 * sidebar switches to Hosts and a cache that died with it would not survive
 * the way people actually browse. Keyed by session as well as path: two hosts
 * both have a `/etc`, and they are not the same `/etc`.
 *
 * Bounded, and oldest-first: a Map iterates in insertion order, so evicting
 * the first key evicts the least recently added. Directory listings are small,
 * but a long session walking a big tree should still not grow without end.
 */
const CACHE = new Map<string, Listing>();
const CACHE_LIMIT = 200;

const cacheKey = (session: string, path: string) => `${session}\u0000${path}`;

/** How many lines the path pane shows before it starts dropping its head. */
const PATH_LINES = 2;

function remember(session: string, path: string, listing: Listing) {
  const key = cacheKey(session, path);
  // Re-insert, so a path visited again becomes the newest and is evicted last.
  CACHE.delete(key);
  CACHE.set(key, listing);
  while (CACHE.size > CACHE_LIMIT) {
    const oldest = CACHE.keys().next().value;
    if (oldest === undefined) break;
    CACHE.delete(oldest);
  }
}

/**
 * Where each session's pane is, and which of its shell's directories it has
 * already followed.
 *
 * `PLACE` is what a pane is showing, so switching away to another session and
 * back returns to it rather than to the home directory. `FOLLOWED` is the last
 * directory the shell reported that the pane acted on, and it is the whole of
 * the rule that the pane moves once per change: a shell sends its directory at
 * every prompt, and a pane the user has browsed elsewhere must stay there
 * until the shell actually moves.
 *
 * Module level for the same reason the cache is: the pane unmounts whenever
 * the sidebar switches to Hosts, and neither of these is worth losing to that.
 */
const PLACE = new Map<string, string>();
const FOLLOWED = new Map<string, string>();

/**
 * Whether this run has already been told that a folder opens on a double
 * click. Module level for the reason the two maps above are: the pane unmounts
 * every time the sidebar switches to Hosts, and a hint that came back after
 * that would be a hint that never stops.
 */
let taught = false;

/** One path, dropped because something has changed underneath it. */
function forget(session: string, path: string) {
  CACHE.delete(cacheKey(session, path));
}

/** Everything remembered about one session, dropped when it ends. */
export function forgetSession(session: string) {
  for (const key of [...CACHE.keys()]) {
    if (key.startsWith(`${session}\u0000`)) CACHE.delete(key);
  }
  PLACE.delete(session);
  FOLLOWED.delete(session);
}

/**
 * What the pane is waiting for, per session state.
 *
 * It said "Connecting…" for every state that was not connected, which for a
 * session that had ended was simply untrue — and it said it forever.
 */
const WAITING: Record<SessionState, string> = {
  connecting: "Connecting…",
  connected: "Listing…",
  error: "This session could not start.",
  lost: "Connection lost."
};

/**
 * Real directory browsing over the session's own SFTP channel.
 *
 * One path through here rather than a local branch and a remote one. The pane
 * that browsed this machine went with the local shells, and did not come back
 * with them: a shell on this machine is already in front of the filesystem
 * this pane would show, and Explorer is one keystroke away. So a local shell
 * gets its name and nothing else, and every other session gets the far end.
 *
 * There is no placeholder data. Until a listing arrives the pane says what it
 * is waiting for, and a failure shows the reason the backend gave.
 */
export function FileBrowser({ session, directory, onStatus }: Props) {
  const [listing, setListing] = useState<Listing | null>(null);
  const [error, setError] = useState<BackendFailure | null>(null);
  const [busy, setBusy] = useState(false);
  const [transfer, setTransfer] = useState<TransferProgress | null>(null);
  const [selected, setSelected] = useState<FileEntry | null>(null);
  /// Path of the entry being renamed, and the name being typed — for that, or
  /// for a new directory when `creating` is set instead. One draft, because
  /// only one of the two editors is ever open.
  const [renaming, setRenaming] = useState<string | null>(null);
  const [creating, setCreating] = useState(false);
  /// Whether the toolbar's upload-and-create menu is open.
  const [adding, setAdding] = useState(false);
  const [draft, setDraft] = useState("");
  /// Path of the entry whose delete is waiting to be confirmed. A remote
  /// delete has nothing behind it — no bin, no undo — so the trash asks first,
  /// on the row itself rather than in a dialog over the pane.
  const [deleting, setDeleting] = useState<string | null>(null);
  /// Whether files dragged in from outside are over this pane.
  const [dropping, setDropping] = useState(false);
  /// The dropped file the pane is asking about, if any, and the way to answer.
  const [collision, setCollision] = useState<CollisionPrompt | null>(null);
  const choose = useRef<CollisionPrompt["choose"] | null>(null);
  /// Where a download lands, so the drag affordance can say so. Read once.
  const [downloads, setDownloads] = useState("");
  /// Which path the head is being dropped from, and how many pieces of it are
  /// standing behind the `..`. Kept together so a new path derives back to
  /// nothing dropped without an effect to reset it.
  const [elided, setElided] = useState({ path: "", drop: 0 });
  const pathBox = useRef<HTMLDivElement>(null);
  const listBox = useRef<HTMLDivElement>(null);
  /// The row a drag started on. The operating system is told nothing about the
  /// drag, so this is the only record that one is under way.
  const dragged = useRef<FileEntry | null>(null);
  // Guards against a slow listing for a previous session overwriting a newer one.
  const request = useRef(0);

  // Depend on identity, not on the session object: it is replaced on every
  // state change, which would otherwise rebuild `load` and send the pane back
  // to the home directory mid-browse.
  const sessionId = session?.id;
  /// Whose pane this is, readable from inside an operation that started under
  /// a different session.
  ///
  /// `request` alone cannot tell the two apart: an operation that outlives a
  /// host switch — an upload is the long one — holds the callbacks it was
  /// created with, so its refresh takes the *newest* ticket and wins, and
  /// host A's listing lands in host B's pane under host B's session id.
  /// Everything asynchronous checks this before it touches the pane. Assigned
  /// during the render, so it is already the new session by the time any
  /// effect for it runs.
  const active = useRef(sessionId);
  active.current = sessionId;
  /// Whether this session is a shell on this machine, which has no far end to
  /// list. Everything asynchronous below is gated through `ready`, so nothing
  /// is asked of a session that could only answer with an error.
  const local = Boolean(session && isLocalShell(session.kind));
  // The backend only knows about a session once its process has been spawned.
  // Listing before that races the spawn and fails with "session does not
  // exist", so the pane waits to be told the session is up.
  const ready = session?.state === "connected" && !local;

  /**
   * Show a directory, from the cache when it has been seen before.
   *
   * `fresh` is what Refresh and a completed upload ask for: everything else is
   * navigation, and navigation should not pay a round trip to show a directory
   * it already has. A cached listing can be out of date — the remote is not
   * afterm's to watch — which is what Refresh is for.
   *
   * `quiet` is for a move the user did not ask for: following the shell. A
   * directory the shell moved to is not always one this pane can list — a
   * nested `ssh`, a `docker exec` or a `sudo` into a different filesystem all
   * report a path belonging to somewhere else — and replacing a good listing
   * with an error nobody asked for is worse than not moving.
   */
  const load = useCallback(
    async (path: string, fresh = false, quiet = false) => {
      if (!sessionId || sessionId !== active.current) return;
      const ticket = (request.current += 1);
      const mine = () => ticket === request.current && sessionId === active.current;
      const cached = fresh ? undefined : CACHE.get(cacheKey(sessionId, path));
      if (cached) {
        PLACE.set(sessionId, path);
        setListing(cached);
        setSelected(null);
        setError(null);
        setBusy(false);
        return;
      }
      setBusy(true);
      setError(null);
      try {
        const next = await listRemoteDirectory(sessionId, path);
        // Remembered whatever happened next: the listing is true of the
        // session it was asked of, whether or not that is still the one on
        // screen, and `forgetSession` drops it when the session ends.
        remember(sessionId, path, next);
        if (!mine()) return;
        PLACE.set(sessionId, path);
        setListing(next);
        setSelected(null);
      } catch (problem) {
        if (!mine()) return;
        if (quiet) return;
        setError(failureOf(problem));
        setListing(null);
      } finally {
        if (mine()) setBusy(false);
      }
    },
    [sessionId]
  );

  /**
   * Where the pane opens, and where it moves to on its own.
   *
   * Three answers, in order.
   *
   * The shell's directory, when it has moved since the pane last followed it.
   * That is one move per `cd` and no more: the directory arrives at every
   * prompt, and `FOLLOWED` is what tells a shell that has moved from a shell
   * that is merely saying where it still is. Without that the pane would snap
   * back the moment the user browsed anywhere else.
   *
   * Otherwise wherever this session's pane already was. Switching to another
   * session and back is not a directory change, and it must not cost the place
   * the user navigated to by hand.
   *
   * Otherwise the session's home directory, which is a pane that has never
   * been opened on this session and a shell that has never said anything.
   */
  useEffect(() => {
    if (!sessionId || !ready) {
      setListing(null);
      setError(null);
      return;
    }
    const follow = directory && directory !== FOLLOWED.get(sessionId) ? directory : null;
    if (follow) FOLLOWED.set(sessionId, follow);
    const known = follow ?? PLACE.get(sessionId);
    let cancelled = false;
    const ticket = (request.current += 1);
    setBusy(true);
    setError(null);
    // Only when there is nothing to go to yet: blanking a pane that is about
    // to show a directory it already has would be a flash for nothing.
    if (!known) setListing(null);
    (async () => {
      try {
        const where = known ?? await remoteHome(sessionId);
        if (cancelled || ticket !== request.current) return;
        await load(where, false, Boolean(follow));
      } catch (problem) {
        if (cancelled || ticket !== request.current) return;
        setError(failureOf(problem));
        setBusy(false);
      }
    })();
    return () => { cancelled = true; };
  }, [sessionId, ready, directory, load]);

  // An open editor belongs to the directory it was opened in, and to nothing
  // else: navigating away or switching session closes it rather than leaving
  // it pointing at a row that is no longer on screen.
  useEffect(() => {
    setRenaming(null);
    setCreating(false);
    setDeleting(null);
  }, [listing?.path, sessionId]);

  // A question about a drop into one session is not answered by switching to
  // another, and never by unmounting. Either way the loop parked on it is let
  // go, with the answer that changes nothing.
  useEffect(() => () => { choose.current?.("skip"); }, [sessionId]);

  useEffect(() => {
    if (!isDesktop()) return;
    void localDownloads().then(setDownloads).catch(() => undefined);
  }, []);

  const path = listing?.path ?? "";
  // Derived, not stored: a path that has just changed has dropped nothing yet.
  const drop = elided.path === path ? elided.drop : 0;
  const shownPath = useMemo(() => elidePath(pathBreaks(path), drop), [path, drop]);

  // Drop one more piece off the head until the tail fits in two lines.
  //
  // Measured rather than reasoned about: whether a piece fits depends on the
  // font and the width of the pane, and dropping one changes where every
  // piece after it wraps. Counting distinct offsets is what asks "how many
  // lines is this on" without being fooled by a horizontal scrollbar, which
  // takes height out of the box the moment a single name is too wide for it.
  //
  // A layout effect, so the loop settles before the browser paints, and it
  // terminates because `drop` only ever climbs and stops at the last piece.
  useLayoutEffect(() => {
    const box = pathBox.current;
    if (!box) return;
    const rows = new Set<number>();
    for (const piece of box.querySelectorAll("span")) rows.add(Math.round(piece.offsetTop));
    if (rows.size > PATH_LINES && drop < pathBreaks(path).length - 1) {
      setElided({ path, drop: drop + 1 });
    }
  });

  // A narrower pane fits less, and a wider one may fit the whole path again,
  // so both start the measurement over from nothing dropped.
  useEffect(() => {
    const box = pathBox.current;
    if (!box) return;
    let width = box.clientWidth;
    const observer = new ResizeObserver(() => {
      if (box.clientWidth === width) return;
      width = box.clientWidth;
      setElided({ path: "", drop: 0 });
    });
    observer.observe(box);
    return () => observer.disconnect();
  }, []);

  // Transfers report progress only over the native transport; the OpenSSH
  // client's one-shot `get` says nothing until it has finished.
  useEffect(() => {
    let stop: (() => void) | undefined;
    void onTransferProgress(value => {
      if (value.id !== sessionId) return;
      setTransfer(value.total !== null && value.done >= value.total ? null : value);
    }).then(fn => { stop = fn; });
    return () => stop?.();
  }, [sessionId]);

  /**
   * Take entries dropped onto the pane, or picked with a button, one at a
   * time.
   *
   * Sequentially rather than at once: they share one SFTP connection, and the
   * progress note has room for one name.
   *
   * A folder and a file are the same shape here — both are sent under one
   * answer about a name already taken, and both come back saying where they
   * landed and how many files went. Which one a path is has to be asked of the
   * backend: a drop carries paths and nothing else, and the page has no
   * filesystem to look at.
   */
  const receive = useCallback(async (paths: string[], into: string) => {
    if (!sessionId) return;
    const uploaded: string[] = [];
    let skipped = 0;
    // Losses from inside the folders, which are not entries the user dropped
    // and do not belong in the count of them.
    let lostSkipped = 0;
    let lostFailed = 0;
    let files = 0;
    try {
      for (const local of paths) {
        const name = baseName(local);
        const folder = await localIsDirectory(local);
        const send = async (choice: Collision): Promise<Transferred> => {
          if (folder) return uploadFolder(sessionId, local, into, choice);
          const landed = await uploadFile(sessionId, local, into, choice);
          return { path: landed, files: 1, skipped: 0, failed: 0 };
        };
        // Never over something already there without asking. The backend
        // refuses with a cause of its own, and that refusal is what the pane
        // asks on — so a stale listing cannot make it skip the question.
        let result: Transferred;
        try {
          result = await send("fail");
        } catch (problem) {
          if (failureOf(problem).cause !== "exists") throw problem;
          // Only the pane on screen can ask, and by now it may be showing a
          // different host. A question nobody can be asked is answered the
          // way an unanswered one is: nothing is replaced.
          if (sessionId !== active.current) { skipped += 1; continue; }
          const choice = await new Promise<Collision | "skip">(resolve => {
            choose.current = resolve;
            setCollision({ name, into, folder, choose: resolve });
          });
          choose.current = null;
          setCollision(null);
          if (choice === "skip") { skipped += 1; continue; }
          result = await send(choice);
        }
        const asName = baseName(result.path);
        const label = asName === name ? name : `${name} as ${asName}`;
        uploaded.push(folder
          ? `${label} (${result.files === 1 ? "1 file" : `${result.files} files`})`
          : label);
        files += result.files;
        lostSkipped += result.skipped;
        lostFailed += result.failed;
      }
      onStatus(withLosses(uploadSummary(uploaded, skipped, files), lostSkipped, lostFailed));
      // The directory has changed, so what is remembered about it is wrong.
      // `load` refreshes the pane when this is still the session it belongs
      // to; when it is not, dropping the entry is all that can be done from
      // here, and the next visit fetches it again.
      if (uploaded.length > 0) {
        forget(sessionId, into);
        void load(into, true);
      }
    } catch (problem) {
      onStatus(failureOf(problem).message);
    } finally {
      if (sessionId === active.current) setTransfer(null);
    }
  }, [sessionId, load, onStatus]);

  /// Where a drop would land, and what would take it. Read at drop time
  /// rather than captured, because the listener below outlives any one
  /// listing: subscribing again on every navigation would churn a webview-wide
  /// subscription for a value the handler can simply look up.
  const dropInto = useRef<string | null>(null);
  dropInto.current = listing ? listing.path : null;
  const receiveNow = useRef(receive);
  receiveNow.current = receive;

  /**
   * Files dragged in from Explorer.
   *
   * The page never sees these. Tauri takes the operating system's drop itself
   * and reports it here with real paths, which is the only way to learn one:
   * a browser's own drop event hands over content, not a location. The
   * position it reports is in physical pixels, so it comes back down through
   * the display scale before it is compared with a layout box.
   */
  useEffect(() => {
    if (!isDesktop()) return;
    let stop: (() => void) | undefined;
    // The subscription is a promise, so a pane that unmounts before it
    // resolves has nothing to cancel yet and would leak a live listener.
    let gone = false;
    const overList = (position: { x: number; y: number }) => {
      const box = listBox.current?.getBoundingClientRect();
      if (!box) return false;
      const scale = window.devicePixelRatio || 1;
      const x = position.x / scale;
      const y = position.y / scale;
      return x >= box.left && x <= box.right && y >= box.top && y <= box.bottom;
    };
    void getCurrentWebview().onDragDropEvent(event => {
      const payload = event.payload;
      if (payload.type === "leave") {
        setDropping(false);
        return;
      }
      // Only an SSH session has anywhere to put them: a local pane is already
      // showing the machine the files are on.
      const into = dropInto.current;
      const inside = into !== null && overList(payload.position);
      if (payload.type === "drop") {
        setDropping(false);
        if (into && inside) void receiveNow.current(payload.paths, into);
      } else {
        setDropping(inside);
      }
    }).then(fn => { if (gone) fn(); else stop = fn; });
    return () => { gone = true; stop?.(); };
  }, []);

  // Above the early returns below, like every hook here: a hook that runs only
  // for a remote session changes the count React sees, and it stops.
  /// The image being looked at, as an index into this directory's images.
  const [viewing, setViewing] = useState<number | null>(null);
  const images = useMemo(() => listing?.entries.filter(previewable) ?? [], [listing]);
  // Another directory or session is another set of images; the index would
  // point into a list that is no longer the one on screen.
  useEffect(() => setViewing(null), [listing?.path, sessionId]);

  if (!session) {
    return <div className="pane-note"><strong>No session</strong>Open a session to browse its files.</div>;
  }

  if (local) {
    return <div className="pane-note"><strong>{session.name}</strong></div>;
  }

  if (!isDesktop()) {
    return <div className="pane-note">
      <strong>Preview mode</strong>
      File browsing reads a real filesystem, so it needs the native backend.
      Run <code>npm run tauri dev</code>.
    </div>;
  }

  /**
   * What a single click does, which is select and nothing else.
   *
   * It used to open a directory. That left no way to *hold* one — and so no
   * way to download one, because every button in the toolbar acts on what is
   * selected, and a folder could not be. Selecting on a click and opening on a
   * double click is what WinSCP, MobaXterm and every file manager on either
   * desktop already do, so it is also what a user arriving here expects.
   *
   * The gesture moved, so the first folder click of the run says where it went
   * to. Once per run rather than once per click: this line is shared with
   * transfers and failures, and a hint that repeats forever is noise over the
   * messages that are not hints.
   */
  const select = (entry: FileEntry) => {
    setSelected(entry);
    // Directories only. A link is the one row where the hint would be a small
    // lie: a double click opens it if it points at a directory, and fails if
    // it points at a file.
    if (entry.kind !== "directory" || taught) return;
    taught = true;
    onStatus(`Double-click to open ${entry.name} — selecting it is what lets Download take the whole folder`);
  };

  const preview = (entry: FileEntry) => {
    const at = images.findIndex(image => image.path === entry.path);
    if (at >= 0) setViewing(at);
  };

  /// What a double click does: opens a directory, shows an image, and for any
  /// other file does nothing and says nothing — it is already selected, and
  /// the click that selected it said so.
  const enter = (entry: FileEntry) => {
    if (previewable(entry)) return preview(entry);
    if (entry.kind === "file") return;
    void load(entry.path);
  };

  const authenticate = async () => {
    if (!sessionId) return;
    setBusy(true);
    try {
      await authenticateSession(sessionId);
      if (sessionId !== active.current) return;
      setError(null);
      await load(listing?.path ?? await remoteHome(sessionId));
    } catch (problem) {
      if (sessionId === active.current) setError(failureOf(problem));
    } finally {
      if (sessionId === active.current) setBusy(false);
    }
  };

  /**
   * Copy one remote entry into the Downloads folder.
   *
   * The backend picks the local name in both cases: never outside Downloads,
   * never over anything already there, so it may land under `name (1)`. The
   * status says which.
   *
   * A folder says it has started, because it may have nothing to show for a
   * while: the progress note below is drawn from transfers, and a folder deep
   * enough to take a moment to walk has none to report yet. A folder also
   * reports what it brought rather than only where it went — see
   * `downloadSummary`.
   */
  const fetchDown = async (entry: FileEntry) => {
    try {
      if (entry.kind === "directory") {
        onStatus(`Downloading ${entry.name}…`);
        const result = await downloadFolder(session.id, entry.path);
        onStatus(transferSummary("Downloaded", entry.name, result));
        return;
      }
      const saved = await downloadFile(session.id, entry.path);
      onStatus(baseName(saved) === entry.name
        ? `Downloaded ${entry.name} to ${saved}`
        : `Downloaded ${entry.name} as ${saved}`);
    } catch (problem) {
      onStatus(failureOf(problem).message);
    } finally {
      // A download that outlived a host switch has no progress of this pane's
      // to clear.
      if (session.id === active.current) setTransfer(null);
    }
  };

  /// The system picker, for a file or for a folder. Two buttons rather than
  /// one: a dialog picks files or directories, never both, on either desktop.
  const pickAndSend = async (directory: boolean) => {
    if (!listing) return;
    try {
      const picked = await openFileDialog({ multiple: false, directory });
      if (typeof picked !== "string") return;
      await receive([picked], listing.path);
    } catch (problem) {
      onStatus(failureOf(problem).message);
    }
  };

  /**
   * Whether a drag was let go outside the list, which is what means download.
   *
   * A webview cannot hand a file to Explorer: the operating system asks for
   * the bytes at the moment of the drop, and a file on another machine has
   * none to give yet. So the gesture is taken for what it means rather than
   * pretended at — the file is fetched to the Downloads folder, the same place
   * the Download button puts it, and the status bar says where it went.
   */
  const releasedOutside = (x: number, y: number) => {
    const box = listBox.current?.getBoundingClientRect();
    if (!box) return false;
    // A drag let go beyond the window has no position worth reading, and that
    // is the case this is most for.
    if (!x && !y) return true;
    return x < box.left || x > box.right || y < box.top || y > box.bottom;
  };

  const commitNew = async () => {
    const name = draft.trim();
    setCreating(false);
    if (!listing || !name) return;
    const problem = nameProblem(name);
    if (problem) { onStatus(problem); return; }
    try {
      const target = joinPath(listing.path, name);
      await createRemoteDirectory(session.id, target);
      onStatus(`Created ${name}`);
      void load(listing.path, true);
    } catch (failure) {
      onStatus(failureOf(failure).message);
    }
  };

  const commitRename = async () => {
    const from = renaming;
    const name = draft.trim();
    setRenaming(null);
    if (!listing || !from) return;
    const entry = listing.entries.find(item => item.path === from);
    // Nothing typed, or nothing changed: not a rename, and not an error either.
    if (!entry || name === entry.name) return;
    const problem = nameProblem(name);
    if (problem) { onStatus(problem); return; }
    try {
      const to = joinPath(listing.path, name);
      await renameRemoteEntry(session.id, from, to);
      onStatus(`Renamed ${entry.name} to ${name}`);
      void load(listing.path, true);
    } catch (failure) {
      onStatus(failureOf(failure).message);
    }
  };

  /** Open the rename editor on one row, closing whatever else was open. */
  const startRename = (entry: FileEntry) => {
    setCreating(false);
    setDeleting(null);
    setRenaming(entry.path);
    setDraft(entry.name);
  };

  const commitDelete = async (entry: FileEntry) => {
    setDeleting(null);
    if (!listing) return;
    try {
      await deleteRemoteEntry(session.id, entry.path, entry.kind === "directory");
      onStatus(`Deleted ${entry.name}`);
      if (selected?.path === entry.path) setSelected(null);
      void load(listing.path, true);
    } catch (failure) {
      onStatus(failureOf(failure).message);
    }
  };

  /** Both editors are one keystroke from committing and one from giving up. */
  const editorKeys = (commit: () => void, cancel: () => void) =>
    (event: React.KeyboardEvent) => {
      // The application's own chords are above this, and would close the tab
      // rather than type a `w`.
      event.stopPropagation();
      if (event.key === "Enter") commit();
      else if (event.key === "Escape") cancel();
    };

  return <>
    {/* Each piece is its own unbreakable span with a <wbr> after it, so the
        only place the line can wrap is a separator. The spans are what does
        the work: a browser breaks at a hyphen by default, which would split
        `customer-integrations` down the middle without them. */}
    <div className="path-full" ref={pathBox} title={path}>
      {listing
        ? shownPath.map((piece, index) =>
            <Fragment key={index}><span>{piece}</span><wbr/></Fragment>)
        : "…"}
    </div>

    {/* Parent, Refresh and Download stay in the row; everything that adds to
        the directory is behind the menu at its end. Six buttons did not fit
        the pane at its narrowest, and these three are the ones used without
        thinking. The menu opens on hover and on a click, so it is still
        reachable with no pointer to hover with. */}
    <div className="file-toolbar">
      <button
        className="square"
        aria-label="Parent directory"
        title="Parent directory"
        disabled={!listing?.parent}
        onClick={() => listing?.parent && void load(listing.parent)}
      ><CornerLeftUp size={14}/></button>
      <button
        className="square"
        aria-label="Refresh"
        title="Refresh"
        disabled={!listing || busy}
        onClick={() => listing && void load(listing.path, true)}
      ><RefreshCw size={14}/></button>
      <button disabled={!selected} onClick={() => selected && void fetchDown(selected)}>
        <Download size={14}/> Download
      </button>
      <div
        className={`file-more ${adding ? "open" : ""}`}
        onMouseEnter={() => listing && setAdding(true)}
        onMouseLeave={() => setAdding(false)}
      >
        <button
          className="square"
          aria-label="Upload or create"
          aria-haspopup="menu"
          aria-expanded={adding}
          title="Upload or create"
          disabled={!listing}
          onClick={() => setAdding(value => !value)}
        ><Ellipsis size={14}/></button>
        {adding && <div className="file-more-menu" role="menu">
          <button role="menuitem" onClick={() => { setAdding(false); void pickAndSend(false); }}>
            <Upload size={14}/> Upload files
          </button>
          <button role="menuitem" onClick={() => { setAdding(false); void pickAndSend(true); }}>
            <FolderUp size={14}/> Upload a folder
          </button>
          <button role="menuitem" onClick={() => { setAdding(false); setRenaming(null); setDraft(""); setCreating(true); }}>
            <FolderPlus size={14}/> New folder
          </button>
        </div>}
      </div>
    </div>

    {error && <div className="pane-note error">
      <strong>{TITLES[error.cause]}</strong>
      {error.message}
      {error.canPrompt
        ? <button disabled={busy} onClick={() => void authenticate()}>
            {busy ? "Connecting…" : "Enter password"}
          </button>
        : <button disabled={busy} onClick={() => void load(listing?.path ?? "/", true)}>
            {busy ? "Trying…" : "Try again"}
          </button>}
    </div>}

    {transfer && <div className="pane-note transfer">
      <strong>{transfer.name}</strong>
      {transfer.total === null
        ? formatBytes(transfer.done)
        : `${formatBytes(transfer.done)} of ${formatBytes(transfer.total)}`}
      <progress value={transfer.done} max={transfer.total ?? undefined}/>
    </div>}

    {collision && <div className="pane-note collision">
      <strong>{collision.name} already exists</strong>
      {collision.folder
        ? <>A folder with this name is already in {collision.into}. Merge into it —
          files with the same name are replaced, and everything else is left
          alone — or keep both, or skip this one.</>
        : <>A file with this name is already in {collision.into}. Replace it, keep
          both, or skip this one.</>}
      <span className="confirm">
        <button className="danger" onClick={() => collision.choose("replace")}>
          {collision.folder ? "Merge" : "Replace"}
        </button>
        <button onClick={() => collision.choose("rename")}>Keep both</button>
        <button onClick={() => collision.choose("skip")}>Skip</button>
      </span>
    </div>}

    {!error && !ready && <div className="pane-note">{WAITING[session.state]}</div>}
    {!error && ready && busy && !listing && <div className="pane-note">Listing…</div>}

    {!error && listing && <div className={`file-list ${dropping ? "dropping" : ""}`} ref={listBox}>
      <div className="file-list-head"><span>Name</span><span>Size</span></div>

      {creating && <div className="file-row directory renaming">
        <span>
          <Folder size={15}/>
          <input
            className="tab-rename"
            autoFocus
            aria-label="New folder name"
            placeholder="New folder"
            value={draft}
            onChange={event => setDraft(event.target.value)}
            onBlur={() => void commitNew()}
            onKeyDown={editorKeys(() => void commitNew(), () => setCreating(false))}
          />
        </span>
      </div>}

      {listing.entries.length === 0 && !creating
        && <div className="pane-note">This directory is empty.</div>}

      {listing.entries.map(entry => {
        const Icon = ICONS[entry.kind];
        // A row being renamed is a div rather than the usual button: an input
        // inside a button cannot be typed into properly, and navigating on
        // every click would fight the caret.
        if (renaming === entry.path) {
          return <div key={entry.path} className={`file-row ${entry.kind} renaming`}>
            <span>
              <Icon size={15}/>
              <input
                className="tab-rename"
                autoFocus
                aria-label={`Rename ${entry.name}`}
                value={draft}
                onChange={event => setDraft(event.target.value)}
                onBlur={() => void commitRename()}
                onKeyDown={editorKeys(() => void commitRename(), () => setRenaming(null))}
              />
            </span>
          </div>;
        }
        // Likewise a div rather than a button while it asks: the row carries
        // two buttons of its own, and a button cannot hold buttons.
        if (deleting === entry.path) {
          return <div key={entry.path} className={`file-row ${entry.kind} confirming`}>
            <span><Icon size={15}/><span>{entry.name}</span></span>
            <span className="confirm">
              {/* A folder goes with everything in it, and the button says so
                  before it is pressed rather than after. */}
              <button
                className="danger"
                title={entry.kind === "directory" ? `Delete ${entry.name} and everything in it` : undefined}
                onClick={() => void commitDelete(entry)}
              >{entry.kind === "directory" ? "Delete all" : "Delete"}</button>
              <button onClick={() => setDeleting(null)}>Cancel</button>
            </span>
          </div>;
        }
        return <button
          key={entry.path}
          className={`file-row ${entry.kind} ${entry.kind !== "file" ? "navigable" : ""} ${selected?.path === entry.path ? "selected" : ""}`}
          onClick={() => select(entry)}
          onDoubleClick={() => enter(entry)}
          // A row is a button, so the keyboard reaches it: Enter opens what a
          // double click opens, and the click Enter also fires is what
          // selects it. Without this a directory could be focused and
          // selected from the keyboard but never entered from it.
          onKeyDown={event => { if (event.key === "Enter") enter(entry); }}
          title={entry.path}
          draggable
          onDragStart={event => {
            dragged.current = entry;
            event.dataTransfer.effectAllowed = "copy";
            // The gesture cannot end in Explorer — see `releasedOutside` — so
            // it says where it will end instead, before the row is let go.
            onStatus(`Let go outside the file pane to download ${entry.name} to ${downloads || "your Downloads folder"}`);
          }}
          onDragEnd={event => {
            const moved = dragged.current;
            dragged.current = null;
            if (moved && releasedOutside(event.clientX, event.clientY)) void fetchDown(moved);
          }}
        >
          <span><Icon size={15}/><span>{entry.name}</span></span>
          <small>{entry.size === null ? "" : formatBytes(entry.size)}</small>
          <span className="row-actions">
            {previewable(entry) && <span
              role="button"
              title={`Preview ${entry.name}`}
              onClick={event => { event.stopPropagation(); preview(entry); }}
              onDoubleClick={event => event.stopPropagation()}
            ><Eye size={12}/></span>}
            <span
              role="button"
              title={`Rename ${entry.name}`}
              onClick={event => { event.stopPropagation(); startRename(entry); }}
              // Two clicks on a row action are two clicks on the row, which
              // would now open the directory underneath it.
              onDoubleClick={event => event.stopPropagation()}
            ><Pencil size={12}/></span>
            <span
              className="delete"
              role="button"
              title={`Delete ${entry.name}`}
              onClick={event => {
                event.stopPropagation();
                setRenaming(null);
                setDeleting(entry.path);
              }}
              onDoubleClick={event => event.stopPropagation()}
            ><Trash2 size={12}/></span>
          </span>
        </button>;
      })}
    </div>}
    {viewing !== null && sessionId && <ImageViewer
      sessionId={sessionId}
      images={images}
      start={viewing}
      onClose={() => setViewing(null)}
      onDownload={entry => void fetchDown(entry)}
    />}
  </>;
}
