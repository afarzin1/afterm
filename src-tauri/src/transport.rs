//! What went wrong reaching a remote host, in a form the interface can act on.
//!
//! v1 handed the interface whatever string the OpenSSH client happened to write
//! and let it guess. The status bar's `needs key auth` was decided by a match
//! that could never fire, and the file pane showed a raw stderr line whether the
//! host was unreachable, refusing credentials, or simply missing the directory.
//!
//! A failure is a cause plus a message. The cause decides what the interface
//! offers — a password prompt, a retry, a named refusal — and the message says
//! what actually happened, in the host's own words wherever there are any.

use serde::{Deserialize, Serialize};

/// Why an operation against a remote host did not succeed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Cause {
    /// The host refused every credential afterm could offer it.
    Auth,
    /// Host key verification failed: unknown, changed, revoked, or a key type
    /// afterm was not willing to negotiate.
    HostKey,
    /// No usable connection: refused, timed out, unresolvable, dropped.
    Unreachable,
    /// afterm will not carry this host over the native transport, because doing
    /// so would silently differ from what the user's `ssh_config` asks for.
    Unsupported,
    /// An OpenSSH command-line tool afterm shells out to could not be run.
    ///
    /// Its own cause because it is a property of this machine rather than of
    /// the host, and everything follows from that. No other host will do
    /// better. The OpenSSH-client transport cannot stand in, because it is the
    /// same binaries. And the fix is an install, not a retry — so the session
    /// must not be pinned to a fallback that is just as absent.
    MissingClient,
    /// Connected and authenticated. The operation itself failed — a missing
    /// directory, a permission error on a file.
    Operation,
    /// Something is already at the destination, and the caller asked not to
    /// replace it. Its own cause because the interface acts on it: this is
    /// the one failure that offers a choice rather than a retry.
    Exists,
    /// afterm's own fault.
    Internal,
}

/// What to do when a transfer's destination is already taken.
///
/// Chosen by the interface, enforced here. The default everywhere is `Fail`,
/// so nothing is ever replaced without someone having said so; a download to
/// the Downloads folder uses `Rename`, the way every browser does, because
/// there is no one to ask mid-transfer and nothing there is precious.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Collision {
    /// Refuse, with `Cause::Exists`, and leave what was there untouched.
    Fail,
    /// Replace what was there once the new file is whole.
    Replace,
    /// Keep both: the new file arrives as `name (1)`, or the next free number.
    Rename,
}

impl Cause {
    /// Whether a failure with this cause means the host itself belongs on the
    /// OpenSSH-client transport from now on.
    ///
    /// Only `Unsupported` does, and the distinction is the whole reason
    /// `MissingClient` exists. The two look alike from here — the native
    /// transport did not open — but a configuration afterm will not reproduce
    /// is a fact about the host that the OpenSSH client can handle, whereas a
    /// client that will not run is a fact about the machine, and the fallback
    /// is those same binaries. Pinning for it would trade one failure for a
    /// less legible one, and outlast the install that fixes it.
    pub fn pins_to_cli(self) -> bool {
        matches!(self, Cause::Unsupported)
    }
}

/// A failure, ready to be shown or acted on.
#[derive(Debug, Clone)]
pub struct Failure {
    pub cause: Cause,
    pub message: String,
    /// Authentication methods the host said it would accept, when it said so.
    /// Empty unless `cause` is `Auth`.
    pub methods: Vec<String>,
}

/// Serialised with `canPrompt` alongside the cause, so the rule for "is there
/// any point offering this user a credential dialog" lives in one place rather
/// than being re-derived in the interface.
impl Serialize for Failure {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        use serde::ser::SerializeStruct;
        let mut out = serializer.serialize_struct("Failure", 4)?;
        out.serialize_field("cause", &self.cause)?;
        out.serialize_field("message", &self.message)?;
        out.serialize_field("methods", &self.methods)?;
        out.serialize_field("canPrompt", &self.is_interactive_auth())?;
        out.end()
    }
}

impl Failure {
    pub fn new(cause: Cause, message: impl Into<String>) -> Self {
        Self { cause, message: message.into(), methods: Vec::new() }
    }

    pub fn auth(message: impl Into<String>, methods: Vec<String>) -> Self {
        Self { cause: Cause::Auth, message: message.into(), methods }
    }

    pub fn host_key(message: impl Into<String>) -> Self {
        Self::new(Cause::HostKey, message)
    }

    pub fn unreachable(message: impl Into<String>) -> Self {
        Self::new(Cause::Unreachable, message)
    }

    pub fn unsupported(message: impl Into<String>) -> Self {
        Self::new(Cause::Unsupported, message)
    }

    pub fn missing_client(message: impl Into<String>) -> Self {
        Self::new(Cause::MissingClient, message)
    }

    /// A tool afterm shells out to could not be started.
    ///
    /// Every spawn failure lands here rather than only `NotFound`: a client
    /// that is present but cannot be executed leaves afterm exactly as unable
    /// to resolve a configuration or read a `known_hosts` as an absent one,
    /// and the interface has the same one thing to say about it.
    pub fn spawn(tool: &str, error: &std::io::Error) -> Self {
        if error.kind() == std::io::ErrorKind::NotFound {
            Self::missing_client(format!(
                "{tool} is not installed on this machine, or is not on PATH."
            ))
        } else {
            Self::missing_client(format!("could not run {tool}: {error}"))
        }
    }

    pub fn operation(message: impl Into<String>) -> Self {
        Self::new(Cause::Operation, message)
    }

    pub fn exists(message: impl Into<String>) -> Self {
        Self::new(Cause::Exists, message)
    }

    pub fn internal(message: impl Into<String>) -> Self {
        Self::new(Cause::Internal, message)
    }

    /// Whether the host would accept an interactive secret, which is what
    /// decides between offering a prompt and offering nothing.
    pub fn is_interactive_auth(&self) -> bool {
        self.cause == Cause::Auth
            && self
                .methods
                .iter()
                .any(|method| method == "password" || method == "keyboard-interactive")
    }
}

impl std::fmt::Display for Failure {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for Failure {}

/// Classify what the OpenSSH client wrote to stderr before giving up.
///
/// This is diagnosis of a finished process's complete output, not a running
/// conversation with one: the client has exited, everything it wrote is in
/// hand, and nothing depends on catching a particular line at a particular
/// moment. It is the same discipline the SFTP client is driven with.
///
/// Note the division of labour with the native transport: there, the accepted
/// methods come from the protocol itself. This text is evidence about a process
/// that has already failed, and is never the source of truth for what to send.
pub fn classify_openssh(stderr: &str) -> Failure {
    let text = stderr.trim();
    let summary = summarise(text);

    // Order matters: a changed host key also prints "Permission denied" on some
    // builds, and the host key is the more important of the two.
    if text.contains("REMOTE HOST IDENTIFICATION HAS CHANGED")
        || text.contains("Host key verification failed")
        || text.contains("key_verify failed")
    {
        return Failure::host_key(summary);
    }

    if let Some(methods) = permission_denied_methods(text) {
        return Failure::auth(summary, methods);
    }
    if text.contains("Permission denied") || text.contains("Too many authentication failures") {
        return Failure::auth(summary, Vec::new());
    }

    const UNREACHABLE: [&str; 9] = [
        "Could not resolve hostname",
        "Name or service not known",
        "Connection refused",
        "Connection timed out",
        "Operation timed out",
        "No route to host",
        "Network is unreachable",
        "Connection closed by",
        "kex_exchange_identification",
    ];
    if UNREACHABLE.iter().any(|needle| text.contains(needle)) {
        return Failure::unreachable(summary);
    }

    Failure::operation(summary)
}

/// `arya@host: Permission denied (publickey,keyboard-interactive).` — the
/// parenthesised list is what the server advertised.
fn permission_denied_methods(text: &str) -> Option<Vec<String>> {
    let start = text.find("Permission denied (")? + "Permission denied (".len();
    let rest = &text[start..];
    let end = rest.find(')')?;
    let methods: Vec<String> = rest[..end]
        .split(',')
        .map(|method| method.trim().to_string())
        .filter(|method| !method.is_empty())
        .collect();
    (!methods.is_empty()).then_some(methods)
}

/// The first few lines, without the banner noise a login prints ahead of them.
fn summarise(text: &str) -> String {
    let lines: Vec<&str> = text
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .filter(|line| !line.starts_with("Warning: Permanently added"))
        .collect();
    if lines.is_empty() {
        return "the ssh client exited without saying why".into();
    }
    lines
        .iter()
        .rev()
        .take(3)
        .rev()
        .copied()
        .collect::<Vec<_>>()
        .join("; ")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The rule that keeps a missing client from pinning a session to a
    /// transport that is just as missing. Stated over every cause, so a new
    /// one has to decide deliberately rather than inherit an answer.
    #[test]
    fn only_an_unsupported_host_pins_the_session_to_the_cli_transport() {
        assert!(Cause::Unsupported.pins_to_cli());
        for cause in [
            Cause::MissingClient,
            Cause::Auth,
            Cause::HostKey,
            Cause::Unreachable,
            Cause::Operation,
            Cause::Exists,
            Cause::Internal,
        ] {
            assert!(!cause.pins_to_cli(), "{cause:?} must not pin the session");
        }
    }

    /// An absent binary is named as an install problem, not as a verdict about
    /// the host and not as afterm's own fault.
    #[test]
    fn an_absent_binary_reads_as_a_missing_client() {
        let error = std::io::Error::new(std::io::ErrorKind::NotFound, "program not found");
        let failure = Failure::spawn("ssh.exe", &error);
        assert_eq!(failure.cause, Cause::MissingClient);
        assert!(failure.message.contains("ssh.exe"));
        assert!(failure.message.contains("not installed"));
    }

    /// A client that is there but will not run leaves afterm exactly as unable
    /// to work, so it lands on the same cause and carries the reason.
    #[test]
    fn a_client_that_will_not_run_lands_on_the_same_cause() {
        let error = std::io::Error::new(std::io::ErrorKind::PermissionDenied, "denied");
        let failure = Failure::spawn("ssh", &error);
        assert_eq!(failure.cause, Cause::MissingClient);
        assert!(failure.message.contains("denied"));
    }

    #[test]
    fn reads_the_advertised_methods_off_a_refusal() {
        let failure = classify_openssh("arya@host: Permission denied (publickey,password).");
        assert_eq!(failure.cause, Cause::Auth);
        assert_eq!(failure.methods, vec!["publickey", "password"]);
        assert!(failure.is_interactive_auth());
    }

    #[test]
    fn a_key_only_host_offers_nothing_to_prompt_for() {
        let failure = classify_openssh("arya@host: Permission denied (publickey).");
        assert_eq!(failure.cause, Cause::Auth);
        assert!(!failure.is_interactive_auth());
    }

    #[test]
    fn a_changed_host_key_is_not_reported_as_an_auth_problem() {
        let failure = classify_openssh(
            "@@@@ WARNING: REMOTE HOST IDENTIFICATION HAS CHANGED! @@@@\n\
             Host key verification failed.",
        );
        assert_eq!(failure.cause, Cause::HostKey);
    }

    #[test]
    fn distinguishes_unreachable_from_refused_credentials() {
        assert_eq!(classify_openssh("ssh: connect to host x port 22: Connection refused").cause, Cause::Unreachable);
        assert_eq!(classify_openssh("ssh: Could not resolve hostname nope: Name or service not known").cause, Cause::Unreachable);
    }

    #[test]
    fn a_missing_directory_is_an_operation_failure() {
        let failure = classify_openssh("Can't ls: \"/nope\" not found");
        assert_eq!(failure.cause, Cause::Operation);
        assert!(failure.message.contains("not found"));
    }

    #[test]
    fn silence_still_produces_something_worth_reading() {
        let failure = classify_openssh("   \n  \n");
        assert_eq!(failure.cause, Cause::Operation);
        assert!(!failure.message.is_empty());
    }

    #[test]
    fn drops_the_known_hosts_banner_that_precedes_the_real_reason() {
        let failure = classify_openssh(
            "Warning: Permanently added 'host' (ED25519) to the list of known hosts.\n\
             arya@host: Permission denied (password).",
        );
        assert!(!failure.message.contains("Permanently added"));
        assert_eq!(failure.methods, vec!["password"]);
    }
}

// ------------------------------------------------------------- the two ways

use crate::fsmodel::Listing;
use crate::metrics::Metrics;
use crate::native;
use crate::remote::{HelperState, RemoteMetrics};
use crate::secrets::Vault;
use crate::sftp::RemoteFiles;
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};

/// How long a failed attempt is remembered. Long enough that a two-second poll
/// cannot retry in a loop, short enough that a host which has come back is
/// reachable again without reopening the session.
const RETRY_AFTER: Duration = Duration::from_secs(30);

/// Both ways afterm reaches a remote host, behind one set of operations.
///
/// A closed set rather than a trait object: there are exactly two, the choice
/// is made once per session, and an enum keeps the async signatures free of
/// boxing.
pub enum Transport {
    /// The OpenSSH client, run once per operation. Permanent, not
    /// transitional: it is what carries hosts whose configuration the native
    /// transport cannot honour faithfully.
    Cli(Arc<CliTransport>),
    /// One authenticated connection, with everything as channels on it.
    Native(Arc<native::Connection>),
}

impl Transport {
    /// What to call this in the interface, so the transport in use is never a
    /// mystery. A key's path and an agent key's comment stay on the backend:
    /// the interface needs the authentication kind, not local identifying
    /// detail that makes the status bar sprawl.
    pub fn label(&self) -> String {
        match self {
            Transport::Cli(_) => "OpenSSH client".into(),
            Transport::Native(connection) => {
                format!("native · {}", authentication_label(&connection.method))
            }
        }
    }

    pub fn alive(&self) -> bool {
        match self {
            // Each operation runs its own client, so there is nothing to die.
            Transport::Cli(_) => true,
            Transport::Native(connection) => connection.alive(),
        }
    }

    pub async fn home(&self) -> Result<String, Failure> {
        match self {
            Transport::Cli(cli) => {
                let files = Arc::clone(&cli.files);
                blocking(move || files.working_directory()).await
            }
            Transport::Native(connection) => connection.home().await,
        }
    }

    pub async fn list(&self, path: &str) -> Result<Listing, Failure> {
        match self {
            Transport::Cli(cli) => {
                let files = Arc::clone(&cli.files);
                let path = path.to_string();
                blocking(move || files.list(&path)).await
            }
            Transport::Native(connection) => connection.list(path).await,
        }
    }

    /// Whether anything is at `path` on the host.
    ///
    /// A courtesy rather than a guarantee, and used as one: every operation
    /// that must not overwrite — `mkdir`, an exclusive create, a `rename` —
    /// refuses a taken name on its own, and the answer here only decides which
    /// question to put to the user about a name that is already gone.
    pub async fn exists(&self, path: &str) -> Result<bool, Failure> {
        match self {
            Transport::Cli(cli) => {
                let files = Arc::clone(&cli.files);
                let path = path.to_string();
                blocking(move || files.exists(&path)).await
            }
            Transport::Native(connection) => connection.exists(path).await,
        }
    }

    pub async fn create_directory(&self, path: &str) -> Result<(), Failure> {
        match self {
            Transport::Cli(cli) => {
                let files = Arc::clone(&cli.files);
                let path = path.to_string();
                blocking(move || files.create_directory(&path)).await
            }
            Transport::Native(connection) => connection.create_directory(path).await,
        }
    }

    /// Narrow a directory so that only its owner can list or enter it — for a
    /// folder afterm puts things in that nobody else on the host should read.
    pub async fn make_private(&self, path: &str) -> Result<(), Failure> {
        match self {
            Transport::Cli(cli) => {
                let files = Arc::clone(&cli.files);
                let path = path.to_string();
                blocking(move || files.make_private(&path)).await
            }
            Transport::Native(connection) => connection.make_private(path).await,
        }
    }

    pub async fn rename(&self, from: &str, to: &str) -> Result<(), Failure> {
        match self {
            Transport::Cli(cli) => {
                let files = Arc::clone(&cli.files);
                let from = from.to_string();
                let to = to.to_string();
                blocking(move || files.rename(&from, &to)).await
            }
            Transport::Native(connection) => connection.rename(from, to).await,
        }
    }

    pub async fn delete(&self, path: &str, directory: bool) -> Result<(), Failure> {
        match self {
            Transport::Cli(cli) => {
                let files = Arc::clone(&cli.files);
                let path = path.to_string();
                blocking(move || files.delete(&path, directory)).await
            }
            Transport::Native(connection) => connection.delete(path, directory).await,
        }
    }

    pub async fn download(
        &self,
        remote: &str,
        local: &str,
        progress: impl Fn(u64, Option<u64>) + Send + 'static,
    ) -> Result<(), Failure> {
        match self {
            Transport::Cli(cli) => {
                // A one-shot `get` reports nothing until it finishes, so there
                // is no progress to report here. The native transport has it.
                let files = Arc::clone(&cli.files);
                let (remote, local) = (remote.to_string(), local.to_string());
                blocking(move || files.download(&remote, &local)).await
            }
            Transport::Native(connection) => connection.download(remote, local, progress).await,
        }
    }

    /// Send `local` into the remote `directory` under `name`, and say where it
    /// landed — which is `name` unless `collision` was `Rename` and the name
    /// was taken. Both transports write to a temporary name first, so nothing
    /// under the real name is ever half a file.
    pub async fn upload(
        &self,
        local: &str,
        directory: &str,
        name: &str,
        collision: Collision,
        progress: impl Fn(u64, Option<u64>) + Send + 'static,
    ) -> Result<String, Failure> {
        match self {
            Transport::Cli(cli) => {
                let files = Arc::clone(&cli.files);
                let (local, directory, name) =
                    (local.to_string(), directory.to_string(), name.to_string());
                blocking(move || files.upload(&local, &directory, &name, collision)).await
            }
            Transport::Native(connection) => {
                connection.upload(local, directory, name, collision, progress).await
            }
        }
    }

    /// Run one command on the host, where there is a connection to run it on.
    ///
    /// Only the native transport has one. The OpenSSH-client transport reaches
    /// the host through separate `sftp` and `ssh` processes that authenticate
    /// separately and carry no general execution channel; standing one up would
    /// be another login and another password prompt, so the feature is reported
    /// as unavailable there rather than half-provided.
    pub async fn run(&self, command: &str) -> Result<native::Executed, Failure> {
        match self {
            Transport::Native(connection) => connection.run(command).await,
            Transport::Cli(_) => Err(Failure::operation(
                "this session reaches its host through the OpenSSH client, which afterm cannot \
                 run a command on. Use the terminal.",
            )),
        }
    }

    /// The latest telemetry, and the reason when there is none. Cheap: it reads
    /// what a background stream has already parsed, and never blocks.
    pub fn telemetry(&self) -> (Option<Metrics>, Option<String>) {
        match self {
            Transport::Cli(cli) => cli.telemetry(),
            Transport::Native(connection) => connection.telemetry(),
        }
    }

    /// Round-trip time to the host, where there is a connection to measure.
    ///
    /// Only the native transport has one: the OpenSSH path's connection lives
    /// inside `ssh.exe`, and reaching it would mean running something on the
    /// host, which measures the host rather than the link.
    pub async fn latency(&self) -> Option<f64> {
        match self {
            Transport::Native(connection) => connection.latency().await,
            Transport::Cli(_) => None,
        }
    }

    pub async fn close(&self) {
        if let Transport::Native(connection) = self {
            connection.close().await;
        }
    }
}

fn authentication_label(method: &str) -> &str {
    let name = method.split_once(" (").map_or(method, |(name, _)| name);
    match name {
        "publickey" | "agent" => "public key",
        "keyboard-interactive" => "interactive",
        _ => name,
    }
}

#[cfg(test)]
mod label_tests {
    use super::authentication_label;

    #[test]
    fn describes_key_auth_without_disclosing_which_key() {
        assert_eq!(
            authentication_label("publickey (/home/me/.ssh/id_ed25519)"),
            "public key"
        );
        assert_eq!(authentication_label("agent (work laptop)"), "public key");
    }

    #[test]
    fn keeps_the_useful_interactive_method_names() {
        assert_eq!(authentication_label("password"), "password");
        assert_eq!(authentication_label("keyboard-interactive"), "interactive");
    }
}

/// Run a blocking OpenSSH-client operation off the async runtime.
///
/// The client talks to the network with blocking reads. Calling one directly
/// from an async command would hold a runtime worker for the whole round trip,
/// which is the shape of the freeze that once took the whole application down.
pub(crate) async fn blocking<T, F>(work: F) -> Result<T, Failure>
where
    T: Send + 'static,
    F: FnOnce() -> Result<T, Failure> + Send + 'static,
{
    match tokio::task::spawn_blocking(work).await {
        Ok(result) => result,
        Err(error) => Err(Failure::internal(format!("operation did not finish: {error}"))),
    }
}

/// The OpenSSH client transport: files through `sftp`, telemetry through a
/// long-lived `ssh`.
pub struct CliTransport {
    files: Arc<RemoteFiles>,
    host: String,
    user: String,
    port: u16,
    telemetry: Mutex<Option<RemoteMetrics>>,
    telemetry_failed: Mutex<Option<(Instant, Failure)>>,
}

impl CliTransport {
    pub fn new(host: &str, user: &str, port: u16) -> Self {
        Self {
            files: Arc::new(RemoteFiles::new(host, user, port)),
            host: host.to_string(),
            user: user.to_string(),
            port,
            telemetry: Mutex::new(None),
            telemetry_failed: Mutex::new(None),
        }
    }

    /// The same transport around a client a test has already aimed somewhere.
    /// `new` builds its own `RemoteFiles`, which can only reach a host through
    /// the developer's own ssh configuration; this is how the live test drives
    /// the whole transport against a throwaway server instead.
    #[cfg(test)]
    pub(crate) fn around(files: RemoteFiles, host: &str, user: &str, port: u16) -> Self {
        Self {
            files: Arc::new(files),
            host: host.to_string(),
            user: user.to_string(),
            port,
            telemetry: Mutex::new(None),
            telemetry_failed: Mutex::new(None),
        }
    }

    /// Prove the host is reachable and the client can authenticate.
    pub fn check(&self) -> Result<(), Failure> {
        self.files.check()
    }

    fn telemetry(&self) -> (Option<Metrics>, Option<String>) {
        let mut slot = self.telemetry.lock().unwrap();

        // A helper that has exited is reported and cleared, rather than left in
        // the slot returning nothing for the life of the session.
        if let Some(helper) = slot.as_ref() {
            if let HelperState::Exited(reason) = helper.state() {
                *self.telemetry_failed.lock().unwrap() = Some((Instant::now(), reason.clone()));
                *slot = None;
                return (None, Some(reason.message));
            }
        }

        if slot.is_none() {
            if let Some((at, reason)) = self.telemetry_failed.lock().unwrap().as_ref() {
                if at.elapsed() < RETRY_AFTER {
                    return (None, Some(reason.message.clone()));
                }
            }
            match RemoteMetrics::connect(&self.host, &self.user, self.port) {
                Ok(helper) => {
                    *self.telemetry_failed.lock().unwrap() = None;
                    *slot = Some(helper);
                }
                Err(reason) => {
                    let message = reason.message.clone();
                    *self.telemetry_failed.lock().unwrap() = Some((Instant::now(), reason));
                    return (None, Some(message));
                }
            }
        }

        let helper = slot.as_ref().expect("just opened");
        match helper.sample() {
            Some(metrics) => (Some(metrics), None),
            None => (None, Some("no data yet".into())),
        }
    }
}

/// One SSH session's remote half: which transport it uses, and the single
/// attempt to establish one.
///
/// The gate is what stops the file pane and a two-second telemetry poll from
/// opening two connections, or raising two credential dialogs, for the same
/// session. Everyone waits on the same attempt.
pub struct RemoteSession {
    target: native::Target,
    current: RwLock<Option<Arc<Transport>>>,
    gate: tokio::sync::Mutex<()>,
    failed: Mutex<Option<(Instant, Failure)>>,
    /// Set once a host has been found to need the OpenSSH client, so the
    /// native transport is not retried on every navigation.
    cli_only: Mutex<Option<String>>,
    /// Where a remembered password for this session's host would come from,
    /// and go. One per process, shared by every session, because the record
    /// of a password the host has just refused has to be shared to be worth
    /// keeping.
    vault: Arc<Vault>,
}

impl RemoteSession {
    pub fn new(
        session: &str,
        profile: Option<&str>,
        host: &str,
        user: &str,
        port: u16,
        vault: Arc<Vault>,
    ) -> Self {
        Self {
            target: native::Target {
                session: session.to_string(),
                profile: profile.map(str::to_string),
                host: host.to_string(),
                user: user.to_string(),
                port,
                // The application always reads the user's own configuration.
                config_file: None,
            },
            vault,
            current: RwLock::new(None),
            gate: tokio::sync::Mutex::new(()),
            failed: Mutex::new(None),
            cli_only: Mutex::new(None),
        }
    }

    /// Where this session goes: host, user and port as the profile gave them.
    pub fn address(&self) -> (&str, &str, u16) {
        (&self.target.host, &self.target.user, self.target.port)
    }

    /// The transport, if one is already established. Never connects, never
    /// blocks.
    pub fn ready(&self) -> Option<Arc<Transport>> {
        let current = self.current.read().unwrap().clone();
        current.filter(|transport| transport.alive())
    }

    /// Why the last attempt failed, while the cooldown lasts.
    pub fn recent_failure(&self) -> Option<Failure> {
        self.failed
            .lock()
            .unwrap()
            .as_ref()
            .filter(|(at, _)| at.elapsed() < RETRY_AFTER)
            .map(|(_, reason)| reason.clone())
    }

    /// Whether an attempt is under way right now.
    pub fn connecting(&self) -> bool {
        self.gate.try_lock().is_err()
    }

    /// Establish the transport, or return the reason it cannot be.
    ///
    /// `interactive` is the difference between a click and a timer. Only a
    /// click may raise a credential dialog; a poll that would need one gets an
    /// `Auth` failure back and the interface offers a button instead.
    pub async fn obtain(
        &self,
        prompts: &native::Prompts,
        interactive: bool,
    ) -> Result<Arc<Transport>, Failure> {
        if let Some(transport) = self.ready() {
            return Ok(transport);
        }

        let _attempt = self.gate.lock().await;
        // Another caller may have finished while this one waited.
        if let Some(transport) = self.ready() {
            return Ok(transport);
        }
        // A deliberate action clears the cooldown; a timer respects it.
        if interactive {
            *self.failed.lock().unwrap() = None;
        } else if let Some(reason) = self.recent_failure() {
            return Err(reason);
        }

        let outcome = self.connect(prompts, interactive).await;
        match outcome {
            Ok(transport) => {
                let transport = Arc::new(transport);
                *self.current.write().unwrap() = Some(Arc::clone(&transport));
                *self.failed.lock().unwrap() = None;
                Ok(transport)
            }
            Err(reason) => {
                *self.failed.lock().unwrap() = Some((Instant::now(), reason.clone()));
                Err(reason)
            }
        }
    }

    async fn connect(
        &self,
        prompts: &native::Prompts,
        interactive: bool,
    ) -> Result<Transport, Failure> {
        let refused = self.cli_only.lock().unwrap().clone();
        if refused.is_none() {
            match native::Connection::open(&self.target, prompts, &self.vault, interactive).await {
                Ok(connection) => return Ok(Transport::Native(Arc::new(connection))),
                // Something about this host the native transport will not
                // pretend to support. The OpenSSH client honours it properly,
                // so the host moves there and stays there.
                Err(reason) if reason.cause.pins_to_cli() => {
                    *self.cli_only.lock().unwrap() = Some(reason.message.clone());
                }
                Err(reason) => return Err(reason),
            }
        }

        let cli = Arc::new(CliTransport::new(
            &self.target.host,
            &self.target.user,
            self.target.port,
        ));
        let probe = Arc::clone(&cli);
        blocking(move || probe.check()).await?;
        Ok(Transport::Cli(cli))
    }

    /// Why this session is on the OpenSSH client rather than the native
    /// transport, when it is.
    pub fn fallback_reason(&self) -> Option<String> {
        self.cli_only.lock().unwrap().clone()
    }

    pub async fn close(&self) {
        let transport = self.current.write().unwrap().take();
        if let Some(transport) = transport {
            transport.close().await;
        }
    }
}
