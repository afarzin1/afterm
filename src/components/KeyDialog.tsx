import { useEffect, useState } from "react";
import { ClipboardPaste, KeyRound, RefreshCw, X } from "lucide-react";
import { discoverPublicKeys, failureOf, installPublicKey } from "../lib/backend";
import type { DiscoveredKey, Session } from "../types";

interface Props {
  session: Session;
  onClose(): void;
}

/**
 * Installs a public key into the remote `~/.ssh/authorized_keys`.
 *
 * The keys already on this machine are found and offered as one-click installs,
 * because that is what nearly every use of this dialog wants. Pasting is still
 * available for a key that lives somewhere afterm does not look, but it is the
 * fallback rather than the first thing you see.
 *
 * Only `.pub` files are ever read. No private key is opened or transferred.
 */
export function KeyDialog({ session, onClose }: Props) {
  const [keys, setKeys] = useState<DiscoveredKey[] | null>(null);
  const [pasting, setPasting] = useState(false);
  const [pasted, setPasted] = useState("");
  const [status, setStatus] = useState("");
  const [busy, setBusy] = useState<string | null>(null);

  const scan = () => {
    setKeys(null);
    void discoverPublicKeys()
      .then(found => {
        setKeys(found);
        // Nothing to click means pasting is the only route; go straight there.
        if (found.length === 0) setPasting(true);
      })
      .catch(() => { setKeys([]); setPasting(true); });
  };

  useEffect(scan, []);

  const install = async (key: string, label: string) => {
    setBusy(label);
    setStatus("Installing…");
    try {
      // What comes back is the host's own answer — which account, which file —
      // rather than afterm reporting that it sent something somewhere.
      const done = await installPublicKey(session.id, key);
      setStatus(done.alreadyPresent
        ? `Already installed for ${done.user} in ${done.path}.`
        : `Installed for ${done.user} in ${done.path}.`);
    } catch (error) {
      setStatus(failureOf(error).message || String(error));
    } finally {
      setBusy(null);
    }
  };

  return <div className="dialog-backdrop" onMouseDown={event => event.target === event.currentTarget && onClose()}>
    <div className="connect-dialog key-dialog">
      <div className="dialog-title">
        <div>
          <span className="eyebrow">KEY AUTHENTICATION</span>
          <h2>Install a public key</h2>
        </div>
        <button type="button" className="icon-button" onClick={onClose}><X size={17}/></button>
      </div>
      <p>
        Appends the key to <code>~/.ssh/authorized_keys</code> on <strong>{session.host}</strong>
        {" "}over afterm's own connection, so it does not matter what the terminal is showing.
        Your private key never leaves this computer.
      </p>

      {keys === null && <div className="pane-note">Looking for keys…</div>}

      {keys && keys.length > 0 && <>
        <div className="key-found-head">
          <span className="section-label">FOUND ON THIS COMPUTER</span>
          <button type="button" className="icon-button" title="Scan again" onClick={scan}>
            <RefreshCw size={13}/>
          </button>
        </div>
        <ul className="key-list">
          {keys.map(key => <li key={key.path}>
            <KeyRound size={15}/>
            <span>
              <strong>{key.name}</strong>
              <small>{key.kind}{key.comment ? ` · ${key.comment}` : ""}</small>
            </span>
            <button
              type="button"
              className="primary"
              disabled={busy !== null}
              onClick={() => void install(key.content, key.name)}
            >{busy === key.name ? "Installing…" : "Install"}</button>
          </li>)}
        </ul>
      </>}

      {keys && keys.length === 0 && <div className="pane-note">
        <strong>No public keys found</strong>
        Nothing matching <code>~/.ssh/*.pub</code> on this computer. Generate one with
        {" "}<code>ssh-keygen -t ed25519</code>, or paste a key below.
      </div>}

      {!pasting
        ? <button type="button" className="link-button" onClick={() => setPasting(true)}>
            <ClipboardPaste size={13}/> Paste a key instead
          </button>
        : <form onSubmit={event => { event.preventDefault(); void install(pasted.trim(), "pasted"); }}>
            <label className="key-field">Public key
              <textarea
                required
                value={pasted}
                onChange={event => setPasted(event.target.value)}
                placeholder="ssh-ed25519 AAAAC3… you@computer"
              />
            </label>
            <div className="dialog-actions">
              <button className="primary" type="submit" disabled={busy !== null || !pasted.trim()}>
                {busy === "pasted" ? "Installing…" : "Install pasted key"}
              </button>
            </div>
          </form>}

      {status && <div className="key-status">{status}</div>}
    </div>
  </div>;
}
