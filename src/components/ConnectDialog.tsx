import { useEffect, useRef, useState } from "react";
import { KeyRound, Layers, Pencil, Plus, Server, SquareTerminal, Trash2, X } from "lucide-react";
import { typingIntoField } from "../lib/keys";
import { isSafeSshIdentity, profileName } from "../lib/profile";
import { LOCAL_SHELLS, type LocalShellKind } from "../lib/shells";
import type { SessionProfile } from "../types";

/**
 * The two halves of the same question. Saved hosts are where most sessions
 * come from, so they are the front page; the form is for the host that is not
 * saved yet.
 */
export type ConnectTab = "hosts" | "new";

/// How many saved hosts answer to a key. Nine, because the numbers are one
/// keystroke each and `0` reads as a tenth to nobody.
const NUMBERED = 9;

/** Which tab to open on: the host list, unless there is no host list. */
export function openingTab(profiles: SessionProfile[]): ConnectTab {
  return profiles.length > 0 ? "hosts" : "new";
}

/** The number a saved host answers to, or none once the list runs past nine. */
export function hostNumber(index: number): number | null {
  return index < NUMBERED ? index + 1 : null;
}

/**
 * The host a keystroke opens, if it opens one.
 *
 * Only the digits, and only as far as the numbers on the rows go: a list of
 * three does nothing with `7` rather than quietly opening something else.
 */
export function hostForKey(
  profiles: SessionProfile[],
  key: string
): SessionProfile | undefined {
  if (!/^[1-9]$/.test(key)) return undefined;
  return profiles.slice(0, NUMBERED)[Number(key) - 1];
}

/** A saved host written the way it is addressed, port included when it matters. */
export function hostAddress(profile: SessionProfile): string {
  const port = profile.port && profile.port !== 22 ? `:${profile.port}` : "";
  return `${profile.user}@${profile.host}${port}`;
}

interface Props {
  profiles: SessionProfile[];
  /** Which tab to open on. */
  tab: ConnectTab;
  onClose(): void;
  onConnect(profile: SessionProfile, remember: boolean): void;
  onOpen(profile: SessionProfile): void;
  onRename(id: string, name: string): void;
  /// Turn a saved host's kept shell on or off. See `persist.rs`.
  onKeepShell(id: string, keep: boolean): void;
  /// Whether the kept-shell feature is on at all, in Settings. Off, none of
  /// its controls are shown.
  keepShells: boolean;
  onForget(id: string): void;
  /// Ids of the hosts with a remembered password, so a row can say so and
  /// offer to throw it away. Empty where the machine has no credential store.
  remembered: string[];
  onForgetPassword(id: string): void;
  /// Open a shell on this machine. There is nothing to configure and nothing
  /// to save, so this takes the kind and the dialog is done.
  onLocalShell(kind: LocalShellKind): void;
}

/**
 * Where to connect to: a host already saved, a new one, or — off to the side —
 * a shell on this machine.
 *
 * This once opened with a row of three equal kinds, and two thirds of it
 * opened a local shell that Windows Terminal does better. The saved hosts took
 * that space, because that is where the sessions actually come from. The local
 * shells are back at the far end of the same strip and are not tabs: they look
 * like the two beside them and behave like the buttons they are, because there
 * is no second screen behind either of them. Nothing about a local shell is
 * worth asking, so nothing is asked.
 */
export function ConnectDialog({
  profiles, tab: initialTab, onClose, onConnect, onOpen, onRename, onKeepShell, keepShells, onForget, remembered,
  onForgetPassword, onLocalShell
}: Props) {
  const [tab, setTab] = useState<ConnectTab>(initialTab);
  const [host, setHost] = useState("");
  /// What to call this host. Optional, because an address is already a name
  /// — just a poor one for anything you connect to twice.
  const [label, setLabel] = useState("");
  const [user, setUser] = useState("");
  const [port, setPort] = useState(22);
  const [remember, setRemember] = useState(true);
  /// Force the terminal through `ssh.exe` rather than letting it ride the
  /// connection afterm opens anyway. Off by default: one connection means one
  /// credential, and two meant typing the same password twice.
  const [useClient, setUseClient] = useState(false);
  /// Keep the shell running on the host in tmux, so a reconnect lands back in
  /// it. Off by default: it trades afterm's own scrollback for tmux's.
  const [keepShell, setKeepShell] = useState(false);
  const [error, setError] = useState("");
  /// Id of the saved host whose name is being edited in place, if any.
  const [renaming, setRenaming] = useState<string | null>(null);
  const [draft, setDraft] = useState("");
  const hostBox = useRef<HTMLDivElement>(null);

  // The host list has no field to autofocus, and the terminal underneath keeps
  // xterm's hidden textarea focused if nothing takes it — which would send
  // every key typed at this dialog to the shell behind it.
  useEffect(() => { if (tab === "hosts") hostBox.current?.focus(); }, [tab]);

  // A number opens the host it is written beside. Capture, so the digit is
  // claimed before the terminal underneath can be handed it as input.
  useEffect(() => {
    if (tab !== "hosts") return;
    const handler = (event: KeyboardEvent) => {
      if (event.ctrlKey || event.altKey || event.metaKey) return;
      // The rename box is a field, and a `3` typed into it is part of a name.
      if (typingIntoField(event.target)) return;
      const profile = hostForKey(profiles, event.key);
      if (!profile) return;
      event.preventDefault();
      event.stopPropagation();
      onOpen(profile);
    };
    window.addEventListener("keydown", handler, { capture: true });
    return () => window.removeEventListener("keydown", handler, { capture: true });
  }, [onOpen, profiles, tab]);

  function startRename(profile: SessionProfile) {
    // A host still called after its address starts empty rather than pre-filled
    // with it: naming one is the point, and the address is already there as the
    // placeholder and on the line below.
    setDraft(profile.name === profile.host ? "" : profile.name);
    setRenaming(profile.id);
  }

  function commitRename() {
    if (renaming) onRename(renaming, draft);
    setRenaming(null);
  }

  function submit(event: React.FormEvent) {
    event.preventDefault();
    // The same rules the backend enforces, applied here so the reason is
    // visible before a connection is ever attempted.
    if (!isSafeSshIdentity(host)) {
      setError("Host must not contain spaces or begin with '-'.");
      return;
    }
    if (!isSafeSshIdentity(user)) {
      setError("Username must not contain spaces or begin with '-'.");
      return;
    }
    if (!Number.isInteger(port) || port < 1 || port > 65535) {
      setError("Port must be between 1 and 65535.");
      return;
    }
    setError("");
    onConnect({
      id: crypto.randomUUID(),
      kind: useClient ? "ssh-cli" : "ssh",
      host,
      user,
      port,
      name: profileName(label, host),
      ...(keepShells && keepShell ? { persistentShell: true } : {})
    }, remember);
  }

  return <div className="dialog-backdrop" onMouseDown={event => event.target === event.currentTarget && onClose()}>
    <div className="connect-dialog">
      <div className="dialog-title">
        <div><span className="eyebrow">NEW SESSION</span><h2>Where are we going?</h2></div>
        <button type="button" className="icon-button" onClick={onClose}><X size={17}/></button>
      </div>

      <div className="dialog-tabs">
        <button
          type="button"
          className={tab === "hosts" ? "active" : ""}
          onClick={() => setTab("hosts")}
        >Saved hosts{profiles.length > 0 && <b>{profiles.length}</b>}</button>
        <button
          type="button"
          className={tab === "new" ? "active" : ""}
          onClick={() => setTab("new")}
        >New host</button>
        {/* Pushed to the far end of the strip and never active, because
            neither of them has a screen to be active on: pressing one opens
            the shell and the dialog closes behind it. */}
        <span className="tab-gap"/>
        {LOCAL_SHELLS.map(shell => <button
          key={shell.kind}
          type="button"
          className="local-shell"
          title={`Open ${shell.label} on this machine`}
          onClick={() => onLocalShell(shell.kind)}
        ><SquareTerminal size={12}/>{shell.label}</button>)}
      </div>

      {tab === "hosts"
        ? <>
          <div className="host-list" tabIndex={-1} ref={hostBox}>
            {profiles.length === 0
              ? <div className="pane-note">
                  No saved hosts yet. Open one under <strong style={{ display: "inline" }}>New host</strong> with
                  {" "}<strong style={{ display: "inline" }}>Remember this host</strong> ticked and it lands here.
                </div>
              : profiles.map((profile, index) => {
                  const number = hostNumber(index);
                  const address = hostAddress(profile);
                  // A row being renamed is a div rather than the usual button:
                  // an input inside a button cannot be typed into properly, and
                  // opening the session on every click would fight the caret.
                  return renaming === profile.id
                    ? <div key={profile.id} className="host renaming">
                        <span className="host-key">{number ?? <Server size={11}/>}</span>
                        <span>
                          <input
                            className="tab-rename"
                            autoFocus
                            aria-label="Host name"
                            placeholder={profile.host}
                            value={draft}
                            onChange={event => setDraft(event.target.value)}
                            onBlur={commitRename}
                            onKeyDown={event => {
                              event.stopPropagation();
                              if (event.key === "Enter") commitRename();
                              else if (event.key === "Escape") setRenaming(null);
                            }}
                          />
                          <small>{address}</small>
                        </span>
                      </div>
                    : <button
                        key={profile.id}
                        type="button"
                        className="host"
                        onClick={() => onOpen(profile)}
                      >
                        <span className="host-key">{number ?? <Server size={11}/>}</span>
                        <span>
                          <strong>{profile.name}</strong>
                          <small>{address}</small>
                        </span>
                        {/* Shown on the row rather than in a settings list,
                            because the host is the thing a password belongs
                            to and this is where the host is. The key icon is
                            the state and the button in one: it is only here
                            when something is saved. */}
                        {remembered.includes(profile.id) && <span
                          className="forget remembered"
                          role="button"
                          title="Forget the saved password for this host"
                          onClick={event => {
                            event.stopPropagation();
                            onForgetPassword(profile.id);
                          }}
                        ><KeyRound size={13}/></span>}
                        {/* On while lit, like the key beside it — but always
                            shown, because unlike a password this is something
                            to turn on from here. */}
                        {keepShells && <span
                          className={`forget keep-shell ${profile.persistentShell ? "on" : ""}`}
                          role="switch"
                          aria-checked={profile.persistentShell === true}
                          title={profile.persistentShell
                            ? "The shell is kept running in tmux on the host, so a reconnect lands back in it. Click to stop keeping it."
                            : "Keep the shell running in tmux on the host, so a dropped connection does not end it"}
                          onClick={event => {
                            event.stopPropagation();
                            onKeepShell(profile.id, !profile.persistentShell);
                          }}
                        ><Layers size={13}/></span>}
                        <span
                          className="forget"
                          role="button"
                          title="Rename this host"
                          onClick={event => { event.stopPropagation(); startRename(profile); }}
                        ><Pencil size={13}/></span>
                        <span
                          className="forget"
                          role="button"
                          title="Forget this host"
                          onClick={event => { event.stopPropagation(); onForget(profile.id); }}
                        ><Trash2 size={13}/></span>
                      </button>;
                })}
          </div>
          <p className="local-note">
            {profiles.length > 0
              ? "Press a host's number to open a session on it, or pick it off the list."
              : "afterm opens no host that was not asked for. The two shells on the right are this machine's own."}
          </p>
          <div className="dialog-actions">
            <button type="button" className="secondary" onClick={onClose}>Cancel</button>
            <button type="button" className="primary" onClick={() => setTab("new")}>
              <Plus size={13}/> New host
            </button>
          </div>
        </>
        : <form onSubmit={submit}>
            <div className="form-grid">
              <label className="span-2">Host
                <input autoFocus required value={host} onChange={event => setHost(event.target.value)} placeholder="server.example.com"/>
              </label>
              <label>Username
                <input required value={user} onChange={event => setUser(event.target.value)} placeholder="arya"/>
              </label>
              <label>Port
                <input required min={1} max={65535} type="number" value={port} onChange={event => setPort(Number(event.target.value))}/>
              </label>
              <label className="span-2">Name <small>optional</small>
                <input
                  value={label}
                  onChange={event => setLabel(event.target.value)}
                  placeholder={host || "Production web"}
                />
              </label>
            </div>
            <label className="remember">
              <input type="checkbox" checked={remember} onChange={event => setRemember(event.target.checked)}/>
              Remember this host
            </label>
            <label className="remember">
              <input type="checkbox" checked={useClient} onChange={event => setUseClient(event.target.checked)}/>
              Run the terminal through the OpenSSH client
            </label>
            {keepShells && <label className="remember">
              <input type="checkbox" checked={keepShell} onChange={event => setKeepShell(event.target.checked)}/>
              Keep the shell running on the host (tmux)
            </label>}
            {keepShells && keepShell && <p className="local-note">
              A dropped connection leaves the shell running in tmux on the host, and the reconnect
              lands back in it; closing the tab ends it. The scrollback becomes tmux's: scroll with
              its copy mode or <code>set -g mouse on</code>. Inside tmux afterm cannot see commands or
              the shell's directory, so the command highlight, jumping, find beyond the screen and the
              file pane following along stop, and an agent can only signal with the bell or by going quiet. A host
              without tmux gets its ordinary shell.
            </p>}
            <p className="local-note">
              {useClient
                ? "The terminal runs as a separate ssh.exe login, so host keys and ~/.ssh/config behave exactly as they do elsewhere on this machine. Files and metrics still use afterm's own connection, so a password host will ask twice."
                : "One connection carries the terminal, files and metrics, so a password is asked for once. afterm falls back to the OpenSSH client by itself for a host it cannot carry."}
            </p>
            {error && <div className="form-error">{error}</div>}
            <div className="dialog-actions">
              <button type="button" className="secondary" onClick={onClose}>Cancel</button>
              <button className="primary" type="submit">Open session <span>↵</span></button>
            </div>
          </form>}
    </div>
  </div>;
}
