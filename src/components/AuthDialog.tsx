import { useEffect, useRef, useState } from "react";
import { KeyRound, X } from "lucide-react";
import { onAuthRequest, onAuthResolved, submitAuthResponse } from "../lib/backend";
import {
  answersFor, dequeueRequest, enqueueRequest, forget, type Typed
} from "../lib/auth-queue";
import type { AuthRequest } from "../types";

/**
 * The one place afterm asks for a credential.
 *
 * It is driven entirely by what the host asked for: a password host sends one
 * hidden field, a PAM or 2FA host sends whatever sequence of questions it likes
 * and not all of them are secrets. The dialog shows the host's own wording
 * rather than a guess at it, because guessing is what makes the `sshpass`
 * pattern fragile.
 *
 * Requests queue rather than replace one another: two sessions opening at once
 * both get their turn, and each answer goes back under its own request id, to
 * the handshake that asked. What was typed is dropped the moment its request
 * is over, and there is no field on a saved profile to persist a secret into.
 *
 * One request can be remembered, and only on the host's and the machine's
 * terms: a password, on a saved host, where there is an operating-system
 * credential store to put it in. The backend decides all three and sends
 * `canRemember`; this dialog offers the checkbox on that alone and never works
 * out why the answer was no. What it sends back is the tick, not the password's
 * destination — a saved password goes into the store from Rust, after the host
 * has accepted it, and no command here can read one back.
 */
export function AuthDialog() {
  const [queue, setQueue] = useState<AuthRequest[]>([]);
  const [typed, setTyped] = useState<Typed | null>(null);
  /// The request whose answer is on its way, so Continue cannot be pressed twice.
  const [sending, setSending] = useState<string | null>(null);
  /// The checkbox, per request id, so answering one dialog does not tick the
  /// box on the next host waiting behind it.
  const [remember, setRemember] = useState<string | null>(null);
  const first = useRef<HTMLInputElement>(null);
  const request = queue[0];
  const answers = answersFor(typed, request);

  useEffect(() => {
    let stop: (() => void) | undefined;
    void onAuthRequest(value => {
      setQueue(old => enqueueRequest(old, value));
    }).then(fn => { stop = fn; });
    return () => stop?.();
  }, []);

  // The backend gives up on a prompt after a while, and abandons every prompt
  // for a session that closes. Either way the dialog must not linger, and
  // neither must what was typed into it.
  useEffect(() => {
    let stop: (() => void) | undefined;
    void onAuthResolved(id => {
      setQueue(old => dequeueRequest(old, id));
      setTyped(old => forget(old, id));
    }).then(fn => { stop = fn; });
    return () => stop?.();
  }, []);

  useEffect(() => { first.current?.focus(); }, [request?.request]);

  if (!request) return null;

  /** The request is over, one way or the other: its typing goes, and so does it. */
  const finish = (id: string) => {
    setTyped(old => forget(old, id));
    setQueue(old => dequeueRequest(old, id));
  };

  const dismiss = () => {
    void submitAuthResponse(request.request, null);
    finish(request.request);
  };

  const submit = (event: React.FormEvent) => {
    event.preventDefault();
    setSending(request.request);
    void submitAuthResponse(request.request, answers, remembering);
    finish(request.request);
    setRemember(null);
  };

  const waiting = queue.length - 1;
  // Ticked, and for this request rather than a previous one. Never true where
  // the backend said it could not be: a stale tick from the host before must
  // not travel to the host in front.
  const remembering = request.canRemember && remember === request.request;

  return <div className="dialog-backdrop" onMouseDown={event => event.target === event.currentTarget && dismiss()}>
    <form className="connect-dialog auth-dialog" onSubmit={submit}>
      <div className="dialog-title">
        <div>
          <span className="eyebrow">{request.name ? request.name.toUpperCase() : "AUTHENTICATION"}</span>
          <h2>{request.user}@{request.host}</h2>
        </div>
        <button type="button" className="icon-button" onClick={dismiss}><X size={17}/></button>
      </div>

      {request.instructions && <p className="local-note">{request.instructions}</p>}

      {request.prompts.map((field, index) => <label key={index} className="auth-field">
        {field.prompt.replace(/:\s*$/, "")}
        <input
          ref={index === 0 ? first : undefined}
          type={field.echo ? "text" : "password"}
          autoComplete="off"
          value={answers[index] ?? ""}
          onChange={event => setTyped({
            id: request.request,
            values: answers.map((value, at) => at === index ? event.target.value : value)
          })}
          onKeyDown={event => event.stopPropagation()}
        />
      </label>)}

      {/* Offered only where all three conditions hold, and the label says
          where it would go: "on this device" is the difference between a
          credential store and a sync service, and it is the thing someone
          deciding this needs to know. */}
      {request.canRemember && <label className="auth-remember">
        <input
          type="checkbox"
          checked={remembering}
          onChange={event => setRemember(event.target.checked ? request.request : null)}
          onKeyDown={event => event.stopPropagation()}
        />
        <span>
          <strong>Remember this password on this device</strong>
        </span>
      </label>}

      <p className="auth-note">
        <KeyRound size={13}/>
        {remembering
          ? <>Saved once this host accepts it, and never sent anywhere else.</>
          : <>Used for this connection and then discarded.</>}
        {waiting > 0 && <> {waiting === 1 ? "One more host is waiting" : `${waiting} more hosts are waiting`} behind this one.</>}
      </p>

      <div className="dialog-actions">
        <button type="button" className="secondary" onClick={dismiss}>Cancel</button>
        <button className="primary" type="submit" disabled={sending === request.request}>Continue <span>↵</span></button>
      </div>
    </form>
  </div>;
}
