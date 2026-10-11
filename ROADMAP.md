# afterm roadmap

What is left. What is built is in `CHANGELOG.md`, and the reviews behind it are
in `OPEN-ISSUE.md`.

afterm speaks SSH itself: one authenticated connection per session carries the
terminal, files and telemetry, and that is the default. The OpenSSH client
remains the transport for hosts whose configuration afterm will not pretend to
reproduce, and a session falls back to it by itself. That is the architecture,
not a migration left half-finished.

## Open work

Small, independent, no architectural commitment. Ordered by likely value.

The two review passes of 7 September 2026 and the follow-up of 9 September are
no longer here: all fourteen findings are fixed, and what was done about each
is in `CHANGELOG.md`. The follow-up's four — the upload staging window, the
native terminal's read/write deadlock, one bad entry emptying the saved hosts,
and a session reported as connected before the host had accepted the shell —
each landed with tests that fail against the code they replaced. What those
fixes could not close is recorded under "Known limits" below rather than left
as open work, because none of it is a matter of effort.

### Smaller pieces

- **Ask about an unrecorded host key in afterm's own dialog.** A host that is
  not yet in `known_hosts` is refused by the native transport before any
  credential is offered — correctly — and the tab then falls back to the
  OpenSSH client, which asks whether to trust the key *in the terminal*. The
  fallback now says so (`fallback_notice`), which is the difference between a
  confusing tab and a rude one, but it is still the wrong shape: afterm asks
  for passwords in a dialog and for host keys in a terminal, on first contact
  with every new host, which is exactly when a user has least idea what afterm
  is doing.

  **What it needs.** `Verifier::check_server_key` returns false today and the
  connection ends there. Asking instead means a request on the same channel the
  password uses — `PromptSink` already carries everything but a "trust this
  key?" shape — and, on a yes, writing the key to `known_hosts` the way
  `ssh-keygen -F` reads it. The write is the part to be careful with: it is the
  user's file, it is how every other SSH tool on the machine decides what to
  trust, and a wrong entry there is worse than no feature. A changed key must
  stay a refusal with no button on it, because that is the case the whole
  mechanism exists for.

  Until then every new host takes the OpenSSH-client path on its first
  connection and the native transport only from the second, which is also why
  the first connection asks for nothing and the second raises a dialog.

- **Say when a long-running command has finished, coding agents included.**
  *The signal half is built (2026-09-22):* OSC 9, OSC 777 `notify`, kitty's
  OSC 99 and `BEL` mark an unwatched tab and flash the taskbar, with ConEmu's
  numbered OSC 9 commands ignored and *Settings ▸ Coding agents* copying each
  agent's setting — see `lib/attention.ts` and `CHANGELOG.md`. Still open from
  what follows: the OSC 133;D "long command finished" notice, the operating-system
  toast, the cosmetic agent name, and the opt-in heuristic for an agent that
  sends nothing.

  Claude Code, Codex and the rest are one foreground command that runs for
  minutes and then waits for you, in a pane you have usually looked away from.
  Every one of them already says so on the byte stream, because they were
  written for terminals that notify — so afterm should listen for the signal
  rather than try to recognise the program.

  **Listen to `BEL` and to OSC 9.** `term.onBell` is the first; the second is
  `ESC ] 9 ; message BEL`, iTerm2's notification, which is one more
  `registerOscHandler` beside the OSC 133 one already in `TerminalPane.tsx`.
  OSC 777's `notify;title;body` is the same handler again for the shells that
  prefer it. All three ride the PTY, so this works over SSH and on the local
  shells with nothing installed on the far end and nothing added to the
  snippet.

  **Not OSC 133;D.** The obvious hook and the wrong one: an agent session is a
  single foreground command, so `D` fires when the agent exits, not when a turn
  ends. `D` is worth reading for the ordinary case — a `make` or a `dd` that
  took four minutes — and that is a separate, simpler feature riding the marks
  afterm already has.

  **Naming the agent is cosmetic, and must stay that way.** The OSC 133 `B`→`C`
  span already carries the command line, so matching it against `claude`,
  `codex`, `grok` is nearly free and lets a tab say which agent is waiting. A
  miss has to mean no badge, never no notification: that list is the only part
  of this that rots as those tools change, and it must not be load-bearing.
  Nothing may parse an agent's message text for meaning — show the string it
  sent.

  **Where the notice goes, cheapest first:** a dot on the tab in the `tabbar`,
  which costs nothing and is the right default; the taskbar, through
  `requestUserAttention`, which is inside `core:window` already; and an
  operating-system toast, which needs `tauri-plugin-notification` in
  `Cargo.toml` and `notification:default` in `capabilities/default.json`. All
  of it gated on the pane not being focused — that state is already threaded
  through as `focused`.

  One thing to decide before the README says anything. **The claim is the
  liability, not the code:** a generic sentence — notifies when a long-running
  command or coding agent finishes, with those tools named as tested examples —
  costs nothing to keep true, while a compatibility matrix makes every one of
  their releases a bug report here. It does not contradict afterm's *no AI*
  stance either — afterm ships no model and calls nothing — but the
  description has to say so in those words: afterm has no AI in it, it just
  notices when yours rings the bell.

  An agent that emits nothing at all is the one case left. The fallback is a
  heuristic — alt screen (`isFullScreenApp()` exists), no output for N seconds,
  pane unfocused — which is noisy enough that it belongs behind a per-pane opt
  in, after the signal-based half has been lived with.
- **Generate a key when none is found**, instead of telling the user to run
  `ssh-keygen`. Turns the empty state into one click, and it is the missing
  half of a dialog that can already install a public key on a host. Less urgent
  than it was: a key is no longer the only way to get a file pane on a password
  host.

  **Generate it in process, not by running `ssh-keygen`.** Shelling out would
  add no new dependency — `ssh-keygen -F` is already how `native/trust.rs`
  resolves host-key trust, and a machine without it cannot connect at all — but
  it buys nothing and costs three things: a spawned process on Windows, which
  is what `winproc::hide_console` exists to keep from flashing a console; `-N
  ""` to stop it prompting for a passphrase afterm has not asked the user for;
  and output to parse for a result the library hands back as a value.

  `russh::keys` re-exports `ssh-key`, which is already in the tree and already
  used this way — `battery.rs` builds the in-process test host's key with it.
  `PrivateKey::random(&mut rand::rng(), Algorithm::Ed25519)` makes the key,
  `write_openssh_file` writes the private half, and `public_key()
  .write_openssh_file` writes the `.pub` beside it, in the format `discover`
  already reads and the installer already sends.

  One wrinkle to design against rather than discover. `write_openssh_file` sets
  `0600` on Unix and nothing on Windows, where the file inherits whatever ACLs
  `%USERPROFILE%\.ssh` carries. Those are normally the user, SYSTEM and
  Administrators, which Win32-OpenSSH accepts — but where they are looser the
  key would work in afterm, whose own transport never checks permissions, and
  be refused by `ssh.exe` with *UNPROTECTED PRIVATE KEY FILE* on the fallback
  path. A key that works in one half of the application and not the other is
  worse than one that fails outright, so the generated key's ACLs want setting
  explicitly and a test that says they were.
- **Build and test on more than one Windows.** The pipeline runs a single
  hosted image, and every Windows-only failure this project has had came from
  the OpenSSH client behaving differently between builds — five of them now,
  the latest being `create_new` onto a directory answering `PermissionDenied`
  on Windows where Linux says `AlreadyExists`. A `windowsImages` parameter
  driving a job matrix would cover both generations that hosted agents still
  offer: `windows-2025`, which is the Windows 11 codebase and ships OpenSSH,
  and `windows-2022`, which is Windows 10's and does not. Worth a step that
  prints `ssh -V`, `winver` and the WebView2 `pv` registry value, so each run
  says which client it actually tested against.

  Three things to know before wiring it. The three cache keys all use
  `"$(Agent.OS)"`, which is `Windows_NT` on every image, so a matrix would
  share one cargo cache between two MSVC toolchains and thrash; the image name
  has to go into the keys. The free tier is one parallel job and 1,800 minutes
  a month against a run of roughly ten, so the second image belongs behind a
  parameter that defaults to off rather than in every run. And
  `windows-2019` was retired on 31 December 2025, so Server 2019 is no longer
  reachable this way at all.

  What no hosted image can cover, because they are all Server SKUs and there
  is no ARM64 one: a machine without the WebView2 runtime, the per-user NSIS
  install (hosted agents run as administrator), SmartScreen on an unsigned
  binary, the Windows 10 caption path, x64-on-ARM emulation, and the interface
  itself. Those stay the manual acceptance test below.

### Remember passwords — built, with persistence still to be walked

The design below is built, and `CHANGELOG.md` records what shipped. Every rule
in it holds: the trait and its bindings are in `src-tauri/src/secrets.rs`, the
reuse and saving are in `native::authenticate`, and the behaviour is covered by
the in-process battery — a remembered password authenticating with no dialog, a
mismatched binding never reaching the host, a refused password never saved and
never retried, a store that will not take the write leaving the session up.

Windows Credential Manager itself is verified as far as one process can verify
it. `secrets::real_store` writes a uniquely named credential on the hosted
Windows agent, reads it back, confirms the fingerprint binding still decides
after the operating system has held the blob, replaces it, deletes it, and
deletes it again — first green in build 244. So `CredWrite`, `CredRead` and
`CredDelete` are known to work from a pipeline process, and the blob is known
to survive the round trip intact.

What one process cannot answer is the part `CRED_PERSIST_LOCAL_MACHINE` is
chosen for: whether the credential is still there after afterm closes, and
after a Windows login. That, and the rest of the acceptance list below, is
still to be walked on a real machine.

Two items were deliberately left where the design put them, and are still
unbuilt: keyboard-interactive answers and private-key passphrases are never
saved.

**Opt in per saved connection, using Windows Credential Manager from Rust.**
Add an unchecked **Remember password on this device** option to the native
SSH password dialog. Save only after authentication succeeds and only when
the user selected it. Normal connections continue to discard the supplied
password.

Use a generic credential (`CRED_TYPE_GENERIC`) with
`CRED_PERSIST_LOCAL_MACHINE`, accessed through the Windows credential APIs
behind a small Rust credential-store interface. That persistence scope retains
the credential for the same Windows user on this machine across logins;
it does not request roaming to other machines. See Microsoft's
[credential structure documentation](https://learn.microsoft.com/en-us/windows/win32/api/wincred/ns-wincred-credentialw)
and [credential read API](https://learn.microsoft.com/en-us/windows/win32/api/wincred/nf-wincred-credreadw).
Do not build an encrypted password file with a key shipped in the executable.
On unsupported platforms, leave saving unavailable until an OS credential
store is implemented; never fall back to plaintext.

The implementation must meet these requirements:

- **Keep secrets out of profiles and the webview on reuse.** Profiles may hold
  an opaque credential reference and consent metadata, but no password or
  encrypted password blob. No secrets in localStorage, exports, logs, errors,
  analytics, command arguments, environment variables,
  or terminal input. Newly entered passwords cross the existing authentication
  IPC once; stored passwords are read and used inside Rust, with no API to
  return them to JavaScript. Clear the dialog state promptly and use buffers
  that are zeroed on drop where practical, without promising that every
  JavaScript or SSH-library copy can be erased.
- **Bind reuse to the intended connection.** Use an app-specific credential
  namespace and a stable saved-profile identity, not a transient tab ID or
  display name. Associate it with the effective SSH hostname, port, and user
  after config resolution. Validate that binding on each use; changes to the
  destination or account require fresh consent. Renaming a display label
  should keep the association. Verify the server's host key before retrieving
  or sending its password; unknown or changed keys must not receive it.
  Honour configured authentication methods and `BatchMode`.
- **Start with the SSH password method only.** Add explicit method metadata
  to authentication requests so the dialog knows when saving is supported.
  A hidden keyboard-interactive field is not proof that it asks for a reusable
  password: it may request an OTP or another challenge. Do not save or
  automatically answer those fields in the first version. Private-key
  passphrases are also outside this feature's initial scope.
- **Handle rejection without retry loops.** Attempt a stored password at most
  once per connection attempt. On rejection, disable automatic reuse of that
  entry until the user supplies a replacement or explicitly retries it;
  present the normal dialog with an update/forget choice. Concurrent tabs must
  not repeatedly submit a credential already known to be rejected. Cancellation
  and failed authentication must never save or replace an entry. A store write
  failure should leave the authenticated connection usable and clearly say
  that the password was not saved.
- **Provide explicit lifecycle controls.** Show whether a saved connection has
  a remembered password, and offer **Forget password** and **Forget all saved
  passwords**. Deleting a saved connection must remove its associated secret;
  report credential-store deletion failures so they can be retried. Exported
  profiles omit credential references and saving consent. Imported profiles
  start without either. Document that an OS credential store protects storage
  but does not protect against malware running as the logged-in user or an
  administrator controlling the machine.
- **Keep reuse inside the native transport.** Do not inject remembered
  passwords into `ssh.exe`, use `sshpass`, or revive `SSH_ASKPASS`. If the
  connection falls back to the OpenSSH client, explain that saved-password
  reuse is unavailable there and leave its interactive authentication intact.
  Its batch SFTP and telemetry helpers still require keys or an agent.

Acceptance checks: opt-in persistence across app restart and Windows login;
no persistence when unchecked; independent accounts, ports, and profiles;
host-key and effective-destination changes; rejected passwords and concurrent
tabs; MFA challenges never cached; cancellation; credential-store read, write,
and delete failures; forgetting and profile deletion; exports without secrets;
and OpenSSH fallback. Use a fake store for deterministic tests and a real
Windows credential-store integration test with uniquely named disposable
entries. Update the authentication dialog and README's current “never saves”
wording when the feature ships.

## Waiting on a person at a Windows machine

Not automatable, and left as manual checks rather than pretended. In rough
order of what would hurt most if it were wrong: the OpenSSH agent over its
named pipe, a real 2FA host, a jump host falling back as it should, and the
native terminal under sustained use — full-screen programs, resize, the typed
input highlight — where the bar is subjective and only a person can judge it.

Four more are Windows-only, all compile-checked from Linux and none of them
exercised there:

- **Dragging a file out of the pane.** It relies on the HTML5 `dragend` firing
  when a drag leaves the WebView2 window. If it does not fire, the gesture does
  nothing — the Download button is unaffected either way.
- **Dropping files onto the pane at a display scale other than 100%.** The drop
  position arrives in physical pixels and is divided by `devicePixelRatio`
  before it is compared with the pane's box. A wrong scale means drops landing
  outside the pane being taken, or drops inside it being ignored.
- **The title bar surviving full screen.** `DwmSetWindowAttribute` is set once
  per scheme change and the attribute lives on the window handle, so leaving
  full screen should not disturb it. If tao's full-screen path resets window
  styles it might, and the fix is re-applying the caption colour when full
  screen ends.
- **`F11` reaching afterm at all.** There is no browser chrome in WebView2 to
  claim it first, so it should, but that is reasoning rather than a run.

Two more came out of the September 2026 fixes. Both are exercised against the
in-process host on every pipeline run, which is what makes them checks on the
world rather than on the code:

- **Installing a public key on a real host, from a shell that is busy.** The
  command now runs on a channel of afterm's own connection, so it should work
  with an editor open, at a `sudo -i` prompt, and inside a nested `ssh` — and
  in the last two cases it should still install for the account the session was
  opened under, not the one in front of the terminal.
- **A host whose `ssh_config` narrows the algorithms past what afterm can
  negotiate.** It should fall back to the OpenSSH client naming the directive,
  rather than connecting with something the configuration forbids. Win32-OpenSSH
  spells these lists the same way, but only a run says so.

`ssh -G` and `ssh-keygen -F` under Win32-OpenSSH used to be on this list. They
are now tested against the real binaries — the config resolver and the trust
oracle each have a test that runs the installed client rather than a fixture,
so the Windows agent answers that question on every run. Both skip rather than
fail where there is no OpenSSH client, so a machine without one does not break
the build.

## Standing constraints

Rules the transport work produced. Anything built after it inherits them.

- **Never silently differ from what the user's `ssh_config` says.** afterm does
  not parse the config; `ssh -G` resolves it, so the user's own client
  evaluates aliases, wildcards, `Include` and `Match` and afterm reads the
  effective answer. The danger was never "unsupported" — it is *silently
  half-supported*, which is what a typed parser produces when it drops the
  directives it has no field for. Anything afterm cannot honour goes to the
  OpenSSH client with a named reason.
- **Trust comes from `ssh-keygen -F`, not a second `known_hosts` parser.**
  russh's reader is not OpenSSH's: it skips `@revoked` and `@cert-authority`,
  does not match wildcard patterns or tab-separated fields, and reports an
  algorithm mismatch as an unknown host. The full probe is in `OPEN-ISSUE.md`.
- **Talk to the OpenSSH binaries by run-to-completion capture**, never by
  framing responses inside an interactive stream. That is the pattern that
  fixed the SFTP client; the other one broke it four times.
- **Render only what afterm can measure.** A value it cannot reads as an em
  dash, and a listing it cannot read shows the error the backend returned.
- **A saved profile structurally cannot hold a secret**, and there is a test
  that says so. The proposed password-saving feature preserves this boundary:
  a profile may reference a credential held in the OS store, never contain it.

## Known limits, and why they exist

**The OpenSSH-client transport needs key or agent authentication.** Its helpers
are separate processes, so they authenticate separately; they connect with
`BatchMode=yes` and fail visibly rather than hanging on a password prompt
nobody can see. This is a property of one transport rather than of afterm: the
native transport authenticates however the host allows, and it is what carries
a password-only host.

**A folder transfer is one file at a time, either way.** No parallelism and no
resume: the walk carries each file over the session's existing transport in
turn. On the native transport that is one connection doing one thing at a time,
which is slower than it could be on a high-latency link and costs nothing else.
On the OpenSSH-client transport it is a login per file, for the reason the
connection-per-operation limit below gives — a folder of two hundred files is
two hundred logins, and two hundred lines in the host's auth log. Batching the
transfers into one `sftp -b` script per directory is the obvious remedy and is
not written: the script would have to be chunked to stay inside a pipe buffer,
since the client's output is not drained until the whole script has been
written.

**A folder transfer that fails part way leaves the part that arrived.** A
single file travels under a temporary name and is moved into place only once it
is whole, so a failed transfer leaves nothing. A folder cannot do that in
either direction: renaming a directory onto a name that already exists is
refused on Windows and replaces an empty directory on POSIX, so the two-step
would behave differently on the platform afterm ships on from the one it is
developed on. The destination folder is therefore made first — exclusively, so
nothing already at the far end is touched — and filled in place. A download
that brought nothing at all takes the empty folder with it; anything with files
behind it keeps them, and the status says how many went. Resuming into a folder
already there would be the feature that makes this a non-issue, and it needs a
way to tell a half-finished transfer from a folder somebody has been editing.

**A link inside a folder is skipped, going either way.** The walk carries
regular files and directories and counts everything else — links, sockets,
devices — as skipped, which is what stops a copy escaping the folder it was
asked for: a link to `/` inside a home directory is the whole filesystem, and a
link pointing back above its own folder is a walk that never ends. The
consequence is at the top level too: the Download button treats a link the way
it treats a file, so a link pointing at a directory fails rather than fetching
what it points at. Resolving links safely means deciding what "inside the
folder" means on the far machine, which is a question afterm cannot answer from
a listing alone.

**Merging a folder into one on the host asks once, not per file.** The upload's
three answers are all about the folder. Keeping both and refusing put every
file under a name nothing held, so neither can overwrite anything; merging is
the only one that writes over a file, and it writes over every file inside that
collides. A folder of four hundred files must not be four hundred questions,
and somebody who has said "merge" about a folder has said it about what is in
it — but it does mean a merge has no per-file "skip this one", which WinSCP and
FileZilla both offer. Each file still goes through the single-file replacement,
so a directory in the way, or anything carrying a setuid bit, is refused rather
than replaced, and those come back in the count of what did not go.

**Remote telemetry assumes Linux.** It reads `/proc` and `df`. Anything else
reports unknown rather than guessing.

**Latency is available only on the native transport.** It is measured with a
no-op request on the connection already carrying the session. The OpenSSH
client owns its connection internally and exposes no equivalent measurement.

**Remote files cost a connection per operation, on the OpenSSH-client
transport.** The client is run once per listing or transfer rather than kept
alive, because framing responses inside a persistent client's output stream
could not be made to work across platforms — batch mode suppresses the command
echo and block-buffers stdout when it is not a console. Three Windows-only
failures came out of assuming otherwise. Running the client to completion and
reading everything it wrote removes the whole class of problem, at the price of
a login per navigation. The native transport pays none of it: one connection,
channels on demand.

**Replacing a file is not atomic on the native transport.**
`posix-rename@openssh.com` would make it one request, and russh-sftp 2.4's
`SftpSession` exposes no extended request to send one with; the version is
pinned deliberately, so this is a limit of the pinned dependency rather than of
the protocol. The original is moved aside and the replacement moved in, and a
connection lost between those two leaves the destination missing. What is
guaranteed instead: the original is never destroyed, it is put back if the
replacement will not go in, and if it cannot be put back the failure names the
path it is under. `Fail` and `Rename` have no such gap on either transport —
both commit with an operation the server refuses on an existing name.

**Renaming a directory on the OpenSSH-client transport keeps a narrow race.**
A file is committed with a hard link, which refuses a taken name atomically. A
directory cannot be hard-linked on any POSIX filesystem, so it goes through a
check and a rename. `rename(2)` still refuses to put a directory over a file or
over a directory with anything in it, which leaves only the case of an *empty*
directory appearing at the destination between the two steps.

**An upload's staging directory is briefly the server's default mode, on the
OpenSSH-client transport.** The batch language cannot pass a mode to `mkdir`,
so the earliest afterm can narrow the staging directory to `0700` is the
command after the one that creates it. What sits in that window is an empty
directory under a 128-bit name, and the `put` that fills it comes after the
`chmod` — so there is never content behind a mode anyone else can pass. The
native transport has no window at all: the mode is in the `open` request.

**Ownership, ACLs and special mode bits are not carried across a
replacement.** SFTP's basic attributes carry uid and gid, which afterm does not
set — a rename keeps the connected user's ownership, as `sftp` does — and ACLs
are not in the protocol. Setuid, setgid and the sticky bit are refused rather
than re-applied: a grant of privilege attached to particular content must not
follow content that arrived from somewhere else.

**Windows-only behaviour is only partly verifiable from the author's machine.**
afterm is developed on Linux. Cross-compiling the Windows target still checks
the real Windows code path, but the SSH crypto backend is C and assembly, so it
wants `cargo-xwin` and `clang-cl` rather than rustup alone — see `CONTRIBUTING.md`.
The `windows_subsystem` attribute and `CREATE_NO_WINDOW` are compile-checked
and unexercised locally. What closed most of this gap is not the cross-check
but the tests that drive the real OpenSSH binaries: they run on the Windows
agent on every pipeline run, and found a Windows-only bug on their first one.

## Rejected

- **`ControlMaster` multiplexing.** Win32-OpenSSH does not implement it. The
  options are accepted and silently do nothing
  ([#1328](https://github.com/PowerShell/Win32-OpenSSH/issues/1328),
  [#405](https://github.com/PowerShell/Win32-OpenSSH/issues/405)).
- **`SSH_ASKPASS`.** No native Windows implementation, and ignored outright on
  Windows 11 ([#2115](https://github.com/PowerShell/Win32-OpenSSH/issues/2115)).
  The proposed remembered-password feature uses the native SSH transport and
  does not change this decision.
- **`panic = "abort"`** in the release profile. Small size win, untestable
  runtime change on a platform that cannot be run locally.
- **LTO in the release profile, thin or full.** Builds 221 and 223 differed in
  that one line: 7.4 minutes and 15.83 MB with thin LTO, 3.3 minutes and
  15.53 MB without. It was costing four minutes a build to produce a larger
  binary, so the profile carries `lto = false`. Full LTO with
  `codegen-units = 1` was rejected earlier on the same grounds and by a wider
  margin.
