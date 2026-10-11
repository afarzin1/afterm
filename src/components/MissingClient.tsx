import { useState } from "react";
import { ClipboardCopy, RefreshCw, TriangleAlert } from "lucide-react";
import { checkOpenssh } from "../lib/backend";
import type { OpensshPresence } from "../types";

interface Props {
  presence: OpensshPresence;
  onRecheck(presence: OpensshPresence): void;
}

/**
 * Says that this machine is missing the OpenSSH tools, and how to fix it.
 *
 * Not dismissible, because nothing will connect until it is fixed: a banner
 * that can be waved away for a condition that makes every session fail would
 * only mean seeing the same failure later with less to go on. Re-check is
 * there instead — the capability lands in a directory already on `PATH`, so
 * afterm can find it without being restarted.
 */
export function MissingClient({ presence, onRecheck }: Props) {
  const [copied, setCopied] = useState(false);
  const [checking, setChecking] = useState(false);

  const copy = async () => {
    await navigator.clipboard.writeText(COMMAND);
    setCopied(true);
    setTimeout(() => setCopied(false), 2000);
  };

  const recheck = () => {
    setChecking(true);
    void checkOpenssh()
      .then(onRecheck)
      .finally(() => setChecking(false));
  };

  return <div className="missing-client" role="alert">
    <TriangleAlert size={15}/>
    <div className="missing-client-body">
      <strong>{presence.missing.join(" and ")} {presence.missing.length > 1 ? "are" : "is"} not
      installed, so afterm cannot connect to anything.</strong>
      <p>
        afterm speaks SSH itself, but it asks your own OpenSSH client which host keys you
        trust and what your <code>~/.ssh/config</code> says, rather than answering those
        differently. Install it from an administrator PowerShell:
      </p>
      <code className="missing-client-command">{COMMAND}</code>
      <p className="missing-client-aside">
        Or Settings → System → Optional features → Add an optional feature → OpenSSH Client.
      </p>
    </div>
    <div className="missing-client-actions">
      <button onClick={copy}><ClipboardCopy size={13}/> {copied ? "Copied" : "Copy command"}</button>
      <button onClick={recheck} disabled={checking}>
        <RefreshCw size={13}/> {checking ? "Checking…" : "Re-check"}
      </button>
    </div>
  </div>;
}

/** The Feature-on-Demand name is exact and version-suffixed; it is not a typo. */
const COMMAND = "Add-WindowsCapability -Online -Name OpenSSH.Client~~~~0.0.1.0";
