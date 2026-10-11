mod caption;
mod clipimage;
mod flow;
mod folder;
mod links;
mod persist;
mod fsmodel;
mod localfs;
mod metrics;
mod native;
mod openssh;
mod remote;
mod secrets;
mod sftp;
mod sshkeys;
#[cfg(test)]
mod tempdir;
mod transport;
mod winproc;

use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use flow::FlowGate;
use fsmodel::Listing;
use metrics::Metrics;
use portable_pty::{native_pty_system, ChildKiller, CommandBuilder, MasterPty, PtySize};
use serde::{Deserialize, Serialize};
use sshkeys::DiscoveredKey;
use std::{
    collections::HashMap,
    io::{Read, Write},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use tauri::{AppHandle, Emitter, Manager, State};
use tokio::sync::oneshot;
use transport::{Cause, Collision, Failure, RemoteSession};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionProfile {
    id: String,
    name: String,
    kind: SessionKind,
    host: Option<String>,
    user: Option<String>,
    port: Option<u16>,
    /// Keep the shell running on the host between connections, in tmux. Off
    /// unless a saved host turns it on; see `persist.rs`.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    persistent_shell: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SessionKind {
    /// SSH. The terminal rides afterm's own connection where it can, and the
    /// OpenSSH client where it cannot.
    ///
    /// One connection means one credential. Two meant typing the same password
    /// twice, once into `ssh.exe` for the terminal and again for files and
    /// telemetry, which is the wrong default for the case this transport was
    /// built for.
    ///
    /// `ssh-native` is the name this had while it was opt-in, kept so hosts
    /// saved under it still open.
    #[serde(alias = "ssh-native")]
    Ssh,
    /// SSH with the terminal always through the OpenSSH client, whatever afterm
    /// could have done itself. The escape hatch: login, host keys and
    /// `~/.ssh/config` behave exactly as they do everywhere else on the
    /// machine. Files and telemetry still use afterm's own connection, so this
    /// is the option that asks twice.
    SshCli,
    /// A shell on this machine, on a local pseudo-terminal.
    ///
    /// These were dropped once and are back on narrower terms. There is no
    /// profile to save, no address to validate, no transport to choose and no
    /// far end to measure: `remote` is `None` for one, and every command that
    /// needs a far end refuses rather than inventing one. That is the whole of
    /// what makes them cheap enough to carry — the version that was dropped
    /// had an `is_remote` branch in the status bar, the tab icon, the file
    /// pane and the telemetry command.
    Powershell,
    Cmd,
}

impl SessionKind {
    /// Whether this session runs here rather than somewhere else.
    fn is_local(self) -> bool {
        matches!(self, SessionKind::Powershell | SessionKind::Cmd)
    }
}

#[derive(Clone, Serialize)]
struct TerminalOutput {
    id: String,
    data: String,
}

/// Why a terminal stopped, which is the difference between closing its tab and
/// offering to reconnect.
#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) enum ExitReason {
    /// The shell ended on its own: `exit`, Ctrl-D, a clean logout. There is
    /// nothing to reconnect to and nothing went wrong.
    Exited,
    /// The connection went away underneath it — a reset, a timeout, a host
    /// that dropped off. The session is gone but the intent behind it is not.
    Lost,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct TerminalExit {
    id: String,
    reason: ExitReason,
}

/// Terminal bytes bypass React state entirely: they go from here to the
/// xterm.js instance.
pub(crate) fn emit_terminal_output(app: &AppHandle, id: &str, data: &[u8]) {
    let _ = app.emit(
        "terminal-output",
        TerminalOutput { id: id.to_string(), data: BASE64.encode(data) },
    );
}

pub(crate) fn emit_terminal_exit(app: &AppHandle, id: &str, reason: ExitReason) {
    let _ = app.emit("terminal-exit", TerminalExit { id: id.to_string(), reason });
}

/// Puts credential requests on the screen.
///
/// The native transport asks through an interface rather than an `AppHandle`,
/// so this is the only place the two are joined and the connection path stays
/// drivable from a test.
struct DialogPrompts(AppHandle);

impl native::PromptSink for DialogPrompts {
    fn request(&self, request: &native::AuthRequest) {
        let _ = self.0.emit("ssh-auth-request", request);
    }

    fn resolved(&self, request: &str) {
        let _ = self.0.emit("ssh-auth-resolved", request);
    }

    fn notice(&self, session: &str, message: &str) {
        let _ = self.0.emit("ssh-auth-notice", AuthNotice {
            session: session.to_string(),
            message: message.to_string(),
        });
    }
}

/// Something worth saying about a session that is otherwise fine. Carries the
/// session so the interface can say which tab it is about.
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct AuthNotice {
    session: String,
    message: String,
}

/// Where a session's keystrokes go.
///
/// Two shapes, one interface. An `ssh.exe` session is a process on a local
/// pseudo-terminal; a native SSH session is a channel with a pseudo-terminal
/// negotiated on the remote host, which is why it needs no local process and
/// no platform-specific code at all.
enum Terminal {
    Pty {
        writer: Mutex<Box<dyn Write + Send>>,
        master: Mutex<Box<dyn MasterPty + Send>>,
        /// Not the child itself: the reader thread owns that, because only a
        /// thread that can block in `wait` can report how the process ended,
        /// and a killer is exactly what portable-pty provides for the other
        /// side of that split.
        killer: Mutex<Box<dyn ChildKiller + Send + Sync>>,
    },
    Channel(native::ShellChannel),
}

impl Terminal {
    fn write(&self, data: &str) -> Result<(), AppError> {
        match self {
            Terminal::Pty { writer, .. } => {
                let mut writer = writer.lock().unwrap();
                writer
                    .write_all(data.as_bytes())
                    .and_then(|_| writer.flush())
                    .map_err(|error| AppError::Terminal(error.to_string()))
            }
            Terminal::Channel(channel) => channel.write(data).map_err(AppError::Remote),
        }
    }

    fn resize(&self, cols: u16, rows: u16) -> Result<(), AppError> {
        match self {
            Terminal::Pty { master, .. } => master
                .lock()
                .unwrap()
                .resize(PtySize { rows, cols, pixel_width: 0, pixel_height: 0 })
                .map_err(|error| AppError::Terminal(error.to_string())),
            Terminal::Channel(channel) => {
                channel.resize(cols, rows);
                Ok(())
            }
        }
    }

    fn shutdown(&self) -> Result<(), AppError> {
        match self {
            Terminal::Pty { killer, .. } => killer
                .lock()
                .unwrap()
                .kill()
                .map_err(|error| AppError::Terminal(error.to_string())),
            Terminal::Channel(channel) => {
                channel.close();
                Ok(())
            }
        }
    }
}

struct ManagedSession {
    terminal: Terminal,
    /// The remote half: which transport carries files and telemetry, and the
    /// one attempt to establish it. `None` for a shell on this machine, which
    /// has no far end for anything to be asked of — every command that needs
    /// one goes through `SessionManager::remote` and is refused here rather
    /// than at each call site.
    remote: Option<Arc<RemoteSession>>,
    /// Held by whatever is reading this terminal, and closed by the interface
    /// when it has more bytes than it has drawn.
    flow: Arc<FlowGate>,
    /// The tmux session the terminal is attached to, for a host that keeps its
    /// shell; what closing the tab ends. See `persist.rs`.
    persistent: Option<String>,
}

#[derive(Default)]
struct SessionManager {
    sessions: Mutex<HashMap<String, Arc<ManagedSession>>>,
    /// Sessions still being opened, each with the handle that cancels the
    /// attempt. A session is tracked from the moment it is asked for, not from
    /// the moment it is ready: a tab closed during a handshake or a password
    /// prompt used to have nothing to close, and the attempt would finish
    /// later and insert a session whose tab was already gone.
    ///
    /// Lock order, where both are held: `pending` first, then `sessions`.
    pending: Mutex<HashMap<String, oneshot::Sender<()>>>,
}

impl SessionManager {
    /// Record that a session is being opened, and hand back what will fire if
    /// it is closed meanwhile. `None` when it is already open or opening.
    fn begin(&self, id: &str) -> Option<oneshot::Receiver<()>> {
        let mut pending = self.pending.lock().unwrap();
        if pending.contains_key(id) || self.sessions.lock().unwrap().contains_key(id) {
            return None;
        }
        let (cancel, cancelled) = oneshot::channel();
        pending.insert(id.to_string(), cancel);
        Some(cancelled)
    }

    /// The attempt is over. Keeps the session if its tab still wants it, and
    /// hands it back for the caller to dismantle if the tab has gone.
    ///
    /// Both locks are held across the move, so there is no instant at which
    /// the session is in neither map and a close could find nothing to close.
    fn finish(&self, id: &str, session: ManagedSession) -> Result<(), ManagedSession> {
        let mut pending = self.pending.lock().unwrap();
        if pending.remove(id).is_none() {
            return Err(session);
        }
        self.sessions.lock().unwrap().insert(id.to_string(), Arc::new(session));
        Ok(())
    }

    /// End an attempt in progress, or forget one that has failed. False when
    /// there was none.
    fn cancel(&self, id: &str) -> bool {
        match self.pending.lock().unwrap().remove(id) {
            Some(cancel) => {
                let _ = cancel.send(());
                true
            }
            None => false,
        }
    }

    fn get(&self, id: &str) -> Result<Arc<ManagedSession>, AppError> {
        self.sessions
            .lock()
            .unwrap()
            .get(id)
            .cloned()
            .ok_or_else(|| AppError::MissingSession(id.to_string()))
    }

    /// The remote half of a session: the session must exist, and must have one.
    fn remote(&self, id: &str) -> Result<Arc<RemoteSession>, AppError> {
        self.get(id)?.remote.clone().ok_or(AppError::NotRemote)
    }
}

#[derive(Debug)]
enum AppError {
    MissingSession(String),
    /// The tab was closed before its session finished opening.
    Cancelled(String),
    InvalidProfile(String),
    InvalidPublicKey,
    /// Asked of a shell on this machine for something only a host can answer:
    /// a listing, a transfer, telemetry, a key to install.
    NotRemote,
    Terminal(String),
    /// The credential store would not do what it was asked. Its own words,
    /// which name the store rather than the platform.
    Credential(String),
    Remote(Failure),
}

impl AppError {
    /// Every error reaches the interface in the same shape, so a caller can act
    /// on the cause instead of matching on the text.
    fn failure(&self) -> Failure {
        match self {
            AppError::MissingSession(id) => {
                Failure::internal(format!("session '{id}' does not exist"))
            }
            AppError::Cancelled(id) => {
                Failure::internal(format!("session '{id}' was closed before it opened"))
            }
            AppError::InvalidProfile(reason) => {
                Failure::internal(format!("invalid SSH profile: {reason}"))
            }
            AppError::InvalidPublicKey => Failure::internal("invalid public key"),
            AppError::NotRemote => Failure::unsupported(
                "this session is a shell on this machine, so there is no host to ask",
            ),
            AppError::Terminal(reason) => Failure::internal(format!("terminal error: {reason}")),
            AppError::Credential(reason) => Failure::operation(reason.clone()),
            AppError::Remote(failure) => failure.clone(),
        }
    }
}

impl From<Failure> for AppError {
    fn from(failure: Failure) -> Self {
        AppError::Remote(failure)
    }
}

impl std::fmt::Display for AppError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.failure().message)
    }
}

impl Serialize for AppError {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        self.failure().serialize(serializer)
    }
}

/// Reject hosts and usernames that could be read as OpenSSH options or split
/// into extra arguments.
fn validate_identity(value: &str, label: &str) -> Result<(), AppError> {
    if value.is_empty() {
        return Err(AppError::InvalidProfile(format!("{label} is required")));
    }
    if value.starts_with('-') || value.contains(char::is_whitespace) {
        return Err(AppError::InvalidProfile(format!(
            "{label} must not contain whitespace or begin with '-'"
        )));
    }
    Ok(())
}

fn ssh_target(profile: &SessionProfile) -> Result<(String, String, u16), AppError> {
    let host = profile.host.as_deref().unwrap_or_default();
    let user = profile.user.as_deref().unwrap_or_default();
    validate_identity(host, "host")?;
    validate_identity(user, "username")?;
    Ok((host.to_string(), user.to_string(), profile.port.unwrap_or(22)))
}

/// What a pseudo-terminal session runs.
///
/// Three commands, two jobs. `ssh.exe` carries the terminal for an SSH session
/// either because the profile asked for it or because afterm declined to carry
/// the host itself; the other two are the machine's own shells, which take no
/// arguments because there is nothing about one to configure.
///
/// The non-Windows spellings are for development: afterm ships for Windows, and
/// `pwsh` and `/bin/sh` are what stand in for the two while it is being worked
/// on somewhere else.
fn command_for(profile: &SessionProfile) -> Result<CommandBuilder, AppError> {
    match profile.kind {
        SessionKind::Ssh | SessionKind::SshCli => {
            let (host, user, port) = ssh_target(profile)?;
            let executable = if cfg!(windows) { "ssh.exe" } else { "ssh" };
            let mut command = CommandBuilder::new(executable);
            command.arg("-p");
            command.arg(port.to_string());
            command.arg("-o");
            command.arg("ServerAliveInterval=30");
            command.arg("-o");
            command.arg("ServerAliveCountMax=3");
            if profile.persistent_shell {
                // A remote command gets no terminal unless one is asked for.
                command.arg("-t");
            }
            command.arg(format!("{user}@{host}"));
            if profile.persistent_shell {
                command.arg(persist::attach_command(&persist::session_name(&profile.id)));
            }
            Ok(command)
        }
        SessionKind::Powershell => {
            let executable = if cfg!(windows) { "powershell.exe" } else { "pwsh" };
            let mut command = CommandBuilder::new(executable);
            command.arg("-NoLogo");
            Ok(command)
        }
        SessionKind::Cmd => {
            let executable = if cfg!(windows) { "cmd.exe" } else { "/bin/sh" };
            Ok(CommandBuilder::new(executable))
        }
    }
}

/// Build the remote half for an SSH profile, whichever terminal it uses.
///
/// `saved` is the id of the saved host this session came from, where it came
/// from one. It is not `profile.id`: that is the tab, made fresh every time a
/// session is opened, and a password remembered against it would belong to one
/// tab and then to nothing.
fn remote_for(
    profile: &SessionProfile,
    saved: Option<&str>,
    vault: Arc<secrets::Vault>,
) -> Result<Arc<RemoteSession>, AppError> {
    let (host, user, port) = ssh_target(profile)?;
    Ok(Arc::new(RemoteSession::new(&profile.id, saved, &host, &user, port, vault)))
}

/// Open a session and put a terminal on it.
///
/// `profile_id` is the saved host this tab was opened from, when it was opened
/// from one. Absent for an address typed in and not saved — a session with
/// nothing durable to remember a password against, and so one that is never
/// offered the choice.
#[tauri::command]
async fn spawn_session(
    app: AppHandle,
    manager: State<'_, SessionManager>,
    prompts: State<'_, native::Prompts>,
    vault: State<'_, Arc<secrets::Vault>>,
    profile: SessionProfile,
    profile_id: Option<String>,
    cols: u16,
    rows: u16,
) -> Result<(), AppError> {
    let Some(cancelled) = manager.begin(&profile.id) else {
        return Ok(());
    };

    // `Ssh` tries afterm's own connection and falls back to the client by
    // itself; `SshCli` was asked for the client outright. A local shell is the
    // same pseudo-terminal with no far end behind it, so it builds no remote
    // half and never asks for a credential.
    let opening = async {
        match profile.kind {
            SessionKind::Ssh => {
                let remote = remote_for(&profile, profile_id.as_deref(), Arc::clone(&vault))?;
                open_ssh_session(&app, &prompts, &profile, remote, cols, rows).await
            }
            SessionKind::SshCli => {
                let remote = remote_for(&profile, profile_id.as_deref(), Arc::clone(&vault))?;
                open_pty_session(&app, &profile, Some(remote), cols, rows)
            }
            SessionKind::Powershell | SessionKind::Cmd => {
                open_pty_session(&app, &profile, None, cols, rows)
            }
        }
    };
    let outcome = tokio::select! {
        // The attempt first. A session that has just finished opening is kept
        // or dismantled below on the strength of the pending record, not on a
        // coin toss between two ready branches.
        biased;
        outcome = opening => outcome,
        // Dropping the attempt mid-handshake drops its connection with it.
        _ = cancelled => Err(AppError::Cancelled(profile.id.clone())),
    };
    let session = match outcome {
        Ok(session) => session,
        Err(error) => {
            manager.cancel(&profile.id);
            return Err(error);
        }
    };

    match manager.finish(&profile.id, session) {
        Ok(()) => Ok(()),
        // Closed while it was opening: the connection was made, and now has
        // no tab. Take it down rather than keep it.
        Err(unwanted) => {
            let _ = dismantle(&unwanted).await;
            Err(AppError::Cancelled(profile.id.clone()))
        }
    }
}

/// Take a session down: release its reader, close its connection, end its
/// terminal. Shared by closing a tab and by an attempt that outlived its tab.
async fn dismantle(session: &ManagedSession) -> Result<(), AppError> {
    // A reader being held back has to reach the end of its stream to report
    // how the session ended, and it cannot do that parked on a closed gate.
    session.flow.set_paused(false);
    if let Some(remote) = session.remote.as_ref() {
        remote.close().await;
    }
    session.terminal.shutdown()
}

/// A session on a local pseudo-terminal: SSH through the OpenSSH client, or one
/// of this machine's own shells.
///
/// The remote half is handed in rather than built here, and is `None` for a
/// local shell. `open_ssh_session` arrives with one it has already tried to
/// connect, and building a second would throw that attempt away and ask for
/// the credential again.
fn open_pty_session(
    app: &AppHandle,
    profile: &SessionProfile,
    remote: Option<Arc<RemoteSession>>,
    cols: u16,
    rows: u16,
) -> Result<ManagedSession, AppError> {
    let pair = native_pty_system()
        .openpty(PtySize { rows, cols, pixel_width: 0, pixel_height: 0 })
        .map_err(|error| AppError::Terminal(error.to_string()))?;
    let mut command = command_for(profile)?;
    command.env("TERM", "xterm-256color");
    command.env("COLORTERM", "truecolor");
    let child = pair.slave.spawn_command(command).map_err(|error| {
        AppError::Terminal(format!("could not start {}: {error}", profile.name))
    })?;
    drop(pair.slave);

    let mut reader = pair
        .master
        .try_clone_reader()
        .map_err(|error| AppError::Terminal(error.to_string()))?;
    let writer = pair
        .master
        .take_writer()
        .map_err(|error| AppError::Terminal(error.to_string()))?;

    let id = profile.id.clone();
    let app = app.clone();
    let killer = child.clone_killer();
    // Only `ssh.exe` reports a connection that died; a local shell exiting 255
    // is a shell that exited with 255, and offering to reconnect to it would
    // be offering to reconnect to this machine.
    let remote_terminal = !profile.kind.is_local();
    let mut child = child;
    let flow = Arc::new(FlowGate::new());
    let reading = Arc::clone(&flow);
    std::thread::spawn(move || {
        let mut buffer = [0_u8; 32 * 1024];
        loop {
            // Before the read rather than after it: a held reader leaves the
            // bytes in the pseudo-terminal, which is where the operating
            // system wants them. The writing process blocks on a full pty the
            // way it would against any terminal that had stopped listening.
            reading.wait();
            match reader.read(&mut buffer) {
                Ok(0) | Err(_) => break,
                Ok(read) => emit_terminal_output(&app, &id, &buffer[..read]),
            }
        }
        // The pty closed, so the process is on its way out; waiting here is
        // what turns that into a reason. OpenSSH documents 255 as its own
        // "something went wrong" and passes anything else through from the
        // remote side, so 255 is the one code that means the connection died
        // rather than the shell exiting.
        let status = child.wait().ok();
        let reason = match status {
            Some(status) if remote_terminal && status.exit_code() == 255 => ExitReason::Lost,
            _ => ExitReason::Exited,
        };
        emit_terminal_exit(&app, &id, reason);
    });

    Ok(ManagedSession {
        terminal: Terminal::Pty {
            writer: Mutex::new(writer),
            master: Mutex::new(pair.master),
            killer: Mutex::new(killer),
        },
        persistent: (profile.persistent_shell && remote.is_some())
            .then(|| persist::session_name(&profile.id)),
        remote,
        flow,
    })
}

/// An SSH session, with the terminal on afterm's own connection where that is
/// possible.
///
/// Falls back to the OpenSSH client for the terminal on any failure, not only
/// on a host afterm declines to carry. The terminal is the one thing that must
/// always work: `ssh.exe` can prompt to accept a host key that afterm will only
/// verify, and can reach a host whose configuration afterm will not reproduce.
/// Files and telemetry report their own trouble separately, where it belongs.
/// What to say when afterm's own connection could not be used and the tab has
/// moved to the OpenSSH client.
///
/// The host-key case is spelled out because it is what every new host hits on
/// first contact, and because what to do about it happens somewhere the
/// message does not: the client asks whether to trust the key in the terminal,
/// and nothing proceeds until that is answered. Telling someone their session
/// fell back, without telling them a question is waiting, is barely better
/// than saying nothing.
fn fallback_notice(reason: &Failure) -> String {
    match reason.cause {
        Cause::HostKey => format!(
            "afterm's own connection refused this host's key: {} The tab is on the OpenSSH \
             client, which asks in the terminal whether to trust it.",
            ended(&reason.message)
        ),
        _ => format!(
            "afterm's own connection could not be used, so this tab is on the OpenSSH client: {}",
            reason.message
        ),
    }
}

/// A sentence that can be followed by another one. Failure messages are
/// written to be read on their own and do not all end in a stop.
fn ended(message: &str) -> String {
    let trimmed = message.trim_end();
    if trimmed.ends_with(['.', '!', '?']) {
        trimmed.to_string()
    } else {
        format!("{trimmed}.")
    }
}

async fn open_ssh_session(
    app: &AppHandle,
    prompts: &native::Prompts,
    profile: &SessionProfile,
    remote: Arc<RemoteSession>,
    cols: u16,
    rows: u16,
) -> Result<ManagedSession, AppError> {
    // Opening a tab is as deliberate as an action gets, so this one may prompt,
    // and it is the only prompt: the terminal rides the same connection.
    let native = match remote.obtain(prompts, true).await {
        Ok(transport) => transport,
        // Falling back is right — the OpenSSH client honours things afterm will
        // not pretend to — but doing it silently turns a question into nothing
        // happening. A host whose key is not yet recorded is refused here
        // *before* any credential is offered, so the tab that appears never
        // asked for a password and never said why it did not, and the only
        // trace is a failure in the file pane. Say what happened.
        Err(reason) => {
            prompts.notice(&profile.id, &fallback_notice(&reason));
            return open_pty_session(app, profile, Some(Arc::clone(&remote)), cols, rows);
        }
    };
    let transport::Transport::Native(connection) = native.as_ref() else {
        return open_pty_session(app, profile, Some(Arc::clone(&remote)), cols, rows);
    };

    let output = app.clone();
    let closed = app.clone();
    let id = profile.id.clone();
    let ending = profile.id.clone();
    let flow = Arc::new(FlowGate::new());
    let persistent = profile.persistent_shell.then(|| persist::session_name(&profile.id));
    let shell = connection
        .open_terminal(
            persistent.as_deref().map(persist::attach_command),
            move |data| emit_terminal_output(&output, &id, data),
            move |exited| {
                let reason = if exited { ExitReason::Exited } else { ExitReason::Lost };
                emit_terminal_exit(&closed, &ending, reason);
            },
            cols,
            rows,
            Arc::clone(&flow),
        )
        .await?;

    Ok(ManagedSession {
        terminal: Terminal::Channel(shell),
        remote: Some(remote),
        flow,
        persistent,
    })
}

#[tauri::command]
fn terminal_write(
    manager: State<'_, SessionManager>,
    id: String,
    data: String,
) -> Result<(), AppError> {
    manager.get(&id)?.terminal.write(&data)
}

/// How far behind the interface is allowed to fall before the reader is held.
///
/// The interface calls this, because it is the only side that knows: it counts
/// the bytes it has been handed against the bytes xterm has finished parsing.
#[tauri::command]
fn set_terminal_flow(
    manager: State<'_, SessionManager>,
    id: String,
    paused: bool,
) -> Result<(), AppError> {
    // A pane on its way out may resume a session that has already gone, and
    // that is not a failure worth showing anyone.
    if let Ok(session) = manager.get(&id) {
        session.flow.set_paused(paused);
    }
    Ok(())
}

#[tauri::command]
fn resize_session(
    manager: State<'_, SessionManager>,
    id: String,
    cols: u16,
    rows: u16,
) -> Result<(), AppError> {
    manager.get(&id)?.terminal.resize(cols, rows)
}

#[tauri::command]
async fn close_session(
    manager: State<'_, SessionManager>,
    prompts: State<'_, native::Prompts>,
    id: String,
    end_shell: Option<bool>,
) -> Result<(), AppError> {
    // Removing the session drops its transport, which closes the connection or
    // kills the helper processes.
    let session = manager.sessions.lock().unwrap().remove(&id);
    let Some(session) = session else {
        // Not open — perhaps not yet. A tab closed during its handshake or its
        // password prompt ends the attempt, and releases any dialog it was
        // waiting on, which is what lets the attempt notice.
        if manager.cancel(&id) {
            prompts.cancel_session(&id);
        }
        return Ok(());
    };
    // A credential dialog for a session that no longer exists can never be
    // answered; release whatever is waiting on it.
    prompts.cancel_session(&id);
    // Only when the user closed the tab. A reconnect closes the session too,
    // on its way to attaching to the very tmux session this would end.
    if end_shell == Some(true) {
        end_persistent_shell(&session).await;
    }
    dismantle(&session).await
}

/// End the tmux session a closed tab was attached to, while there is still a
/// connection to do it on. Best effort and bounded: the tab is already gone,
/// and nothing the host says here would be shown to anyone.
///
/// The native transport runs it on a channel of the connection it already
/// has. Otherwise there is no such channel, so it gets one more `ssh`, in
/// batch mode so that it can never stop to ask for a password — on a host
/// that needs one this does nothing, and the session outlives the tab.
async fn end_persistent_shell(session: &ManagedSession) {
    let (Some(name), Some(remote)) = (session.persistent.as_deref(), session.remote.as_ref()) else {
        return;
    };
    let command = persist::end_command(name);
    let ready = remote.ready();
    match ready.as_deref() {
        Some(transport @ transport::Transport::Native(_)) => {
            let _ = tokio::time::timeout(Duration::from_secs(3), transport.run(&command)).await;
        }
        // The OpenSSH client, or nothing established for files yet — a tab
        // closed before its file pane ever connected.
        _ => {
            let (host, user, port) = remote.address();
            let mut builder = std::process::Command::new(if cfg!(windows) { "ssh.exe" } else { "ssh" });
            let _ = winproc::hide_console(&mut builder)
                .args([
                    "-o", "BatchMode=yes",
                    "-o", "ConnectTimeout=10",
                    "-p", &port.to_string(),
                    &format!("{user}@{host}"),
                    &command,
                ])
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .spawn();
        }
    }
}

/// Where a public key ended up, once the host has confirmed it did.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct KeyInstalled {
    /// The account the key was installed for, as the *host* named it, which is
    /// not necessarily the account the session was opened under.
    user: String,
    /// The absolute path of the file it went into.
    path: String,
    /// True when the key was already there and nothing was appended.
    already_present: bool,
}

/// Append a public key to the remote `~/.ssh/authorized_keys`, and say whether
/// it worked.
///
/// Run on a channel of afterm's own authenticated connection rather than typed
/// into the terminal. Writing it into the terminal was wrong in three separate
/// ways, all of them the same mistake — the terminal is not a place to execute
/// things, it is a place the *user* is executing things:
///
/// * After `sudo -i` or a nested `ssh`, the command ran as that shell's account
///   on that shell's host, while the dialog went on naming the session's. A key
///   installed for the wrong account, on the wrong machine, reported as success.
/// * An editor, a pager, or anything else at the front of the terminal received
///   the command as keystrokes.
/// * The command ended with a `printf` that announced success unconditionally,
///   so a full disk, a read-only home or a refused `mkdir` printed "public key
///   installed" all the same.
///
/// Here the host's own exit status decides, the account and home directory come
/// back from the host and are checked, and nothing is claimed that was not
/// confirmed.
#[tauri::command]
async fn install_public_key(
    manager: State<'_, SessionManager>,
    prompts: State<'_, native::Prompts>,
    id: String,
    public_key: String,
) -> Result<KeyInstalled, AppError> {
    let key = public_key.trim();
    let valid_prefix = [
        "ssh-ed25519 ",
        "ssh-rsa ",
        "ecdsa-sha2-nistp256 ",
        "sk-ssh-ed25519@openssh.com ",
    ]
    .iter()
    .any(|prefix| key.starts_with(prefix));
    if !valid_prefix || key.contains(['\r', '\n']) {
        return Err(AppError::InvalidPublicKey);
    }

    let remote = manager.remote(&id)?;
    // Interactive: this is a button, and a host that wants a password for the
    // connection may raise its dialog here.
    let transport = remote.obtain(&prompts, true).await?;

    // The key travels base64-encoded, so nothing in it is ever interpreted by
    // the shell. `set -e` is what makes the exit status mean something: every
    // step before the report has to succeed for the report to be printed.
    let encoded = BASE64.encode(key.as_bytes());
    let script = format!(
        "set -e; umask 077; \
         mkdir -p \"$HOME/.ssh\"; \
         chmod 700 \"$HOME/.ssh\"; \
         touch \"$HOME/.ssh/authorized_keys\"; \
         chmod 600 \"$HOME/.ssh/authorized_keys\"; \
         k=$(printf '%s' '{encoded}' | base64 -d); \
         if grep -qxF \"$k\" \"$HOME/.ssh/authorized_keys\"; then present=yes; \
         else printf '%s\\n' \"$k\" >> \"$HOME/.ssh/authorized_keys\"; present=no; fi; \
         grep -qxF \"$k\" \"$HOME/.ssh/authorized_keys\"; \
         unset k; \
         printf 'afterm-user %s\\n' \"$(id -un)\"; \
         printf 'afterm-path %s\\n' \"$HOME/.ssh/authorized_keys\"; \
         printf 'afterm-present %s\\n' \"$present\""
    );

    let outcome = transport.run(&script).await?;
    if outcome.truncated {
        return Err(AppError::Remote(Failure::operation(
            "the host wrote more than afterm will read in answer to a three-line command, so \
             nothing about the result can be trusted",
        )));
    }
    if !outcome.succeeded() {
        return Err(AppError::Remote(Failure::operation(format!(
            "the host did not install the key: {}",
            outcome.complaint()
        ))));
    }

    let field = |name: &str| -> Option<String> {
        outcome
            .stdout
            .lines()
            .find_map(|line| line.trim().strip_prefix(&format!("{name} ")))
            .map(|value| value.trim().to_string())
            // An empty field is a host that answered the shape of the question
            // and not the question — `id` missing, `$HOME` unset. Not a
            // confirmation.
            .filter(|value| !value.is_empty())
    };
    // The host confirming what it did. Without all three, the command may have
    // succeeded and printed nothing afterm can read, which is not a confirmed
    // installation.
    let (Some(user), Some(path), Some(present)) =
        (field("afterm-user"), field("afterm-path"), field("afterm-present"))
    else {
        return Err(AppError::Remote(Failure::operation(
            "the host reported no account or path for the key, so afterm cannot confirm it \
             was installed",
        )));
    };
    if !path.starts_with('/') {
        return Err(AppError::Remote(Failure::operation(format!(
            "the host resolved the key file to {path}, which is not an absolute path"
        ))));
    }

    Ok(KeyInstalled { user, path, already_present: present == "yes" })
}

/// The command that installs the snippet, given it base64-encoded.
///
/// A function rather than an inline `format!` so that a test can run a shell's
/// syntax check over it. It is POSIX sh: the account's login shell is what
/// runs it, and that may be dash or busybox even where the *interactive* shell
/// being integrated is bash.
fn integration_script(encoded: &str) -> String {
    format!(
        "set -e; \
         login=$(getent passwd \"$(id -un)\" 2>/dev/null | cut -d: -f7 || true); \
         [ -n \"$login\" ] || login=\"$SHELL\"; \
         name=${{login##*/}}; \
         printf 'afterm-shell %s\\n' \"$name\"; \
         case $name in \
           bash) rc=\"$HOME/.bashrc\";; \
           zsh) rc=\"$HOME/.zshrc\";; \
           *) printf 'afterm-unsupported %s\\n' \"$name\"; exit 0;; \
         esac; \
         touch \"$rc\"; \
         if grep -q 'afterm shell integration' \"$rc\"; then present=yes; \
         else printf '%s\\n' \"$(printf '%s' '{encoded}' | base64 -d)\" >> \"$rc\"; present=no; fi; \
         grep -q 'afterm shell integration' \"$rc\"; \
         profile=no; \
         if [ \"$name\" = bash ]; then \
           profile=yes; \
           for f in \"$HOME/.bash_profile\" \"$HOME/.bash_login\" \"$HOME/.profile\"; do \
             [ -f \"$f\" ] || continue; \
             if grep -q bashrc \"$f\"; then profile=no; fi; \
             break; \
           done; \
         fi; \
         printf 'afterm-path %s\\n' \"$rc\"; \
         printf 'afterm-present %s\\n' \"$present\"; \
         printf 'afterm-profile %s\\n' \"$profile\""
    )
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct IntegrationInstalled {
    /// The absolute path of the startup file it went into.
    path: String,
    /// True when the snippet was already there and nothing was appended.
    already_present: bool,
    /// The login shell the host reported, so the interface can say which file
    /// it chose and why.
    shell: String,
    /// True when the login shell is bash and the file it reads at login does
    /// not source `~/.bashrc`.
    ///
    /// An SSH session gets a *login* shell, which reads `~/.bash_profile`,
    /// `~/.bash_login` or `~/.profile` — not `~/.bashrc`. It works on most
    /// distributions only because their profile sources the rc file. Where it
    /// does not, the snippet is installed and will never run, and saying so is
    /// the difference between a feature that failed and one that failed
    /// silently.
    needs_profile: bool,
}

/// Append the shell-integration snippet to the remote startup file, and say
/// what happened.
///
/// The snippet travels from the interface rather than being spelled out again
/// here: `integration.ts` is where it is written and where the tests check it,
/// and a second copy would be a second thing to keep in step.
///
/// This edits a file in someone's home directory on a machine that is not
/// afterm's. That is why it is a button and never automatic — the same bargain
/// as `install_public_key`, which has always written to `~/.ssh/authorized_keys`
/// on the same terms: asked for explicitly, reported back in full.
#[tauri::command]
async fn install_shell_integration(
    manager: State<'_, SessionManager>,
    prompts: State<'_, native::Prompts>,
    id: String,
    snippet: String,
) -> Result<IntegrationInstalled, AppError> {
    // Enough to know this is the snippet and not something that arrived by
    // another route. Control characters are the part that matters: the text is
    // decoded into a file the shell will run at every login.
    if !snippet.contains("afterm shell integration")
        || snippet.chars().any(|c| c.is_control() && c != '\n' && c != '\t')
    {
        return Err(AppError::Remote(Failure::operation(
            "that does not look like the afterm shell-integration snippet, so afterm did not \
             write it to the host",
        )));
    }

    let remote = manager.remote(&id)?;
    let transport = remote.obtain(&prompts, true).await?;

    // Base64 for the same reason the key is: the snippet is full of quotes and
    // backslashes, and nothing in it should ever be read by the shell that
    // installs it.
    let script = integration_script(&BASE64.encode(snippet.as_bytes()));

    let outcome = transport.run(&script).await?;
    if outcome.truncated {
        return Err(AppError::Remote(Failure::operation(
            "the host wrote more than afterm will read in answer to a short command, so \
             nothing about the result can be trusted",
        )));
    }
    if !outcome.succeeded() {
        return Err(AppError::Remote(Failure::operation(format!(
            "the host did not install the snippet: {}",
            outcome.complaint()
        ))));
    }

    let field = |name: &str| -> Option<String> {
        outcome
            .stdout
            .lines()
            .find_map(|line| line.trim().strip_prefix(&format!("{name} ")))
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
    };

    let shell = field("afterm-shell").unwrap_or_else(|| "unknown".to_string());
    if let Some(name) = field("afterm-unsupported") {
        return Err(AppError::Remote(Failure::operation(format!(
            "this account's login shell is {name}, and afterm only knows where bash and zsh \
             read their startup files. Copy the snippet and place it yourself."
        ))));
    }
    let (Some(path), Some(present)) = (field("afterm-path"), field("afterm-present")) else {
        return Err(AppError::Remote(Failure::operation(
            "the host reported no path for the snippet, so afterm cannot confirm it was \
             installed",
        )));
    };
    if !path.starts_with('/') {
        return Err(AppError::Remote(Failure::operation(format!(
            "the host resolved the startup file to {path}, which is not an absolute path"
        ))));
    }

    Ok(IntegrationInstalled {
        path,
        already_present: present == "yes",
        shell,
        needs_profile: field("afterm-profile").as_deref() == Some("yes"),
    })
}

// --------------------------------------------------------------- window

/// Paint the native title bar in the active colour scheme's colours.
///
/// Called by the interface whenever the scheme changes, with the same two
/// values the stylesheet is using — the file pane's ground and its text — so
/// the caption is never a colour the rest of the window does not contain.
#[tauri::command]
fn set_caption_colour(window: tauri::WebviewWindow, caption: String, text: String) {
    caption::apply(&window, &caption, &text);
}

// ------------------------------------------------------------- clipboard

/// The clipboard's text, for right-click paste in the terminal.
///
/// The webview's own `navigator.clipboard.readText` is not usable here: reading
/// the clipboard is a permission-gated API, and WebView2 answers it with a
/// prompt. Nothing is written back and nothing is retained — the string is
/// handed to the caller and the clipboard handle is dropped.
#[tauri::command]
fn clipboard_text() -> Result<String, AppError> {
    // An empty clipboard is reported as an error by every platform backend, and
    // it is not one: there is simply nothing to paste.
    match arboard::Clipboard::new().and_then(|mut clipboard| clipboard.get_text()) {
        Ok(text) => Ok(text),
        Err(arboard::Error::ContentNotAvailable) => Ok(String::new()),
        Err(error) => Err(AppError::Remote(Failure::operation(error.to_string()))),
    }
}

/// Send the clipboard's image to session `id`'s host, and return the path it
/// landed at — or `None` when the clipboard holds no image.
///
/// For the paste key when there is no text to paste: a coding agent on the host
/// cannot see this machine's clipboard, but takes an image's path in its place.
/// Interactive, because it is a key the user pressed: it may open the
/// connection, and ask for a password to do it.
///
/// Images older than `clipimage::KEEP_DAYS` are cleared out afterwards, in the
/// background, so the path is typed without waiting for a listing.
#[tauri::command]
async fn paste_clipboard_image(
    manager: State<'_, SessionManager>,
    prompts: State<'_, native::Prompts>,
    id: String,
) -> Result<Option<String>, AppError> {
    let session = manager.remote(&id)?;
    let Some(png) = transport::blocking(clipimage::clipboard_png).await? else {
        return Ok(None);
    };
    let transport = session.obtain(&prompts, true).await?;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or(0);
    let home = transport.home().await?;
    let (landed, directory) = clipimage::send(&transport, &home, png, now).await?;
    tauri::async_runtime::spawn(async move { clipimage::tidy(&transport, &directory, now).await });
    Ok(Some(landed))
}

// ------------------------------------------------------------ credentials

/// The interface answering a credential dialog.
///
/// `answers` is `None` when the user dismissed it. Nothing is stored here: the
/// answers go straight to the handshake that asked, which drops them when it
/// finishes.
///
/// `remember` is the checkbox, and it is the only thing about a password that
/// the interface gets to decide. Where the password then goes is settled in
/// Rust: after the host has accepted it, into the credential store, and only
/// for a request that said it could be. A `remember` on anything else is
/// ignored rather than trusted.
#[tauri::command]
fn submit_auth_response(
    prompts: State<'_, native::Prompts>,
    request: String,
    answers: Option<Vec<String>>,
    remember: Option<bool>,
) {
    prompts.answer(&request, answers, remember.unwrap_or(false));
}

/// Whether this machine can remember passwords at all, and why not when it
/// cannot.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct PasswordSupport {
    available: bool,
    /// Present only when unavailable, phrased for the settings panel to show
    /// as it stands.
    reason: Option<String>,
}

#[tauri::command]
fn password_support(vault: State<'_, Arc<secrets::Vault>>) -> PasswordSupport {
    match vault.store().availability() {
        secrets::Availability::Ready => PasswordSupport { available: true, reason: None },
        secrets::Availability::Unavailable(reason) => {
            PasswordSupport { available: false, reason: Some(reason) }
        }
    }
}

/// Which of these saved hosts have a remembered password.
///
/// Asked about by id rather than enumerated: the interface already knows its
/// saved hosts, and a store that cannot be searched is one fewer capability to
/// depend on. A store that cannot be read answers "none", which is what the
/// interface would show anyway — the failure that matters is a write, and that
/// one is reported.
#[tauri::command]
fn remembered_passwords(
    vault: State<'_, Arc<secrets::Vault>>,
    profiles: Vec<String>,
) -> Vec<String> {
    profiles
        .into_iter()
        .filter(|profile| vault.store().holds(profile).unwrap_or(false))
        .collect()
}

/// Forget the password for one saved host.
///
/// Reported rather than swallowed: a password the user believes is gone and is
/// not is the one outcome this feature must never produce.
#[tauri::command]
fn forget_password(
    vault: State<'_, Arc<secrets::Vault>>,
    profile: String,
) -> Result<(), AppError> {
    vault.forget(&profile).map_err(|error| AppError::Credential(error.to_string()))
}

/// Forget every remembered password afterm knows about.
///
/// Every one it can name, which is every saved host. A host deleted while its
/// entry could not be removed is not in this list; that write failure was
/// reported when it happened.
#[tauri::command]
fn forget_all_passwords(
    vault: State<'_, Arc<secrets::Vault>>,
    profiles: Vec<String>,
) -> Result<(), AppError> {
    let mut failures = Vec::new();
    for profile in &profiles {
        if let Err(error) = vault.forget(profile) {
            failures.push(error.to_string());
        }
    }
    if failures.is_empty() {
        return Ok(());
    }
    Err(AppError::Credential(failures.join("; ")))
}

/// What is carrying this session's files and telemetry, and why.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct TransportReport {
    /// `null` until a transport has been established.
    label: Option<String>,
    connecting: bool,
    /// Why this session is on the OpenSSH client rather than afterm's own
    /// connection, when it is.
    fallback_reason: Option<String>,
}

#[tauri::command]
fn session_transport(
    manager: State<'_, SessionManager>,
    id: String,
) -> Result<TransportReport, AppError> {
    let remote = manager.remote(&id)?;
    Ok(TransportReport {
        label: remote.ready().map(|transport| transport.label()),
        connecting: remote.connecting(),
        fallback_reason: remote.fallback_reason(),
    })
}

/// Establish the transport, prompting for credentials if the host wants them.
///
/// This is the only path that may raise a dialog, and it is only ever reached
/// from something the user did.
#[tauri::command]
async fn authenticate_session(
    manager: State<'_, SessionManager>,
    prompts: State<'_, native::Prompts>,
    id: String,
) -> Result<String, AppError> {
    let remote = manager.remote(&id)?;
    let transport = remote.obtain(&prompts, true).await?;
    Ok(transport.label())
}

// ---------------------------------------------------------------- files

#[tauri::command]
fn local_downloads() -> String {
    localfs::downloads()
}

/// Progress on one transfer, for the file pane.
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct TransferProgress {
    id: String,
    name: String,
    done: u64,
    total: Option<u64>,
}

#[tauri::command]
async fn remote_home(
    manager: State<'_, SessionManager>,
    prompts: State<'_, native::Prompts>,
    id: String,
) -> Result<String, AppError> {
    let remote = manager.remote(&id)?;
    let transport = remote.obtain(&prompts, false).await?;
    Ok(transport.home().await?)
}

#[tauri::command]
async fn list_remote_directory(
    manager: State<'_, SessionManager>,
    prompts: State<'_, native::Prompts>,
    id: String,
    path: String,
) -> Result<Listing, AppError> {
    let remote = manager.remote(&id)?;
    let transport = remote.obtain(&prompts, false).await?;
    Ok(transport.list(&path).await?)
}

#[tauri::command]
async fn create_remote_directory(
    manager: State<'_, SessionManager>,
    prompts: State<'_, native::Prompts>,
    id: String,
    path: String,
) -> Result<(), AppError> {
    let remote = manager.remote(&id)?;
    let transport = remote.obtain(&prompts, false).await?;
    Ok(transport.create_directory(&path).await?)
}

#[tauri::command]
async fn rename_remote_entry(
    manager: State<'_, SessionManager>,
    prompts: State<'_, native::Prompts>,
    id: String,
    from: String,
    to: String,
) -> Result<(), AppError> {
    let remote = manager.remote(&id)?;
    let transport = remote.obtain(&prompts, false).await?;
    Ok(transport.rename(&from, &to).await?)
}

/// Open a link from the terminal in the default browser. Web links only: the
/// rule, and why, is in `links.rs`.
#[tauri::command]
fn open_link(url: String) -> Result<(), AppError> {
    links::open(&url).map_err(|reason| AppError::Remote(Failure::operation(reason)))
}

#[tauri::command]
async fn delete_remote_entry(
    manager: State<'_, SessionManager>,
    prompts: State<'_, native::Prompts>,
    id: String,
    path: String,
    directory: bool,
) -> Result<(), AppError> {
    let remote = manager.remote(&id)?;
    let transport = remote.obtain(&prompts, false).await?;
    // A directory goes with everything in it, as it does in WinSCP and
    // `rm -r`: the pane asked about the folder, not about its contents.
    if directory {
        Ok(folder::remove(&transport, &path).await?)
    } else {
        Ok(transport.delete(&path, false).await?)
    }
}

/// Copy a remote file into the Downloads folder, and say where it landed.
///
/// The interface names the remote file and nothing else. Where it goes is
/// decided here: the remote name is reduced to one plain component, the bytes
/// arrive under a `.part` name reserved before any of them move, and only a
/// finished file is moved to its real name — or the first free `name (n)`,
/// because nothing already in Downloads is ever replaced. A transfer that
/// fails leaves nothing behind. See `localfs` for the boundary itself.
#[tauri::command]
async fn download_file(
    app: AppHandle,
    manager: State<'_, SessionManager>,
    prompts: State<'_, native::Prompts>,
    id: String,
    remote: String,
) -> Result<String, AppError> {
    let session = manager.remote(&id)?;
    let transport = session.obtain(&prompts, false).await?;
    let shown = remote_name(&remote).to_string();
    let name = localfs::safe_name(&shown);
    let directory = localfs::downloads_dir();

    let part = {
        let (directory, name) = (directory.clone(), name.clone());
        transport::blocking(move || localfs::claim(&directory, &localfs::part_name(&name))).await?
    };
    let reporter = progress_reporter(&app, &id, &shown);
    let local = part.to_string_lossy().to_string();
    if let Err(failure) = transport.download(&remote, &local, reporter).await {
        localfs::discard(&part);
        return Err(failure.into());
    }
    let landed = transport::blocking(move || localfs::commit(&part, &directory, &name)).await?;
    Ok(landed.to_string_lossy().to_string())
}

/// The largest file a preview will fetch. A preview is for looking at, and an
/// image past this is one to download on purpose rather than pull through the
/// window's memory to glance at.
const PREVIEW_LIMIT: u64 = 64 * 1024 * 1024;

/// Fetch a remote file for the interface to show, without keeping it.
///
/// The same transfer a download makes, into a temporary file with a name
/// nobody can guess, read back and removed before the bytes go anywhere: the
/// point of a preview is that it leaves nothing in Downloads to find and clean
/// up afterwards. Handed over as raw bytes rather than as base64 text, which
/// would be a third larger and parsed twice. What it is and whether it can be
/// shown is the interface's call; this only carries it.
#[tauri::command]
async fn preview_remote_file(
    app: AppHandle,
    manager: State<'_, SessionManager>,
    prompts: State<'_, native::Prompts>,
    id: String,
    remote: String,
) -> Result<tauri::ipc::Response, AppError> {
    let session = manager.remote(&id)?;
    let transport = session.obtain(&prompts, false).await?;
    let shown = remote_name(&remote).to_string();

    let scratch = std::env::temp_dir();
    let name = format!("afterm-preview-{:016x}.part", rand::random::<u64>());
    let part = transport::blocking(move || localfs::claim(&scratch, &name)).await?;
    let local = part.to_string_lossy().to_string();
    let fetched = transport.download(&remote, &local, progress_reporter(&app, &id, &shown)).await;
    let read = fetched.map(|()| {
        let file = part.clone();
        move || -> Result<Vec<u8>, Failure> {
            let size = std::fs::metadata(&file).map(|meta| meta.len()).unwrap_or(0);
            if size > PREVIEW_LIMIT {
                return Err(Failure::operation(format!(
                    "{shown} is too large to preview; download it instead"
                )));
            }
            std::fs::read(&file).map_err(|error| Failure::internal(format!("could not read the preview: {error}")))
        }
    });
    let bytes = match read {
        Ok(read) => transport::blocking(read).await,
        Err(failure) => Err(failure),
    };
    localfs::discard(&part);
    Ok(tauri::ipc::Response::new(bytes?))
}

/// What a folder transfer carried, and where it went.
///
/// The same shape both ways, because the same three things are worth saying
/// about either: a folder can arrive partly, and a line that named only the
/// destination would be claiming a whole copy of something that may be missing
/// half of it.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Transferred {
    /// The folder the transfer made, at the end it was going to. Never one
    /// that was already there, unless the user asked to merge into it.
    path: String,
    /// Regular files that arrived.
    files: u32,
    /// Entries passed over: every link, and anything that is not a file or a
    /// directory.
    skipped: u32,
    /// Entries that could not be copied: one end or the other refused them.
    failed: u32,
}

/// Copy a remote folder into the Downloads folder, whole.
///
/// The destination is claimed before anything is fetched, with the same
/// exclusive create a file download uses: a folder already in Downloads is
/// never written into and never replaced, so a second download of `reports`
/// makes `reports (1)`. What the walk will and will not do is in `folder`.
///
/// Counts come back rather than a path alone because a folder can succeed
/// partly — a link passed over, a file the user cannot read — and a status
/// line that said only where it went would be claiming more than happened.
#[tauri::command]
async fn download_folder(
    app: AppHandle,
    manager: State<'_, SessionManager>,
    prompts: State<'_, native::Prompts>,
    id: String,
    remote: String,
) -> Result<Transferred, AppError> {
    let session = manager.remote(&id)?;
    let transport = session.obtain(&prompts, false).await?;
    let name = localfs::safe_name(remote_name(&remote));
    let downloads = localfs::downloads_dir();
    let into = transport::blocking(move || localfs::claim_dir(&downloads, &name)).await?;

    match folder::download(&transport, &remote, &into, |label| progress_reporter(&app, &id, label))
        .await
    {
        Ok(tally) => Ok(Transferred {
            path: into.to_string_lossy().to_string(),
            files: tally.files,
            skipped: tally.skipped,
            failed: tally.failed,
        }),
        Err(failure) => {
            // The folder this attempt claimed, with nothing in it, is litter
            // in the user's Downloads. `remove_dir` refuses a directory with
            // anything in it, which is exactly where the line belongs:
            // whatever did arrive before the connection went is kept, and the
            // failure says why there is no more of it.
            let empty = into.clone();
            let _ = transport::blocking(move || {
                std::fs::remove_dir(&empty).map_err(|error| Failure::internal(error.to_string()))
            })
            .await;
            Err(failure.into())
        }
    }
}

/// Send a local file into a remote directory under its own name, and say
/// where it landed. `on_exists` is the interface's answer to a name already
/// taken; the default it sends is `fail`, and it asks the user before sending
/// anything else.
#[tauri::command]
async fn upload_file(
    app: AppHandle,
    manager: State<'_, SessionManager>,
    prompts: State<'_, native::Prompts>,
    id: String,
    local: String,
    directory: String,
    on_exists: Collision,
) -> Result<String, AppError> {
    let name = std::path::Path::new(&local)
        .file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .ok_or_else(|| AppError::Remote(Failure::internal(format!("{local} has no file name"))))?;
    let session = manager.remote(&id)?;
    let transport = session.obtain(&prompts, false).await?;
    let reporter = progress_reporter(&app, &id, name);
    Ok(transport.upload(&local, &directory, name, on_exists, reporter).await?)
}

/// Send a local folder into a remote directory, whole.
///
/// `on_exists` is the interface's answer to a folder already at that name on
/// the host. The default it sends is `fail`, which makes the host's refusal
/// into the pane's question rather than a decision taken for the user; `merge`
/// arrives here as `replace`, because what it replaces is the files inside
/// that collide, one at a time and each with the single-file care.
#[tauri::command]
async fn upload_folder(
    app: AppHandle,
    manager: State<'_, SessionManager>,
    prompts: State<'_, native::Prompts>,
    id: String,
    local: String,
    directory: String,
    on_exists: Collision,
) -> Result<Transferred, AppError> {
    let session = manager.remote(&id)?;
    let transport = session.obtain(&prompts, false).await?;
    let from = std::path::PathBuf::from(&local);
    let (path, tally) = folder::upload(&transport, &from, &directory, on_exists, |label| {
        progress_reporter(&app, &id, label)
    })
    .await?;
    Ok(Transferred { path, files: tally.files, skipped: tally.skipped, failed: tally.failed })
}

/// Whether a local path is a directory, so the pane knows which upload a
/// dropped path wants.
///
/// The drop event carries paths and nothing else — no kind, no size — and the
/// page has no filesystem to ask. Links are followed here, because a link to a
/// folder is a folder to everything else on this machine and the user dropped
/// what they were looking at; the walk that follows refuses to follow any link
/// *inside* it, which is where following one would be a way out of the folder.
#[tauri::command]
fn local_is_directory(path: String) -> bool {
    std::path::Path::new(&path).is_dir()
}

/// The last segment of a remote path, which is what the pane shows.
fn remote_name(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

/// How often a transfer may report progress. A 64 KiB chunk on a fast link is
/// thousands of events a second, and a progress bar that updates ten times a
/// second looks exactly the same as one that updates ten thousand.
const PROGRESS_INTERVAL: Duration = Duration::from_millis(100);

fn progress_reporter(
    app: &AppHandle,
    id: &str,
    name: &str,
) -> impl Fn(u64, Option<u64>) + Send + 'static {
    let app = app.clone();
    let id = id.to_string();
    let name = name.to_string();
    let last = Mutex::new(Instant::now() - PROGRESS_INTERVAL);
    move |done, total| {
        // Always report the final chunk, so the pane never stops just short.
        let finished = total.is_some_and(|total| done >= total);
        {
            let mut last = last.lock().unwrap();
            if !finished && last.elapsed() < PROGRESS_INTERVAL {
                return;
            }
            *last = Instant::now();
        }
        let _ = app.emit(
            "transfer-progress",
            TransferProgress { id: id.clone(), name: name.clone(), done, total },
        );
    }
}

// -------------------------------------------------------------- telemetry

/// What the status bar gets back. `note` carries the reason there are no
/// numbers, and `cause` says what kind of reason it is, so the bar can offer
/// the right thing rather than showing a row of dashes with no explanation.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct TelemetryReport {
    metrics: Option<Metrics>,
    note: Option<String>,
    cause: Option<Cause>,
    methods: Vec<String>,
    /// Whether offering a credential dialog would achieve anything. Decided
    /// here, by the same rule the file pane's errors use.
    can_prompt: bool,
    transport: Option<String>,
}

impl TelemetryReport {
    fn note(note: impl Into<String>) -> Self {
        Self {
            metrics: None,
            note: Some(note.into()),
            cause: None,
            methods: Vec::new(),
            can_prompt: false,
            transport: None,
        }
    }

    fn failed(failure: Failure) -> Self {
        Self {
            metrics: None,
            note: Some(failure.message.clone()),
            cause: Some(failure.cause),
            can_prompt: failure.is_interactive_auth(),
            methods: failure.methods,
            transport: None,
        }
    }
}

#[tauri::command]
async fn session_metrics(
    manager: State<'_, SessionManager>,
    prompts: State<'_, native::Prompts>,
    id: String,
) -> Result<TelemetryReport, AppError> {
    let remote = manager.remote(&id)?;

    // The status bar polls every two seconds. It may open a connection — that
    // is how telemetry starts on a host that needs no credential — but it must
    // never raise a dialog, and it must never queue behind one.
    let transport = match remote.ready() {
        Some(transport) => transport,
        None if remote.connecting() => return Ok(TelemetryReport::note("connecting…")),
        None => match remote.obtain(&prompts, false).await {
            Ok(transport) => transport,
            Err(failure) => return Ok(TelemetryReport::failed(failure)),
        },
    };

    let (mut metrics, note) = transport.telemetry();
    // Measured per poll rather than carried in the telemetry stream: the
    // stream reports the host, and this reports the link to it.
    if let Some(metrics) = metrics.as_mut() {
        metrics.latency = transport.latency().await;
    }
    Ok(TelemetryReport {
        metrics,
        note,
        cause: None,
        methods: Vec::new(),
        can_prompt: false,
        transport: Some(transport.label()),
    })
}

/// Public keys found on this machine, offered as one-click installs.
#[tauri::command]
fn discover_public_keys() -> Vec<DiscoveredKey> {
    sshkeys::discover()
}

/// Whether the OpenSSH tools afterm shells out to can be run at all.
///
/// Asked once at startup rather than discovered on the first failed
/// connection, because the answer is about this machine and does not change
/// while afterm is open — and because the failure it prevents looks like a
/// broken host rather than a missing install.
#[tauri::command]
fn check_openssh() -> openssh::Presence {
    openssh::check()
}

// --------------------------------------------------------------- profiles

fn profiles_path(app: &AppHandle) -> Result<std::path::PathBuf, AppError> {
    let directory = app
        .path()
        .app_config_dir()
        .map_err(|error| AppError::Remote(Failure::internal(error.to_string())))?;
    std::fs::create_dir_all(&directory)
        .map_err(|error| AppError::Remote(Failure::internal(error.to_string())))?;
    Ok(directory.join("profiles.json"))
}

/// The entries in the store, still as JSON.
///
/// Read as values rather than straight into `Vec<SessionProfile>` on purpose.
/// The whole-array deserialize is what the September 2026 review found: one
/// entry an older or newer afterm wrote — a `powershell` profile from before
/// the local shells were dropped was the reproduction — failed the array, and
/// `unwrap_or_default` turned every saved host into none. Every entry is now
/// decoded on its own, and the ones that cannot be are still returned here so
/// a save can put them back.
///
/// A file that is not there is an empty store, which is a new install. A file
/// that is there and cannot be read or is not a JSON array is reported: it
/// holds the user's saved hosts, and answering "you have none" invites the
/// next save to make that true.
fn stored_entries(path: &std::path::Path) -> Result<Vec<serde_json::Value>, AppError> {
    let contents = match std::fs::read_to_string(path) {
        Ok(contents) => contents,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => {
            return Err(AppError::Remote(Failure::operation(format!(
                "your saved hosts are at {} and could not be read: {error}",
                path.display()
            ))))
        }
    };
    // An empty file is a store that was created and never written to.
    if contents.trim().is_empty() {
        return Ok(Vec::new());
    }
    serde_json::from_str(&contents).map_err(|error| {
        AppError::Remote(Failure::operation(format!(
            "your saved hosts are at {} and could not be understood: {error}. Nothing has been \
             changed there.",
            path.display()
        )))
    })
}

/// Split the store into the hosts the interface can show and everything else.
///
/// "Everything else" is an entry this version has no row for: a local shell
/// left behind by an older afterm, an entry with no address, something that
/// does not decode at all. They are carried through a save verbatim rather
/// than dropped, because an entry afterm cannot read is not an entry afterm
/// gets to delete — a newer version, or a fixed one, may know exactly what it
/// is.
fn partition_entries(
    entries: Vec<serde_json::Value>,
) -> (Vec<SessionProfile>, Vec<serde_json::Value>) {
    let mut hosts = Vec::new();
    let mut kept = Vec::new();
    for entry in entries {
        match serde_json::from_value::<SessionProfile>(entry.clone()) {
            Ok(profile) if is_shown_host(&profile) => hosts.push(profile),
            _ => kept.push(entry),
        }
    }
    (hosts, kept)
}

/// Whether a decoded entry is a saved host the list can show: an SSH session
/// with somewhere to go. A local shell is opened from a button and has nothing
/// to save, so one in the store is a leftover rather than a row.
fn is_shown_host(profile: &SessionProfile) -> bool {
    !profile.kind.is_local()
        && profile.host.as_deref().is_some_and(|host| !host.is_empty())
        && profile.user.as_deref().is_some_and(|user| !user.is_empty())
}

#[tauri::command]
fn load_profiles(app: AppHandle) -> Result<Vec<SessionProfile>, AppError> {
    read_hosts(&profiles_path(&app)?)
}

fn read_hosts(path: &std::path::Path) -> Result<Vec<SessionProfile>, AppError> {
    Ok(partition_entries(stored_entries(path)?).0)
}

/// Write the saved hosts, keeping whatever else the store held.
///
/// The interface sends the list it is showing, which is the list `load` gave
/// it. Anything `load` could not show is read back off disk and appended, so a
/// save is not a way to lose an entry nobody could see. An id the interface is
/// now using wins over a kept entry with the same one, which is what makes
/// re-saving a repaired host repair it.
///
/// A store that cannot be read at all is moved aside rather than written over:
/// the file is the only copy of it, and afterm has to stay usable.
#[tauri::command]
fn save_profiles(app: AppHandle, profiles: Vec<SessionProfile>) -> Result<(), AppError> {
    write_hosts(&profiles_path(&app)?, profiles)
}

fn write_hosts(path: &std::path::Path, profiles: Vec<SessionProfile>) -> Result<(), AppError> {
    let kept = match stored_entries(path) {
        Ok(entries) => partition_entries(entries).1,
        Err(_) => {
            let aside = path.with_extension("json.unreadable");
            let _ = std::fs::rename(path, &aside);
            Vec::new()
        }
    };
    let taken: Vec<&str> = profiles.iter().map(|profile| profile.id.as_str()).collect();
    let mut entries: Vec<serde_json::Value> = profiles
        .iter()
        .map(serde_json::to_value)
        .collect::<Result<_, _>>()
        .map_err(|error| AppError::Remote(Failure::internal(error.to_string())))?;
    entries.extend(kept.into_iter().filter(|entry| {
        let id = entry.get("id").and_then(serde_json::Value::as_str);
        !id.is_some_and(|id| taken.contains(&id))
    }));

    let contents = serde_json::to_string_pretty(&entries)
        .map_err(|error| AppError::Remote(Failure::internal(error.to_string())))?;
    std::fs::write(path, contents)
        .map_err(|error| AppError::Remote(Failure::internal(error.to_string())))
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .manage(SessionManager::default())
        // Opened once. A store that cannot be opened is not a startup failure:
        // it is a machine that does not save passwords, which the settings
        // panel says and nothing else has to handle.
        .manage(Arc::new(secrets::open()))
        .setup(|app| {
            let handle = app.handle().clone();
            app.manage(native::Prompts::new(Box::new(DialogPrompts(handle))));
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            spawn_session,
            set_terminal_flow,
            terminal_write,
            resize_session,
            close_session,
            install_public_key,
            install_shell_integration,
            clipboard_text,
            set_caption_colour,
            submit_auth_response,
            password_support,
            remembered_passwords,
            forget_password,
            forget_all_passwords,
            session_transport,
            authenticate_session,
            local_downloads,
            create_remote_directory,
            rename_remote_entry,
            delete_remote_entry,
            open_link,
            paste_clipboard_image,
            preview_remote_file,
            remote_home,
            list_remote_directory,
            download_file,
            download_folder,
            upload_file,
            upload_folder,
            local_is_directory,
            session_metrics,
            discover_public_keys,
            check_openssh,
            load_profiles,
            save_profiles
        ])
        .run(tauri::generate_context!())
        .expect("error while running afterm");
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ssh_profile(host: &str, user: &str) -> SessionProfile {
        SessionProfile {
            id: "1".into(),
            name: "test".into(),
            kind: SessionKind::Ssh,
            host: Some(host.into()),
            user: Some(user.into()),
            port: Some(22),
            persistent_shell: false,
        }
    }

    #[test]
    fn rejects_option_injection_in_ssh_host() {
        assert!(command_for(&ssh_profile("-oProxyCommand=evil", "me")).is_err());
    }

    #[test]
    fn rejects_option_injection_in_ssh_user() {
        assert!(command_for(&ssh_profile("example.com", "-oProxyCommand=evil")).is_err());
    }

    #[test]
    fn rejects_argument_splitting_via_whitespace() {
        assert!(command_for(&ssh_profile("example.com -D 1080", "me")).is_err());
        assert!(command_for(&ssh_profile("example.com", "me extra")).is_err());
    }

    #[test]
    fn accepts_an_ordinary_target() {
        let profile = ssh_profile("prod-01.internal", "deploy");
        assert!(command_for(&profile).is_ok());
        let (host, user, port) = ssh_target(&profile).unwrap();
        assert_eq!((host.as_str(), user.as_str(), port), ("prod-01.internal", "deploy", 22));
    }

    #[test]
    fn requires_both_host_and_user_for_ssh() {
        let mut profile = ssh_profile("example.com", "me");
        profile.host = None;
        assert!(command_for(&profile).is_err());
        let mut profile = ssh_profile("example.com", "me");
        profile.user = None;
        assert!(command_for(&profile).is_err());
    }

    /// The same validation guards the socket as guards the command line, and
    /// the default kind now reaches both.
    #[test]
    fn a_native_session_validates_its_target_too() {
        let profile = ssh_profile("-oProxyCommand=evil", "me");
        assert!(remote_for(&profile, None, Arc::new(secrets::open())).is_err());
        let mut forced = ssh_profile("-oProxyCommand=evil", "me");
        forced.kind = SessionKind::SshCli;
        assert!(remote_for(&forced, None, Arc::new(secrets::open())).is_err());
        assert!(command_for(&forced).is_err());
    }

    /// A local shell has nothing to validate and no far end to reach, which is
    /// the whole of what makes it cheap to carry.
    #[test]
    fn a_local_shell_needs_no_address_and_has_no_remote_half() {
        for kind in [SessionKind::Powershell, SessionKind::Cmd] {
            let mut profile = ssh_profile("", "");
            profile.kind = kind;
            profile.host = None;
            profile.user = None;
            assert!(kind.is_local());
            assert!(command_for(&profile).is_ok(), "{kind:?} should not need a host");
        }
        // And the SSH kinds still do.
        assert!(!SessionKind::Ssh.is_local());
        assert!(!SessionKind::SshCli.is_local());
    }

    /// Asked of a local shell for something only a host can answer, afterm
    /// says so rather than reaching for a connection that does not exist.
    #[test]
    fn a_session_with_no_far_end_refuses_what_only_a_host_can_answer() {
        let manager = SessionManager::default();
        manager.begin("1").unwrap();
        let mut session = managed("1");
        session.remote = None;
        assert!(manager.finish("1", session).is_ok());

        assert!(manager.get("1").is_ok(), "the session itself is perfectly real");
        let refused = manager.remote("1").err().expect("but it has no host");
        assert_eq!(refused.failure().cause, Cause::Unsupported);
        assert!(refused.failure().message.contains("shell on this machine"));
    }

    /// Hosts saved while the native terminal was opt-in still open, under the
    /// name it had then.
    #[test]
    fn a_profile_saved_as_ssh_native_still_loads() {
        let json = r#"{"id":"1","name":"old","kind":"ssh-native","host":"h","user":"u","port":22}"#;
        let profile: SessionProfile = serde_json::from_str(json).unwrap();
        assert_eq!(profile.kind, SessionKind::Ssh);
    }

    fn managed(id: &str) -> ManagedSession {
        ManagedSession {
            persistent: None,
            terminal: Terminal::Channel(native::ShellChannel::detached()),
            remote: Some(Arc::new(RemoteSession::new(
                id,
                None,
                "example.com",
                "me",
                22,
                Arc::new(secrets::open()),
            ))),
            flow: Arc::new(FlowGate::new()),
        }
    }

    /// A session is known from the moment it is asked for, so there is never a
    /// window in which closing its tab has nothing to act on.
    #[test]
    fn a_session_is_pending_from_the_moment_it_is_asked_for() {
        let manager = SessionManager::default();
        assert!(manager.begin("1").is_some());
        // Asked for twice, opened once.
        assert!(manager.begin("1").is_none());
        assert!(manager.finish("1", managed("1")).is_ok());
        assert!(manager.get("1").is_ok());
        // And once open, it is not pending again either.
        assert!(manager.begin("1").is_none());
    }

    /// Closing during the handshake or the password prompt: the attempt is
    /// told, and a session it produces anyway is handed back to be taken down
    /// rather than kept without a tab.
    #[test]
    fn closing_a_pending_session_cancels_the_attempt_and_refuses_its_result() {
        let manager = SessionManager::default();
        let mut cancelled = manager.begin("1").unwrap();
        assert!(manager.cancel("1"));
        assert!(cancelled.try_recv().is_ok(), "the attempt hears about the close");
        let refused = manager.finish("1", managed("1"));
        assert!(refused.is_err(), "a session whose tab has gone is not kept");
        assert!(manager.get("1").is_err());
        // Nothing left to cancel, and nothing stops the id being used again.
        assert!(!manager.cancel("1"));
        assert!(manager.begin("1").is_some());
    }

    #[test]
    fn a_failed_attempt_leaves_no_pending_record_behind() {
        let manager = SessionManager::default();
        manager.begin("1").unwrap();
        // What `spawn_session` does when the attempt itself fails.
        manager.cancel("1");
        assert!(manager.begin("1").is_some(), "the id is free to be tried again");
    }

    #[test]
    fn a_remote_name_is_its_last_segment() {
        assert_eq!(remote_name("/srv/logs/app.log"), "app.log");
        assert_eq!(remote_name("app.log"), "app.log");
        // A trailing slash names nothing; `safe_name` turns that into a name.
        assert_eq!(localfs::safe_name(remote_name("/srv/logs/")), "download");
    }

    /// Every error reaches the interface as a cause plus a message, so the file
    /// pane can offer a password prompt for one and a retry for another without
    /// matching on strings.
    #[test]
    fn errors_carry_a_cause_the_interface_can_act_on() {
        let refused = AppError::Remote(Failure::auth("nope", vec!["password".into()]));
        assert_eq!(refused.failure().cause, Cause::Auth);
        assert!(refused.failure().is_interactive_auth());

        let missing = AppError::MissingSession("7".into());
        assert_eq!(missing.failure().cause, Cause::Internal);
    }

    // ------------------------------------------------------------ profiles

    /// A store holding one entry this version cannot show must not cost the
    /// user the ones it can.
    ///
    /// The reproduction of the September 2026 finding: a valid SSH host beside
    /// a `powershell` profile left over from before the local shells were
    /// dropped. The whole array was deserialized into `Vec<SessionProfile>`
    /// and `unwrap_or_default` turned the one failure into an empty list, so
    /// every saved host disappeared from the dialog — and the next save wrote
    /// that emptiness back.
    #[test]
    fn one_entry_this_version_cannot_read_does_not_cost_the_others() {
        let dir = tempdir::Dir::new("profiles-mixed");
        let path = dir.path().join("profiles.json");
        std::fs::write(&path, r#"[
            {"id":"1","name":"prod","kind":"ssh","host":"prod.example.com","user":"deploy","port":22},
            {"id":"2","name":"Shell","kind":"powershell"},
            {"id":"3","name":"broken","kind":"telepathy","host":"h","user":"u","port":22},
            {"id":"4","name":"nowhere","kind":"ssh","host":"","user":"","port":22},
            {"id":"5","name":"staging","kind":"ssh-native","host":"staging.example.com","user":"deploy","port":2222}
        ]"#).unwrap();

        let hosts = read_hosts(&path).expect("a store with a bad entry still loads");
        let names: Vec<&str> = hosts.iter().map(|host| host.name.as_str()).collect();
        assert_eq!(names, ["prod", "staging"], "every valid host survives");
        // And the legacy spelling is migrated rather than merely tolerated.
        assert_eq!(hosts[1].kind, SessionKind::Ssh);
    }

    /// The other half of the same finding: saving after a partial load must
    /// not quietly finish what the load started.
    #[test]
    fn a_save_keeps_the_entries_it_could_not_show() {
        let dir = tempdir::Dir::new("profiles-keep");
        let path = dir.path().join("profiles.json");
        std::fs::write(&path, r#"[
            {"id":"1","name":"prod","kind":"ssh","host":"prod.example.com","user":"deploy","port":22},
            {"id":"2","name":"Shell","kind":"powershell"},
            {"id":"3","name":"broken","kind":"telepathy"}
        ]"#).unwrap();

        // What the interface does when a host is deleted: it sends back the
        // list it was given, minus one.
        let mut hosts = read_hosts(&path).unwrap();
        hosts.clear();
        write_hosts(&path, hosts).expect("save");

        let after: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        let ids: Vec<&str> = after
            .as_array()
            .unwrap()
            .iter()
            .map(|entry| entry["id"].as_str().unwrap())
            .collect();
        assert_eq!(ids, ["2", "3"], "the host went; what afterm cannot read stayed");
    }

    /// A store that cannot be understood is reported rather than answered
    /// with "you have none", which is the answer that invites the next save to
    /// make it true.
    #[test]
    fn an_unreadable_store_is_an_error_and_not_an_empty_list() {
        let dir = tempdir::Dir::new("profiles-broken");
        let path = dir.path().join("profiles.json");

        // Nothing there at all is a new install, and that one is empty.
        assert!(read_hosts(&path).unwrap().is_empty());

        std::fs::write(&path, "{ not json at all").unwrap();
        let failure = read_hosts(&path).expect_err("invalid JSON must be reported");
        assert!(failure.failure().message.contains("could not be understood"), "{failure}");

        // A path that cannot be read at all — here a directory standing where
        // the file should be — is reported too, and separately.
        let blocked = dir.path().join("blocked.json");
        std::fs::create_dir(&blocked).unwrap();
        let failure = read_hosts(&blocked).expect_err("an unreadable store must be reported");
        assert!(failure.failure().message.contains("could not be read"), "{failure}");
    }

    /// And afterm stays usable afterwards: the store that could not be read is
    /// moved aside rather than written over, so the next save works and the
    /// original is still on disk to be looked at.
    #[test]
    fn a_save_over_an_unreadable_store_moves_it_aside_rather_than_losing_it() {
        let dir = tempdir::Dir::new("profiles-aside");
        let path = dir.path().join("profiles.json");
        std::fs::write(&path, "{ not json at all").unwrap();

        write_hosts(&path, vec![ssh_profile("prod.example.com", "deploy")]).expect("save");

        let hosts = read_hosts(&path).expect("the store reads again");
        assert_eq!(hosts.len(), 1);
        let aside = std::fs::read_to_string(dir.path().join("profiles.json.unreadable")).unwrap();
        assert_eq!(aside, "{ not json at all", "the original is kept verbatim");
    }

    /// A saved profile has nowhere to put a secret. Not a convention — there is
    /// no field, so a password cannot be persisted by accident.
    #[test]
    fn a_saved_profile_structurally_cannot_hold_a_credential() {
        let profile = ssh_profile("example.com", "me");
        let json = serde_json::to_string(&profile).unwrap();
        let fields: serde_json::Value = serde_json::from_str(&json).unwrap();
        let mut keys: Vec<&str> = fields.as_object().unwrap().keys().map(String::as_str).collect();
        keys.sort_unstable();
        assert_eq!(keys, ["host", "id", "kind", "name", "port", "user"]);
    }

    /// The silent fallback is what made a new password-only host look like a
    /// tab that simply never asked for anything: the host key is not recorded,
    /// afterm's own connection refuses it before offering a credential, and
    /// the tab lands on the OpenSSH client with nothing said. The notice has
    /// to name the key *and* point at the terminal, because that is where the
    /// question the user has to answer actually appears.
    #[test]
    fn a_host_key_fallback_says_the_terminal_is_where_the_question_is() {
        let said = fallback_notice(&Failure::new(
            Cause::HostKey,
            "the host offered a key that is not in known_hosts",
        ));
        assert!(said.contains("known_hosts"), "{said}");
        assert!(said.contains("terminal"), "{said}");
        // Two sentences, so the reason does not run into what to do about it.
        assert!(said.contains(". "), "{said}");
    }

    /// Everything else still says why, rather than falling back in silence.
    #[test]
    fn any_other_fallback_still_carries_its_reason() {
        let said = fallback_notice(&Failure::new(Cause::Unreachable, "the host stopped answering"));
        assert!(said.contains("the host stopped answering"), "{said}");
        assert!(said.contains("OpenSSH client"), "{said}");
    }

    /// A message that already ends in a stop does not get a second one.
    #[test]
    fn a_reason_is_punctuated_once() {
        assert_eq!(ended("no route to host."), "no route to host.");
        assert_eq!(ended("no route to host"), "no route to host.");
        assert_eq!(ended("is this a host?"), "is this a host?");
    }

    /// The installer runs on the account's login shell, which may be dash or
    /// busybox. A syntax error in it would otherwise only ever be found on
    /// someone's host, by failing to install anything.
    ///
    /// Unix only, because the script never runs on the machine afterm runs on
    /// — it is sent to the host — so there is nothing to learn from checking
    /// it on Windows and something to lose. `bash` there is the WSL launcher,
    /// which spawns happily and then exits non-zero to say no distribution is
    /// installed, which is indistinguishable from a shell rejecting the script.
    #[cfg(unix)]
    #[test]
    fn the_integration_installer_is_posix_shell() {
        use std::io::Write;
        use std::process::{Command, Stdio};
        let script = integration_script("QUZURVJN");
        for shell in ["sh", "dash", "bash"] {
            let Ok(mut child) = Command::new(shell)
                .arg("-n")
                .stdin(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
            else {
                continue; // Not every machine running the tests has every shell.
            };
            child.stdin.take().unwrap().write_all(script.as_bytes()).unwrap();
            let done = child.wait_with_output().unwrap();
            assert!(
                done.status.success(),
                "{shell} rejected the installer: {}",
                String::from_utf8_lossy(&done.stderr)
            );
        }
    }

    /// The `case` arm that gives up is reachable: an account whose login shell
    /// is neither bash nor zsh must be told, not quietly given a bashrc.
    ///
    /// Unix only, for the reason above: on Windows this passes or fails on
    /// whichever `sh.exe` happens to be on PATH, which is not what it is
    /// asking about.
    #[cfg(unix)]
    #[test]
    fn the_integration_installer_reports_an_unknown_login_shell() {
        use std::process::Command;
        let script = integration_script("QUZURVJN");
        // An empty PATH is how the test reaches the `$SHELL` fallback: where
        // `getent` exists it is authoritative and answers with the account's
        // real shell. It doubles as the busybox case — no `getent`, no `cut` —
        // which `|| true` is in the script to survive.
        let done = Command::new("sh")
            .arg("-c")
            .arg(format!("SHELL=/usr/bin/fish; export SHELL; PATH=; {script}"))
            .env("HOME", std::env::temp_dir())
            .output()
            .unwrap();
        let out = String::from_utf8_lossy(&done.stdout);
        assert!(out.contains("afterm-unsupported"), "said: {out}");
    }
}
