import { useEffect, useState } from "react";
import { Activity, Cpu, Gauge, HardDrive, MemoryStick, Network, Server } from "lucide-react";
import { authenticateSession, failureOf, sessionMetrics } from "../lib/backend";
import { UNKNOWN, formatBytes, formatLatency, formatPercent, formatRatio, formatRate, formatUptime } from "../lib/format";
import { isLocalShell } from "../lib/shells";
import type { FailureCause, Metrics, Session } from "../types";

interface Props {
  session: Session | undefined;
  /**
   * Where the poll below found the shell. Reported from here rather than from
   * a poll of its own because this component already asks for the block the
   * directory arrives in, and `session_metrics` measures the link's latency on
   * every call — a second poller would double that traffic to learn something
   * this one is already being told.
   */
  onDirectory?(id: string, path: string): void;
}

const POLL_INTERVAL = 2000;
const NATIVE_TRANSPORT = "native · ";

/**
 * The native backend records enough detail to diagnose which credential won,
 * including a key's local path. The bar only needs the kind of credential:
 * paths are private implementation detail and make the session label sprawl.
 */
export function authenticationOf(transport: string | null): string | null {
  if (!transport?.startsWith(NATIVE_TRANSPORT)) return null;
  const method = transport.slice(NATIVE_TRANSPORT.length);
  if (method.startsWith("publickey") || method.startsWith("public key") || method.startsWith("agent")) {
    return "public key";
  }
  return method === "keyboard-interactive" ? "interactive" : method;
}

/** Renders a measurement, or a dimmed em dash when there is nothing to show. */
function Value({ text }: { text: string }) {
  return text === UNKNOWN ? <b className="unknown">{UNKNOWN}</b> : <b>{text}</b>;
}

/**
 * Live host telemetry for the active session.
 *
 * An SSH session reports the host at the far end, sampled over a dedicated
 * connection. When a metric is not available — no key auth, a host without
 * /proc, a session still connecting — it shows as an em dash rather than a
 * plausible-looking number.
 *
 * A local shell reports nothing at all, and the bar goes empty for one. That
 * is the deliberate shape of it: everything this bar says is about a far end,
 * a local shell has none, and afterm sampling the machine it is running on was
 * the branch that used to make every reading here ambiguous. The bar stays in
 * the layout so the terminal does not resize as tabs are switched.
 */
export function StatusBar({ session, onDirectory }: Props) {
  const [metrics, setMetrics] = useState<Metrics | null>(null);
  const [note, setNote] = useState<string | null>(null);
  const [cause, setCause] = useState<FailureCause | null>(null);
  /// True when the host would actually accept a secret, so the bar offers the
  /// dialog only where it would achieve something.
  const [canPrompt, setCanPrompt] = useState(false);
  const [transport, setTransport] = useState<string | null>(null);
  const [authenticating, setAuthenticating] = useState(false);

  // Nothing about a local shell is polled: it has no remote half, so the ask
  // would be a round trip to an error every two seconds.
  const local = Boolean(session && isLocalShell(session.kind));

  useEffect(() => {
    setMetrics(null);
    setNote(null);
    setCause(null);
    setCanPrompt(false);
    setTransport(null);
    if (!session || local || session.state !== "connected") return;

    let cancelled = false;
    const poll = async () => {
      try {
        const report = await sessionMetrics(session.id);
        if (cancelled) return;
        setMetrics(report.metrics);
        if (report.metrics?.directory) onDirectory?.(session.id, report.metrics.directory);
        setNote(report.note);
        setCause(report.cause);
        setTransport(report.transport);
        setCanPrompt(report.canPrompt);
      } catch (problem) {
        if (cancelled) return;
        const failure = failureOf(problem);
        setMetrics(null);
        setNote(failure.message);
        setCause(failure.cause);
        setCanPrompt(failure.canPrompt);
      }
    };
    void poll();
    const timer = window.setInterval(poll, POLL_INTERVAL);
    return () => { cancelled = true; window.clearInterval(timer); };
  }, [session?.id, session?.state, local, onDirectory]);

  const authenticate = async () => {
    if (!session) return;
    setAuthenticating(true);
    try {
      setTransport(await authenticateSession(session.id));
      setNote(null);
      setCause(null);
      setCanPrompt(false);
    } catch (problem) {
      const failure = failureOf(problem);
      setNote(failure.message);
      setCause(failure.cause);
      setCanPrompt(failure.canPrompt);
    } finally {
      setAuthenticating(false);
    }
  };

  // Empty, and empty of everything: no host, no dot, no readings. There is no
  // half of this bar that means anything for a shell on this machine.
  if (local) return <footer className="statusbar"/>;

  const authentication = authenticationOf(transport);
  const label = session
    ? `SSH${authentication ? ` (${authentication})` : ""} · ${session.user}@${session.host}`
    : "No session";

  // No session at all carries no state class, which leaves the dot at the
  // idle grey `.connection i` already paints.
  const state = session?.state ?? "";

  return <footer className="statusbar">
    <div className="status-left">
      <span className={`connection ${state}`}>
        <i/>
        <Server size={12}/>
        {/* The one thing in the bar with no bound on its length: a host name
            and a user name, both the user's. It is what gives way when the
            window is too narrow to hold everything, rather than the readings
            running into it. */}
        <span className="who">{label}</span>
      </span>
      {transport && !authentication &&
        <span className="transport" title="What is carrying files and telemetry for this session">
          {transport}
        </span>}
      {note && <span className={`telemetry-note ${cause ?? ""}`} title="Why there is no host telemetry">{note}</span>}
      {canPrompt && <button className="statusbar-action" disabled={authenticating} onClick={() => void authenticate()}>
        {authenticating ? "Connecting…" : "Enter password"}
      </button>}
      {session && metrics?.latency !== null && metrics?.latency !== undefined &&
        <span className="latency" title="Round-trip time over the session's own connection">
          <Gauge size={13}/><Value text={formatLatency(metrics.latency)}/>
        </span>}
    </div>
    {/* Three spacings do the grouping here, and `pair` is the tightest of them:
        see the status bar's own note in the stylesheet. What it wraps is a
        reading that is one thing said in two parts — a ratio, or the two
        directions of a link — and must not be read as two metrics that happen
        to share an icon. */}
    <div className="metrics">
      <span title="CPU"><Cpu size={13}/><Value text={formatPercent(metrics?.cpu)}/></span>
      <span title="Memory">
        <MemoryStick size={13}/>
        <span className="pair">
          <Value text={formatBytes(metrics?.usedMemory)}/>
          <span className="unknown">/</span>
          {formatBytes(metrics?.totalMemory)}
        </span>
      </span>
      <span title="Network">
        <Network size={13}/>
        <span className="pair"><em>↓</em><Value text={formatRate(metrics?.downloadRate)}/></span>
        <span className="pair"><em className="up">↑</em><Value text={formatRate(metrics?.uploadRate)}/></span>
      </span>
      <span title="Disk">
        <HardDrive size={13}/>
        <Value text={formatRatio(metrics?.diskUsed ?? null, metrics?.diskTotal ?? null)}/>
      </span>
      <span title="Uptime"><Activity size={13}/><Value text={formatUptime(metrics?.uptime)}/></span>
    </div>
  </footer>;
}
