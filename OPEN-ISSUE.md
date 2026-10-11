# Pre-launch code review: what was found, and what was done

Recorded 2026-09-06 from a source review and passing local frontend/Rust checks.
These are implementation issues found after the native transport work. Seven
of the eight were fixed the same day; the eighth needs a Windows machine. Each
entry keeps the finding as it was written, followed by what was done about it,
so the reasoning is not re-derived. The Windows application and live SSH
fixtures still need to exercise the platform-specific paths.

The following findings were added by a follow-up source review on 2026-09-06
and fixed the same day. The first two were reproduced before they were fixed —
one against a local `sftp-server`, one against the in-process host — and each
has a test that fails against the code as it was. The telemetry buffers get
tests for the limits themselves, there being no old behaviour for a test to
fail on. The third is a frontend race in a component the suite has no harness
for, and what was done about that instead is written out under it. Where a
suggested remedy was not the one taken, the reason is under the finding.

## Follow-up review

### Fallback uploads can follow a remote symlink

The OpenSSH fallback uploader uses the predictable temporary name
`.name.afterm-part` ([`src-tauri/src/sftp.rs`](src-tauri/src/sftp.rs)). A user
with write access to the destination directory can create that name as a
symlink to another writable file before an upload starts. The `put` command
follows the symlink, so the upload overwrites the symlink target. This bypasses
the later collision checks and can corrupt an unrelated file on the remote
host. The behavior was reproduced locally with OpenSSH's direct local SFTP
server mode.

Use a server-side exclusively-created temporary file or a private staging
directory, and verify that the temporary object is a regular file before
writing or renaming it.

**Fixed 2026-09-06.** The temporary is now `.afterm-<32 hex>.part`: 128 bits
from the process's generator, carrying no part of the destination name. A name
nobody can guess is a name nobody can plant a symlink at, and creating the file
is the first anyone learns of it — `mkstemp`'s own argument. Dropping the
destination name from the temporary also fixes a smaller thing on the way: a
250-character name plus a fixed suffix is a name the server rejects, and the
new one is always the same length.

Neither remedy as written was available, and both were tried first.
`put` is the batch language's only way to send a file and there is no
exclusive-create flag to give it. A private staging directory is worse here
than an unguessable name rather than better: `mkdir` and the `put` that follows
it are two separate requests, and anyone who can write to the parent can remove
the empty directory in between and leave a symlink under its name, so the
window the fix is supposed to close is still open — narrower, and dependent on
timing rather than on planting a name in advance, which is the wrong direction
for a remedy. The regular-file check cannot be expressed at all: `ls -l` on a
single path *follows* a symlink — asked against a local server it reported the
planted link as `-rw-rw-r--` with its target's size — so the client cannot be
made to lstat one name. Only a full directory listing shows the link type, and
that means listing a directory that may hold anything, for a check a name
nobody can guess does not need.

Both the finding and the fix were run against a real `sftp-server`, and the
client's `-D` mode — which starts one as a subprocess and speaks the protocol
to it over a pipe, no ssh and no network — is now how the fallback transport is
tested: `an_upload_does_not_write_through_a_symlink_left_at_the_old_temporary_name`
plants the link at the old fixed name and requires the target to survive, and
`the_fallback_transport_commits_uploads_the_way_the_pane_expects` drives a free
name, a taken one, Keep both and Replace through the same real client and
server. Both fail against the old name. They skip on a machine with no
`sftp-server` binary, which includes the Windows agent.

### Native Replace can lose the original when the move fails

The native transfer path deletes the destination before renaming the completed
temporary upload ([`src-tauri/src/native/mod.rs`](src-tauri/src/native/mod.rs)).
If the subsequent rename fails, cleanup removes the temporary as well, leaving
neither the original nor the replacement. This was reproduced against a local
SFTP server that refused the rename request.

Make replacement atomic where the server supports it, or retain the original
until the new name has been installed successfully and restore it on failure.

**Fixed 2026-09-06.** The second remedy, because the first is not reachable
from here: the atomic form is `posix-rename@openssh.com`, and russh-sftp 2.4's
`SftpSession` exposes no way to send an extended request — the raw session that
could is private — while the version is pinned deliberately, for the reason
written into `Cargo.toml`. So `place` moves the original aside to
`.name.afterm-old` first, installs the upload under the real name, and only
then lets the old one go; a move that fails puts the original back before the
failure is returned. Replacing a directory with a file is now refused by name
rather than by accident — the old order refused it because the protocol's
`remove` will not take a directory, which was the right answer for the wrong
reason.

The in-process host grew one rule to make this testable: a name it will not let
a finished upload be moved onto, which is what a quota or a read-only remount
looks like from the client. `a_replacement_that_cannot_be_moved_in_gives_the_original_back`
requires the original back under its own name with nothing left aside, and
`replace_will_not_swap_a_directory_for_a_file` covers the other case. Against
the old order the first leaves nothing at all, which is the finding.

### An upload can refresh the wrong session after a host switch

`FileBrowser` captures the `load` callback in an upload operation and calls it
when the upload finishes ([`src/components/FileBrowser.tsx`](src/components/FileBrowser.tsx)).
If the user switches from host A to host B while A's upload is running, the
completion refreshes A's path into B's pane. The pane then combines B's session
ID with paths displayed from A. This race was reproduced with the component's
callbacks and mocked backend responses.

Associate each operation with a session and generation token, and discard its
completion or refresh when the active session has changed.

**Fixed 2026-09-06.** The generation token was already there and was the reason
the bug was invisible: `request` is bumped by every load, and a stale operation
bumping it makes its own refresh the newest rather than the oldest, so it wins.
What was missing was the session half. A ref holds whose pane it is, assigned
during the render so it is already the new session before any effect for it
runs, and everything asynchronous now checks it: `load` refuses to touch the
pane for a session that is no longer on screen, the upload loop does not clear
a progress note or ask a collision question that belongs to another host — a
question nobody can be asked is answered the way an unanswered one is, and
nothing is replaced — and the same guard covers a download, and the
authenticate-and-retry path that had the same shape. A refresh that cannot be
shown drops the cached listing for that path instead, so the directory is
fetched again the next time the user goes back to it rather than showing a
listing taken before the upload.

Not covered by a test, and this is the part worth saying plainly. The frontend
suite has no component harness — every test in it drives a module in `src/lib`,
and nothing renders React — so a test for this would mean adding a testing
library and the first component test in the project, which is a bigger decision
than the fix and not one to take while closing a review finding. The fix was
verified by reading every asynchronous path in the component against the ref,
which is how the two beyond the reported one were found.

### Telemetry parsing has unbounded line and block buffers

The native telemetry reader appends every received chunk until it sees a
newline or the `===afterm===` delimiter ([`src-tauri/src/native/mod.rs`](src-tauri/src/native/mod.rs)).
The OpenSSH fallback reader has the same unbounded line behavior
([`src-tauri/src/remote.rs`](src-tauri/src/remote.rs)). A compromised or
malfunctioning authenticated host can send an endless unterminated line or
block and grow the process indefinitely, eventually exhausting memory.

Bound individual lines and complete blocks, reject or reset malformed samples,
and terminate the telemetry stream once the limits are exceeded.

**Fixed 2026-09-06.** Both readers now reassemble through one `Blocks` in
`remote.rs`, which holds at most 8 KiB of an unfinished line and 256 KiB of an
unfinished block. Both limits are far above anything a real host produces — the
longest legitimate line is a `/proc/net/dev` row, and a block is five lines plus
one per interface, so a thousand interfaces still fits, which is a case the
test covers. Passing either limit ends the stream rather than trimming it:
whatever is on the other end is not telemetry. The native reader closes its
channel, which is how the far end is told to stop; the OpenSSH helper kills the
client, and the reason it was killed is what the status bar reports rather than
whatever the client happened to write to stderr.

Two things came free with the shared reassembler. The helper's reader no longer
uses `BufRead::lines`, which was the unbounded half of the finding and could
not be bounded as written; and a multi-byte character split across two chunks
is now one character rather than two replacement marks, because the bytes are
converted a whole line at a time instead of a chunk at a time.

## Release blockers

### Remote download names can escape the Downloads folder

`FileBrowser` appended the remote entry name directly to the local Downloads
directory before calling `download_file`. Remote names can contain backslashes
even when the remote host is Unix. On Windows, a name such as
`..\\Documents\\report.txt` is interpreted as a path and resolves outside
Downloads. The Rust download path then created that file without enforcing
containment.

**Fixed 2026-09-06.** The interface no longer sends a local path at all:
`download_file` takes the remote path and returns where the file landed. In
Rust, `localfs::safe_name` reduces the remote name to one plain component
under Windows' rules on every platform — separators of both kinds,
`< > : " | ? *` and control characters replaced, trailing dots and spaces
trimmed, `CON`/`COM1`-style device names prefixed, and `..` or an empty result
falling back to `download` — and `localfs::claim` then refuses anything the
standard library's own path parser does not read as a single normal component,
before creating the file with `create_new` inside the Downloads directory.
Tests cover both separators, `.` and `..`, a drive prefix, a UNC-like name,
reserved device names, an existing file, an existing directory, and that a
claimed path's parent is the Downloads directory. Nothing the renderer sends
can widen either check.

### Transfers overwrite existing files and expose partial results

Downloads used `File::create`, and uploads used the SFTP create operation. A
second download silently replaced the first local file, and an interrupted
transfer could leave a partial replacement. The UI offered no collision choice.

**Fixed 2026-09-06.** Downloads arrive under `name.part`, reserved with
`create_new` before a byte moves, and are moved to their real name only when
whole. A name already taken in Downloads gets `name (1)`, the way every browser
does it, and the status line says so; there is nobody to ask mid-transfer and
nothing there is precious. A transfer that fails, or whose connection drops,
removes the part file.

Uploads over the native transport are written to `.name.afterm-part`, created
with the protocol's exclusive flag so two uploads cannot share it, and moved
into place with the protocol's `rename`, which OpenSSH refuses when the target
exists. The new `Collision` choice is `fail` by default: the file pane asks —
Replace, Keep both, or Skip — only after the backend has said the name is
taken, so a stale listing cannot skip the question. Replace removes the old
file and moves the new one in; Keep both numbers the newcomer. The OpenSSH
client transport does the same through `put` to the temporary and the client's
`rename`, with an `ls` check first, because the batch language has no
exclusive create; that check is not atomic, and it is the fallback transport's
trade. A remote temporary can only be left behind if the connection itself is
gone before it can be removed, in which case it is a dotfile the next upload
of that name does not collide with. There is still no cancel button: a transfer
ends by finishing, by failing, or by losing its connection.

Tested against the in-process host: a download arrives whole with its size
reported; an upload to a fresh name lands and leaves no temporary; a taken
name is refused with the original untouched and nothing left behind; Replace
swaps the file; Keep both yields `(1)` then `(2)`; and a host that refuses
writes leaves nothing under any name. Locally: a claimed part commits beside
an existing file as `(1)` without touching it, and a failed commit releases
both names.

## High-priority lifecycle and compatibility issues

### Closing a connecting tab does not cancel its pending connection

`spawn_session` inserted the session only after authentication and terminal
setup finished, and `close_session` returned immediately when the session was
not yet in the manager, so closing a tab during a prompt or handshake could
not cancel that work. The attempt could later complete and insert a session
whose tab was gone.

**Fixed 2026-09-06.** `SessionManager` tracks a session from `spawn_session`'s
first line. `begin` records it as pending with a cancellation handle, the
attempt runs under `tokio::select!` against that handle, and `finish` inserts
the session only if the record is still there, holding both locks across the
move so there is no instant at which a close finds nothing. `close_session`
on an id that is not open cancels the pending attempt and releases its
credential dialog, which is what lets a handshake parked on a prompt notice.
An attempt that completes after its tab has gone — the case where cancelling
the prompt used to make the native attempt fail and fall back to spawning
`ssh.exe` for a tab that no longer existed — is dismantled instead of
inserted. Dropping the attempt mid-handshake drops the russh handle, which
ends the connection. The state machine is unit-tested: pending once, a cancel
refuses the late result, a failed attempt frees the id. Closing during host
lookup, a password prompt, the native handshake and the OpenSSH fallback all
pass through the same two calls, but none of the four has been exercised in
the application on Windows; that belongs to the acceptance test below.

### Native authentication does not fully honor effective OpenSSH settings

The `ssh -G` approach was good, but the native model retained only selected
directives. It did not preserve settings such as `IdentitiesOnly` and
`IdentityAgent`, while authentication still tried the agent. That could offer
an identity the user's OpenSSH configuration explicitly excluded.

**Fixed 2026-09-06.** `config.rs` now carries the three-bucket taxonomy in one
table, with the reasoning beside it.

*Implemented:* `IdentitiesOnly` — the agent may sign only for keys named by
`IdentityFile`, matched on key material through the `.pub` file or derived
from the private file, which is how OpenSSH reads it; `IdentityAgent` —
`none`, the default, or a path with `~`, `${VAR}`, `%d` and `%u` expanded;
`PubkeyAuthentication`, `PasswordAuthentication` and
`KbdInteractiveAuthentication` as switches on each method; and `BatchMode`,
which forbids prompting even on a click, with a failure that names the
directive and offers no button.

*Refused by name, so the host stays on the OpenSSH client:* `ProxyJump`,
`ProxyCommand`, `ProxyUseFdpass`, `BindAddress`, `BindInterface`,
`AddressFamily`, `HostKeyAlias`, `CertificateFile`, `RevokedHostKeys`,
`KnownHostsCommand`, `HostbasedAuthentication`, `PKCS11Provider`,
`SecurityKeyProvider`, `ForwardAgent`, `ForwardX11`, `LocalForward`,
`RemoteForward`, `DynamicForward`, `Tunnel`, `RemoteCommand`,
`PermitLocalCommand`, `SessionType`, `RequestTTY`, `StdinNull` and
`ForkAfterAuthentication` — each judged against what the client prints for
its default, so nothing is refused merely for appearing in the output.

*Honoured by narrowing what afterm negotiates:* `Ciphers`, `MACs`,
`KexAlgorithms`, `HostKeyAlgorithms` and `PubkeyAcceptedAlgorithms`. These were
in the list below until the September 2026 review, on the reasoning that russh
negotiates from a modern set and the outcome would differ only in which strong
algorithm was picked. That is true of what afterm would *choose* and beside the
point of what the configuration *forbids*: a narrowed list is a policy, and a
known host key does not license negotiating something the policy rules out. The
effective lists are intersected with what russh supports, in the
configuration's own order; an empty intersection is a refusal by directive
name, never a quiet return to the defaults. The result is the client config
russh also rekeys from.

*Read and deliberately not acted on,* recorded in the source: `ConnectTimeout`
and keep-alives, `Compression`, `ControlMaster`, the known-hosts writing
options, and `GSSAPIAuthentication`, which is on by default on some
distributions.

Tests: fixtures for every refusal and for every default the client prints; the
in-process host proves `PubkeyAuthentication no` offers no key and that
`BatchMode yes` refuses to prompt on a click; the `IdentitiesOnly` filter and
the reading of public halves are tested directly. Two tests run the installed
`ssh -G`: one on a plain host, one on a configuration written for the test
that sets `IdentitiesOnly`, `IdentityAgent none`, `PubkeyAuthentication no`
and `BatchMode yes`, then `ForwardAgent`, `LocalForward`, `RemoteCommand` and
`ProxyJump` in turn. Both run wherever an OpenSSH client is installed, which
includes the Windows agent, so Win32-OpenSSH's spelling of each directive is
what gets checked there.

### Authentication prompts are single-slot and retain answer state

`AuthDialog` stored one request. If two sessions requested credentials, the
later request replaced the first while the first backend waiter remained
active. Submit and dismiss also cleared the visible request without clearing
the `answers` array, leaving the last secret referenced in React state until
the next prompt or unmount.

**Fixed 2026-09-06.** Requests queue oldest-first (`src/lib/auth-queue.ts`).
The front one is shown, with a line saying how many hosts wait behind it, and
each answer is sent under its own request id — the backend already routed on
that id, so nothing else changed there. What is typed is keyed to the request
it was typed into, so the next prompt's fields cannot inherit it, and it is
dropped on submit, on dismiss, and on the backend's resolved event. Tests
cover order, no duplicate on a re-announced request, removal by id, one blank
per prompt, typing scoped to its own request, and forgetting on resolution.
This is memory-retention hygiene, not a claim that JavaScript can erase every
copy it has made.

### Saved hosts prevent session and workspace restoration

`startupSessions` returned no restored sessions whenever any saved host
existed; `openingWorkspace` made the same choice. Saving a host changed the
next launch from restoring the user's work to showing the landing dialog.

**Fixed 2026-09-06.** `startupSessions` reads only the remembered sessions, and
`startupWorkspace` restores their split; the connect dialog opens unasked only
when nothing was open, and opens on the saved hosts when there are any. Worth
saying plainly: `README.md` and `ROADMAP.md` described the old behaviour as
deliberate — "Restore is skipped when saved hosts exist" — so this reverses a
documented decision, and both are updated. The reviewer's argument won because
the restore setting stayed on and quietly stopped doing anything the moment a
host was saved. Test: a saved host alongside two remembered tabs in a right
split comes back as two tabs in two panes with the dialog down. While there,
the roadmap's claim that restore is "switchable in Settings" turned out to be
wrong — there is no such control; the `afterm.restoreSessions` key in local
storage is the only switch. Corrected in the roadmap rather than built.

## Product-behavior follow-up

### Dragging a remote file out does not provide a real Explorer destination

The behaviour downloads to the configured Downloads folder when a drag is
released outside the pane. That is a reasonable WebView limitation, but it does
not match normal Explorer drag semantics. Keep the explicit Download action as
the reliable path, and either make the drag affordance say where the file will
go or remove it until a real OS drag-out integration exists.

**Done 2026-09-06, the first way.** Starting a drag on a file puts "Let go
outside the file pane to download *name* to *Downloads path*" in the status
line, and the drag's allowed effect is `copy`. The Download button is
unchanged. Whether `dragend` fires when a drag leaves the WebView2 window is
still the unverified item the roadmap lists.

### First-connection and fallback behavior needs a Windows acceptance test

The native transport refuses an unrecorded host key, after which the terminal
path can fall back to OpenSSH and show its familiar acceptance prompt. Confirm
that the terminal fallback, subsequent SFTP/telemetry state, and repeated
authentication are explained and recover correctly. Test a fresh Raspberry Pi,
a changed key, a password-only host and a host with unsupported routing config.

**Still open.** Needs a Windows machine and a real host; nothing above changes
what it has to cover. This round adds three things to its list: closing a tab
while its password prompt is up, which should leave no session behind and no
dialog; dropping a file onto a directory that already has it, which should
show the Replace / Keep both / Skip note; and a download of a file whose name
Windows cannot take, which should land under a sanitised name in Downloads.

## Found on the way

`kills_a_client_that_never_finishes` put a stub `sftp` on `PATH` for the
duration of the test. `PATH` belongs to the whole process and `cargo test` runs
tests in parallel, so the first tests to spawn the real client — the two added
for the symlink finding — took the stub instead, waited on its `sleep 300`, and
then failed on files it had never written. The stub is named directly now,
through a client field on `RemoteFiles` that only a test sets, and nothing in
the suite mutates process-wide state any more. The self-removing temporary
directory the battery had is now a shared test module, because both transports
need one.

The OpenSSH `sftp` client's `rename` uses the `posix-rename@openssh.com`
extension against OpenSSH servers, which is `rename(2)` and replaces an
existing target. The comment in `sftp.rs` said the opposite; it was describing
the legacy request, which the client only sends to servers without the
extension. The pane's rename over the client transport now checks first. The
native transport sends the legacy request, which OpenSSH does refuse, so it
was already right.

## Verification status

At review time: 97 frontend tests passed, 84 Rust tests passed, and the
production build passed. One frontend test and four Rust integration tests were
skipped. No Windows UI run was available, and the live SSH scripts were not
run.

After the fixes, 2026-09-06: 103 frontend tests pass and 1 is skipped; 113
Rust tests pass and 4 are ignored, the ones that need the live sshd fixtures;
the production build passes. Two of the Rust tests drive the installed OpenSSH
client and ran here against OpenSSH 9.6; on the Windows agent the same tests
run against Win32-OpenSSH. The Windows cross-compile check (`npm run
check:windows`) could not run on this machine, which has no `clang-cl`, so
the one Windows-only change — the agent connecting to a configured named pipe
— is compile-checked by the Azure pipeline and nowhere else yet. No Windows UI run was
available here, and the live SSH scripts were not run. The issues above
therefore include both confirmed code paths and Windows-specific behaviour
that still needs the acceptance test.

After the follow-up review's fixes, 2026-09-06: 120 Rust tests pass and 4 are
ignored; 103 frontend tests pass and 1 is skipped; `tsc` and
`cargo check --all-targets` pass. Six of the Rust tests are new, and each was
run against the code as it was before its fix and seen to fail. Both live
scripts were run this time and pass: `scripts/live-sftp-test.sh` against a
throwaway sshd, and `scripts/live-native-test.sh` for the native transport's
three. `npm run check:windows` still cannot run here. Nothing in this round
touched a Windows-only path, but two of the four fixes are in code whose
behaviour has differed between OpenSSH builds before, and the new
`sftp-server` tests skip where there is no such binary — which is exactly the
Windows agent. The Windows acceptance test above still covers them.

---

# Closed issue: files and telemetry do not work on password-only hosts

**Decided 2026-09-04: option C, the full native transport, starting now.** Two
reviewers recommended option C scoped to password-only hosts as a side channel.
The scoping was there to protect users who have key authentication from
regressions; afterm's users are the author, who asked for the whole thing. The
decision, what was checked to reach it, and the staging are in
[Decision](#decision) below. Everything above it is the record of how the
question was framed, kept because the rejected options should stay rejected.

## The gap

On an SSH host that only accepts an interactive password, afterm gives you a
working terminal and nothing else. The file browser reports `needs key auth` and
the status bar shows em dashes.

Both are helper connections that authenticate separately from the terminal, and
both use `BatchMode=yes`, which refuses to prompt. That is deliberate — a
prompt written to a subprocess nobody can see would hang — but the result is
that two of afterm's three headline features are unavailable unless the user has
key or agent authentication.

**Why this is major rather than a limitation:** afterm is intended as a
general-purpose Windows terminal, and password-only hosts are common —
appliances, legacy boxes, hosts with policies against key auth, and anyone who
simply has not set a key up. For those users the file pane and telemetry are
dead surfaces, and the failure is quiet.

Users who do have keys are unaffected. The author is in that category, which is
part of why this was mis-prioritised for most of the session.

## Why it cannot be fixed within the current design

The terminal is an `ssh.exe` process in a PTY. It holds an authenticated
connection, but that connection cannot be shared:

- **`ControlMaster` multiplexing**, the standard answer, is not implemented by
  Win32-OpenSSH. The options are accepted and silently do nothing
  ([#1328](https://github.com/PowerShell/Win32-OpenSSH/issues/1328),
  [#405](https://github.com/PowerShell/Win32-OpenSSH/issues/405)).
- **`SSH_ASKPASS`**, which would let afterm hand a cached password to the
  helpers, has no native Windows implementation and is ignored outright on
  Windows 11 ([#2115](https://github.com/PowerShell/Win32-OpenSSH/issues/2115)).

Both were investigated and rejected. Do not re-check them.

## The three options

### A. Leave it. Password hosts get a terminal only.

No work, no risk. The gap stays, and stays quiet — which is the part worth
arguing about. At minimum the interface should say plainly that files and
metrics need key authentication and offer to install a key, which the sidebar
already does.

### B. Drive `sftp` through a PTY and answer the password prompt.

afterm already depends on `portable-pty` for local shells. Spawning `sftp` on a
pseudo-terminal instead of pipes makes it prompt, and afterm can type the
password the user supplied, then run the batch as it does now.

Far cheaper than C. Three real objections:

1. **afterm would hold the user's password in memory**, which it never does
   today — the user types it straight into `ssh.exe`. That is a genuine change
   in security posture and deserves a deliberate decision, not an
   implementation detail.
2. **It reintroduces interactive-stream parsing**, which caused four
   consecutive Windows-only failures in this very component (see History). The
   surface is much smaller — detect one prompt — but it is the same class of
   problem, on the same platform, that has already burned this project.
3. It is the `sshpass` pattern, widely held to be fragile: prompt wording
   varies, and multi-prompt keyboard-interactive or 2FA flows do not fit it.

### C. Native SSH for files and telemetry — v2 in `ROADMAP.md`.

One authenticated connection per session, with SFTP and telemetry as channels on
it. This is how MobaXterm, WinSCP and Termius work, and why they handle
password-only hosts. Keep `ssh.exe` for the terminal, so login, host key
handling and `~/.ssh/config` stay untouched there.

`spikes/native-ssh/` already proves every piece against a real server: publickey
and password authentication, keyboard-interactive, an exec channel, an SFTP
subsystem and a PTY channel, all on one connection. Its security battery runs
entirely in-process and so executes on the hosted Windows agent — which is where
every regression in this component has hidden.

Against it:

- **It forfeits `~/.ssh/config` fidelity**, which the current approach gets free
  by driving the real client: `ProxyJump`, host aliases, `IdentityFile`,
  `known_hosts`, agent. A parser exists (`ssh2-config`) but silently discards
  `UserKnownHostsFile`, `StrictHostKeyChecking` and `PreferredAuthentications`,
  so the mitigation is to enumerate directives against an allowlist and fall
  back to `ssh.exe` for anything affecting routing or security. Designed, not
  built; details in `ROADMAP.md`.
- **`known_hosts` verification becomes afterm's job** for that connection.
  `russh` implements it, and the spike verified match, changed and unknown, plus
  that no credential is offered to a host that fails the check. (This paragraph
  originally said `check_server_key` defaults to accepting every key, so getting
  it wrong is silent. That is backwards — the default rejects every key. See
  [Decision](#decision).)
- It is a substantial build.

## What a second opinion should weigh

1. Is B's trade — afterm holding a password — acceptable to close the gap
   cheaply, or exactly the kind of shortcut a terminal should not take?
2. Given four platform-specific failures already came from parsing an
   interactive client's output on Windows, is B's smaller version of that
   surface a reasonable risk, or a repeat of the same mistake?
3. Is losing `~/.ssh/config` fidelity a bigger regression than the author now
   credits? It was undervalued for most of the session, then arguably
   overvalued once it became the only argument against C.
4. Is there a fourth option nobody has considered?

## Second opinion: Reviewer1_fable

Recorded 2026-09-04 after reading this file, `README.md`, `ROADMAP.md`,
`src-tauri/src/sftp.rs`, `src-tauri/src/remote.rs`, `src-tauri/src/lib.rs` and
`spikes/native-ssh/`, and checking the `russh` 0.63.2 source for the facts
relied on below.

**Recommendation: option C, scoped to the gap.** Build the native connection as
a side channel that is used only when the `ssh.exe` helper fails for
authentication reasons. Leave every path that works today exactly as it is.

How it works:

1. **Keep today's helpers.** When the sftp reachability check fails, its stderr
   already names the methods the server accepts:
   `Permission denied (publickey,password)`. If that list contains `password`
   or `keyboard-interactive`, the sidebar shows a password prompt. Key and
   agent users never see it.
2. **One connection, one handshake.** Open a single `russh` connection with the
   password, run SFTP and the telemetry exec channel as channels on it, and
   drop the password once authentication completes. Nothing is retained,
   because nothing needs replaying. This is the material difference from
   option B, which spawns a client per operation and therefore has to hold the
   password for the whole session and type it on every click.
3. **Host key check, no trust prompt.** The terminal's `ssh.exe` has already
   recorded this host in `known_hosts`, so the side connection only verifies.
   Known proceeds. Unknown or changed refuses with a message and offers no
   credential. `russh` handles hashed `|1|` entries. Set `Preferred.key` to the
   algorithms present in the matching entry, or `russh` may negotiate a key
   type `ssh.exe` never saved and report the host as unknown.
4. **`ssh_config`, conservatively.** If a `Host` block matches and contains
   anything beyond `HostName`, `User` and `Port`, refuse with a named reason.
   Password-only hosts rarely carry more than that, and hosts with `ProxyJump`
   or `IdentityFile` almost always have keys, which keeps them on the current
   path. The fidelity concern belongs to a later phase, not this one.
5. **Fix the detection bug at the same time.** `RemoteMetrics::connect` returns
   Ok as soon as the process spawns and discards stderr, so on a password host
   telemetry reads `no data yet` forever and the dead handle in the slot is
   never retried. The status bar never actually says `needs key auth`. Capture
   stderr, notice the exit, report the reason and clear the slot. That same
   stderr line is the signal step 1 needs.

**What it costs.** The user types the password twice in this phase, once into
the terminal and once into the dialog. The prompt should say so. It is one
extra prompt per session against two features that currently do not exist for
these users.

**Why not B.** Beyond holding the password for the session, B cannot pre-type
it: OpenSSH reads the passphrase from the console rather than stdin, and the
Unix build discards pending input when it turns echo off, so the prompt must be
detected. The Windows build has its own implementation of that path, which is
the lesson of the History section. Keyboard-interactive and 2FA hosts do not
fit the pattern at all.

**Why not the full v2 first.** Making the native transport the default for key
and agent hosts puts `known_hosts`, the Windows agent named pipe and
`ssh_config` fidelity on the critical path for users who have no problem today.
Scoping to password hosts means any regression is measured against "nothing
works", and the connection manager gets exercised on Windows before it is asked
to carry anyone else.

Where it leads, not part of this recommendation: once the side connection has
run on Windows for a while, making it the default helper transport, with the
`ssh.exe` helpers as the allowlist fallback `ROADMAP.md` already designs,
removes the login per click, the audit noise and the missing transfer progress
for everyone. A native PTY channel for password hosts, so the password is typed
once, comes after that and opt-in.

## Second opinion: Reviewer2_Sol

Recorded 2026-09-04 after reading this file, `README.md`, `ROADMAP.md`, the
current frontend and backend session lifecycle, and the native SSH spike and
its pinned dependency sources.

**Recommendation: accept Reviewer1's direction, with changes to trust,
configuration and authentication discovery.** Choose option C as a narrowly
scoped native side connection for password-only hosts. Keep `ssh.exe` as the
terminal and keep the existing OpenSSH helpers for key and agent users. Do not
start with a native PTY, and do not implement option B.

I would tackle it in this order:

1. **Fix the current failure reporting independently.** `RemoteMetrics::connect`
   reports success once the process spawns, throws stderr away, and leaves the
   exited helper in the session slot. Give it an explicit lifecycle —
   connecting, ready, or exited with a reason — and clear the slot on exit.
   Classify SFTP failures into typed causes as well, instead of making the UI
   infer capability from an arbitrary error string.
2. **Add one session-level native side connection.** Its state should move from
   unprobed, to authentication required, to connecting, ready or disconnected.
   File and telemetry requests must share one in-flight connection attempt so
   concurrent polling cannot create duplicate connections or password prompts.
   Once authenticated, SFTP and the telemetry exec stream are channels on that
   connection. A disconnect discards it and requires the user to authenticate
   again; no password is retained for replay.
3. **Offer the side connection only after the existing helper cannot
   authenticate.** This leaves every currently working path untouched. The UI
   should expose an explicit `Enable files and metrics with password` action;
   a two-second metrics poll should never open a credential dialog by itself.
4. **Resolve configuration conservatively, but do not reject harmless config.**
   First spike `ssh -G -p <port> -l <user> <host>` on Win32-OpenSSH. It may let
   the installed client evaluate aliases, wildcards, `Include` and `Match`,
   while afterm consumes only the effective fields it understands. Refuse a
   side connection with a named reason when effective routing or security
   requires unsupported behaviour such as `ProxyJump`, `ProxyCommand`, a
   host-key alias or an unsupported known-hosts policy. If `ssh -G` is not
   reliable on Windows, use an explicit supported/ignored/refused directive
   taxonomy. Rejecting every block containing anything beyond `HostName`,
   `User` and `Port` would also reject common harmless `Host *` settings.
5. **Make host trust a hard gate of its own.** A changed key always refuses, and
   no credential is offered before verification succeeds. An unknown key gets
   an explicit fingerprint prompt and is recorded only after acceptance, or is
   refused with an instruction to finish the terminal connection and retry.
   Test multiple user and system known-hosts files, non-default ports, hashed
   entries, `HostKeyAlias`, and `@cert-authority` and `@revoked` entries as well
   as the cases already listed in `ROADMAP.md`.
6. **Discover authentication methods through SSH, not stderr.** After the host
   is trusted, use `authenticate_none` and its `remaining_methods` result to
   drive the UI. Password authentication gets a one-shot secret prompt.
   Keyboard-interactive needs a real challenge/response loop because PAM and
   2FA may send several prompts and not all of them are passwords.
7. **Be precise about credential hygiene.** Profiles, logs and long-lived
   session state must be structurally unable to retain a password. Clear the
   dialog immediately and zeroize owned backend buffers where possible. The
   renderer, IPC and `russh` still handle the secret transiently, so the honest
   claim is `handled for authentication but never retained`, not that afterm
   never touches it or can prove every memory copy was erased.
8. **Ship behind the in-process security battery and exercise it on Windows.**
   In addition to the roadmap cases, cover the connection state machine,
   simultaneous file and metrics requests, helper exit and retry, native
   disconnect and re-authentication, and the race between terminal startup and
   side-channel trust. Broaden native SSH to key users only after this narrow
   path has accumulated real Windows use.

**Perspective on Reviewer1.** The proposed blast-radius reduction is the right
way to sequence option C, and the telemetry diagnosis is correct. Avoiding B,
using one authenticated connection, not retaining the password for repeated
operations, and constraining host-key negotiation to keys the user already
trusts are all sound.

Three details need changing before implementation:

- The terminal cannot be assumed to have recorded the host key. The frontend
  marks a session connected immediately after `ssh.exe` is spawned, before it
  knows whether the user accepted a key or authenticated. The SFTP, metrics and
  native connections can therefore race one another. Host trust needs an
  explicit state or prompt rather than a timing assumption.
- Parsing `Permission denied (publickey,password)` is useful diagnostic evidence
  but should not be the source of truth for authentication methods. It is still
  client-output parsing, whereas the native protocol already returns the
  methods structurally.
- The three-directive `ssh_config` allowlist is simultaneously too restrictive
  for harmless global settings and incomplete for the settings that affect
  routing and trust. Evaluate the effective configuration and refuse only
  behaviour the side connection cannot honour safely.

One documentation detail should also be corrected during this work: the source
of the pinned `russh` 0.63.2 dependency says the default `check_server_key`
implementation rejects all keys, while this file and `ROADMAP.md` say it
accepts every key. afterm should still override it and keep the no-credential-
before-trust test, but the recorded default is inaccurate.

— **Reviewer2_Sol**

## Decision

**Option C, unscoped: the native transport becomes afterm's SSH transport, and
`ssh.exe` stays as the fallback for hosts whose configuration afterm cannot
honour faithfully.** Recorded 2026-09-04.

Both reviewers recommended C scoped to password-only hosts as a side channel,
sequenced so that key and agent users were never exposed to it. That sequencing
protects a user population afterm does not have. The author is the key-auth
user, and asked for the whole thing. With the "protect existing users" argument
removed, what remains is a verification problem, not a blast-radius problem:
afterm is developed on Linux and cannot be run on Windows locally, and every
regression in this component has hidden on Windows. The staging below is
organised around that, and around nothing else.

Two facts make it cheap, both checked rather than assumed:

- `cargo test --release` already runs on the hosted Windows agent
  (`azure-pipelines.yml`), so anything in `src-tauri` that can be tested
  in-process is tested on Windows for free. The spike's `inproc` binary proves
  an SSH server, an SFTP subsystem and the client all run inside one test
  process, with no `sshd` and no `sftp-server` binary.
- `tauri::async_runtime` is a thin wrapper over the tokio that Tauri already
  runs (`block_on`, `spawn`, `spawn_blocking` are all present in 2.11.5), so
  russh runs on the runtime that is already there rather than nesting a second
  one inside it.

### What was checked before deciding

Recorded so none of it is re-derived. All of it is mechanical and reproducible.

**The telemetry bug both reviewers found is real.** `RemoteMetrics::connect`
passes `.stderr(Stdio::null())` and its only failure path is "could not start
ssh", so an authentication failure returns `Ok` with a dead helper in the
session slot. `sample()` then returns `None` for the life of the session and the
status bar reads `no data yet` forever. The `needs key auth` branch in
`session_metrics` is unreachable in practice — the string in `README.md` and
`ROADMAP.md` describes behaviour that has never shipped.

**The frontend marks a session connected before it is authenticated.**
`TerminalPane` calls `onReady` as soon as `spawnSession` resolves, which is PTY
spawn — not authentication, not host key acceptance. Reviewer2 is right that
nothing may assume the terminal has already recorded the host key.

**`files_channel` has no in-flight deduplication.** Two callers can both run
`check()` and spawn a client; the status bar polls every two seconds. Reviewer2's
single shared connection attempt is required, not a refinement.

**`russh` 0.63.2's `check_server_key` default rejects every key.** It is
`async { Ok(false) }` at `src/client/mod.rs`, with a doc comment saying so.
Reviewer2 is right and this file, `ROADMAP.md` and the spike's own comment were
all wrong. The footgun is the opposite of the one recorded: forget the override
and nothing connects, loudly.

**`russh`'s `known_hosts` parser is not OpenSSH's.** Compiled against 0.63.2 and
probed directly:

| known_hosts shape | `check_known_hosts_path` says |
| --- | --- |
| exact match, comma lists, trailing comment, CRLF, hashed `\|1\|` | correct |
| changed key, same algorithm | `Err(KeyChanged { line })` — correct |
| port-scoped `[host]:2222` vs plain `host` | correct in both directions |
| server key algorithm differs from the recorded one | `Ok(false)` — reads as **unknown** |
| `@revoked` entry | silently skipped — reads as **unknown** |
| `@cert-authority` entry | silently skipped — reads as **unknown** |
| wildcard pattern `*.example` | not matched — reads as **unknown** |
| tab-separated fields | not matched — reads as **unknown** |
| missing file | `Ok(false)` — indistinguishable from unknown |

Every failure is toward "unknown", which is safe only while unknown always
refuses. It is not safe under a trust-on-first-use prompt: a `@revoked` entry
reads as unknown, so a prompt would offer to accept a key the user has
explicitly revoked. And the algorithm-mismatch row makes Reviewer1's
`Preferred.key` warning mandatory rather than defensive — negotiate ed25519
against a host whose recorded entry is the ecdsa key `ssh.exe` saved, and afterm
refuses a host the user has trusted for years.

**`ssh -G` resolves the effective configuration, and reports what matters.** Run
locally against a config with aliases and a jump host, it emits 90 normalised
directives including `hostname`, `user`, `port`, `identityfile`, `proxyjump`,
`hostkeyalias`, `preferredauthentications`, `stricthostkeychecking`,
`userknownhostsfile` (absolute) and `globalknownhostsfile`. It is the user's own
client evaluating `Include`, `Match` and wildcards.

**`ssh-keygen -F` is a known_hosts oracle.** It resolves hashed entries and
wildcard patterns, searches whichever file it is given, exits 1 when nothing
matches, and labels markers in its comment line — `found: line 2 REVOKED`,
`found: line 3 CA`.

Both are run-to-completion captures of a non-interactive, machine-readable
output, which is the pattern that fixed the SFTP client (see History), not the
interactive-stream pattern that broke it four times.

### The reviewers' disagreements, ruled

1. **`ssh_config` — Reviewer2.** Reviewer1's three-directive allowlist would
   refuse any host under a `Host *` block carrying `ServerAliveInterval`, which
   is the shape in afterm's own spike fixture. `ssh -G` is strictly better and
   gets more valuable as the honoured set grows.
2. **Host trust — Reviewer1, for now.** Verify only, no trust-on-first-use,
   while `ssh.exe` still owns first contact: an unknown key refuses and says to
   finish connecting the terminal first. Reviewer2's premise is right and is
   satisfied by refusing rather than by prompting. The prompt becomes necessary
   only in stage 4, when afterm owns first contact — which is why the trust
   layer is built on `ssh-keygen -F`, which can see the markers russh's parser
   cannot, from the start.
3. **Authentication discovery — both, in their own places.** `remaining_methods`
   after `authenticate_none` is the source of truth for what to send. The
   `sftp` stderr line stays useful as a diagnostic for why the CLI transport
   failed, and nothing more.
4. **`check_server_key`'s default — Reviewer2.** Corrected here, in
   `ROADMAP.md`, and in the spike comment.
5. **Password held in memory — neither reviewer's worry survives.** One
   connection means one handshake: the secret is used once and dropped. Option B
   would have had to hold it for the session and retype it per operation. It is
   never written to a profile, and never logged.

### Staging

Ordered so that each stage is provable before the next depends on it. Stage
numbers are referenced by `CHANGELOG.md`.

**Stage 0 — make the current failure honest.** Typed failure causes for both
helpers, a real lifecycle on the telemetry helper, the dead handle cleared on
exit. Not throwaway: `ssh.exe` stays permanently as the fallback transport, and
the transport selector needs exactly this classification.

**Stage 1 — the seam.** One `Transport` type with a `Cli` variant and a `Native`
variant, mirroring today's method set; remote commands become `async`; the
terminal's three `portable-pty` fields move behind a `Terminal` enum with one
variant. No behaviour change.

**Stage 2 — connection, trust, authentication.** One connection per session,
channels on demand, a single shared in-flight attempt. Trust via `ssh-keygen -F`
over the files `ssh -G` names, with `Preferred.key` pinned to the recorded
algorithms. Configuration via `ssh -G`, refusing `ProxyJump`, `ProxyCommand`,
`HostKeyAlias` and unsupported host-key policy with a named reason so those
hosts stay on the CLI transport. All four authentication methods, because the
author's own hosts are key hosts and daily use on Windows is the only mileage
this project can actually accumulate.

**Stage 3 — the default.** Native becomes the transport for files and telemetry;
the CLI transport becomes the fallback. The password-only gap closes here, along
with the login per click, the audit noise and the missing transfer progress.

**Stage 4 — the terminal.** The PTY channel, as a separate tab kind first so it
can be compared side by side with the `ssh.exe` tab. Least reuse from the
earlier stages and the most subjective bar, so it goes last. Trust-on-first-use
arrives with it, because this is the point at which afterm owns first contact.

*What happened:* it shipped as its own kind, and the comparison it was there to
enable answered a different question than expected — a split default meant a
password host asked for the same password twice, once for the terminal and once
for everything else, which is the friction the whole transport existed to
remove. The default flipped a few days later and the tick survives inverted.
Trust-on-first-use did not arrive with it and is not needed: a session whose
host afterm will only verify falls back to the OpenSSH client for its terminal,
where the accept prompt is the one the user already knows.

### As built

Recorded 2026-09-04, after the stages above were implemented.

All five stages landed, and the native transport is the default for everything:
the terminal, files and telemetry ride one connection, so a host that wants a
password asks once. That last part took one more turn than planned — the
terminal shipped opt-in behind a tick, and the split default meant the same
password twice — but the tick survives inverted as the escape hatch, and a host
afterm cannot carry falls back to the OpenSSH client by itself.

Three things the implementation changed from the plan:

- **`ssh -G` refuses on more than routing.** `CertificateFile` and
  `RevokedHostKeys` join `ProxyJump`, `ProxyCommand` and `HostKeyAlias`, because
  each of them changes what a connection trusts and afterm implements none of
  them. A refusal sends the host to the OpenSSH client with the directive named.
- **`@cert-authority` is a fallback, not a refusal.** A revoked key means *no
  one* should connect, so it refuses outright. A certificate authority only
  means *afterm* cannot verify this host, which is what the OpenSSH client is
  still there for. They are different causes and behave differently.
- **The crypto backend has a build cost.** russh brings a C and assembly crypto
  library, which ends the era of cross-compiling the Windows target from Linux
  with nothing but rustup. `prebuilt-nasm` keeps the Azure agent from needing
  NASM; `npm run check:windows` now needs `cargo-xwin` and `clang-cl`. It is
  documented in `README.md` rather than discovered.

**Confirmed working, 2026-09-04.** From a Windows build against a password-only
Ubuntu server: the transport authenticates with a password, and files and
telemetry run on that connection. The status bar offers the credential instead
of showing em dashes. Opened as a native session, one password covers the
terminal too. The issue this file was opened for is closed.

The Windows agent also found a bug on its first run that no Linux test could
have: `ssh -G` under Win32-OpenSSH reports its system paths with a literal
`__PROGRAMDATA__` token, so afterm was silently skipping the system-wide
`known_hosts` — the file a managed estate provisions host keys into. That is the
argument for the tests that drive the real binaries, made on their first run.

**Still unproven.** The OpenSSH agent over its named pipe, a real 2FA host, a
jump host falling back as it should, and the native terminal under sustained
use.

Two things that would have been on that list are not, because they are now
tested against the installed binaries rather than against fixture text: `ssh -G`
resolving a host, and `ssh-keygen -F` finding, refusing and resolving the entry
shapes russh's own reader gets wrong. Those tests run wherever an OpenSSH client
exists, which includes the Windows agent, and skip where one does not. The
lesson from the History section is why they exist — this component behaves
differently between OpenSSH builds in ways that are neither documented nor
visible from Linux, so the parts that talk to it are checked by talking to it.

### What was dropped

The key-installer remedy — wiring afterm's existing one-click
`authorized_keys` install into the file pane's empty state as the cheap fix for
password-only hosts. It was worth proposing when the alternative was a
password-only side channel serving a handful of sessions. With stage 3 landing
it is redundant: native password authentication works on hosts that have no key
and on hosts that will never accept one. The sidebar button stays.

## The other costs of the current design, measured

Not the main issue, but part of option C's case. Measured against this project's
own test host during real use:

| | |
| --- | --- |
| One sftp login | ~190 ms on a LAN |
| Logins during a day of testing | 47 |
| Busiest minute | 6 |

- **Latency per action.** Negligible on a LAN. Over a 100 ms RTT link an SSH
  handshake is four to six round trips, so half a second to a second per
  directory click.
- **Audit noise.** Every click writes an `Accepted publickey` record. On a
  monitored host that is a lot of lines to explain.
- **Login-time cost.** Each connection runs the host's full session setup — LDAP
  lookups, automounted home directories — paid per click, and it fills `last`.
- **No transfer progress.** A one-shot `get` reports nothing until it finishes.

An unclaimed easy win, independent of the decision: **cache directory listings**
so navigating back up does not re-login.

## History: four Windows-only failures, all resolved

Kept so nobody retries a disproved hypothesis. Remote files originally used one
long-lived `sftp -b -` process with responses framed inside its output stream.
That failed on Windows four times while passing on Linux:

1. Matching the `sftp> <command>` echo exactly. Batch mode enforces quiet mode,
   so the echo is documented to be suppressed. It appears on the Linux build
   used for development.
2. Matching that echo leniently. The text is not there to match.
3. Using a `pwd` line as the terminator. Also nothing — the client
   block-buffers stdout when it is not a console, so a persistent client may
   emit nothing at all until it exits.
4. Alongside those, the application froze entirely: `session_metrics` opened the
   file channel and the status bar polls every two seconds, so each poll spawned
   another client and blocked a worker until the pool was exhausted.

**The diagnosis that settled it.** The remote under test was the development
machine, so the server's own log was readable:

```
Accepted publickey for arya from 10.101.1.20 port 52707 ssh2: RSA SHA256:T7Y7Cuh…
```

Authentication succeeded every time, zero failures. Connection, auth and
subsystem were all fine; only reading the client's output was broken.

**The fix.** Run the client once per operation. Closing stdin makes it process
the batch and exit, and a process that exits flushes its buffers, so afterm
reads everything it wrote and parses it whole. Nothing depends on prompts,
echoes, ordering or flush behaviour. Confirmed working on Windows by the user on
2026-09-04.

The lesson worth carrying: this component behaves differently between OpenSSH
builds in ways that are neither documented nor visible from Linux. Anything that
parses the client's interactive output should be assumed broken on Windows until
proven otherwise on Windows.
