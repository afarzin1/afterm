//! afterm speaking SSH itself: one authenticated connection per session, with
//! files, telemetry and (optionally) the terminal as channels on it.
//!
//! This is what MobaXterm, WinSCP and Termius do, and it is why they work on
//! hosts that only accept an interactive password. The OpenSSH client cannot be
//! made to share its connection on Windows — `ControlMaster` is not implemented
//! and `SSH_ASKPASS` is ignored — so the only way to authenticate once and use
//! the result for more than the terminal is to speak the protocol.
//!
//! Three rules shape everything here:
//!
//! 1. **Nothing is trusted that the user's own client would not trust.**
//!    `ssh -G` resolves the configuration and `ssh-keygen -F` resolves
//!    `known_hosts`; see `config` and `trust`. A host afterm cannot honour
//!    faithfully is refused by name and stays on the OpenSSH client.
//! 2. **No credential is offered before the host key is verified.** russh
//!    aborts the connection when `check_server_key` returns false, before
//!    authentication begins, and there is a test that counts credential checks
//!    on a refused connection and requires zero.
//! 3. **A secret is used once and dropped.** One connection means one
//!    handshake, so nothing needs replaying. Passwords are never written to a
//!    profile, never logged, and never held for a later operation.

#[cfg(test)]
mod battery;
#[cfg(test)]
mod live;
pub mod config;
pub mod crypto;
pub mod trust;

use crate::flow::FlowGate;
use crate::fsmodel::{
    carried, join, local_mode, numbered, sort_entries, EntryKind, FileEntry, Listing,
    STAGING_MODE,
};
use crate::metrics::Metrics;
use crate::remote::{parse_block, Blocks, CpuSample, NetSample, PROBE};
use crate::secrets::{self, Vault};
use crate::transport::{Cause, Collision, Failure};
use russh::client::{self, AuthResult, KeyboardInteractiveAuthResponse};
use russh::keys::agent::AgentIdentity;
use russh::keys::{load_public_key, load_secret_key, Algorithm, PrivateKeyWithHashAlg, PublicKey};
use russh::{ChannelMsg, Disconnect, MethodKind};
use russh_sftp::client::fs::{File as RemoteFile, Metadata};
use russh_sftp::client::SftpSession;
use russh_sftp::protocol::{OpenFlags, StatusCode};
use serde::Serialize;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex as StdMutex};
use std::time::{Duration, Instant};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::{mpsc, oneshot, Mutex, Notify};
use zeroize::Zeroizing;

/// How long a credential dialog may stay unanswered before the attempt is
/// abandoned. Long enough to find a phone for a one-time code, short enough
/// that a forgotten dialog does not hold the session's connection gate.
const PROMPT_TIMEOUT: Duration = Duration::from_secs(180);

/// How long to wait for the whole handshake.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(20);

/// Bytes per chunk when transferring a file. Large enough to keep the pipe
/// full, small enough that progress moves visibly on a slow link.
const TRANSFER_CHUNK: usize = 64 * 1024;

/// The most of one command's output afterm will hold. Everything it runs
/// itself prints a few lines; a host that prints more than this is not
/// answering the question that was asked.
const EXEC_CAPTURE: usize = 64 * 1024;

/// How long a host has to acknowledge a channel request before afterm gives up
/// on it. The answer is one packet on a channel that is already open, so the
/// only thing this waits out is a host that is never going to send one.
const SETUP_TIMEOUT: Duration = Duration::from_secs(30);

/// How long the courtesy close to the far end is worth waiting on.
///
/// A host that has stopped reading its socket can leave it waiting for as long
/// as it likes — the connection's own event loop is blocked writing, so even
/// handing it the message blocks — and by this point the session is over
/// either way.
const CLOSE_TIMEOUT: Duration = Duration::from_secs(5);

/// The most a host may say while it is still answering a channel request.
/// A shell greeting arrives in the same breath as the acknowledgment and is
/// worth keeping; a host that streams instead of answering is not.
const SETUP_CAPTURE: usize = 64 * 1024;

/// How much input may be waiting to go out before afterm stops taking more.
///
/// The writer sends one message at a time and the SSH window decides how fast,
/// so a host that has stopped reading is a host this queue would otherwise
/// grow against until the machine ran out of memory. Generous, because the
/// thing being bounded is somebody pasting repeatedly into a wedged terminal.
const INPUT_BACKLOG: usize = 8 * 1024 * 1024;

/// How long a latency ping is worth waiting for. The status bar polls every
/// two seconds, so a ping that outlives its own poll is not a measurement any
/// more.
const PING_TIMEOUT: Duration = Duration::from_secs(3);

/// What one command did on the host.
#[derive(Debug, Clone)]
pub struct Executed {
    /// The host's own exit status, when it sent one. `None` is a command that
    /// ended without reporting — a signal, or a server that did not say — and
    /// is never read as success.
    pub status: Option<u32>,
    pub stdout: String,
    pub stderr: String,
    /// Whether either stream hit the capture limit.
    pub truncated: bool,
}

impl Executed {
    pub fn succeeded(&self) -> bool {
        self.status == Some(0)
    }

    /// The best line to show when it did not.
    pub fn complaint(&self) -> String {
        let text = if self.stderr.trim().is_empty() { &self.stdout } else { &self.stderr };
        let summary: Vec<&str> = text
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .take(3)
            .collect();
        if summary.is_empty() {
            return match self.status {
                Some(code) => format!("the command failed with status {code} and said nothing"),
                None => "the command ended without reporting a status".into(),
            };
        }
        summary.join("; ")
    }
}

// --------------------------------------------------------------- prompting

/// One question the host asked that only the user can answer.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AuthPrompt {
    pub prompt: String,
    /// False for a secret, which the interface must not echo.
    pub echo: bool,
}

/// Which method a request belongs to.
///
/// The interface cannot work this out from the fields. A hidden field is not
/// proof of a reusable password: a keyboard-interactive round asks for
/// whatever the host's PAM stack wants, which may be a one-time code, and
/// remembering that would save six digits that were only ever good once. So
/// the method is stated, and only one of them can be remembered.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Method {
    Password,
    KeyboardInteractive,
    KeyPassphrase,
}

/// What came back from a dialog.
#[derive(Debug, Clone)]
pub struct Answer {
    pub answers: Vec<String>,
    /// True when the user asked for the password to be remembered. Only ever
    /// true for a `Password` request the store could take.
    pub remember: bool,
}

/// A credential request on its way to the interface.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AuthRequest {
    pub session: String,
    pub request: String,
    pub host: String,
    pub user: String,
    /// What the host called this exchange, when it said.
    pub name: String,
    pub instructions: String,
    pub prompts: Vec<AuthPrompt>,
    /// What is being asked for, which decides whether the dialog offers to
    /// remember the answer.
    pub method: Method,
    /// True when this answer could be saved: a password, on a saved host,
    /// with a credential store to put it in. False is the reason the checkbox
    /// is not there, and the interface does not have to know which of the
    /// three it was.
    pub can_remember: bool,
}

/// Where a credential request goes to be answered.
///
/// The application puts a dialog on the screen. A test answers it, or refuses
/// to. Keeping this an interface rather than an `AppHandle` is what lets the
/// whole connection path — configuration, trust, authentication, channels — be
/// driven against a real SSH server with no application running.
pub trait PromptSink: Send + Sync {
    fn request(&self, request: &AuthRequest);
    /// The request is over, answered or not, so any dialog can be dismissed.
    fn resolved(&self, request: &str);
    /// Something the user asked for did not happen, on a connection that
    /// worked anyway. Saving a password is the case this exists for: the
    /// handshake succeeded and the credential store refused the write, and
    /// silently dropping the request would leave someone believing their
    /// password was saved when it was not.
    fn notice(&self, session: &str, message: &str);
}

/// A sink that answers nothing. Every interactive attempt against it fails,
/// which is what a non-interactive caller wants. Only the tests need one: the
/// application always has a dialog to show.
#[cfg(test)]
pub struct NoPrompts;

#[cfg(test)]
impl PromptSink for NoPrompts {
    fn request(&self, _: &AuthRequest) {}
    fn resolved(&self, _: &str) {}
    fn notice(&self, _: &str, _: &str) {}
}

/// Credential dialogs waiting for an answer.
///
/// The answers cross the process once, into the handshake that asked for them,
/// and are dropped when it finishes. Nothing here outlives a connection
/// attempt, which is why there is no map of secrets anywhere in afterm.
pub struct Prompts {
    sink: Box<dyn PromptSink>,
    waiting: StdMutex<HashMap<String, oneshot::Sender<Option<Answer>>>>,
    next: AtomicU64,
}

impl Prompts {
    pub fn new(sink: Box<dyn PromptSink>) -> Self {
        Self { sink, waiting: StdMutex::new(HashMap::new()), next: AtomicU64::new(0) }
    }

    /// Deliver what the user typed, or `None` if they dismissed the dialog.
    /// Returns false when nothing was waiting, which is the normal outcome of
    /// answering a dialog whose attempt has already timed out.
    pub fn answer(&self, request: &str, answers: Option<Vec<String>>, remember: bool) -> bool {
        let Some(sender) = self.waiting.lock().unwrap().remove(request) else {
            return false;
        };
        let answer = answers.map(|answers| Answer { answers, remember });
        sender.send(answer).is_ok()
    }

    /// Pass a message about this session on to the interface.
    pub fn notice(&self, session: &str, message: &str) {
        self.sink.notice(session, message);
    }

    /// Abandon every dialog for one session, so closing a tab does not leave a
    /// handshake waiting on an answer that can no longer arrive.
    pub fn cancel_session(&self, session: &str) {
        let prefix = format!("{session}/");
        let mut waiting = self.waiting.lock().unwrap();
        let ids: Vec<String> = waiting
            .keys()
            .filter(|id| id.starts_with(&prefix))
            .cloned()
            .collect();
        for id in ids {
            if let Some(sender) = waiting.remove(&id) {
                let _ = sender.send(None);
            }
        }
    }

    async fn ask(
        &self,
        session: &str,
        host: &str,
        user: &str,
        name: &str,
        instructions: &str,
        prompts: Vec<AuthPrompt>,
        method: Method,
        can_remember: bool,
    ) -> Result<Answer, Failure> {
        let id = format!("{session}/{}", self.next.fetch_add(1, Ordering::Relaxed));
        let (sender, receiver) = oneshot::channel();
        self.waiting.lock().unwrap().insert(id.clone(), sender);

        let request = AuthRequest {
            session: session.to_string(),
            request: id.clone(),
            host: host.to_string(),
            user: user.to_string(),
            name: name.to_string(),
            instructions: instructions.to_string(),
            prompts,
            method,
            can_remember,
        };
        self.sink.request(&request);

        let answered = tokio::time::timeout(PROMPT_TIMEOUT, receiver).await;
        self.waiting.lock().unwrap().remove(&id);
        self.sink.resolved(&id);
        match answered {
            Ok(Ok(Some(answer))) => Ok(answer),
            Ok(Ok(None)) | Ok(Err(_)) => {
                Err(Failure::new(Cause::Auth, "authentication was cancelled"))
            }
            Err(_) => Err(Failure::new(Cause::Auth, "the credential prompt timed out")),
        }
    }
}

// -------------------------------------------------------- host key checking

/// The client handler. Its whole job is refusing host keys the user has not
/// already accepted.
struct Verifier {
    trusted: trust::Trusted,
    /// What the host actually presented, for the refusal message.
    offered: Arc<StdMutex<Option<String>>>,
}

impl client::Handler for Verifier {
    type Error = russh::Error;

    async fn check_server_key(
        &mut self,
        key: &russh::keys::PublicKeyOrCertificate,
    ) -> Result<bool, Self::Error> {
        let offered: PublicKey = match key {
            russh::keys::PublicKeyOrCertificate::PublicKey { key, .. } => key.clone(),
            // A certificate means the host expects to be verified through an
            // authority. `trust` refuses those before the connection is made,
            // so reaching here at all means something changed underneath.
            russh::keys::PublicKeyOrCertificate::Certificate(certificate) => {
                certificate.public_key().clone().into()
            }
        };
        *self.offered.lock().unwrap() = Some(offered.fingerprint(Default::default()).to_string());
        Ok(self.trusted.accepts(&offered))
    }
}

// ------------------------------------------------------------- connection

/// One authenticated connection, with everything afterm needs riding on it.
pub struct Connection {
    handle: client::Handle<Verifier>,
    /// Opened on first use and kept: the whole point is not paying for a
    /// handshake per click.
    sftp: Mutex<Option<Arc<SftpSession>>>,
    latest: Arc<StdMutex<Option<Metrics>>>,
    telemetry_note: Arc<StdMutex<Option<String>>>,
    /// Which method actually worked, so the interface can say so.
    pub method: String,
}

/// Everything needed to open one connection.
pub struct Target {
    pub session: String,
    /// The saved host's stable id, where this session came from one.
    ///
    /// Not the session: a tab's id is made fresh every time one is opened, and
    /// a password remembered against it would be remembered for exactly one
    /// tab and then orphaned. `None` is an address typed in and not saved,
    /// which has no identity to remember anything against — and so cannot
    /// remember a password, which is the honest answer rather than a
    /// limitation to work around.
    pub profile: Option<String>,
    pub host: String,
    pub user: String,
    pub port: u16,
    /// An `ssh_config` to read instead of the user's own. `None` everywhere in
    /// the application; the live test sets it because OpenSSH resolves `~`
    /// through the password database rather than `$HOME`, so there is no other
    /// way to point it at a throwaway configuration.
    pub config_file: Option<PathBuf>,
}

impl Connection {
    /// Resolve the configuration, verify the host key, authenticate, and start
    /// telemetry. `interactive` is false for anything a timer drives: a
    /// two-second poll must never raise a credential dialog by itself.
    pub async fn open(
        target: &Target,
        prompts: &Prompts,
        vault: &Vault,
        interactive: bool,
    ) -> Result<Self, Failure> {
        let effective =
            config::resolve(&target.host, &target.user, target.port, target.config_file.as_deref())?;
        let trusted = trust::lookup(&effective.hostname, effective.port, &effective.known_hosts)?;

        if trusted.is_empty() {
            // Verify only. afterm does not offer to accept a key here, because
            // the check it would be accepting against cannot see a `@revoked`
            // entry — and because the terminal is right there and does this
            // properly, with the prompt the user already knows.
            let policy = effective.strict_host_key_checking.to_ascii_lowercase();
            let aside = if policy == "no" || policy == "off" || policy == "accept-new" {
                format!(
                    " Your ssh_config sets StrictHostKeyChecking {policy}, which would add it \
                     automatically; afterm's own connection only ever verifies."
                )
            } else {
                String::new()
            };
            return Err(Failure::host_key(format!(
                "{} is not in known_hosts, so afterm will not offer it a credential. \
                 Connect the terminal first and accept the key there.{aside}",
                effective.hostname
            )));
        }

        // Negotiate only what the user's configuration permits, intersected
        // with the host key algorithms they have actually recorded. The same
        // list is what a rekey negotiates from, so the policy holds for the
        // life of the connection rather than only its first handshake.
        let preferred = crypto::preferences(&effective.crypto, &trusted.algorithms)?;
        let config = Arc::new(client::Config {
            preferred,
            inactivity_timeout: None,
            keepalive_interval: Some(Duration::from_secs(15)),
            keepalive_max: 3,
            ..Default::default()
        });

        let offered = Arc::new(StdMutex::new(None));
        let verifier = Verifier { trusted, offered: Arc::clone(&offered) };
        let address = (effective.hostname.clone(), effective.port);
        let handle = match tokio::time::timeout(
            CONNECT_TIMEOUT,
            client::connect(config, address, verifier),
        )
        .await
        {
            Ok(Ok(handle)) => handle,
            Ok(Err(error)) => return Err(connect_failure(error, &effective, &offered)),
            Err(_) => {
                return Err(Failure::unreachable(format!(
                    "no answer from {} within {}s",
                    effective.hostname,
                    CONNECT_TIMEOUT.as_secs()
                )))
            }
        };

        let mut handle = handle;
        // The key this connection was accepted on. Part of what a remembered
        // password is bound to, so a host that comes back with a different key
        // is asked about rather than sent the old password — and it is read
        // here because by now the handshake has happened and `Verifier` has
        // written down what was actually presented.
        let fingerprint = offered.lock().unwrap().clone().unwrap_or_default();
        let method =
            authenticate(&mut handle, &effective, target, prompts, vault, &fingerprint, interactive)
                .await?;

        let latest = Arc::new(StdMutex::new(None));
        // Starts with a reason rather than silence. A status bar showing em
        // dashes and no explanation is indistinguishable from a broken one,
        // which is exactly how this was first reported.
        let telemetry_note = Arc::new(StdMutex::new(Some("no data yet".to_string())));
        let connection = Self {
            handle,
            sftp: Mutex::new(None),
            latest: Arc::clone(&latest),
            telemetry_note: Arc::clone(&telemetry_note),
            method,
        };
        connection.start_telemetry(latest, telemetry_note).await;
        Ok(connection)
    }

    pub fn alive(&self) -> bool {
        !self.handle.is_closed()
    }

    /// The most recent telemetry sample, and the reason there is none.
    /// Round-trip time to the host, in milliseconds.
    ///
    /// A ping is a no-op the protocol obliges the far end to answer, so this
    /// times the connection itself rather than anything the host has to run —
    /// no shell, no process, nothing that a loaded machine would slow down. It
    /// is the one number the terminal's own connection can report about
    /// itself.
    ///
    /// A ping that does not come back inside the timeout is reported as no
    /// measurement rather than as a very large one: past a few seconds the
    /// figure has stopped being latency and started being a problem, and the
    /// status bar has other ways to say that.
    pub async fn latency(&self) -> Option<f64> {
        let started = std::time::Instant::now();
        match tokio::time::timeout(PING_TIMEOUT, self.handle.send_ping()).await {
            Ok(Ok(())) => Some(started.elapsed().as_secs_f64() * 1000.0),
            _ => None,
        }
    }

    pub fn telemetry(&self) -> (Option<Metrics>, Option<String>) {
        (
            self.latest.lock().unwrap().clone(),
            self.telemetry_note.lock().unwrap().clone(),
        )
    }

    /// The SFTP session, opened on first use.
    async fn sftp(&self) -> Result<Arc<SftpSession>, Failure> {
        let mut slot = self.sftp.lock().await;
        if let Some(session) = slot.as_ref() {
            return Ok(Arc::clone(session));
        }
        let channel = self
            .handle
            .channel_open_session()
            .await
            .map_err(|error| Failure::unreachable(format!("could not open a channel: {error}")))?;
        channel
            .request_subsystem(true, "sftp")
            .await
            .map_err(|error| {
                Failure::operation(format!("this host has no SFTP subsystem: {error}"))
            })?;
        let session = SftpSession::new(channel.into_stream())
            .await
            .map_err(|error| Failure::operation(format!("could not start SFTP: {error}")))?;
        let session = Arc::new(session);
        *slot = Some(Arc::clone(&session));
        Ok(session)
    }

    pub async fn home(&self) -> Result<String, Failure> {
        self.sftp()
            .await?
            .canonicalize(".")
            .await
            .map_err(|error| Failure::operation(format!("could not find the home directory: {error}")))
    }

    pub async fn list(&self, path: &str) -> Result<Listing, Failure> {
        let sftp = self.sftp().await?;
        let read = sftp
            .read_dir(path.to_string())
            .await
            .map_err(|error| Failure::operation(format!("could not list {path}: {error}")))?;

        let mut entries: Vec<FileEntry> = read
            .map(|entry| {
                let metadata = entry.metadata();
                let kind = if metadata.is_symlink() {
                    EntryKind::Link
                } else if metadata.is_dir() {
                    EntryKind::Directory
                } else {
                    EntryKind::File
                };
                FileEntry {
                    name: entry.file_name(),
                    path: join(path, &entry.file_name()),
                    kind,
                    // Directory sizes are inode sizes, not content.
                    size: (kind == EntryKind::File).then_some(metadata.size).flatten(),
                    modified: metadata.mtime.map(format_mtime),
                }
            })
            .collect();
        sort_entries(&mut entries);

        let trimmed = path.trim_end_matches('/');
        let parent = trimmed.rfind('/').map(|index| {
            if index == 0 {
                "/".to_string()
            } else {
                trimmed[..index].to_string()
            }
        });
        Ok(Listing {
            path: path.to_string(),
            parent: parent.filter(|value| value != path),
            entries,
        })
    }

    /// Whether anything is at `path`, a dangling symlink included.
    pub async fn exists(&self, path: &str) -> Result<bool, Failure> {
        let sftp = self.sftp().await?;
        exists(&sftp, path).await
    }

    pub async fn create_directory(&self, path: &str) -> Result<(), Failure> {
        self.sftp()
            .await?
            .create_dir(path.to_string())
            .await
            .map_err(|error| Failure::operation(format!("could not create {path}: {error}")))
    }

    /// Narrow a directory to its owner: `0700`.
    pub async fn make_private(&self, path: &str) -> Result<(), Failure> {
        let sftp = self.sftp().await?;
        let mut attributes = Metadata::default();
        attributes.permissions = Some(0o700);
        sftp.set_metadata(path.to_string(), attributes)
            .await
            .map_err(|error| Failure::operation(format!("could not make {path} private: {error}")))
    }

    /// Rename one entry. The protocol's own `rename` refuses to replace an
    /// existing destination, which is the behaviour the pane wants: a rename
    /// is a typo away from being the wrong name.
    pub async fn rename(&self, from: &str, to: &str) -> Result<(), Failure> {
        self.sftp()
            .await?
            .rename(from.to_string(), to.to_string())
            .await
            .map_err(|error| Failure::operation(format!("could not rename {from}: {error}")))
    }

    /// Remove one entry. A directory goes through `rmdir`, which the protocol
    /// refuses on a directory that is not empty; emptying one first is
    /// `folder::remove`, which calls this per entry.
    pub async fn delete(&self, path: &str, directory: bool) -> Result<(), Failure> {
        let sftp = self.sftp().await?;
        let result = if directory {
            sftp.remove_dir(path.to_string()).await
        } else {
            sftp.remove_file(path.to_string()).await
        };
        result.map_err(|error| Failure::operation(format!("could not delete {path}: {error}")))
    }

    pub async fn download(
        &self,
        remote: &str,
        local: &str,
        progress: impl Fn(u64, Option<u64>),
    ) -> Result<(), Failure> {
        let sftp = self.sftp().await?;
        let total = sftp.metadata(remote.to_string()).await.ok().and_then(|m| m.size);
        let mut source = sftp
            .open(remote.to_string())
            .await
            .map_err(|error| Failure::operation(format!("could not open {remote}: {error}")))?;
        let mut sink = tokio::fs::File::create(local)
            .await
            .map_err(|error| Failure::operation(format!("could not write {local}: {error}")))?;

        let mut buffer = vec![0_u8; TRANSFER_CHUNK];
        let mut done: u64 = 0;
        loop {
            let read = source
                .read(&mut buffer)
                .await
                .map_err(|error| Failure::operation(format!("transfer failed: {error}")))?;
            if read == 0 {
                break;
            }
            sink.write_all(&buffer[..read])
                .await
                .map_err(|error| Failure::operation(format!("could not write {local}: {error}")))?;
            done += read as u64;
            progress(done, total);
        }
        sink.flush()
            .await
            .map_err(|error| Failure::operation(format!("could not write {local}: {error}")))?;
        Ok(())
    }

    /// Send `local` into `directory` as `name`, and say where it landed.
    ///
    /// Written under a temporary name first — created exclusively, so two
    /// uploads cannot share it — and moved into place only once every byte has
    /// arrived. A transfer that fails removes the temporary, so nothing that
    /// is half a file is ever visible under any name. `collision` decides
    /// what happens when `name` is already taken; the protocol's own `rename`
    /// refuses an existing target on OpenSSH, so `Fail` holds even if
    /// something appears between the check and the move.
    pub async fn upload(
        &self,
        local: &str,
        directory: &str,
        name: &str,
        collision: Collision,
        progress: impl Fn(u64, Option<u64>),
    ) -> Result<String, Failure> {
        let sftp = self.sftp().await?;
        let mut source = tokio::fs::File::open(local)
            .await
            .map_err(|error| Failure::operation(format!("could not read {local}: {error}")))?;
        let metadata = source.metadata().await.ok();
        let total = metadata.as_ref().map(|m| m.len());
        // Decided before anything is written, so a destination afterm will not
        // replace is refused before the transfer rather than after it.
        let intended = intended_mode(&sftp, directory, name, collision, metadata.as_ref()).await?;
        let (part, mut sink) =
            create_exclusive(&sftp, directory, &format!(".{name}.afterm-part"), STAGING_MODE)
                .await?;

        let written: Result<(), Failure> = async {
            let mut buffer = vec![0_u8; TRANSFER_CHUNK];
            let mut done: u64 = 0;
            loop {
                let read = source
                    .read(&mut buffer)
                    .await
                    .map_err(|error| Failure::operation(format!("could not read {local}: {error}")))?;
                if read == 0 {
                    break;
                }
                sink.write_all(&buffer[..read])
                    .await
                    .map_err(|error| Failure::operation(format!("transfer failed: {error}")))?;
                done += read as u64;
                progress(done, total);
            }
            sink.flush()
                .await
                .map_err(|error| Failure::operation(format!("transfer failed: {error}")))
        }
        .await;
        // Closed before it is moved: a server may refuse to rename an open
        // file, and a close that fails means the last bytes may not be there.
        let closed = sink
            .close()
            .await
            .map_err(|error| Failure::operation(format!("transfer failed: {error}")));
        if let Err(failure) = written.and(closed) {
            let _ = sftp.remove_file(part).await;
            return Err(failure);
        }

        // The mode is set on the staging file, before the move, so the file
        // never exists under its real name wearing the wrong one. A failure
        // here abandons the upload: the destination is still whatever it was,
        // and a file that cannot be given the protection it is supposed to have
        // is not one to install quietly.
        if let Err(failure) = wear(&sftp, &part, intended).await {
            let _ = sftp.remove_file(part).await;
            return Err(failure);
        }

        match place(&sftp, &part, directory, name, collision).await {
            Ok(path) => Ok(path),
            Err(failure) => {
                let _ = sftp.remove_file(part).await;
                Err(failure)
            }
        }
    }

    /// Run one command on this connection and hand back what it did.
    ///
    /// A channel of its own, on the connection that is already authenticated:
    /// nothing goes through the terminal, so nothing depends on what the
    /// terminal is currently showing, which shell is in front of it, or whether
    /// anything is waiting at a prompt. The exit status is the host's own,
    /// which is the difference between "afterm sent a command" and "the command
    /// worked".
    ///
    /// Both streams are bounded. A command that will not stop talking is a
    /// command afterm stops listening to rather than one it holds in memory.
    pub async fn run(&self, command: &str) -> Result<Executed, Failure> {
        let mut channel = self
            .handle
            .channel_open_session()
            .await
            .map_err(|error| Failure::unreachable(format!("could not open a channel: {error}")))?;
        channel
            .exec(true, command)
            .await
            .map_err(|error| Failure::operation(format!("this host refused the command: {error}")))?;

        let mut out = Vec::new();
        let mut err = Vec::new();
        let mut status = None;
        let mut truncated = false;
        let keep = |sink: &mut Vec<u8>, data: &[u8], truncated: &mut bool| {
            let room = EXEC_CAPTURE.saturating_sub(sink.len());
            let take = room.min(data.len());
            sink.extend_from_slice(&data[..take]);
            *truncated |= take < data.len();
        };
        while let Some(message) = channel.wait().await {
            match message {
                ChannelMsg::Data { ref data } => keep(&mut out, data, &mut truncated),
                ChannelMsg::ExtendedData { ref data, .. } => keep(&mut err, data, &mut truncated),
                ChannelMsg::ExitStatus { exit_status } => status = Some(exit_status),
                ChannelMsg::Eof | ChannelMsg::Close => break,
                _ => {}
            }
        }
        let _ = channel.close().await;
        Ok(Executed {
            status,
            stdout: String::from_utf8_lossy(&out).into_owned(),
            stderr: String::from_utf8_lossy(&err).into_owned(),
            truncated,
        })
    }

    /// An exec channel streaming `/proc` reads, exactly as the OpenSSH helper
    /// does — except that it costs no extra login, because it is a channel on a
    /// connection that already exists.
    async fn start_telemetry(
        &self,
        latest: Arc<StdMutex<Option<Metrics>>>,
        note: Arc<StdMutex<Option<String>>>,
    ) {
        // The channel is opened here rather than inside the task: a connection
        // handle cannot be cloned, and a channel can be moved.
        let channel = match self.handle.channel_open_session().await {
            Ok(channel) => channel,
            Err(error) => {
                *note.lock().unwrap() = Some(format!("no telemetry channel: {error}"));
                return;
            }
        };
        if let Err(error) = channel.exec(true, PROBE).await {
            *note.lock().unwrap() = Some(format!("no telemetry: {error}"));
            return;
        }
        // The remote emits a block every two seconds and the parser needs two
        // of them before it can report a rate, so the first sample is a few
        // seconds out. That is a state, not a failure.
        *note.lock().unwrap() = Some("waiting for the first sample".into());

        tokio::spawn(async move {
            let mut channel = channel;
            let mut blocks = Blocks::new();
            let mut previous_cpu: Option<CpuSample> = None;
            let mut previous_net: Option<(NetSample, Instant)> = None;
            // Set when the stream was stopped for sending more than afterm will
            // hold, which is what the status bar should say rather than that
            // the stream simply ended.
            let mut refused: Option<&'static str> = None;

            while let Some(message) = channel.wait().await {
                let data = match message {
                    ChannelMsg::Data { ref data } => data.to_vec(),
                    ChannelMsg::Eof | ChannelMsg::Close | ChannelMsg::ExitStatus { .. } => break,
                    _ => continue,
                };
                let outcome = blocks.feed(&data, |block| {
                    let metrics = parse_block(block, &mut previous_cpu, &mut previous_net);
                    // A block that parses to nothing means the host answered
                    // but has no /proc to read — a BSD, a Windows host, a
                    // restricted shell. Say which, rather than showing dashes.
                    *note.lock().unwrap() = if metrics.total_memory.is_none()
                        && metrics.uptime.is_none()
                    {
                        Some("this host reports no /proc".into())
                    } else {
                        None
                    };
                    *latest.lock().unwrap() = Some(metrics);
                });
                if let Err(reason) = outcome {
                    refused = Some(reason);
                    break;
                }
            }
            *latest.lock().unwrap() = None;
            *note.lock().unwrap() = Some(refused.unwrap_or("the telemetry stream ended").into());
            // Nobody is reading this channel now. Closing it is how the far end
            // is told to stop writing to it, which is the point when the reason
            // for leaving was that it would not stop.
            let _ = channel.close().await;
        });
    }

    /// A shell on this connection, with a pseudo-terminal negotiated remotely:
    /// the terminal, without a local `ssh.exe`.
    ///
    /// Output and the end of the session are handed to callbacks rather than
    /// emitted from here, which keeps this module free of the application's
    /// event names — and lets the battery drive a real shell channel with no
    /// application at all.
    ///
    /// `on_close` is called once when the channel ends, with whether the remote
    /// shell reported an exit of its own. False means it ended without one,
    /// which is what a dropped connection looks like from this side.
    ///
    /// `flow` is how the interface says it is falling behind. A closed gate
    /// stops this reading the channel, which stops adjusting its window, which
    /// is the protocol's own way of telling the far end to wait — so a flood
    /// is held on the host that is producing it rather than queued here. It
    /// gates reading and nothing else: whatever is flooding the terminal is
    /// usually stopped by something the user types, so Ctrl-C has to get
    /// through while the gate is shut. That used to be a comment on a select
    /// arm and is now the shape of the code.
    ///
    /// Two tasks, not one, and that is the whole shape of this function. A
    /// single task doing both used to await `data_bytes` in the same place it
    /// drained the channel, and the two deadlock against each other on any
    /// host that answers while it is being written to: the write parks waiting
    /// for send window, the channel's receive queue fills because nobody is
    /// draining it, the connection's event loop blocks trying to push into
    /// that queue, and the window adjustment the write is waiting for is
    /// sitting behind the block. A 12 MiB paste at an echoing host stopped
    /// dead at 3,178,496 bytes with a 20-byte banner on screen. Reading and
    /// writing never wait on each other now.
    pub async fn open_shell(
        &self,
        on_output: impl Fn(&[u8]) + Send + 'static,
        on_close: impl FnOnce(bool) + Send + 'static,
        cols: u16,
        rows: u16,
        flow: Arc<FlowGate>,
    ) -> Result<ShellChannel, Failure> {
        self.open_terminal(None, on_output, on_close, cols, rows, flow).await
    }

    /// `open_shell`, running `command` on the terminal instead of the login
    /// shell when there is one — which is how a tab keeps its shell in tmux
    /// (see `persist.rs`). The same channel, pseudo-terminal and flow control;
    /// only the request that starts it differs.
    pub async fn open_terminal(
        &self,
        command: Option<String>,
        on_output: impl Fn(&[u8]) + Send + 'static,
        on_close: impl FnOnce(bool) + Send + 'static,
        cols: u16,
        rows: u16,
        flow: Arc<FlowGate>,
    ) -> Result<ShellChannel, Failure> {
        let mut channel = self
            .handle
            .channel_open_session()
            .await
            .map_err(|error| Failure::unreachable(format!("could not open a channel: {error}")))?;

        // Anything the host said before it had finished answering, kept so the
        // terminal opens on it rather than losing it. A shell that greets the
        // moment it starts is ordinary; the acknowledgment and the greeting
        // race, and either order is correct.
        let mut early: Vec<Vec<u8>> = Vec::new();
        let setup = async {
            channel
                .request_pty(true, "xterm-256color", cols as u32, rows as u32, 0, 0, &[])
                .await
                .map_err(|error| {
                    Failure::operation(format!("this host refused a terminal: {error}"))
                })?;
            accepted(&mut channel, "a terminal", &mut early, SETUP_TIMEOUT).await?;
            match &command {
                None => {
                    channel.request_shell(true).await.map_err(|error| {
                        Failure::operation(format!("this host refused a shell: {error}"))
                    })?;
                    accepted(&mut channel, "a shell", &mut early, SETUP_TIMEOUT).await
                }
                Some(command) => {
                    channel.exec(true, command.as_bytes()).await.map_err(|error| {
                        Failure::operation(format!("this host refused to run the terminal's command: {error}"))
                    })?;
                    accepted(&mut channel, "the terminal's command", &mut early, SETUP_TIMEOUT).await
                }
            }
        };
        if let Err(failure) = setup.await {
            let _ = channel.close().await;
            return Err(failure);
        }

        let (sender, mut receiver) = mpsc::unbounded_channel::<ShellCommand>();
        let (mut read, write) = channel.split();
        let queued = Arc::new(AtomicUsize::new(0));
        let closing = Arc::new(Notify::new());
        let deliberate = Arc::new(AtomicBool::new(false));
        // Raised by the writer when it stops, whatever stopped it. A host that
        // is listening answers a close with a close and the reader would see
        // it anyway; this is for the host that is not, where waiting to be
        // answered is waiting forever.
        let closed = Arc::new(Notify::new());

        // The writer. It never touches the read half, so a write parked on the
        // send window holds up nothing but the next write.
        let ending = Arc::clone(&closing);
        let ended = Arc::clone(&closed);
        let sent = Arc::clone(&queued);
        let by_us = Arc::clone(&deliberate);
        tokio::spawn(async move {
            loop {
                let command = tokio::select! {
                    // The close first, so a tab that is on its way out is not
                    // made to wait behind whatever is still queued.
                    biased;
                    _ = ending.notified() => break,
                    command = receiver.recv() => command,
                };
                match command {
                    Some(ShellCommand::Data(bytes)) => {
                        let size = bytes.len();
                        // Raced against the close, because this is the await
                        // that blocks: a paste larger than the window sits
                        // here until the host reads it, and closing the tab
                        // must not have to outlast that.
                        let written = tokio::select! {
                            biased;
                            _ = ending.notified() => false,
                            outcome = write.data_bytes(bytes) => outcome.is_ok(),
                        };
                        sent.fetch_sub(size, Ordering::SeqCst);
                        if !written {
                            break;
                        }
                    }
                    Some(ShellCommand::Resize { cols, rows }) => {
                        let _ = write.window_change(cols as u32, rows as u32, 0, 0).await;
                    }
                    // The interface let the terminal go, which is as
                    // deliberate as pressing the close button.
                    None => {
                        by_us.store(true, Ordering::SeqCst);
                        break;
                    }
                }
            }
            // The reader goes first, and unconditionally. It is what reports
            // the end of the session, and it must not be made to wait on the
            // far end being told anything: a host that has stopped reading its
            // socket blocks the connection's event loop, and `close` below
            // then blocks handing it the message. That host is exactly the one
            // whose tab is being closed — releasing the reader afterwards left
            // a tab closed out of a stuck write never reporting that it had
            // ended at all, which is what the Windows agent caught and Linux's
            // larger loopback buffers hid.
            ended.notify_one();
            // Still sent, because a host that is listening should be told.
            // Bounded, because one that is not must not keep this alive.
            let _ = tokio::time::timeout(CLOSE_TIMEOUT, write.close()).await;
        });

        // The reader.
        let stopped = Arc::clone(&closing);
        let done = Arc::clone(&closed);
        let ours = Arc::clone(&deliberate);
        tokio::spawn(async move {
            for chunk in early {
                on_output(&chunk);
            }
            // A shell that ends on its own says so first: `exit`, Ctrl-D and a
            // logout all send an exit status or an exit signal ahead of the
            // channel closing. A connection that drops sends neither, which is
            // the whole difference between closing the tab and offering to
            // reconnect. Closing on our own side counts as deliberate too.
            let mut exited = false;
            loop {
                tokio::select! {
                    message = read.wait(), if !flow.is_paused() => match message {
                        Some(ChannelMsg::Data { ref data }) => on_output(data),
                        Some(ChannelMsg::ExtendedData { ref data, .. }) => on_output(data),
                        Some(ChannelMsg::ExitStatus { .. }) | Some(ChannelMsg::ExitSignal { .. }) => {
                            exited = true;
                        }
                        Some(ChannelMsg::Eof) | Some(ChannelMsg::Close) | None => break,
                        Some(_) => {}
                    },
                    // Nothing to do but go round again, with the branch above
                    // enabled once more.
                    _ = flow.opened(), if flow.is_paused() => {}
                    // afterm ended this one. Ungated, unlike the read above:
                    // a tab closed while the interface is behind must not wait
                    // for the interface to catch up first. Whatever is still
                    // on the wire is output for a tab that has gone.
                    _ = done.notified() => break,
                }
            }
            // Nothing will be read from this channel again, so there is
            // nothing left for the writer to be waiting on.
            stopped.notify_one();
            on_close(exited || ours.load(Ordering::SeqCst));
        });

        Ok(ShellChannel { input: sender, queued, closing, deliberate })
    }

    pub async fn close(&self) {
        let _ = self
            .handle
            .disconnect(Disconnect::ByApplication, "", "en")
            .await;
    }
}

/// What the writer takes. There is no `Close`: closing has to reach the writer
/// while it is parked on a write, and a queue is exactly the wrong place to
/// put something that must overtake what is already in it.
enum ShellCommand {
    Data(Vec<u8>),
    Resize { cols: u16, rows: u16 },
}

/// The writing half of a native terminal, shaped like the PTY it replaces.
pub struct ShellChannel {
    input: mpsc::UnboundedSender<ShellCommand>,
    /// Bytes handed over and not yet written. The queue itself is unbounded
    /// and this is what bounds it — see `INPUT_BACKLOG`.
    queued: Arc<AtomicUsize>,
    /// Raised to end the session, and it does not travel through the queue:
    /// the write at the front of the queue is the one thing that can be stuck
    /// for minutes, and closing the tab is what someone does about that.
    closing: Arc<Notify>,
    /// Whether the end was afterm's doing rather than the host's, which is the
    /// difference between closing the tab and offering to reconnect.
    deliberate: Arc<AtomicBool>,
}

#[cfg(test)]
impl ShellChannel {
    /// A channel with nobody on the other end, for a test that needs the shape
    /// of a session without a host behind it.
    pub fn detached() -> Self {
        let (input, _nobody) = mpsc::unbounded_channel();
        Self {
            input,
            queued: Arc::new(AtomicUsize::new(0)),
            closing: Arc::new(Notify::new()),
            deliberate: Arc::new(AtomicBool::new(false)),
        }
    }

    /// A channel whose queue nobody is draining, which is what a host that has
    /// stopped reading looks like from this side. The receiver is handed back
    /// because dropping it would make every write fail for the other reason.
    pub fn stalled() -> (Self, mpsc::UnboundedReceiver<ShellCommand>) {
        let (input, waiting) = mpsc::unbounded_channel();
        let channel = Self {
            input,
            queued: Arc::new(AtomicUsize::new(0)),
            closing: Arc::new(Notify::new()),
            deliberate: Arc::new(AtomicBool::new(false)),
        };
        (channel, waiting)
    }
}

impl ShellChannel {
    /// Hand the host some input, unless too much is already waiting.
    ///
    /// The limit is on the backlog rather than on any one write: a write is
    /// one thing the user did — a keystroke, a paste — and refusing half of a
    /// paste would be worse than refusing the whole of it. So a single write
    /// of any size is taken when the queue is under the limit, and what is
    /// held is at most `INPUT_BACKLOG` plus whatever is in flight.
    pub fn write(&self, data: &str) -> Result<(), Failure> {
        if self.queued.load(Ordering::SeqCst) >= INPUT_BACKLOG {
            return Err(Failure::operation(
                "this host is not reading its input as fast as afterm is sending it; \
                 wait for it to catch up",
            ));
        }
        let bytes = data.as_bytes().to_vec();
        let size = bytes.len();
        self.queued.fetch_add(size, Ordering::SeqCst);
        self.input.send(ShellCommand::Data(bytes)).map_err(|_| {
            // Nobody will ever take those bytes off the queue, so the count
            // has to come back down here or it never does.
            self.queued.fetch_sub(size, Ordering::SeqCst);
            Failure::unreachable("this terminal has closed")
        })
    }

    pub fn resize(&self, cols: u16, rows: u16) {
        let _ = self.input.send(ShellCommand::Resize { cols, rows });
    }

    pub fn close(&self) {
        self.deliberate.store(true, Ordering::SeqCst);
        self.closing.notify_one();
    }
}

/// Wait for the host's answer to a channel request.
///
/// `request_pty` and `request_shell` only hand the request to the connection.
/// The acknowledgment comes back separately as `ChannelMsg::Success` or
/// `ChannelMsg::Failure`, and the receive loop used to ignore both — so a host
/// that refused the shell outright produced a session afterm reported as
/// connected, on a terminal that would never show anything. The status bar lit
/// green and the tab stayed blank.
///
/// Anything the host says while it is still answering is kept in `early` and
/// written to the terminal once it opens, bounded, because a host that talks
/// instead of answering is not one to hold in memory.
async fn accepted(
    channel: &mut russh::Channel<client::Msg>,
    what: &str,
    early: &mut Vec<Vec<u8>>,
    patience: Duration,
) -> Result<(), Failure> {
    let mut held = 0_usize;
    let answer = tokio::time::timeout(patience, async {
        while let Some(message) = channel.wait().await {
            match message {
                ChannelMsg::Success => return Some(true),
                ChannelMsg::Failure => return Some(false),
                ChannelMsg::Data { ref data } | ChannelMsg::ExtendedData { ref data, .. } => {
                    if held < SETUP_CAPTURE {
                        held += data.len();
                        early.push(data.to_vec());
                    }
                }
                ChannelMsg::Eof | ChannelMsg::Close => return None,
                _ => {}
            }
        }
        None
    })
    .await;

    match answer {
        Ok(Some(true)) => Ok(()),
        Ok(Some(false)) => {
            Err(Failure::operation(format!("this host refused {what} for this session")))
        }
        Ok(None) => Err(Failure::unreachable(format!(
            "this host closed the session while afterm was asking it for {what}"
        ))),
        Err(_) => Err(Failure::unreachable(format!(
            "this host did not answer afterm asking it for {what}"
        ))),
    }
}

// ---------------------------------------------------------- authentication

/// Work through the methods the host says it will accept, in the order that
/// asks the user for least.
///
/// The accepted methods come from the protocol — `authenticate_none` returns
/// them — not from parsing what a client wrote to stderr. Returns the name of
/// the method that worked.
#[allow(clippy::too_many_arguments)]
async fn authenticate(
    handle: &mut client::Handle<Verifier>,
    effective: &config::Effective,
    target: &Target,
    prompts: &Prompts,
    vault: &Vault,
    fingerprint: &str,
    interactive: bool,
) -> Result<String, Failure> {
    // `BatchMode yes` is the user saying never ask, and it is honoured ahead
    // of anything else: a prompt this configuration forbids is not a prompt.
    let may_prompt = interactive && !effective.batch_mode;
    let user = effective.user.clone();
    let mut remaining = match handle.authenticate_none(&user).await {
        Ok(AuthResult::Success) => return Ok("none".into()),
        Ok(AuthResult::Failure { remaining_methods, .. }) => remaining_methods,
        Err(error) => return Err(Failure::unreachable(format!("authentication failed: {error}"))),
    };

    let offers = |set: &russh::MethodSet, kind: MethodKind| set.iter().any(|m| *m == kind);

    // How many keys may be offered before afterm stops and asks the user.
    //
    // Every rejected credential counts against the server's `MaxAuthTries`,
    // which defaults to six, and `none` has already spent one. A machine with
    // several keys and an agent can therefore burn the whole budget on
    // publickey and be disconnected before the password it was always going to
    // need is ever offered — which reads as "this host is unreachable" on
    // exactly the password-only hosts the native transport exists for.
    let interactive_available = offers(&remaining, MethodKind::Password)
        || offers(&remaining, MethodKind::KeyboardInteractive);
    let mut budget: usize = if interactive_available { 3 } else { 5 };

    // Which RSA signature the host will accept, asked of the host.
    //
    // `PrivateKeyWithHashAlg::new(key, None)` signs an RSA key with SHA-1, and
    // OpenSSH has refused `ssh-rsa` by default since 8.8 — so an RSA key would
    // be rejected by every current server while `ssh.exe`, which negotiates
    // `rsa-sha2-*`, sails through. The symptom is a password prompt on a host
    // where the user's key works perfectly.
    let rsa_hash = rsa_hash_for(handle).await;
    // ... and which of those the user's own configuration would allow. `None`
    // is RSA ruled out entirely, which is not the same as "sign with SHA-1".
    let rsa_hash = permitted_rsa_hash(&effective.crypto, rsa_hash);
    let mut offered_keys = 0_usize;

    // 1. Keys named by the configuration, which is what a key user has.
    if offers(&remaining, MethodKind::PublicKey) && effective.allows("publickey") {
        for path in effective.identity_files.iter() {
            if budget == 0 {
                break;
            }
            let Some(key) = load_identity(path, prompts, target, may_prompt).await? else {
                continue;
            };
            // A key the configuration would not let the user's own client sign
            // with is not offered here either. Skipped rather than counted:
            // it spends none of the server's `MaxAuthTries`.
            let Some(hash) = signable(&effective.crypto, &key.algorithm(), rsa_hash) else {
                continue;
            };
            let attempt = handle
                .authenticate_publickey(&user, PrivateKeyWithHashAlg::new(Arc::new(key), hash))
                .await;
            budget -= 1;
            offered_keys += 1;
            match attempt {
                Ok(AuthResult::Success) => return Ok(format!("publickey ({})", path.display())),
                Ok(AuthResult::Failure { remaining_methods, .. }) => remaining = remaining_methods,
                Err(error) => {
                    return Err(Failure::unreachable(format!("authentication failed: {error}")))
                }
            }
            // The server stops listing publickey once it has had enough of
            // them. Offering more is how the connection gets dropped.
            if !offers(&remaining, MethodKind::PublicKey) {
                break;
            }
        }
    }

    // 2. The agent, which on Windows is a named pipe rather than a socket.
    let mut agent_state = AgentState::NotTried;
    if budget > 0 && offers(&remaining, MethodKind::PublicKey) && effective.allows("publickey") {
        // `IdentitiesOnly yes` reads the way OpenSSH reads it: the agent may
        // sign for a key the configuration names, and is not a source of any
        // other. Without it, the agent's keys are offered as they come.
        let allowed = effective
            .identities_only
            .then(|| configured_public_keys(&effective.identity_files));
        let (method, state) = try_agent(
            handle,
            &user,
            budget,
            rsa_hash,
            &effective.crypto,
            &effective.identity_agent,
            allowed.as_deref(),
        )
        .await?;
        agent_state = state;
        if let Some(method) = method {
            return Ok(method);
        }
    }

    // 3. A password already saved for exactly this destination.
    //
    // Ahead of the gate below, and deliberately: that gate is about asking the
    // user, and this asks nobody. It is the whole value of the feature — a
    // two-second telemetry poll and a click on the file pane both connect on a
    // remembered password, where before either one had to give up and wait for
    // someone to type it.
    //
    // Tried once. A password the host refuses counts against its own attempt
    // limit, so trying it twice walks a saved host towards a lockout it did
    // nothing to earn; the vault is told about the refusal so the other tabs on
    // the same saved host do not each spend another attempt on it either.
    let binding = target.profile.as_ref().map(|profile| secrets::Binding {
        profile: profile.clone(),
        user: user.clone(),
        hostname: effective.hostname.clone(),
        port: effective.port,
        fingerprint: fingerprint.to_string(),
    });
    let mut stored_refused = false;
    if offers(&remaining, MethodKind::Password) && effective.allows("password") {
        if let Some(binding) = binding.as_ref() {
            if let Some(password) = vault.remembered(binding) {
                match handle.authenticate_password(&user, password.as_str()).await {
                    Ok(AuthResult::Success) => return Ok("password".into()),
                    Ok(AuthResult::Failure { remaining_methods, .. }) => {
                        vault.reject(&binding.profile);
                        stored_refused = true;
                        remaining = remaining_methods;
                    }
                    Err(error) => {
                        return Err(Failure::unreachable(format!("authentication failed: {error}")))
                    }
                }
            }
        }
    }

    // 4. Anything that needs the user. A poll must never reach this.
    //
    // Read from `remaining` after the attempt above rather than before it: a
    // refused password can take methods off the table, and offering a dialog
    // for one the host has stopped accepting is a dialog that cannot work.
    let wants_password = offers(&remaining, MethodKind::Password) && effective.allows("password");
    let wants_interactive = offers(&remaining, MethodKind::KeyboardInteractive)
        && effective.allows("keyboard-interactive");

    if (wants_password || wants_interactive) && !may_prompt {
        if interactive {
            // Asked to prompt, and forbidden to by the user's own
            // configuration. No method is handed back, so the interface offers
            // no button that would only arrive here again.
            return Err(Failure::auth(
                "this host wants a credential, and your ssh_config sets BatchMode yes, \
                 which forbids asking for one",
                Vec::new(),
            ));
        }
        // The status bar prints this next to the session's own label, so the
        // host is already on screen; say what needs the credential instead of
        // repeating where it is going.
        let what = if wants_password {
            "files and metrics need a password"
        } else {
            "files and metrics need you to sign in"
        };
        return Err(Failure::auth(what, method_names(&remaining)));
    }

    if wants_interactive && keyboard_interactive(handle, &user, target, prompts).await?.is_some() {
        return Ok("keyboard-interactive".into());
    }

    // The dialog, which is also where a refused stored password lands — with
    // the refusal said out loud, because otherwise it looks like afterm forgot
    // the password rather than like the host said no to it.
    if wants_password {
        let instructions = if stored_refused {
            format!(
                "{}@{} refused the saved password. Enter it again to replace what is saved.",
                user, effective.hostname
            )
        } else {
            format!("Password for {}@{}", user, effective.hostname)
        };
        let answer = prompts
            .ask(
                &target.session,
                &effective.hostname,
                &user,
                "Password",
                &instructions,
                vec![AuthPrompt { prompt: "Password:".into(), echo: false }],
                Method::Password,
                vault.may_remember(target.profile.as_deref()),
            )
            .await?;
        let password =
            Zeroizing::new(answer.answers.into_iter().next().unwrap_or_default());
        let attempt = handle.authenticate_password(&user, password.as_str()).await;
        match attempt {
            Ok(AuthResult::Success) => {
                // Saved only here: after the host has said yes, and only
                // because the user asked. A cancelled dialog and a refused
                // password never reach this line, so neither can replace
                // an entry that was working.
                if answer.remember {
                    if let Some(binding) = binding.as_ref() {
                        if let Err(error) = vault.remember(binding, &password) {
                            // The connection is up and stays up. What
                            // failed is the saving, and the only wrong
                            // thing to do is let it pass in silence.
                            prompts.notice(
                                &target.session,
                                &format!("Connected, but the password was not saved: {error}"),
                            );
                        }
                    }
                }
                return Ok("password".into());
            }
            Ok(AuthResult::Failure { remaining_methods, .. }) => remaining = remaining_methods,
            Err(error) => {
                return Err(Failure::unreachable(format!("authentication failed: {error}")))
            }
        }
    }

    // Say what was actually offered. "Refused everything" with no detail is how
    // a key that is present but unusable — the wrong signature algorithm, an
    // agent that could not be reached — looks exactly like having no key.
    Err(Failure::auth(
        format!(
            "this host refused every credential afterm could offer ({}, {})",
            match offered_keys {
                0 => "no usable key file".to_string(),
                1 => "1 key file".to_string(),
                many => format!("{many} key files"),
            },
            agent_state.describe(),
        ),
        method_names(&remaining),
    ))
}

/// What happened when afterm looked for an agent, for the failure message.
enum AgentState {
    NotTried,
    /// `IdentityAgent none`.
    Disabled,
    /// Nothing answered, at the usual place or at the configured one.
    Absent(Option<String>),
    Empty,
    /// It holds keys, but none the configuration names, and `IdentitiesOnly`
    /// says those are the only ones that may be offered.
    Filtered,
    Offered(usize),
}

impl AgentState {
    fn describe(&self) -> String {
        match self {
            AgentState::NotTried => "the agent was not tried".into(),
            AgentState::Disabled => "the agent is off by IdentityAgent none".into(),
            AgentState::Absent(None) => "no agent running".into(),
            AgentState::Absent(Some(at)) => format!("no agent at {at}"),
            AgentState::Empty => "the agent holds no keys".into(),
            AgentState::Filtered => {
                "the agent holds no key named by IdentityFile, and IdentitiesOnly is set".into()
            }
            AgentState::Offered(1) => "1 agent key".into(),
            AgentState::Offered(count) => format!("{count} agent keys"),
        }
    }
}

/// The RSA signature hash to use, once `PubkeyAcceptedAlgorithms` has had its
/// say.
///
/// `wanted` is what the host asked for. If the configuration forbids that
/// signature, the next best permitted one is used instead; `None` for the whole
/// result means RSA is not permitted at all, which is a different thing from
/// `Some(None)` — the legacy `ssh-rsa` signature.
fn permitted_rsa_hash(
    policy: &config::Crypto,
    wanted: Option<russh::keys::HashAlg>,
) -> Option<Option<russh::keys::HashAlg>> {
    use russh::keys::HashAlg;
    let allows = |hash: Option<HashAlg>| {
        policy.allows_signature(Algorithm::Rsa { hash }.as_str())
    };
    if allows(wanted) {
        return Some(wanted);
    }
    [Some(HashAlg::Sha512), Some(HashAlg::Sha256), None]
        .into_iter()
        .find(|hash| allows(*hash))
}

/// Whether a key may be offered under this policy, and with which RSA hash.
///
/// The outer `None` is "the configuration does not allow this key's signature
/// algorithm"; the inner one is an RSA key signed the legacy way.
fn signable(
    policy: &config::Crypto,
    algorithm: &Algorithm,
    rsa_hash: Option<Option<russh::keys::HashAlg>>,
) -> Option<Option<russh::keys::HashAlg>> {
    if algorithm.clone().is_rsa() {
        return rsa_hash;
    }
    policy.allows_signature(algorithm.as_str()).then_some(None)
}

/// Ask the host which RSA signature algorithm it will accept.
///
/// `None` means either that the host did not say — an OpenSSH older than 7.8,
/// where the legacy signature is the right answer anyway — or that it has no
/// preference afterm can act on.
async fn rsa_hash_for(handle: &client::Handle<Verifier>) -> Option<russh::keys::HashAlg> {
    // Bounded: a host that never sends the extension must not stall the
    // handshake behind a wait for it.
    match tokio::time::timeout(Duration::from_secs(5), handle.best_supported_rsa_hash()).await {
        Ok(Ok(Some(hash))) => hash,
        _ => None,
    }
}

async fn keyboard_interactive(
    handle: &mut client::Handle<Verifier>,
    user: &str,
    target: &Target,
    prompts: &Prompts,
) -> Result<Option<()>, Failure> {
    let mut response = handle
        .authenticate_keyboard_interactive_start(user, None)
        .await
        .map_err(|error| Failure::unreachable(format!("authentication failed: {error}")))?;

    // PAM and 2FA hosts ask more than once, and not every question is a
    // password. Each round is put to the user as the host worded it.
    loop {
        match response {
            KeyboardInteractiveAuthResponse::Success => return Ok(Some(())),
            KeyboardInteractiveAuthResponse::Failure { .. } => return Ok(None),
            KeyboardInteractiveAuthResponse::InfoRequest { name, instructions, prompts: asked } => {
                let questions: Vec<AuthPrompt> = asked
                    .iter()
                    .map(|prompt| AuthPrompt {
                        prompt: prompt.prompt.clone(),
                        echo: prompt.echo,
                    })
                    .collect();
                // A host can send an information-only round with no questions.
                let answers = if questions.is_empty() {
                    Vec::new()
                } else {
                    prompts
                        .ask(
                            &target.session,
                            &target.host,
                            user,
                            &name,
                            &instructions,
                            questions,
                            Method::KeyboardInteractive,
                            false,
                        )
                        .await?
                        .answers
                };
                response = handle
                    .authenticate_keyboard_interactive_respond(answers)
                    .await
                    .map_err(|error| {
                        Failure::unreachable(format!("authentication failed: {error}"))
                    })?;
            }
        }
    }
}

async fn try_agent(
    handle: &mut client::Handle<Verifier>,
    user: &str,
    budget: usize,
    rsa_hash: Option<Option<russh::keys::HashAlg>>,
    policy: &config::Crypto,
    setting: &config::Agent,
    allowed: Option<&[PublicKey]>,
) -> Result<(Option<String>, AgentState), Failure> {
    let configured = match setting {
        config::Agent::None => return Ok((None, AgentState::Disabled)),
        config::Agent::Default => None,
        config::Agent::At(path) => Some(path.clone()),
    };
    #[cfg(windows)]
    let agent = match &configured {
        Some(path) => russh::keys::agent::client::AgentClient::connect_named_pipe(path).await,
        None => {
            russh::keys::agent::client::AgentClient::connect_named_pipe(
                r"\\.\pipe\openssh-ssh-agent",
            )
            .await
        }
    };
    #[cfg(not(windows))]
    let agent = match &configured {
        Some(path) => russh::keys::agent::client::AgentClient::connect_uds(path).await,
        None => russh::keys::agent::client::AgentClient::connect_env().await,
    };

    // No agent is an ordinary situation, not a failure — but it is worth
    // saying, because "no agent" and "the agent refused" look identical from
    // the outside and only one of them is a problem.
    let location = configured.as_ref().map(|path| path.display().to_string());
    let Ok(mut agent) = agent else {
        return Ok((None, AgentState::Absent(location)));
    };
    let Ok(identities) = agent.request_identities().await else {
        return Ok((None, AgentState::Absent(location)));
    };
    if identities.is_empty() {
        return Ok((None, AgentState::Empty));
    }
    let identities = match allowed {
        Some(allowed) => {
            let kept = agent_allowed(identities, allowed);
            if kept.is_empty() {
                return Ok((None, AgentState::Filtered));
            }
            kept
        }
        None => identities,
    };
    let mut offered = 0_usize;

    for identity in identities.into_iter().take(budget) {
        let AgentIdentity::PublicKey { key, comment } = identity else {
            // Certificates from an agent need the authority side afterm does
            // not implement yet.
            continue;
        };
        // The same signature policy the configured identities are held to.
        let Some(hash) = signable(policy, &key.algorithm(), rsa_hash) else {
            continue;
        };
        offered += 1;
        match handle
            .authenticate_publickey_with(user, key, hash, &mut agent)
            .await
        {
            Ok(AuthResult::Success) => {
                let label = if comment.is_empty() { "agent".into() } else { comment };
                return Ok((Some(format!("agent ({label})")), AgentState::Offered(offered)));
            }
            Ok(AuthResult::Failure { .. }) => continue,
            Err(_) => continue,
        }
    }
    Ok((None, AgentState::Offered(offered)))
}

/// The agent's identities that the configuration names, by key material
/// rather than by comment: the comment on a `.pub` file and the one the agent
/// carries are routinely different for the same key.
fn agent_allowed(identities: Vec<AgentIdentity>, allowed: &[PublicKey]) -> Vec<AgentIdentity> {
    identities
        .into_iter()
        .filter(|identity| match identity {
            AgentIdentity::PublicKey { key, .. } => {
                allowed.iter().any(|named| named.key_data() == key.key_data())
            }
            // A certificate is not something `IdentityFile` could have named.
            _ => false,
        })
        .collect()
}

/// The public halves of the configured identities, for matching against what
/// an agent offers. The `.pub` beside each key is read first, as OpenSSH does,
/// so an encrypted key can be matched without its passphrase; a key with no
/// `.pub` is derived from the private file if that can be read.
fn configured_public_keys(paths: &[PathBuf]) -> Vec<PublicKey> {
    paths
        .iter()
        .filter_map(|path| {
            let mut public = path.as_os_str().to_owned();
            public.push(".pub");
            if let Ok(key) = load_public_key(&public) {
                return Some(key);
            }
            load_secret_key(path, None).ok().map(|key| key.public_key().clone())
        })
        .collect()
}

/// Load one private key, asking for a passphrase if it is encrypted.
///
/// Returns `Ok(None)` when the file is missing or cannot be used, because
/// `ssh -G` lists OpenSSH's default identity paths whether or not they exist.
async fn load_identity(
    path: &PathBuf,
    prompts: &Prompts,
    target: &Target,
    interactive: bool,
) -> Result<Option<russh::keys::PrivateKey>, Failure> {
    if !path.exists() {
        return Ok(None);
    }
    match load_secret_key(path, None) {
        Ok(key) => Ok(Some(key)),
        Err(error) if is_encrypted(&error) => {
            if !interactive {
                return Ok(None);
            }
            let answer = prompts
                .ask(
                    &target.session,
                    &target.host,
                    &target.user,
                    "Key passphrase",
                    &format!("Passphrase for {}", path.display()),
                    vec![AuthPrompt { prompt: "Passphrase:".into(), echo: false }],
                    Method::KeyPassphrase,
                    false,
                )
                .await?;
            let passphrase =
                Zeroizing::new(answer.answers.into_iter().next().unwrap_or_default());
            match load_secret_key(path, Some(passphrase.as_str())) {
                Ok(key) => Ok(Some(key)),
                // A wrong passphrase is worth saying out loud; anything else
                // about this file is not worth stopping the whole attempt for.
                Err(_) => Err(Failure::new(
                    Cause::Auth,
                    format!("that passphrase did not unlock {}", path.display()),
                )),
            }
        }
        Err(_) => Ok(None),
    }
}

fn is_encrypted(error: &russh::keys::Error) -> bool {
    matches!(error, russh::keys::Error::KeyIsEncrypted)
}

fn method_names(methods: &russh::MethodSet) -> Vec<String> {
    methods
        .iter()
        .map(|method| match method {
            MethodKind::None => "none",
            MethodKind::Password => "password",
            MethodKind::PublicKey => "publickey",
            MethodKind::HostBased => "hostbased",
            MethodKind::KeyboardInteractive => "keyboard-interactive",
            MethodKind::GssapiWithMic => "gssapi-with-mic",
        })
        .map(str::to_string)
        .collect()
}

/// Turn a handshake error into something that says which of the three things
/// went wrong: the route, the host key, or the credential.
fn connect_failure(
    error: russh::Error,
    effective: &config::Effective,
    offered: &Arc<StdMutex<Option<String>>>,
) -> Failure {
    // russh reports a rejected host key as a refused connection, so the
    // verifier's own record is what distinguishes the two.
    if let Some(fingerprint) = offered.lock().unwrap().clone() {
        return Failure::host_key(format!(
            "{} presented host key {fingerprint}, which is not one of the keys \
             recorded for it. afterm offered no credential.",
            effective.hostname
        ));
    }
    Failure::unreachable(format!("could not connect to {}: {error}", effective.hostname))
}

// ------------------------------------------------------------ remote files

/// Whether anything is at `path`, a dangling symlink included.
async fn exists(sftp: &SftpSession, path: &str) -> Result<bool, Failure> {
    Ok(look(sftp, path).await?.is_some())
}

/// What is at `path`, without following a symlink there: the link itself is
/// what a replacement replaces, and what a listing shows.
async fn look(sftp: &SftpSession, path: &str) -> Result<Option<Metadata>, Failure> {
    match sftp.symlink_metadata(path.to_string()).await {
        Ok(metadata) => Ok(Some(metadata)),
        Err(error) if is_missing(&error) => Ok(None),
        Err(error) => Err(Failure::operation(format!("could not check {path}: {error}"))),
    }
}

fn is_missing(error: &russh_sftp::client::error::Error) -> bool {
    matches!(
        error,
        russh_sftp::client::error::Error::Status(status)
            if status.status_code == StatusCode::NoSuchFile
    )
}

/// Create a file that did not exist a moment ago, under `base` or the first
/// numbered variant of it that is free, and hand back its path and handle.
///
/// `mode` is carried in the create request rather than applied afterwards, so
/// the file has never existed under any other permissions.
///
/// The protocol's exclusive-create flag is what makes this atomic. Version 3
/// reports "already exists" as a generic failure, so a refused create is
/// followed by a look: if something is there now, the next name is tried; if
/// not, the failure was something else and is reported as itself.
async fn create_exclusive(
    sftp: &SftpSession,
    directory: &str,
    base: &str,
    mode: u32,
) -> Result<(String, RemoteFile), Failure> {
    let flags = OpenFlags::CREATE | OpenFlags::WRITE | OpenFlags::EXCLUDE;
    let mut attributes = Metadata::default();
    attributes.permissions = Some(mode);
    for attempt in 0..100 {
        let candidate = if attempt == 0 { base.to_string() } else { numbered(base, attempt) };
        let path = join(directory, &candidate);
        match sftp
            .open_with_flags_and_attributes(path.clone(), flags, attributes.clone())
            .await
        {
            Ok(file) => return Ok((path, file)),
            Err(error) => {
                if exists(sftp, &path).await? {
                    continue;
                }
                return Err(Failure::operation(format!("could not create {path}: {error}")));
            }
        }
    }
    Err(Failure::operation(format!("too many files called {base} in {directory}")))
}

/// What the uploaded file must end up as.
///
/// A replacement wears what it replaces, so a private file stays private and
/// an executable stays executable. Anything landing under a name nothing holds
/// takes the local file's own mode where the platform has one, which is what
/// the OpenSSH client's `put` does; on Windows, where there is no such mode,
/// it takes `NEW_FILE_MODE`.
///
/// A destination carrying setuid, setgid or the sticky bit is refused rather
/// than replaced: see `fsmodel::carried`. The rules live there because both
/// transports have to reach the same answer.
async fn intended_mode(
    sftp: &SftpSession,
    directory: &str,
    name: &str,
    collision: Collision,
    local: Option<&std::fs::Metadata>,
) -> Result<u32, Failure> {
    if collision == Collision::Replace {
        let destination = join(directory, name);
        if let Some(existing) = look(sftp, &destination).await? {
            // A regular file only. A symlink's own mode is `0777` on every
            // Unix and means nothing; carrying it onto the file that replaces
            // the link would be a widening dressed up as preservation.
            if existing.is_regular() {
                if let Some(mode) = existing.permissions {
                    return carried(mode).map_err(|bits| {
                        Failure::operation(format!(
                            "{name} has special permission bits ({bits}), which afterm will not \
                             re-apply to a replacement; change them on the host first"
                        ))
                    });
                }
            }
        }
    }
    Ok(local_mode(local))
}

/// Set the permissions of a file that is not yet at its destination.
async fn wear(sftp: &SftpSession, path: &str, mode: u32) -> Result<(), Failure> {
    let mut attributes = Metadata::default();
    attributes.permissions = Some(mode);
    sftp.set_metadata(path.to_string(), attributes).await.map_err(|error| {
        Failure::operation(format!(
            "this host would not set the permissions the file needs ({mode:04o}): {error}"
        ))
    })
}

/// Move a finished temporary to its real name, as `collision` allows.
///
/// What each choice is worth, exactly. `Fail` and `Rename` are atomic
/// no-overwrite: the protocol's own `rename` is what OpenSSH's server
/// implements with `link(2)` and a check, so it refuses a destination that
/// exists, and the refusal is reported as `Exists`. The `exists` call before
/// it is a courtesy that gives a better message; the guarantee comes from the
/// rename, so a writer that appears in between is still refused rather than
/// overwritten.
///
/// `Replace` is not atomic, and cannot be made so from here.
/// `posix-rename@openssh.com` would make it one request; russh-sftp 2.4's
/// `SftpSession` exposes no extended request to send it with and the version
/// is pinned on purpose. So the original is moved aside first, and the window
/// between the two renames is real: a connection lost inside it leaves the
/// destination missing. What is guaranteed instead is that the original is
/// never destroyed and never anonymous — it is at `.name.afterm-old`, it is
/// put back if the replacement will not go in, and if it cannot be put back
/// the failure says where it is.
async fn place(
    sftp: &SftpSession,
    part: &str,
    directory: &str,
    name: &str,
    collision: Collision,
) -> Result<String, Failure> {
    let destination = join(directory, name);
    match collision {
        Collision::Fail => {
            if exists(sftp, &destination).await? {
                return Err(Failure::exists(format!("{name} already exists in {directory}")));
            }
            move_into(sftp, part, &destination).await?;
            Ok(destination)
        }
        Collision::Replace => {
            // Nothing is let go before the replacement is installed. This
            // removed the old file first and moved the new one in second,
            // which loses both when the move is refused — and a refused move
            // is not hypothetical: a quota, a read-only remount, a server that
            // will not rename across its own boundary. The upload's own
            // cleanup then took the temporary as well, so the directory ended
            // up with neither file.
            //
            // `posix-rename@openssh.com` would make this one atomic request.
            // russh-sftp 2.4's `SftpSession` does not expose the extended
            // requests it would take to send one, and the version is pinned on
            // purpose, so the original is moved aside instead and put back if
            // anything goes wrong.
            let kept = step_aside(sftp, directory, name).await?;
            match move_into(sftp, part, &destination).await {
                Ok(()) => {
                    if let Some(kept) = kept {
                        let _ = sftp.remove_file(kept).await;
                    }
                    Ok(destination)
                }
                Err(failure) => {
                    // A rollback that fails leaves the original under a name
                    // nobody was told about, which is the same as losing it.
                    // Say where it is.
                    if let Some(kept) = kept {
                        if move_into(sftp, &kept, &destination).await.is_err() {
                            return Err(Failure::new(
                                failure.cause,
                                format!(
                                    "{}; the original {name} could not be put back and is \
                                     at {kept}",
                                    failure.message
                                ),
                            ));
                        }
                    }
                    Err(failure)
                }
            }
        }
        Collision::Rename => {
            for attempt in 0..100 {
                let candidate = if attempt == 0 { name.to_string() } else { numbered(name, attempt) };
                let path = join(directory, &candidate);
                if exists(sftp, &path).await? {
                    continue;
                }
                match move_into(sftp, part, &path).await {
                    Ok(()) => return Ok(path),
                    // Taken between the look and the move. The next number.
                    Err(failure) if failure.cause == Cause::Exists => continue,
                    Err(failure) => return Err(failure),
                }
            }
            Err(Failure::operation(format!("too many files called {name} in {directory}")))
        }
    }
}

/// Move whatever is at `directory/name` out of the way, and say where it went.
///
/// `None` means there was nothing there to move. A directory is refused rather
/// than moved: "Replace" offers to replace a file with a file, and the version
/// that removed the destination first refused a directory by accident, because
/// the protocol's `remove` will not take one. That accident was the right
/// answer and is now the deliberate one.
async fn step_aside(
    sftp: &SftpSession,
    directory: &str,
    name: &str,
) -> Result<Option<String>, Failure> {
    let destination = join(directory, name);
    let Some(metadata) = look(sftp, &destination).await? else {
        return Ok(None);
    };
    if metadata.is_dir() {
        return Err(Failure::operation(format!(
            "{name} is a directory in {directory}"
        )));
    }
    let base = format!(".{name}.afterm-old");
    for attempt in 0..100 {
        let candidate = if attempt == 0 { base.clone() } else { numbered(&base, attempt) };
        let aside = join(directory, &candidate);
        match move_into(sftp, &destination, &aside).await {
            Ok(()) => return Ok(Some(aside)),
            // Taken by an older upload that lost its connection before it
            // could tidy up. The next number.
            Err(failure) if failure.cause == Cause::Exists => continue,
            Err(failure) => return Err(failure),
        }
    }
    Err(Failure::operation(format!(
        "could not move {name} aside in {directory}"
    )))
}

/// The protocol's `rename`, which OpenSSH refuses when the target exists. A
/// refusal is reported as `Exists` when that is what it was.
async fn move_into(sftp: &SftpSession, from: &str, to: &str) -> Result<(), Failure> {
    match sftp.rename(from.to_string(), to.to_string()).await {
        Ok(()) => Ok(()),
        Err(error) => {
            if exists(sftp, to).await? {
                return Err(Failure::exists(format!("{to} already exists")));
            }
            Err(Failure::operation(format!("could not move {from} to {to}: {error}")))
        }
    }
}

// --------------------------------------------------------------- utilities

/// `Mon D HH:MM`, matching the shape the OpenSSH client prints, so a listing
/// looks the same whichever transport produced it.
///
/// Computed in UTC. The OpenSSH client formats in the client's local zone; a
/// remote file's stamp is ambiguous either way, and UTC is at least the same
/// answer on every machine.
fn format_mtime(seconds: u32) -> String {
    const MONTHS: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    let days = (seconds / 86_400) as i64;
    let rest = seconds % 86_400;
    let (_year, month, day) = civil_from_days(days);
    format!(
        "{} {} {:02}:{:02}",
        MONTHS[(month - 1) as usize],
        day,
        rest / 3600,
        (rest % 3600) / 60,
    )
}

/// Howard Hinnant's days-from-civil, inverted. Twenty lines against a date
/// dependency afterm would otherwise use for one label.
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let day_of_era = (z - era * 146_097) as u64;
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let year = year_of_era as i64 + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let mp = (5 * day_of_year + 2) / 153;
    let day = (day_of_year - (153 * mp + 2) / 5 + 1) as u32;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if month <= 2 { year + 1 } else { year }, month, day)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_a_timestamp_the_way_the_openssh_client_does() {
        // 2026-09-04 01:50:00 UTC
        assert_eq!(format_mtime(1_788_486_600), "Sep 4 01:50");
        // The epoch itself, as a boundary.
        assert_eq!(format_mtime(0), "Jan 1 00:00");
    }

    #[test]
    fn joins_paths_without_doubling_the_separator() {
        assert_eq!(join("/srv", "a.txt"), "/srv/a.txt");
        assert_eq!(join("/", "a.txt"), "/a.txt");
    }

}
