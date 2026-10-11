//! The security battery for the native transport, run entirely in process.
//!
//! An SSH server, an SFTP subsystem and afterm's own client all run inside this
//! test process on loopback. That matters more than it sounds: it makes the
//! whole battery a plain `cargo test`, which means it runs on the hosted
//! Windows agent — a machine with no `sshd` and no `sftp-server` binary, and
//! the only Windows this project ever executes on. Every regression this
//! component has had was a Windows-only one.
//!
//! The property these tests exist for is the first one: **a host that fails
//! verification is never offered a credential.** The server counts every
//! credential check it is asked to make, and a refused connection must leave
//! that count untouched.

use super::*;
use crate::secrets::Store as _;
use std::borrow::Cow;
use crate::fsmodel::EntryKind;
use crate::tempdir;
use russh::keys::PrivateKey;
use russh::server::{self, Auth, Msg, Server as _, Session};
use russh::keys::ssh_encoding::bytes::Bytes;
use russh::{Channel, ChannelId, MethodSet, Pty};
use russh_sftp::protocol::{
    Attrs, Data, File, FileAttributes, Handle, Name, OpenFlags, Status, StatusCode, Version,
};
use std::sync::atomic::{AtomicUsize, Ordering as AtomicOrdering};

/// The remote filesystem the in-process host serves: path to contents. Shared
/// with the test so it can look at what an upload actually left behind.
type Files = Arc<StdMutex<HashMap<String, Vec<u8>>>>;

/// The permission bits of each path, kept beside the contents so a test can
/// ask what mode a file ended up with. A host that models only bytes cannot
/// answer the question the permission fixes exist for.
type Modes = Arc<StdMutex<HashMap<String, u32>>>;

/// Every mode a file was *created* with, in order, so a test can prove the
/// staging file never existed under a wider one rather than only that it ended
/// up correct.
type Creations = Arc<StdMutex<Vec<(String, Option<u32>)>>>;

/// What the host has been asked to run, and everything typed at its shell.
///
/// Two separate records on purpose: the point of running a command on its own
/// channel is that nothing is typed at the shell, and only a host that watches
/// both can say so.
#[derive(Default)]
struct Watched {
    executed: StdMutex<Vec<String>>,
    typed: StdMutex<Vec<u8>>,
    /// Set by a test to make every command fail, the way a full disk or a
    /// read-only home does.
    refuse: std::sync::atomic::AtomicBool,
    /// How this host answers a request for a pseudo-terminal or a shell. Both
    /// have three answers in the protocol — yes, no, and nothing — and afterm
    /// used to treat all three as yes.
    pty: StdMutex<Answer>,
    shell: StdMutex<Answer>,
    /// Set to make the host greet before it acknowledges the pseudo-terminal,
    /// which is the ordinary race between a shell saying hello and the server
    /// getting round to answering.
    early: std::sync::atomic::AtomicBool,
    /// Set to make the host stop reading its input: the handler parks instead
    /// of consuming, so no window adjustment goes back and a writer with more
    /// than a window's worth to send is stuck until something else moves.
    stalled: std::sync::atomic::AtomicBool,
    /// Set to make the host answer each chunk with one byte instead of echoing
    /// it. See `data` for why a full echo cannot be used to drive a large
    /// write.
    terse: std::sync::atomic::AtomicBool,
}

/// What the host does when it is asked for something. The protocol allows all
/// four, and afterm used to treat every one of them as yes.
#[derive(Default, Clone, Copy)]
enum Answer {
    #[default]
    Yes,
    No,
    /// Silence: no success, no failure, and the channel left open.
    Nothing,
    /// Hangs up instead of answering.
    Hangup,
}

/// The directories the host starts with. Anything else that is not in `Files`
/// is missing until something makes it.
const DIRECTORIES: [&str; 6] = ["/", "/home/arya", "/srv", "/srv/sub", "/a", "/b"];

/// The directories the host has now. A set rather than the constant, because
/// an upload makes directories and the host has to answer for them afterwards
/// the way it answers for the ones it was born with.
type Dirs = Arc<StdMutex<std::collections::HashSet<String>>>;

fn seeded() -> (Files, Modes) {
    let mut files = HashMap::new();
    files.insert("/srv/afile.txt".to_string(), b"hello\n".to_vec());
    // One level down, so a folder download has a tree rather than a list.
    files.insert("/srv/sub/deep.txt".to_string(), b"further in\n".to_vec());
    files.insert("/srv/file with space.txt".to_string(), b"hi".to_vec());
    // The one name this host will not let an upload be moved onto; see
    // `rename`. Seeded so there is something to lose.
    files.insert("/srv/refuses.txt".to_string(), b"the original".to_vec());
    // A private file, an executable, and one carrying a setuid bit: the three
    // shapes a replacement has to get right.
    files.insert("/srv/private.txt".to_string(), b"for my eyes".to_vec());
    files.insert("/srv/run.sh".to_string(), b"#!/bin/sh\n".to_vec());
    files.insert("/srv/setuid.bin".to_string(), b"privileged".to_vec());
    // A name whose permissions this host refuses to change; see `setstat`.
    files.insert("/srv/rigid.txt".to_string(), b"unchangeable".to_vec());
    // A name whose original cannot be put back once it has been moved aside;
    // see `rename`.
    files.insert("/srv/stuck.txt".to_string(), b"the original".to_vec());

    let mut modes = HashMap::new();
    modes.insert("/srv/afile.txt".to_string(), 0o644);
    modes.insert("/srv/file with space.txt".to_string(), 0o644);
    modes.insert("/srv/refuses.txt".to_string(), 0o644);
    modes.insert("/srv/private.txt".to_string(), 0o600);
    modes.insert("/srv/run.sh".to_string(), 0o755);
    modes.insert("/srv/setuid.bin".to_string(), 0o4755);
    modes.insert("/srv/rigid.txt".to_string(), 0o644);
    modes.insert("/srv/stuck.txt".to_string(), 0o644);

    (Arc::new(StdMutex::new(files)), Arc::new(StdMutex::new(modes)))
}

const USER: &str = "arya";
const PASSWORD: &str = "hunter2";

// ------------------------------------------------------ the in-process host

/// Every credential the server was asked to check. The point of host key
/// verification is that a host failing it is never asked to check one.
#[derive(Default)]
struct Counters {
    credentials: AtomicUsize,
    publickey: AtomicUsize,
}

#[derive(Clone)]
struct TestServer {
    counters: Arc<Counters>,
    seen_pty: Arc<StdMutex<Vec<(u32, u32)>>>,
    files: Files,
    dirs: Dirs,
    modes: Modes,
    creations: Creations,
    watched: Arc<Watched>,
}

impl server::Server for TestServer {
    type Handler = TestHandler;
    fn new_client(&mut self, _: Option<std::net::SocketAddr>) -> TestHandler {
        TestHandler {
            counters: Arc::clone(&self.counters),
            seen_pty: Arc::clone(&self.seen_pty),
            files: Arc::clone(&self.files),
            dirs: Arc::clone(&self.dirs),
            modes: Arc::clone(&self.modes),
            creations: Arc::clone(&self.creations),
            watched: Arc::clone(&self.watched),
            channels: HashMap::new(),
        }
    }
}

struct TestHandler {
    counters: Arc<Counters>,
    seen_pty: Arc<StdMutex<Vec<(u32, u32)>>>,
    files: Files,
    dirs: Dirs,
    modes: Modes,
    creations: Creations,
    watched: Arc<Watched>,
    channels: HashMap<ChannelId, Channel<Msg>>,
}

impl server::Handler for TestHandler {
    type Error = russh::Error;

    async fn auth_none(&mut self, _: &str) -> Result<Auth, Self::Error> {
        Ok(Auth::Reject {
            proceed_with_methods: Some(MethodSet::from(
                &[MethodKind::PublicKey, MethodKind::Password][..],
            )),
            partial_success: false,
        })
    }

    async fn auth_password(&mut self, user: &str, password: &str) -> Result<Auth, Self::Error> {
        self.counters.credentials.fetch_add(1, AtomicOrdering::SeqCst);
        if user == USER && password == PASSWORD {
            Ok(Auth::Accept)
        } else {
            Ok(Auth::Reject { proceed_with_methods: None, partial_success: false })
        }
    }

    /// Always refuses, and keeps offering both methods — the shape of a host
    /// that takes a password and has no key of yours installed.
    async fn auth_publickey(
        &mut self,
        _: &str,
        _: &russh::keys::PublicKey,
    ) -> Result<Auth, Self::Error> {
        self.counters.credentials.fetch_add(1, AtomicOrdering::SeqCst);
        self.counters.publickey.fetch_add(1, AtomicOrdering::SeqCst);
        Ok(Auth::Reject {
            proceed_with_methods: Some(MethodSet::from(
                &[MethodKind::PublicKey, MethodKind::Password][..],
            )),
            partial_success: false,
        })
    }

    async fn channel_open_session(
        &mut self,
        channel: Channel<Msg>,
        reply: server::ChannelOpenHandle,
        _: &mut Session,
    ) -> Result<(), Self::Error> {
        reply.accept().await;
        self.channels.insert(channel.id(), channel);
        Ok(())
    }

    async fn subsystem_request(
        &mut self,
        channel_id: ChannelId,
        name: &str,
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        if name != "sftp" {
            session.channel_failure(channel_id)?;
            return Ok(());
        }
        let channel = self
            .channels
            .remove(&channel_id)
            .ok_or(russh::Error::WrongChannel)?;
        session.channel_success(channel_id)?;
        let files = Arc::clone(&self.files);
        let dirs = Arc::clone(&self.dirs);
        let modes = Arc::clone(&self.modes);
        let creations = Arc::clone(&self.creations);
        tokio::spawn(async move {
            russh_sftp::server::run(
                channel.into_stream(),
                Sftp::new(files, dirs, modes, creations),
            )
            .await;
        });
        Ok(())
    }

    /// Telemetry gets two `/proc` blocks, so the parser has a baseline and a
    /// delta to work from. Anything else is a command, and is answered the way
    /// a shell would: what it printed, and a status that says whether it
    /// worked.
    async fn exec_request(
        &mut self,
        channel: ChannelId,
        command: &[u8],
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        let command = String::from_utf8_lossy(command).into_owned();
        self.watched.executed.lock().unwrap().push(command.clone());
        // The delimiter rather than the first token: what makes a command the
        // telemetry probe is the block format it emits, and keying on the
        // opening word meant a probe that gained a variable assignment in
        // front of its loop stopped being recognised here.
        if !command.contains("===afterm===") {
            session.channel_success(channel)?;
            let (status, text) = self.answer(&command);
            if !text.is_empty() {
                session.data(channel, Bytes::from(text.into_bytes()))?;
            }
            session.exit_status_request(channel, status)?;
            session.eof(channel)?;
            session.close(channel)?;
            return Ok(());
        }
        session.channel_success(channel)?;
        // Two samples 100 busy jiffies and 200 total jiffies apart, which is
        // the 50% the test expects; the parser reports no rate until it has a
        // baseline, so one block would prove nothing.
        let block = |busy: u32, idle: u32, received: u32| {
            format!(
                "===afterm===\n\
                 12345.67 98765.43\n\
                 MemTotal:        8000000 kB\n\
                 MemAvailable:    2000000 kB\n\
                 cpu  {busy} 0 0 {idle} 0 0 0 0 0 0\n\
                   eth0: {received} 0 0 0 0 0 0 0 2000 0 0 0 0 0 0 0\n\
                 /dev/sda1  62599772  37810448  21575148  64% /\n\
                 afterm-cwd /srv/www\n"
            )
        };
        let payload = format!(
            "{}{}===afterm===\n",
            block(100, 800, 1000),
            block(200, 900, 3000)
        );
        session.data(channel, Bytes::from(payload.into_bytes()))?;
        Ok(())
    }

    async fn pty_request(
        &mut self,
        channel: ChannelId,
        _: &str,
        col_width: u32,
        row_height: u32,
        _: u32,
        _: u32,
        _: &[(Pty, u32)],
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        self.seen_pty.lock().unwrap().push((col_width, row_height));
        if self.watched.early.load(AtomicOrdering::SeqCst) {
            session.data(channel, Bytes::from_static(b"afterm-early\n"))?;
        }
        match *self.watched.pty.lock().unwrap() {
            Answer::Yes => session.channel_success(channel)?,
            Answer::No => session.channel_failure(channel)?,
            Answer::Nothing => {}
            Answer::Hangup => session.close(channel)?,
        }
        Ok(())
    }

    async fn window_change_request(
        &mut self,
        channel: ChannelId,
        col_width: u32,
        row_height: u32,
        _: u32,
        _: u32,
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        self.seen_pty.lock().unwrap().push((col_width, row_height));
        session.channel_success(channel)?;
        Ok(())
    }

    async fn shell_request(
        &mut self,
        channel: ChannelId,
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        // Let go of the `Channel` this session was opened with. Only the sftp
        // subsystem uses one; a shell is driven entirely through `session` and
        // the handler callbacks, and a `Channel` nobody reads is a hundred-deep
        // queue russh fills and then blocks the whole server session on. That
        // is a defect of this host and not of any real one — sshd consumes
        // channel data into the pty as it arrives — but it stops the server
        // adjusting the window at about 3 MB, which looks exactly like the
        // client-side deadlock the tests below are about and would let a
        // regression in afterm hide behind it.
        self.channels.remove(&channel);
        match *self.watched.shell.lock().unwrap() {
            Answer::Yes => {
                session.channel_success(channel)?;
                session.data(channel, Bytes::from_static(b"afterm-native-shell\n"))?;
            }
            Answer::No => session.channel_failure(channel)?,
            Answer::Nothing => {}
            Answer::Hangup => session.close(channel)?,
        }
        Ok(())
    }

    async fn data(
        &mut self,
        channel: ChannelId,
        data: &[u8],
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        // A host that has stopped reading. russh adjusts the window after this
        // returns, so parking here is the real thing rather than a simulation
        // of it: nothing more will be accepted on any channel of this session.
        if self.watched.stalled.load(AtomicOrdering::SeqCst) {
            std::future::pending::<()>().await;
        }
        self.watched.typed.lock().unwrap().extend_from_slice(data);
        // Ordinarily the host echoes, so a test can prove keystrokes reach the
        // remote end. `terse` answers each chunk with a single byte instead,
        // and it exists for one test: driving a multi-megabyte write against a
        // host that answers.
        //
        // A full echo cannot do that here. Both ends of this fixture are russh
        // session loops, and a russh session loop writing a large payload is
        // not reading while it does. Saturate both directions and they wedge
        // against each other at the socket rather than at the SSH window —
        // which Linux's loopback buffers are large enough to hide and
        // Windows', a fraction of the size, are not: build 246 stopped at
        // 393,216 bytes sent and 65,556 echoed, and stayed there. That is a
        // property of this host, not of afterm; a real sshd reads while it
        // writes.
        //
        // One byte per chunk keeps what the test is actually for. The original
        // deadlock was a writer that did not drain the channel while it waited
        // for window, so the client's hundred-deep receive queue filled and
        // the connection stopped processing the window adjustments the write
        // needed. What fills that queue is a count of messages, not a weight
        // of bytes — and a few hundred bytes travelling the other way fit in
        // any socket buffer there is.
        let reply = if self.watched.terse.load(AtomicOrdering::SeqCst) {
            Bytes::from_static(b".")
        } else {
            Bytes::from(data.to_vec())
        };
        session.data(channel, reply)?;
        Ok(())
    }
}

impl TestHandler {
    /// What running `command` prints, and the status it leaves behind.
    ///
    /// Only as much of a shell as the key installer needs: it recognises its
    /// own script, appends the key it carries to an in-memory
    /// `authorized_keys`, and reports the account and path the way the real one
    /// does. `refuse` makes every command fail, which is the shape of a
    /// read-only home or a full disk.
    fn answer(&self, command: &str) -> (u32, String) {
        if self.watched.refuse.load(AtomicOrdering::SeqCst) {
            return (1, String::new());
        }
        if command.contains("afterm-flood") {
            return (0, "x".repeat(EXEC_CAPTURE * 2));
        }
        if !command.contains("afterm-user") {
            return (0, String::new());
        }
        let Some(encoded) = command
            .split_once("printf '%s' '")
            .and_then(|(_, rest)| rest.split_once('\''))
            .map(|(value, _)| value.to_string())
        else {
            return (1, String::new());
        };
        use base64::{engine::general_purpose::STANDARD, Engine as _};
        let Ok(key) = STANDARD.decode(encoded) else {
            return (1, String::new());
        };
        let path = format!("/home/{USER}/.ssh/authorized_keys");
        let mut files = self.files.lock().unwrap();
        let file = files.entry(path.clone()).or_default();
        let present = file
            .split(|byte| *byte == b'\n')
            .any(|line| line == key.as_slice());
        if !present {
            file.extend_from_slice(&key);
            file.push(b'\n');
        }
        (
            0,
            format!(
                "afterm-user {USER}\nafterm-path {path}\nafterm-present {}\n",
                if present { "yes" } else { "no" }
            ),
        )
    }
}

/// A directory with the shapes that have caused trouble before: a name with
/// spaces, a symlink, a directory that must sort first, and a size that must
/// survive — and, behind the listing, an in-memory filesystem that behaves the
/// way OpenSSH's `sftp-server` does where it matters: an exclusive create of
/// a taken name fails with the generic status, and a `rename` onto a taken
/// name is refused.
struct Sftp {
    drained: HashMap<String, bool>,
    files: Files,
    dirs: Dirs,
    modes: Modes,
    creations: Creations,
    /// Open handles, by the id handed to the client, to the path each is for.
    handles: HashMap<String, String>,
    next: u32,
}

impl Sftp {
    fn new(files: Files, dirs: Dirs, modes: Modes, creations: Creations) -> Self {
        Self {
            drained: HashMap::new(),
            files,
            dirs,
            modes,
            creations,
            handles: HashMap::new(),
            next: 0,
        }
    }

    fn is_directory(&self, path: &str) -> bool {
        self.dirs.lock().unwrap().contains(path)
    }

    fn attributes(&self, path: &str) -> Option<FileAttributes> {
        if self.is_directory(path) {
            let mut directory = FileAttributes::default();
            directory.set_dir(true);
            return Some(directory);
        }
        let size = self.files.lock().unwrap().get(path)?.len() as u64;
        let mut regular = FileAttributes::default();
        regular.set_regular(true);
        regular.size = Some(size);
        // The mode belongs in the attributes the same way the size does: a
        // client that has to preserve permissions can only read them here.
        let mode = self.modes.lock().unwrap().get(path).copied().unwrap_or(0o644);
        regular.permissions = Some(regular.permissions.unwrap_or(0) | mode);
        Some(regular)
    }

    fn path_of(&self, handle: &str) -> Result<String, StatusCode> {
        self.handles.get(handle).cloned().ok_or(StatusCode::Failure)
    }
}

fn ok(id: u32) -> Status {
    Status {
        id,
        status_code: StatusCode::Ok,
        error_message: "Ok".into(),
        language_tag: "en-US".into(),
    }
}

impl russh_sftp::server::Handler for Sftp {
    type Error = StatusCode;

    fn unimplemented(&self) -> Self::Error {
        StatusCode::OpUnsupported
    }

    async fn open(
        &mut self,
        id: u32,
        filename: String,
        pflags: OpenFlags,
        attrs: FileAttributes,
    ) -> Result<Handle, Self::Error> {
        {
            let mut files = self.files.lock().unwrap();
            let exists = files.contains_key(&filename) || self.is_directory(&filename);
            if pflags.contains(OpenFlags::CREATE) {
                // Version 3 has no "already exists": OpenSSH maps EEXIST to
                // the generic failure, and so does this.
                if exists && pflags.contains(OpenFlags::EXCLUDE) {
                    return Err(StatusCode::Failure);
                }
                if !exists || pflags.contains(OpenFlags::TRUNCATE) {
                    files.insert(filename.clone(), Vec::new());
                    // OpenSSH's server passes the requested permissions to
                    // `open(2)`, so the file has never had any other mode.
                    let requested = attrs.permissions.map(|bits| bits & 0o7777);
                    self.creations
                        .lock()
                        .unwrap()
                        .push((filename.clone(), requested));
                    self.modes
                        .lock()
                        .unwrap()
                        .insert(filename.clone(), requested.unwrap_or(0o644));
                }
            } else if !exists {
                return Err(StatusCode::NoSuchFile);
            }
        }
        let handle = format!("h{}", self.next);
        self.next += 1;
        self.handles.insert(handle.clone(), filename);
        Ok(Handle { id, handle })
    }

    async fn read(
        &mut self,
        id: u32,
        handle: String,
        offset: u64,
        len: u32,
    ) -> Result<Data, Self::Error> {
        let path = self.path_of(&handle)?;
        let files = self.files.lock().unwrap();
        let file = files.get(&path).ok_or(StatusCode::NoSuchFile)?;
        let start = offset as usize;
        if start >= file.len() {
            return Err(StatusCode::Eof);
        }
        let end = (start + len as usize).min(file.len());
        Ok(Data { id, data: file[start..end].to_vec() })
    }

    /// A path with `poison` in its name accepts the open and refuses every
    /// write, which is what a disk filling up looks like from the client.
    async fn write(
        &mut self,
        id: u32,
        handle: String,
        offset: u64,
        data: Vec<u8>,
    ) -> Result<Status, Self::Error> {
        let path = self.path_of(&handle)?;
        if path.contains("poison") {
            return Err(StatusCode::Failure);
        }
        let mut files = self.files.lock().unwrap();
        let file = files.get_mut(&path).ok_or(StatusCode::NoSuchFile)?;
        let start = offset as usize;
        let end = start + data.len();
        if file.len() < end {
            file.resize(end, 0);
        }
        file[start..end].copy_from_slice(&data);
        Ok(ok(id))
    }

    /// Change a file's permissions. A path with `rigid` in its name refuses,
    /// which is what a server that will not let the connected user chmod a
    /// file looks like from the client.
    async fn setstat(
        &mut self,
        id: u32,
        path: String,
        attrs: FileAttributes,
    ) -> Result<Status, Self::Error> {
        if path.contains("rigid") {
            return Err(StatusCode::PermissionDenied);
        }
        if !self.files.lock().unwrap().contains_key(&path) {
            return Err(StatusCode::NoSuchFile);
        }
        if let Some(bits) = attrs.permissions {
            self.modes.lock().unwrap().insert(path, bits & 0o7777);
        }
        Ok(ok(id))
    }

    async fn fsetstat(
        &mut self,
        id: u32,
        handle: String,
        attrs: FileAttributes,
    ) -> Result<Status, Self::Error> {
        let path = self.path_of(&handle)?;
        self.setstat(id, path, attrs).await
    }

    async fn stat(&mut self, id: u32, path: String) -> Result<Attrs, Self::Error> {
        self.attributes(&path).map(|attrs| Attrs { id, attrs }).ok_or(StatusCode::NoSuchFile)
    }

    async fn lstat(&mut self, id: u32, path: String) -> Result<Attrs, Self::Error> {
        self.stat(id, path).await
    }

    async fn fstat(&mut self, id: u32, handle: String) -> Result<Attrs, Self::Error> {
        let path = self.path_of(&handle)?;
        self.stat(id, path).await
    }

    /// The legacy request, as OpenSSH serves it: refused when anything is
    /// already at the new name.
    async fn rename(
        &mut self,
        id: u32,
        oldpath: String,
        newpath: String,
    ) -> Result<Status, Self::Error> {
        let mut files = self.files.lock().unwrap();
        if files.contains_key(&newpath) || self.dirs.lock().unwrap().contains(&newpath) {
            return Err(StatusCode::Failure);
        }
        // A name a finished upload cannot be moved onto, though everything
        // else about it works. That is what a quota, a read-only remount or a
        // per-name permission looks like from the client, and it is the only
        // way to reach the moment when a replacement has to be undone.
        if newpath.contains("refuses") && oldpath.contains("afterm-part") {
            return Err(StatusCode::Failure);
        }
        // And a name where the undo fails too, which is where the original
        // ends up under a name nobody was told about unless afterm says so.
        // Moving it aside is allowed; moving anything back onto it is not.
        if newpath.contains("stuck")
            && (oldpath.contains("afterm-part") || oldpath.contains("afterm-old"))
        {
            return Err(StatusCode::Failure);
        }
        let data = files.remove(&oldpath).ok_or(StatusCode::NoSuchFile)?;
        files.insert(newpath.clone(), data);
        let mut modes = self.modes.lock().unwrap();
        if let Some(mode) = modes.remove(&oldpath) {
            modes.insert(newpath, mode);
        }
        Ok(ok(id))
    }

    async fn remove(&mut self, id: u32, filename: String) -> Result<Status, Self::Error> {
        self.modes.lock().unwrap().remove(&filename);
        self.files
            .lock()
            .unwrap()
            .remove(&filename)
            .map(|_| ok(id))
            .ok_or(StatusCode::NoSuchFile)
    }

    /// Make a directory, refusing a name anything is already at.
    ///
    /// Version 3 has no "already exists" status, so the refusal is the generic
    /// `Failure` — which is what a real server sends, and what the folder
    /// upload has to tell apart from a refusal about anything else by looking
    /// at the name afterwards.
    async fn mkdir(
        &mut self,
        id: u32,
        path: String,
        _: FileAttributes,
    ) -> Result<Status, Self::Error> {
        if self.files.lock().unwrap().contains_key(&path) || self.is_directory(&path) {
            return Err(StatusCode::Failure);
        }
        self.dirs.lock().unwrap().insert(path);
        Ok(ok(id))
    }

    async fn init(&mut self, _: u32, _: HashMap<String, String>) -> Result<Version, Self::Error> {
        Ok(Version::new())
    }

    async fn realpath(&mut self, id: u32, path: String) -> Result<Name, Self::Error> {
        Ok(Name {
            id,
            files: vec![File::dummy(if path == "." { "/home/arya" } else { &path })],
        })
    }

    async fn opendir(&mut self, id: u32, path: String) -> Result<Handle, Self::Error> {
        if path == "/nope" {
            return Err(StatusCode::NoSuchFile);
        }
        self.drained.insert(path.clone(), false);
        Ok(Handle { id, handle: path })
    }

    async fn readdir(&mut self, id: u32, handle: String) -> Result<Name, Self::Error> {
        // The second call must report EOF or the client loops forever.
        if self.drained.get(&handle).copied().unwrap_or(true) {
            return Err(StatusCode::Eof);
        }
        self.drained.insert(handle.clone(), true);

        let mut directory = FileAttributes::default();
        directory.set_dir(true);
        directory.size = Some(4096);
        let mut regular = FileAttributes::default();
        regular.set_regular(true);
        regular.size = Some(6);
        regular.mtime = Some(1_788_486_600);
        let mut spaced = FileAttributes::default();
        spaced.set_regular(true);
        spaced.size = Some(2);
        let mut link = FileAttributes::default();
        link.set_symlink(true);
        let mut deep = FileAttributes::default();
        deep.set_regular(true);
        deep.size = Some(11);

        // Per directory, not the same answer to every handle. A host that
        // served `sub` inside `sub` inside `sub` would be an endless tree to
        // anything that walks one, which is a thing afterm now does.
        if handle == "/srv/sub" {
            return Ok(Name {
                id,
                files: vec![File {
                    filename: "deep.txt".into(),
                    longname: String::new(),
                    attrs: deep,
                }],
            });
        }

        Ok(Name {
            id,
            files: vec![
                File { filename: "afile.txt".into(), longname: String::new(), attrs: regular },
                File { filename: "file with space.txt".into(), longname: String::new(), attrs: spaced },
                File { filename: "alink".into(), longname: String::new(), attrs: link },
                File { filename: "sub".into(), longname: String::new(), attrs: directory },
            ],
        })
    }

    async fn close(&mut self, id: u32, handle: String) -> Result<Status, Self::Error> {
        self.handles.remove(&handle);
        Ok(ok(id))
    }
}

// ------------------------------------------------------------- the harness

struct Host {
    port: u16,
    key: PublicKey,
    counters: Arc<Counters>,
    seen_pty: Arc<StdMutex<Vec<(u32, u32)>>>,
    files: Files,
    dirs: Dirs,
    modes: Modes,
    creations: Creations,
    watched: Arc<Watched>,
}

impl Host {
    /// What the host has at `path`, if anything.
    fn file(&self, path: &str) -> Option<Vec<u8>> {
        self.files.lock().unwrap().get(path).cloned()
    }

    /// Whether the host has a directory at `path`.
    fn directory(&self, path: &str) -> bool {
        self.dirs.lock().unwrap().contains(path)
    }

    /// The permission bits at `path`, if anything is there.
    fn mode(&self, path: &str) -> Option<u32> {
        self.modes.lock().unwrap().get(path).copied()
    }

    /// The mode a path was created with, and whether one was asked for at all.
    fn created_as(&self, needle: &str) -> Vec<Option<u32>> {
        self.creations
            .lock()
            .unwrap()
            .iter()
            .filter(|(path, _)| path.contains(needle))
            .map(|(_, mode)| *mode)
            .collect()
    }

    /// Every path on the host with `needle` in it. The way a test asks
    /// whether a temporary was left behind.
    fn paths_containing(&self, needle: &str) -> Vec<String> {
        let mut found: Vec<String> = self
            .files
            .lock()
            .unwrap()
            .keys()
            .filter(|path| path.contains(needle))
            .cloned()
            .collect();
        found.sort();
        found
    }
}

async fn start_host() -> Host {
    let key = PrivateKey::random(&mut rand::rng(), russh::keys::Algorithm::Ed25519).unwrap();
    let public = key.public_key().clone();
    let config = Arc::new(server::Config { keys: vec![key], ..Default::default() });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let counters = Arc::new(Counters::default());
    let seen_pty = Arc::new(StdMutex::new(Vec::new()));
    let (files, modes) = seeded();
    let dirs: Dirs = Arc::new(StdMutex::new(DIRECTORIES.iter().map(|d| d.to_string()).collect()));
    let creations: Creations = Arc::new(StdMutex::new(Vec::new()));
    let watched: Arc<Watched> = Arc::new(Watched::default());
    let mut server = TestServer {
        counters: Arc::clone(&counters),
        seen_pty: Arc::clone(&seen_pty),
        files: Arc::clone(&files),
        dirs: Arc::clone(&dirs),
        modes: Arc::clone(&modes),
        creations: Arc::clone(&creations),
        watched: Arc::clone(&watched),
    };
    tokio::spawn(async move {
        let _ = server.run_on_socket(config, &listener).await;
    });
    Host { port, key: public, counters, seen_pty, files, dirs, modes, creations, watched }
}

/// Connect exactly as `Connection::open` does — the same `Verifier`, the same
/// refusal behaviour — with the trusted set the test chooses.
async fn connect(host: &Host, trusted: trust::Trusted) -> Result<client::Handle<Verifier>, Failure> {
    let config = Arc::new(client::Config {
        preferred: russh::Preferred {
            key: Cow::Owned(if trusted.algorithms.is_empty() {
                vec![russh::keys::Algorithm::Ed25519]
            } else {
                trusted.algorithms.clone()
            }),
            ..Default::default()
        },
        ..Default::default()
    });
    let offered = Arc::new(StdMutex::new(None));
    let verifier = Verifier { trusted, offered: Arc::clone(&offered) };
    client::connect(config, ("127.0.0.1", host.port), verifier)
        .await
        .map_err(|error| {
            if offered.lock().unwrap().is_some() {
                Failure::host_key(format!("refused: {error}"))
            } else {
                Failure::unreachable(error.to_string())
            }
        })
}

fn trusting(key: &PublicKey) -> trust::Trusted {
    trust::Trusted {
        keys: vec![key.clone()],
        algorithms: vec![key.algorithm()],
    }
}

/// A connection wrapped in the type the rest of afterm uses, so the tests
/// exercise the real listing and telemetry code rather than a copy of it.
fn wrap(handle: client::Handle<Verifier>) -> Connection {
    Connection {
        handle,
        sftp: Mutex::new(None),
        latest: Arc::new(StdMutex::new(None)),
        telemetry_note: Arc::new(StdMutex::new(None)),
        method: "password".into(),
    }
}

async fn authenticated(host: &Host) -> Connection {
    let mut handle = connect(host, trusting(&host.key)).await.expect("connect");
    let result = handle.authenticate_password(USER, PASSWORD).await.expect("auth");
    assert!(result.success(), "the test host should accept the test password");
    wrap(handle)
}

// ------------------------------------------------------------------ tests

#[tokio::test]
async fn a_matching_host_key_is_accepted_and_carries_everything() {
    let host = start_host().await;
    let connection = authenticated(&host).await;
    assert!(connection.alive());
    assert_eq!(connection.home().await.unwrap(), "/home/arya");
}

/// The property the whole design rests on. A host that fails verification is
/// refused before authentication begins, so it never sees a credential.
#[tokio::test]
async fn a_changed_host_key_is_refused_and_no_credential_is_offered() {
    let host = start_host().await;
    let other = PrivateKey::random(&mut rand::rng(), russh::keys::Algorithm::Ed25519)
        .unwrap()
        .public_key()
        .clone();

    let attempt = connect(&host, trusting(&other)).await;
    let failure = attempt.err().expect("a changed host key must abort the connection");
    assert_eq!(failure.cause, Cause::HostKey);
    assert_eq!(
        host.counters.credentials.load(AtomicOrdering::SeqCst),
        0,
        "the host must not have been asked to check any credential"
    );
}

#[tokio::test]
async fn an_unknown_host_key_is_refused_and_no_credential_is_offered() {
    let host = start_host().await;
    let attempt = connect(&host, trust::Trusted::default()).await;
    let failure = attempt.err().expect("an unknown host key must abort the connection");
    assert_eq!(failure.cause, Cause::HostKey);
    assert_eq!(host.counters.credentials.load(AtomicOrdering::SeqCst), 0);
}

/// russh's own `known_hosts` reader reports a key of an unexpected algorithm as
/// an unknown host. afterm pins negotiation to the algorithms actually
/// recorded, so this cannot silently pass.
#[tokio::test]
async fn a_key_of_the_wrong_algorithm_does_not_slip_through() {
    let host = start_host().await;
    let rsa = russh::keys::parse_public_key_base64(
        "AAAAB3NzaC1yc2EAAAADAQABAAABAQCI4Bw7W4uh4QgEM/y2NJdLfs/XePmjyaH52PgaQqLnYYc1Csw5\
         kmxbfp82CCz8dpYPijomgf2DTfiLeyCUkL4pyg9KHuHVTgIxlRzMLeHQ4gzqZ6hUOi7+dzulhLfLhrNC\
         MbTQHoG8VztTM/oQJHc6q84HzopwPkgqNVKDrLN/7DckPeiqA9IO31ddnC4E5WgIi3CWlS0ypIJZqlf3\
         gxtBP+8BEtKEA0nw2vf9MF+ZQ/BYP0w/5dkzNXge4LrB2Izm1J0rZtrCiB3zxF3KpDiolvHkdtIqdxrF\
         5wPtMzf2A1ph0djOjqdUu1kH2L87lwS+y0kJ9xQ8N0lc2ljn96yP",
    )
    .unwrap();
    // Only an RSA key is recorded, and the host serves ed25519.
    let attempt = connect(&host, trusting(&rsa)).await;
    assert!(attempt.is_err());
    assert_eq!(host.counters.credentials.load(AtomicOrdering::SeqCst), 0);
}

#[tokio::test]
async fn a_wrong_password_fails_without_taking_the_connection_down() {
    let host = start_host().await;
    let mut handle = connect(&host, trusting(&host.key)).await.expect("connect");
    let result = handle.authenticate_password(USER, "wrong").await.unwrap();
    assert!(!result.success());
    assert_eq!(host.counters.credentials.load(AtomicOrdering::SeqCst), 1);
}

/// The server's advertised methods come from the protocol, not from parsing
/// what a client wrote to stderr.
#[tokio::test]
async fn the_server_reports_which_methods_it_will_accept() {
    let host = start_host().await;
    let mut handle = connect(&host, trusting(&host.key)).await.expect("connect");
    let result = handle.authenticate_none(USER).await.unwrap();
    let AuthResult::Failure { remaining_methods, .. } = result else {
        panic!("the test host does not accept `none`");
    };
    assert_eq!(method_names(&remaining_methods), vec!["publickey", "password"]);
}

#[tokio::test]
async fn a_listing_has_directories_first_sizes_symlinks_and_spaced_names() {
    let host = start_host().await;
    let connection = authenticated(&host).await;
    let listing = connection.list("/srv").await.expect("list");

    let names: Vec<&str> = listing.entries.iter().map(|e| e.name.as_str()).collect();
    assert_eq!(names, vec!["sub", "afile.txt", "alink", "file with space.txt"]);

    assert_eq!(listing.entries[0].kind, EntryKind::Directory);
    // A directory's 4096 is the inode size, not content, so it is not shown.
    assert_eq!(listing.entries[0].size, None);

    let file = listing.entries.iter().find(|e| e.name == "afile.txt").unwrap();
    assert_eq!(file.size, Some(6));
    assert_eq!(file.path, "/srv/afile.txt");
    assert_eq!(file.modified.as_deref(), Some("Sep 4 01:50"));

    let link = listing.entries.iter().find(|e| e.name == "alink").unwrap();
    assert_eq!(link.kind, EntryKind::Link);

    assert_eq!(listing.parent.as_deref(), Some("/"));
}

#[tokio::test]
async fn a_missing_directory_is_an_error_not_an_empty_listing() {
    let host = start_host().await;
    let connection = authenticated(&host).await;
    let failure = connection.list("/nope").await.expect_err("a missing path must fail");
    assert_eq!(failure.cause, Cause::Operation);
}

/// The v1 subprocess implementation could interleave two listings. One SFTP
/// session multiplexes by request id, and this keeps it honest.
#[tokio::test]
async fn concurrent_listings_do_not_interleave() {
    let host = start_host().await;
    let connection = authenticated(&host).await;
    let (first, second, third) = tokio::join!(
        connection.list("/a"),
        connection.list("/b"),
        connection.list("/c"),
    );
    for (listing, expected) in [(first, "/a"), (second, "/b"), (third, "/c")] {
        let listing = listing.expect("listing");
        assert_eq!(listing.path, expected);
        assert_eq!(listing.entries.len(), 4);
    }
}

/// Telemetry rides the same connection: no second login, and the parser is the
/// one the OpenSSH helper uses.
#[tokio::test]
async fn telemetry_arrives_on_a_channel_of_the_same_connection() {
    let host = start_host().await;
    let connection = authenticated(&host).await;
    let latest = Arc::clone(&connection.latest);
    let note = Arc::clone(&connection.telemetry_note);
    connection.start_telemetry(latest, note).await;

    let metrics = wait_for(|| connection.telemetry().0).await.expect("a telemetry sample");
    assert_eq!(metrics.total_memory, Some(8_000_000 * 1024));
    assert_eq!(metrics.disk_total, Some(62_599_772 * 1024));
    // Two blocks, so the second one can report a rate.
    assert_eq!(metrics.cpu, Some(50.0));
    // The file pane follows this; it rides the same block as the metrics.
    assert_eq!(metrics.directory.as_deref(), Some("/srv/www"));
}

#[tokio::test]
async fn a_shell_channel_carries_input_output_and_resize() {
    let host = start_host().await;
    let connection = authenticated(&host).await;
    let output: Arc<StdMutex<Vec<u8>>> = Arc::new(StdMutex::new(Vec::new()));
    let sink = Arc::clone(&output);
    let ended = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let closed = Arc::clone(&ended);
    // What `on_close` was told, which is what decides between closing the tab
    // and offering to reconnect.
    let deliberate = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let reported = Arc::clone(&deliberate);

    let shell = connection
        .open_shell(
            move |data| sink.lock().unwrap().extend_from_slice(data),
            move |exited| {
                reported.store(exited, AtomicOrdering::SeqCst);
                closed.store(true, AtomicOrdering::SeqCst);
            },
            80,
            24,
            Arc::new(crate::flow::FlowGate::new()),
        )
        .await
        .expect("shell");

    // The shell greeting proves output reaches the sink.
    wait_for(|| {
        let seen = output.lock().unwrap().clone();
        String::from_utf8_lossy(&seen).contains("afterm-native-shell").then_some(())
    })
    .await
    .expect("shell output");

    shell.write("hello\n").expect("write");
    wait_for(|| {
        let seen = output.lock().unwrap().clone();
        String::from_utf8_lossy(&seen).contains("hello").then_some(())
    })
    .await
    .expect("the remote end to echo what was typed");

    shell.resize(120, 40);

    let sizes = wait_for(|| {
        let seen = host.seen_pty.lock().unwrap().clone();
        (seen.len() >= 2).then_some(seen)
    })
    .await
    .expect("a pty request and a window change");
    assert_eq!(sizes[0], (80, 24));
    assert_eq!(sizes[1], (120, 40));

    // Closing the channel ends the pump, which is what tears a session down.
    shell.close();
    wait_for(|| ended.load(AtomicOrdering::SeqCst).then_some(())).await.expect("the shell to end");
    assert!(
        deliberate.load(AtomicOrdering::SeqCst),
        "closing the channel ourselves is a deliberate end, not a lost connection"
    );
}

/// The deadlock the split reader and writer exist for.
///
/// A paste six times the SSH window, at a host that answers while it is being
/// written to. A single task awaiting the write in the same place it drains
/// the channel cannot finish it: the write parks waiting for window, the
/// hundred-deep receive queue fills because nothing is draining it, the
/// connection's event loop blocks pushing into that queue, and the window
/// adjustment the write needs is behind the block. The reported reproduction
/// stopped at 3,178,496 bytes with the 20-byte banner on screen; against the
/// single-task version this test stops around 4 MB and never moves again.
///
/// The host answers one byte per chunk rather than echoing — see `data`, where
/// what that is for and what it is not are written down. What fills the queue
/// is the count of replies, and there are hundreds of them.
#[tokio::test]
async fn a_paste_larger_than_the_window_finishes_while_output_keeps_draining() {
    const PASTE: usize = 12 * 1024 * 1024;
    /// Long enough that only a stall fails this, rather than a slow agent, and
    /// no longer: a wedge pays the whole of it before the build says so.
    const PATIENCE: Duration = Duration::from_secs(60);

    let host = start_host().await;
    let connection = authenticated(&host).await;
    let seen = Arc::new(AtomicUsize::new(0));
    let counting = Arc::clone(&seen);

    let shell = connection
        .open_shell(
            move |data| {
                counting.fetch_add(data.len(), AtomicOrdering::SeqCst);
            },
            |_| {},
            80,
            24,
            Arc::new(crate::flow::FlowGate::new()),
        )
        .await
        .expect("shell");
    host.watched.terse.store(true, AtomicOrdering::SeqCst);

    // One write, because one paste is one thing the user did. The backlog
    // rule has to let it through whole or the fix is no fix.
    shell.write(&"x".repeat(PASTE)).expect("a paste is not refused for its size");

    // How far it got is reported rather than only whether it arrived: a
    // deadlock stops at a repeatable figure and a slow machine does not, and
    // that is the only way to tell them apart from a build log.
    let progress = || {
        format!(
            "{} of {PASTE} sent, {} answers back",
            host.watched.typed.lock().unwrap().len(),
            seen.load(AtomicOrdering::SeqCst)
        )
    };
    let received = wait_within(PATIENCE, || {
        let typed = host.watched.typed.lock().unwrap().len();
        (typed >= PASTE).then_some(typed)
    })
    .await;
    assert!(received.is_some(), "the host never received the whole paste: {}", progress());
    // And the answers kept coming the other way at the same time, which is the
    // half of the deadlock that used to stop first. Well past the hundred the
    // queue holds, because a hundred is where a reader that has stopped
    // draining brings the whole connection down.
    let answered = seen.load(AtomicOrdering::SeqCst);
    assert!(answered > 300, "output stopped draining: {}", progress());
}

/// Closing a tab does not wait for a write that cannot finish.
///
/// The host stops reading, so the writer is parked on the send window with
/// megabytes still to go. Closing has to overtake it, which is why the close
/// signal is not a message on the same queue the write is at the front of.
#[tokio::test]
async fn closing_during_a_blocked_write_does_not_wait_for_the_write() {
    let host = start_host().await;
    let connection = authenticated(&host).await;
    let ended = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let closed = Arc::clone(&ended);
    let deliberate = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let reported = Arc::clone(&deliberate);

    let shell = connection
        .open_shell(
            |_| {},
            move |exited| {
                reported.store(exited, AtomicOrdering::SeqCst);
                closed.store(true, AtomicOrdering::SeqCst);
            },
            80,
            24,
            Arc::new(crate::flow::FlowGate::new()),
        )
        .await
        .expect("shell");

    host.watched.stalled.store(true, AtomicOrdering::SeqCst);
    // Comfortably more than the 2 MiB window, so the writer cannot get to the
    // end of it without the host reading — which the host will not do.
    shell.write(&"x".repeat(8 * 1024 * 1024)).expect("write");
    // Long enough for the window to be spent and the writer to be parked.
    tokio::time::sleep(Duration::from_millis(200)).await;

    let started = Instant::now();
    shell.close();
    // The property is that closing does not wait on the write, and the write
    // can never finish — the host will not read another byte for the rest of
    // this test — so any finite answer is the whole of it. The bound is
    // generous on purpose: on a hosted agent a tight one measures the agent.
    let ending = wait_within(Duration::from_secs(30), || {
        ended.load(AtomicOrdering::SeqCst).then_some(())
    })
    .await;
    assert!(ending.is_some(), "the shell never ended: still waiting after {:?}", started.elapsed());
    assert!(
        deliberate.load(AtomicOrdering::SeqCst),
        "closing the tab is a deliberate end even out of a stuck write"
    );
}

/// Input is queued, not hoarded. One write of any size is taken, because a
/// paste is one thing somebody did; a backlog past the limit is refused, and
/// says so rather than being dropped on the floor.
#[test]
fn input_past_the_backlog_is_refused_rather_than_held() {
    let (shell, _waiting) = ShellChannel::stalled();
    // Nothing is draining `_waiting`, so everything written stays queued.
    shell.write(&"x".repeat(INPUT_BACKLOG)).expect("the first write is always taken");
    let refused = shell.write("more").expect_err("a full queue must refuse");
    assert!(refused.message.contains("not reading its input"), "{refused}");
    // A single write larger than the whole limit still goes, on an empty
    // queue: refusing half a paste would be worse than refusing all of it.
    let (fresh, _also_waiting) = ShellChannel::stalled();
    assert!(fresh.write(&"y".repeat(INPUT_BACKLOG * 2)).is_ok());
}

/// A host that refuses the pseudo-terminal is a failure, not a blank terminal.
///
/// `request_pty` only submits the request; the answer is a separate channel
/// message and the receive loop ignored the failure variant, so this host
/// produced a session afterm called connected and a tab that never showed
/// anything.
#[tokio::test]
async fn a_refused_pseudo_terminal_is_reported_rather_than_connected() {
    let host = start_host().await;
    let connection = authenticated(&host).await;
    *host.watched.pty.lock().unwrap() = Answer::No;

    let failure = connection
        .open_shell(|_| {}, |_| {}, 80, 24, Arc::new(crate::flow::FlowGate::new()))
        .await
        .err()
        .expect("a refused terminal must not open a session");
    assert!(failure.message.contains("refused a terminal"), "{failure}");
}

/// And the same for the shell itself, which is the one the reproduction used.
#[tokio::test]
async fn a_refused_shell_is_reported_rather_than_connected() {
    let host = start_host().await;
    let connection = authenticated(&host).await;
    *host.watched.shell.lock().unwrap() = Answer::No;

    let failure = connection
        .open_shell(|_| {}, |_| {}, 80, 24, Arc::new(crate::flow::FlowGate::new()))
        .await
        .err()
        .expect("a refused shell must not open a session");
    assert!(failure.message.contains("refused a shell"), "{failure}");
}

/// A host that hangs up while it is being asked says so as a failure, rather
/// than leaving a session waiting on an answer that is not coming.
#[tokio::test]
async fn a_host_that_closes_during_setup_is_reported() {
    let host = start_host().await;
    let connection = authenticated(&host).await;
    *host.watched.shell.lock().unwrap() = Answer::Hangup;

    let failure = connection
        .open_shell(|_| {}, |_| {}, 80, 24, Arc::new(crate::flow::FlowGate::new()))
        .await
        .err()
        .expect("a host that hangs up must not open a session");
    assert!(failure.message.contains("closed the session"), "{failure}");
}

/// A host that answers nothing at all is waited out and then reported.
///
/// The wait is checked at its own short patience rather than the thirty
/// seconds `open_shell` allows, because the branch under test is the timeout
/// and not the number.
#[tokio::test]
async fn a_request_that_is_never_acknowledged_times_out() {
    let host = start_host().await;
    let connection = authenticated(&host).await;
    *host.watched.pty.lock().unwrap() = Answer::Nothing;

    let mut channel = connection.handle.channel_open_session().await.expect("channel");
    channel.request_pty(true, "xterm-256color", 80, 24, 0, 0, &[]).await.expect("ask");
    let mut early = Vec::new();
    let failure = accepted(&mut channel, "a terminal", &mut early, Duration::from_millis(300))
        .await
        .expect_err("silence is not consent");
    assert!(failure.message.contains("did not answer"), "{failure}");
}

/// Output that arrives before the acknowledgment is kept and shown, rather
/// than read off the channel during setup and thrown away.
#[tokio::test]
async fn a_greeting_that_beats_the_acknowledgment_still_reaches_the_terminal() {
    let host = start_host().await;
    let connection = authenticated(&host).await;
    host.watched.early.store(true, AtomicOrdering::SeqCst);
    let output: Arc<StdMutex<Vec<u8>>> = Arc::new(StdMutex::new(Vec::new()));
    let sink = Arc::clone(&output);

    let _shell = connection
        .open_shell(
            move |data| sink.lock().unwrap().extend_from_slice(data),
            |_| {},
            80,
            24,
            Arc::new(crate::flow::FlowGate::new()),
        )
        .await
        .expect("shell");

    wait_for(|| {
        let seen = String::from_utf8_lossy(&output.lock().unwrap()).into_owned();
        (seen.contains("afterm-early") && seen.contains("afterm-native-shell")).then_some(())
    })
    .await
    .expect("both the early greeting and the shell banner");
}

#[tokio::test]
async fn the_connection_can_time_itself() {
    let host = start_host().await;
    let connection = authenticated(&host).await;

    // A ping is answered by the protocol, not by the host: nothing is spawned
    // there and nothing is read, so this is the link and only the link.
    let first = connection.latency().await.expect("a round trip to be measured");
    assert!(first >= 0.0, "a duration cannot be negative, got {first}");
    // Loopback, against a host in this process. A figure in seconds would mean
    // the ping was not what was being timed.
    assert!(first < 1000.0, "a loopback round trip should not take {first} ms");

    // Repeatable: the measurement is not a one-off side effect of connecting.
    assert!(connection.latency().await.is_some(), "a second ping to be answered");
}

/// A local file with `contents`, in a directory that removes itself.
fn local_file(directory: &tempdir::Dir, name: &str, contents: &[u8]) -> String {
    let path = directory.path().join(name);
    std::fs::write(&path, contents).unwrap();
    path.to_string_lossy().to_string()
}

fn target_for(host: &Host, session: &str) -> Target {
    Target {
        session: session.into(),
        // An address typed in rather than a saved host: nothing to remember a
        // password against, which is what most of this battery is about.
        profile: None,
        host: "127.0.0.1".into(),
        user: USER.into(),
        port: host.port,
        config_file: None,
    }
}

/// The same, for a session opened from a saved host.
fn saved_target_for(host: &Host, session: &str, profile: &str) -> Target {
    Target { profile: Some(profile.into()), ..target_for(host, session) }
}

/// The host key fingerprint a binding for this host has to carry.
fn fingerprint_of(host: &Host) -> String {
    host.key.fingerprint(Default::default()).to_string()
}

/// A vault over a store held in memory, and a handle on the store.
fn vault_with_store() -> (Arc<secrets::fake::Fake>, Vault) {
    let store = Arc::new(secrets::fake::Fake::default());
    (Arc::clone(&store), Vault::new(store))
}

/// A vault with no store behind it, for every path that is not about
/// remembering. Saving is unavailable, so nothing is looked up and nothing is
/// written however the dialog is answered.
fn no_vault() -> Vault {
    Vault::new(Arc::new(secrets::Absent::new("no credential store in this test")))
}

/// A prompt sink that answers, so the password path can be driven without a
/// user. `remember` is what the checkbox would have been.
struct Typing {
    password: String,
    remember: bool,
    /// Every notice the connection sent back, which is how a failed save is
    /// observed.
    notices: Arc<StdMutex<Vec<String>>>,
    /// The dialogs raised, so a test can prove one was *not*.
    raised: Arc<AtomicUsize>,
    answered: Arc<StdMutex<Option<Arc<Prompts>>>>,
}

impl PromptSink for Typing {
    fn request(&self, request: &AuthRequest) {
        self.raised.fetch_add(1, AtomicOrdering::SeqCst);
        let prompts = self.answered.lock().unwrap().clone();
        let answers = vec![self.password.clone()];
        let remember = self.remember;
        let id = request.request.clone();
        // Answered off-thread: `request` is called from inside the handshake,
        // which is waiting on the answer.
        if let Some(prompts) = prompts {
            tokio::spawn(async move {
                prompts.answer(&id, Some(answers), remember);
            });
        }
    }

    fn resolved(&self, _: &str) {}

    fn notice(&self, _: &str, message: &str) {
        self.notices.lock().unwrap().push(message.to_string());
    }
}

/// A `Prompts` that answers every request with `password`, and the counters
/// that say what it was asked.
fn typing(password: &str, remember: bool) -> (Arc<Prompts>, Arc<AtomicUsize>, Arc<StdMutex<Vec<String>>>) {
    let raised = Arc::new(AtomicUsize::new(0));
    let notices = Arc::new(StdMutex::new(Vec::new()));
    let answered = Arc::new(StdMutex::new(None));
    let prompts = Arc::new(Prompts::new(Box::new(Typing {
        password: password.to_string(),
        remember,
        notices: Arc::clone(&notices),
        raised: Arc::clone(&raised),
        answered: Arc::clone(&answered),
    })));
    *answered.lock().unwrap() = Some(Arc::clone(&prompts));
    (prompts, raised, notices)
}

// ---------------------------------------------------------------- transfers

/// A download is read through the connection into whatever local path it is
/// given, whole, with its size known up front for the progress bar.
#[tokio::test]
async fn a_download_arrives_whole_and_reports_its_size() {
    let host = start_host().await;
    let connection = authenticated(&host).await;
    let directory = tempdir::Dir::new("download");
    let local = directory.path().join("afile.txt.part");
    let seen: Arc<StdMutex<Vec<(u64, Option<u64>)>>> = Arc::new(StdMutex::new(Vec::new()));
    let progress = Arc::clone(&seen);

    connection
        .download("/srv/afile.txt", &local.to_string_lossy(), move |done, total| {
            progress.lock().unwrap().push((done, total));
        })
        .await
        .expect("download");

    assert_eq!(std::fs::read(&local).unwrap(), b"hello\n");
    let reports = seen.lock().unwrap().clone();
    assert_eq!(reports.last(), Some(&(6, Some(6))), "{reports:?}");
}

/// A whole folder, over a real connection: every regular file arrives at the
/// same place in the tree it had on the host, the link is passed over rather
/// than followed, and the progress the pane draws names files by where they
/// sit inside the folder.
#[tokio::test]
async fn a_folder_download_brings_the_tree_and_skips_the_link() {
    let host = start_host().await;
    let transport = crate::transport::Transport::Native(Arc::new(authenticated(&host).await));
    let directory = tempdir::Dir::new("folder");
    let into = crate::localfs::claim_dir(directory.path(), "srv").expect("claim");
    let labelled: Arc<StdMutex<Vec<String>>> = Arc::new(StdMutex::new(Vec::new()));

    let seen = Arc::clone(&labelled);
    let tally = crate::folder::download(&transport, "/srv", &into, |label| {
        seen.lock().unwrap().push(label.to_string());
        |_, _| {}
    })
    .await
    .expect("folder download");

    assert_eq!(std::fs::read_to_string(into.join("afile.txt")).unwrap(), "hello\n");
    assert_eq!(std::fs::read_to_string(into.join("file with space.txt")).unwrap(), "hi");
    // The nested file is under the nested directory, not flattened beside it.
    assert_eq!(std::fs::read_to_string(into.join("sub/deep.txt")).unwrap(), "further in\n");
    // The link is not followed and leaves nothing behind under its own name.
    assert!(!into.join("alink").exists(), "a link is skipped, not fetched");

    assert_eq!(tally.files, 3, "every regular file the host listed arrived");
    assert_eq!(tally.skipped, 1, "the link, and nothing else");
    assert_eq!(tally.failed, 0);

    let mut labels = labelled.lock().unwrap().clone();
    labels.sort();
    assert!(labels.contains(&"sub/deep.txt".to_string()), "{labels:?}");
    assert!(labels.contains(&"afile.txt".to_string()), "{labels:?}");
}

/// A local tree with the shapes that matter in it, so the folder upload tests
/// below all start from the same one: a file at the top, a file one level
/// down, and a link that must not be followed.
///
/// The link points at the directory *above* the folder, which is what makes it
/// the case worth testing: following it would walk out of the folder being
/// uploaded and into everything beside it.
fn local_tree(directory: &tempdir::Dir) -> std::path::PathBuf {
    let root = directory.path().join("reports");
    std::fs::create_dir_all(root.join("sub")).unwrap();
    std::fs::write(root.join("top.txt"), b"at the top").unwrap();
    std::fs::write(root.join("sub/deep.txt"), b"further in").unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink(directory.path(), root.join("out")).unwrap();
    root
}

/// A whole folder, over a real connection: the tree is made on the host, every
/// regular file arrives where it sat locally, and the link is stepped over
/// rather than followed out of the folder.
#[tokio::test]
async fn a_folder_upload_makes_the_tree_and_never_follows_a_link() {
    let host = start_host().await;
    let transport = crate::transport::Transport::Native(Arc::new(authenticated(&host).await));
    let directory = tempdir::Dir::new("upload-folder");
    let root = local_tree(&directory);

    let (landed, tally) =
        crate::folder::upload(&transport, &root, "/srv", Collision::Fail, |_| |_, _| {})
            .await
            .expect("folder upload");

    assert_eq!(landed, "/srv/reports");
    assert!(host.directory("/srv/reports"), "the folder was made");
    assert!(host.directory("/srv/reports/sub"), "and so was the one inside it");
    assert_eq!(host.file("/srv/reports/top.txt").as_deref(), Some(&b"at the top"[..]));
    assert_eq!(host.file("/srv/reports/sub/deep.txt").as_deref(), Some(&b"further in"[..]));
    assert_eq!(tally.files, 2);
    // The link, where this machine has links to make.
    assert_eq!(tally.skipped, if cfg!(unix) { 1 } else { 0 });
    assert_eq!(tally.failed, 0);
    assert!(host.paths_containing("afterm-part").is_empty(), "no temporary is left behind");
}

/// The default answer to a folder already on the host is to ask rather than to
/// decide. Nothing is sent, and the refusal carries the cause the pane opens
/// its question on.
#[tokio::test]
async fn a_folder_onto_a_taken_name_is_refused_by_default_and_sends_nothing() {
    let host = start_host().await;
    let transport = crate::transport::Transport::Native(Arc::new(authenticated(&host).await));
    let directory = tempdir::Dir::new("upload-folder-taken");

    // `/srv/sub` is a directory this host was born with.
    let mine = directory.path().join("sub");
    std::fs::create_dir_all(&mine).unwrap();
    std::fs::write(mine.join("mine.txt"), b"local").unwrap();

    let failure = crate::folder::upload(&transport, &mine, "/srv", Collision::Fail, |_| |_, _| {})
        .await
        .expect_err("a taken name is refused");

    assert_eq!(failure.cause, Cause::Exists, "{failure:?}");
    assert!(host.file("/srv/sub/mine.txt").is_none(), "nothing was sent");
}

/// "Keep both" takes the next free name and leaves what was there alone, the
/// same way a second download of a folder lands in `reports (1)`.
#[tokio::test]
async fn keeping_both_puts_the_folder_under_the_next_free_name() {
    let host = start_host().await;
    let transport = crate::transport::Transport::Native(Arc::new(authenticated(&host).await));
    let directory = tempdir::Dir::new("upload-folder-both");
    let mine = directory.path().join("sub");
    std::fs::create_dir_all(&mine).unwrap();
    std::fs::write(mine.join("mine.txt"), b"local").unwrap();

    let (landed, tally) =
        crate::folder::upload(&transport, &mine, "/srv", Collision::Rename, |_| |_, _| {})
            .await
            .expect("folder upload");

    assert_eq!(landed, "/srv/sub (1)");
    assert_eq!(host.file("/srv/sub (1)/mine.txt").as_deref(), Some(&b"local"[..]));
    // The directory that was there keeps what was in it.
    assert_eq!(host.file("/srv/sub/deep.txt").as_deref(), Some(&b"further in\n"[..]));
    assert_eq!(tally.files, 1);
}

/// Merging is the one answer under which a file on the host is written over,
/// and it replaces only the names that collide.
#[tokio::test]
async fn merging_replaces_the_names_that_collide_and_leaves_the_rest() {
    let host = start_host().await;
    let transport = crate::transport::Transport::Native(Arc::new(authenticated(&host).await));
    let directory = tempdir::Dir::new("upload-folder-merge");
    let mine = directory.path().join("sub");
    std::fs::create_dir_all(&mine).unwrap();
    std::fs::write(mine.join("deep.txt"), b"mine now").unwrap();
    std::fs::write(mine.join("fresh.txt"), b"new here").unwrap();

    let (landed, tally) =
        crate::folder::upload(&transport, &mine, "/srv", Collision::Replace, |_| |_, _| {})
            .await
            .expect("folder upload");

    assert_eq!(landed, "/srv/sub", "merged into the folder that was there");
    assert_eq!(host.file("/srv/sub/deep.txt").as_deref(), Some(&b"mine now"[..]));
    assert_eq!(host.file("/srv/sub/fresh.txt").as_deref(), Some(&b"new here"[..]));
    assert_eq!((tally.files, tally.failed), (2, 0));
    assert!(host.paths_containing("afterm-part").is_empty(), "no temporary is left behind");
}

/// The ordinary case: a name nobody has. It arrives under that name, having
/// travelled under a temporary one that is gone by the time anyone looks.
#[tokio::test]
async fn an_upload_lands_under_its_name_and_leaves_no_temporary() {
    let host = start_host().await;
    let connection = authenticated(&host).await;
    let directory = tempdir::Dir::new("upload");
    let local = local_file(&directory, "new.txt", b"fresh");

    let landed = connection
        .upload(&local, "/srv", "new.txt", Collision::Fail, |_, _| {})
        .await
        .expect("upload");

    assert_eq!(landed, "/srv/new.txt");
    assert_eq!(host.file("/srv/new.txt").as_deref(), Some(&b"fresh"[..]));
    assert!(host.paths_containing("afterm-part").is_empty(), "the temporary is gone");
}

/// The default answer to a taken name is no. The file that was there is not
/// touched, and the one that was on its way is not left lying about.
#[tokio::test]
async fn an_upload_onto_a_taken_name_is_refused_by_default_and_changes_nothing() {
    let host = start_host().await;
    let connection = authenticated(&host).await;
    let directory = tempdir::Dir::new("upload-taken");
    let local = local_file(&directory, "afile.txt", b"the newcomer");

    let failure = connection
        .upload(&local, "/srv", "afile.txt", Collision::Fail, |_, _| {})
        .await
        .expect_err("a taken name must be refused");

    assert_eq!(failure.cause, Cause::Exists);
    assert_eq!(host.file("/srv/afile.txt").as_deref(), Some(&b"hello\n"[..]), "untouched");
    assert!(host.paths_containing("afterm-part").is_empty(), "nothing left behind");
}

#[tokio::test]
async fn replace_swaps_in_the_whole_file_and_keep_both_numbers_the_newcomer() {
    let host = start_host().await;
    let connection = authenticated(&host).await;
    let directory = tempdir::Dir::new("upload-choices");
    let local = local_file(&directory, "afile.txt", b"the newcomer");

    let kept = connection
        .upload(&local, "/srv", "afile.txt", Collision::Rename, |_, _| {})
        .await
        .expect("keep both");
    assert_eq!(kept, "/srv/afile (1).txt");
    assert_eq!(host.file("/srv/afile.txt").as_deref(), Some(&b"hello\n"[..]), "the original stays");
    assert_eq!(host.file("/srv/afile (1).txt").as_deref(), Some(&b"the newcomer"[..]));

    // And again: the next free number, not a second copy of the first.
    let again = connection
        .upload(&local, "/srv", "afile.txt", Collision::Rename, |_, _| {})
        .await
        .expect("keep both, again");
    assert_eq!(again, "/srv/afile (2).txt");

    let replaced = connection
        .upload(&local, "/srv", "afile.txt", Collision::Replace, |_, _| {})
        .await
        .expect("replace");
    assert_eq!(replaced, "/srv/afile.txt");
    assert_eq!(host.file("/srv/afile.txt").as_deref(), Some(&b"the newcomer"[..]));
    assert!(host.paths_containing("afterm-part").is_empty());
}

/// Replace has to survive the move it ends with being refused.
///
/// It used to remove the destination first and move the upload in second, so a
/// server that said no to the move left the directory with neither file: the
/// original had already been deleted, and the upload's own cleanup then took
/// the temporary. Nothing is let go now until the replacement answers to the
/// real name.
#[tokio::test]
async fn a_replacement_that_cannot_be_moved_in_gives_the_original_back() {
    let host = start_host().await;
    let connection = authenticated(&host).await;
    let directory = tempdir::Dir::new("upload-refused");
    let local = local_file(&directory, "refuses.txt", b"the newcomer");

    let failure = connection
        .upload(&local, "/srv", "refuses.txt", Collision::Replace, |_, _| {})
        .await
        .expect_err("a refused move must fail the upload");

    assert_eq!(failure.cause, Cause::Operation);
    assert_eq!(
        host.file("/srv/refuses.txt").as_deref(),
        Some(&b"the original"[..]),
        "the original must be back under its own name"
    );
    assert!(host.paths_containing("afterm-old").is_empty(), "nothing left aside");
    assert!(host.paths_containing("afterm-part").is_empty(), "and no temporary");
}

/// Replace replaces a file. A directory of the same name is refused rather
/// than moved out of the way — the old order refused it by accident, because
/// the protocol's `remove` will not take a directory, and that was the right
/// answer.
#[tokio::test]
async fn replace_will_not_swap_a_directory_for_a_file() {
    let host = start_host().await;
    let connection = authenticated(&host).await;
    let directory = tempdir::Dir::new("upload-onto-directory");
    let local = local_file(&directory, "sub", b"not a directory");

    let failure = connection
        .upload(&local, "/srv", "sub", Collision::Replace, |_, _| {})
        .await
        .expect_err("a directory must not be replaced by a file");

    assert_eq!(failure.cause, Cause::Operation);
    assert!(host.paths_containing("afterm").is_empty(), "nothing left behind");
}

/// A transfer that dies partway — the host refuses a write, the way a full
/// disk does — leaves neither a half file under the real name nor a stray
/// temporary.
#[tokio::test]
async fn a_transfer_that_fails_leaves_nothing_under_any_name() {
    let host = start_host().await;
    let connection = authenticated(&host).await;
    let directory = tempdir::Dir::new("upload-poison");
    let local = local_file(&directory, "poison.bin", b"never arrives");

    let failure = connection
        .upload(&local, "/srv", "poison.bin", Collision::Fail, |_, _| {})
        .await
        .expect_err("a refused write must fail the transfer");

    assert_eq!(failure.cause, Cause::Operation);
    assert_eq!(host.file("/srv/poison.bin"), None, "nothing under the real name");
    assert!(
        host.paths_containing("poison").is_empty(),
        "and no temporary: {:?}",
        host.paths_containing("poison")
    );
}

// ------------------------------------------------------ running a command

/// The command runs on its own channel of the authenticated connection, and
/// the terminal never sees it.
///
/// That is the whole point of the fix: the previous version typed the command
/// into the active terminal, so after `sudo -i` or a nested `ssh` it ran as
/// that shell's account on that shell's host while the dialog went on naming
/// the session's — and an editor at the front of the terminal simply received
/// it as keystrokes.
#[tokio::test]
async fn a_command_runs_on_its_own_channel_and_not_through_the_terminal() {
    let host = start_host().await;
    let connection = authenticated(&host).await;
    // A live shell channel, which is what the old code wrote into.
    let _shell = connection
        .open_shell(|_| {}, |_| {}, 80, 24, Arc::new(crate::flow::FlowGate::new()))
        .await
        .expect("shell");

    let outcome = connection.run("true").await.expect("run");

    assert!(outcome.succeeded(), "{outcome:?}");
    assert_eq!(
        host.watched.executed.lock().unwrap().iter().filter(|c| !c.starts_with("while :;")).count(),
        1,
        "the command reached the host once, as a command"
    );
    assert!(
        host.watched.typed.lock().unwrap().is_empty(),
        "nothing was typed at the shell: {:?}",
        String::from_utf8_lossy(&host.watched.typed.lock().unwrap())
    );
}

/// A command that fails is reported as a failure, whatever it printed.
///
/// The old installer ended with an unconditional `printf` announcing success,
/// so a refused `mkdir` or a read-only home said "public key installed" too.
#[tokio::test]
async fn a_command_that_fails_is_never_reported_as_having_worked() {
    let host = start_host().await;
    let connection = authenticated(&host).await;
    host.watched.refuse.store(true, AtomicOrdering::SeqCst);

    let outcome = connection.run("anything").await.expect("the channel itself works");

    assert!(!outcome.succeeded(), "a non-zero status is not success");
    assert_eq!(outcome.status, Some(1));
    assert!(!outcome.complaint().is_empty(), "and it says something about why");
}

/// A host that will not stop talking is one afterm stops listening to.
#[tokio::test]
async fn a_command_that_floods_is_cut_off_rather_than_held() {
    let host = start_host().await;
    let connection = authenticated(&host).await;

    let outcome = connection.run("afterm-flood").await.expect("run");

    assert!(outcome.stdout.len() <= EXEC_CAPTURE, "{} bytes kept", outcome.stdout.len());
    assert!(outcome.truncated, "and the caller is told it is not the whole answer");
}

// ---------------------------------------------------------- permissions

/// The finding this exists for: a private file must not come back readable to
/// everyone because it was replaced.
///
/// Reproduced against an OpenSSH server with a `022` umask, where a `0600`
/// file became `0644` after an upload replaced it — the staging file was
/// created with no permissions of its own and inherited the server's umask,
/// and the rename carried that mode onto the destination.
#[tokio::test]
async fn a_replacement_wears_the_permissions_of_what_it_replaced() {
    let host = start_host().await;
    let connection = authenticated(&host).await;
    let directory = tempdir::Dir::new("upload-private");
    let local = local_file(&directory, "private.txt", b"still mine");

    connection
        .upload(&local, "/srv", "private.txt", Collision::Replace, |_, _| {})
        .await
        .expect("replace");

    assert_eq!(host.file("/srv/private.txt").as_deref(), Some(&b"still mine"[..]));
    assert_eq!(host.mode("/srv/private.txt"), Some(0o600), "a private file stays private");

    // The same in the other direction: an executable keeps its bit, or the
    // file the user replaced stops being a program.
    let script = local_file(&directory, "run.sh", b"#!/bin/sh\necho hi\n");
    connection
        .upload(&script, "/srv", "run.sh", Collision::Replace, |_, _| {})
        .await
        .expect("replace the script");
    assert_eq!(host.mode("/srv/run.sh"), Some(0o755), "an executable stays executable");
}

/// The staging file is created private and has never been anything else, so
/// there is no window in which the content sits in a shared directory under
/// the server's umask.
#[tokio::test]
async fn the_staging_file_is_private_from_the_moment_it_exists() {
    let host = start_host().await;
    let connection = authenticated(&host).await;
    let directory = tempdir::Dir::new("upload-staging");
    let local = local_file(&directory, "new.txt", b"fresh");

    connection
        .upload(&local, "/srv", "new.txt", Collision::Fail, |_, _| {})
        .await
        .expect("upload");

    assert_eq!(
        host.created_as("afterm-part"),
        vec![Some(0o600)],
        "the staging file must ask for 0600 at creation, not afterwards"
    );
}

/// A name nothing holds is not a replacement, so there is no mode to carry.
/// It lands as a deliberate default rather than as whatever the staging file
/// happened to be.
#[tokio::test]
async fn a_new_upload_lands_under_a_mode_somebody_chose() {
    let host = start_host().await;
    let connection = authenticated(&host).await;
    let directory = tempdir::Dir::new("upload-new");
    let local = local_file(&directory, "new.txt", b"fresh");

    connection
        .upload(&local, "/srv", "new.txt", Collision::Fail, |_, _| {})
        .await
        .expect("upload");

    let landed = host.mode("/srv/new.txt").expect("a mode");
    // On Unix the local file's own mode is copied, the way `sftp put` does;
    // everywhere else the default applies. Either way it is not the staging
    // mode, and it is not whatever a umask would have made of it.
    assert!(landed == 0o644 || landed == 0o600 || landed == local_permissions(&local), "{landed:o}");
    assert_ne!(landed & 0o7000, 0o7000);
}

#[cfg(unix)]
fn local_permissions(path: &str) -> u32 {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path).unwrap().permissions().mode() & 0o777
}

#[cfg(not(unix))]
fn local_permissions(_: &str) -> u32 {
    0o644
}

/// A setuid destination is refused rather than replaced with the bit silently
/// dropped or — far worse — silently kept over content from somewhere else.
#[tokio::test]
async fn a_destination_with_special_bits_is_refused_rather_than_re_granted() {
    let host = start_host().await;
    let connection = authenticated(&host).await;
    let directory = tempdir::Dir::new("upload-setuid");
    let local = local_file(&directory, "setuid.bin", b"mine now");

    let failure = connection
        .upload(&local, "/srv", "setuid.bin", Collision::Replace, |_, _| {})
        .await
        .expect_err("a setuid destination must be refused");

    assert!(failure.message.contains("special permission"), "{}", failure.message);
    assert_eq!(host.file("/srv/setuid.bin").as_deref(), Some(&b"privileged"[..]), "untouched");
    assert_eq!(host.mode("/srv/setuid.bin"), Some(0o4755));
    assert!(host.paths_containing("afterm").is_empty(), "and nothing was staged");
}

/// A host that will not set the permissions the file needs abandons the
/// upload rather than installing it under whatever mode it could manage.
#[tokio::test]
async fn a_host_that_refuses_the_permissions_abandons_the_upload() {
    let host = start_host().await;
    let connection = authenticated(&host).await;
    let directory = tempdir::Dir::new("upload-rigid");
    let local = local_file(&directory, "rigid.txt", b"the newcomer");

    let failure = connection
        .upload(&local, "/srv", "rigid.txt", Collision::Replace, |_, _| {})
        .await
        .expect_err("a refused chmod must fail the upload");

    assert!(failure.message.contains("permissions"), "{}", failure.message);
    assert_eq!(
        host.file("/srv/rigid.txt").as_deref(),
        Some(&b"unchangeable"[..]),
        "the original is untouched"
    );
    assert!(host.paths_containing("afterm-part").is_empty(), "and no temporary is left");
}

/// The original is moved aside before the replacement is moved in, which is a
/// window. When the way back is also refused, the file is not lost — but it is
/// under a name nobody would guess, so the failure has to say which one.
#[tokio::test]
async fn a_rollback_that_fails_says_where_the_original_ended_up() {
    let host = start_host().await;
    let connection = authenticated(&host).await;
    let directory = tempdir::Dir::new("upload-stuck");
    let local = local_file(&directory, "stuck.txt", b"the newcomer");

    let failure = connection
        .upload(&local, "/srv", "stuck.txt", Collision::Replace, |_, _| {})
        .await
        .expect_err("a refused move must fail the upload");

    let aside = host.paths_containing("afterm-old");
    assert_eq!(aside.len(), 1, "the original is still on the host: {aside:?}");
    assert!(
        failure.message.contains(&aside[0]),
        "the failure must name {}: {}",
        aside[0],
        failure.message
    );
    assert_eq!(host.file(&aside[0]).as_deref(), Some(&b"the original"[..]));
}

// -------------------------------------------------- remembered passwords

/// A binding for the in-process host, as `authenticate` would build one.
fn binding_for(host: &Host, profile: &str) -> secrets::Binding {
    secrets::Binding {
        profile: profile.into(),
        user: USER.into(),
        hostname: "127.0.0.1".into(),
        port: host.port,
        fingerprint: fingerprint_of(host),
    }
}

fn password_only(host: &Host) -> config::Effective {
    config::Effective {
        hostname: "127.0.0.1".into(),
        user: USER.into(),
        port: host.port,
        pubkey_authentication: false,
        ..Default::default()
    }
}

/// The point of the feature: a password already saved for this destination
/// authenticates without anybody being asked for it.
///
/// `NoPrompts` is what proves it. It answers nothing, so a run that reaches
/// the dialog cannot succeed — success here means the password came out of the
/// store, and the host's counter says it was checked exactly once.
#[tokio::test]
async fn a_remembered_password_authenticates_without_a_dialog() {
    let host = start_host().await;
    let (store, vault) = vault_with_store();
    store.save(&binding_for(&host, "saved-1"), PASSWORD).expect("save");
    let mut handle = connect(&host, trusting(&host.key)).await.expect("connect");

    let method = authenticate(
        &mut handle,
        &password_only(&host),
        &saved_target_for(&host, "remembered", "saved-1"),
        &Prompts::new(Box::new(NoPrompts)),
        &vault,
        &fingerprint_of(&host),
        // Not interactive, which is the other half of the claim: a poll can
        // use a remembered password, because using one asks nobody anything.
        false,
    )
    .await
    .expect("the saved password should have been enough");

    assert_eq!(method, "password");
    assert_eq!(host.counters.credentials.load(AtomicOrdering::SeqCst), 1);
}

/// The binding is checked before the password is sent, not after. A host
/// presenting a different key is a different host, whatever the saved profile
/// says, and it is never shown the password.
#[tokio::test]
async fn a_password_is_not_sent_to_a_host_with_another_key() {
    let host = start_host().await;
    let (store, vault) = vault_with_store();
    let elsewhere =
        secrets::Binding { fingerprint: "SHA256:somewhere-else".into(), ..binding_for(&host, "saved-1") };
    store.save(&elsewhere, PASSWORD).expect("save");
    let mut handle = connect(&host, trusting(&host.key)).await.expect("connect");

    let failure = authenticate(
        &mut handle,
        &password_only(&host),
        &saved_target_for(&host, "moved", "saved-1"),
        &Prompts::new(Box::new(NoPrompts)),
        &vault,
        &fingerprint_of(&host),
        false,
    )
    .await
    .expect_err("a mismatched binding leaves nothing to authenticate with");

    assert_eq!(failure.cause, Cause::Auth);
    assert_eq!(
        host.counters.credentials.load(AtomicOrdering::SeqCst),
        0,
        "the saved password was sent to a host it was not saved for"
    );
}

/// Saving happens after the host says yes, and because the user asked.
#[tokio::test]
async fn a_password_is_saved_only_once_the_host_has_accepted_it() {
    let host = start_host().await;
    let (store, vault) = vault_with_store();
    let (prompts, raised, _notices) = typing(PASSWORD, true);
    let mut handle = connect(&host, trusting(&host.key)).await.expect("connect");

    let method = authenticate(
        &mut handle,
        &password_only(&host),
        &saved_target_for(&host, "saving", "saved-1"),
        &prompts,
        &vault,
        &fingerprint_of(&host),
        true,
    )
    .await
    .expect("the typed password is the right one");

    assert_eq!(method, "password");
    assert_eq!(raised.load(AtomicOrdering::SeqCst), 1, "the user should have been asked once");
    assert!(store.holds("saved-1").unwrap(), "the password was not saved");
    // And it comes back for this destination, which is the whole point of
    // having saved it.
    assert!(vault.remembered(&binding_for(&host, "saved-1")).is_some());
}

/// A password the host refuses is never saved, however the checkbox was left.
/// Nothing to replace a working entry with, and nothing to store for a
/// credential that does not work.
#[tokio::test]
async fn a_refused_password_is_never_saved() {
    let host = start_host().await;
    let (store, vault) = vault_with_store();
    let (prompts, _raised, _notices) = typing("not the password", true);
    let mut handle = connect(&host, trusting(&host.key)).await.expect("connect");

    let failure = authenticate(
        &mut handle,
        &password_only(&host),
        &saved_target_for(&host, "wrong", "saved-1"),
        &prompts,
        &vault,
        &fingerprint_of(&host),
        true,
    )
    .await
    .expect_err("the wrong password cannot authenticate");

    assert_eq!(failure.cause, Cause::Auth);
    assert!(!store.holds("saved-1").unwrap(), "a refused password was saved");
}

/// The connection is worth more than the saving. A store that will not take
/// the entry leaves the session up and says what did not happen.
#[tokio::test]
async fn a_store_that_refuses_the_write_still_leaves_the_session_connected() {
    let host = start_host().await;
    let (store, vault) = vault_with_store();
    *store.refuse_writes.lock().unwrap() = Some("the credential store is full".into());
    let (prompts, _raised, notices) = typing(PASSWORD, true);
    let mut handle = connect(&host, trusting(&host.key)).await.expect("connect");

    let method = authenticate(
        &mut handle,
        &password_only(&host),
        &saved_target_for(&host, "unsaveable", "saved-1"),
        &prompts,
        &vault,
        &fingerprint_of(&host),
        true,
    )
    .await
    .expect("a store failure is not an authentication failure");

    assert_eq!(method, "password");
    let said = notices.lock().unwrap().clone();
    assert_eq!(said.len(), 1, "the failure to save must be said out loud: {said:?}");
    assert!(said[0].contains("not saved"), "{}", said[0]);
}

/// A stored password the host has refused is tried once and then left alone.
///
/// Two attempts, one vault. The second must not spend another of the host's
/// authentication attempts on a credential already known to be wrong — which
/// is what four tabs on one saved host would otherwise do, four times.
#[tokio::test]
async fn a_stored_password_the_host_refused_is_not_tried_again() {
    let host = start_host().await;
    let (store, vault) = vault_with_store();
    store.save(&binding_for(&host, "saved-1"), "no longer the password").expect("save");

    for expected in [1, 1] {
        let mut handle = connect(&host, trusting(&host.key)).await.expect("connect");
        let _ = authenticate(
            &mut handle,
            &password_only(&host),
            &saved_target_for(&host, "stale", "saved-1"),
            &Prompts::new(Box::new(NoPrompts)),
            &vault,
            &fingerprint_of(&host),
            false,
        )
        .await;
        assert_eq!(
            host.counters.credentials.load(AtomicOrdering::SeqCst),
            expected,
            "the refused password was offered again"
        );
    }

    // Still saved: only the user throws a password away, and the refusal is
    // remembered in the vault rather than by deleting their entry.
    assert!(store.holds("saved-1").unwrap());
}

/// An address typed in and never saved has no identity to bind to, so nothing
/// is looked up and the dialog does not offer to remember.
#[tokio::test]
async fn an_unsaved_host_is_never_offered_the_choice() {
    let host = start_host().await;
    let (store, vault) = vault_with_store();
    let (prompts, _raised, _notices) = typing(PASSWORD, true);
    let mut handle = connect(&host, trusting(&host.key)).await.expect("connect");

    authenticate(
        &mut handle,
        &password_only(&host),
        // No profile: this is the "New session" form, not a saved host.
        &target_for(&host, "ad-hoc"),
        &prompts,
        &vault,
        &fingerprint_of(&host),
        true,
    )
    .await
    .expect("the typed password is the right one");

    assert_eq!(store.count(), 0, "an ad-hoc session saved a password");
    assert!(!vault.may_remember(None));
}

// ------------------------------------------------------ honouring the config

/// `PubkeyAuthentication no` in the user's configuration means exactly that:
/// no key file and no agent key is offered, whatever is on the machine.
#[tokio::test]
async fn a_method_the_configuration_turned_off_is_never_offered() {
    let host = start_host().await;
    let (mut effective, _keys) = identities(2);
    effective.pubkey_authentication = false;
    let mut handle = connect(&host, trusting(&host.key)).await.expect("connect");

    let failure = authenticate(
        &mut handle,
        &effective,
        &target_for(&host, "switched-off"),
        &Prompts::new(Box::new(NoPrompts)),
        &no_vault(),
        &fingerprint_of(&host),
        false,
    )
    .await
    .expect_err("with keys off and no prompt allowed, nothing can succeed");

    assert_eq!(failure.cause, Cause::Auth);
    assert!(failure.is_interactive_auth(), "the password is still on offer: {failure:?}");
    assert_eq!(
        host.counters.publickey.load(AtomicOrdering::SeqCst),
        0,
        "PubkeyAuthentication no, and a key was offered anyway"
    );
}

/// `BatchMode yes` is the user saying never ask. A deliberate click still
/// does not ask, and the failure names the directive rather than offering a
/// button that would only arrive back here.
#[tokio::test]
async fn batch_mode_forbids_the_prompt_the_host_wants() {
    let host = start_host().await;
    let effective = config::Effective {
        hostname: "127.0.0.1".into(),
        user: USER.into(),
        batch_mode: true,
        pubkey_authentication: false,
        ..Default::default()
    };
    let mut handle = connect(&host, trusting(&host.key)).await.expect("connect");

    let failure = authenticate(
        &mut handle,
        &effective,
        &target_for(&host, "batch"),
        &Prompts::new(Box::new(NoPrompts)),
        &no_vault(),
        &fingerprint_of(&host),
        true,
    )
    .await
    .expect_err("a forbidden prompt cannot be answered");

    assert_eq!(failure.cause, Cause::Auth);
    assert!(!failure.is_interactive_auth(), "no method is handed back: {failure:?}");
    assert!(failure.message.contains("BatchMode"), "{}", failure.message);
    assert_eq!(host.counters.credentials.load(AtomicOrdering::SeqCst), 0);
}

/// `IdentitiesOnly yes`: of everything the agent holds, only a key the
/// configuration names may be offered — matched on the key itself, because
/// the agent's comment and the `.pub` file's routinely differ.
#[test]
fn identities_only_keeps_the_agent_to_the_keys_the_configuration_names() {
    let named = PrivateKey::random(&mut rand::rng(), russh::keys::Algorithm::Ed25519).unwrap();
    let other = PrivateKey::random(&mut rand::rng(), russh::keys::Algorithm::Ed25519).unwrap();
    let mut as_the_agent_has_it = named.public_key().clone();
    as_the_agent_has_it.set_comment("something else entirely");

    let identities = vec![
        AgentIdentity::PublicKey { key: as_the_agent_has_it, comment: "work laptop".into() },
        AgentIdentity::PublicKey { key: other.public_key().clone(), comment: "personal".into() },
    ];
    let kept = agent_allowed(identities, &[named.public_key().clone()]);
    assert_eq!(kept.len(), 1);
    assert!(matches!(&kept[0], AgentIdentity::PublicKey { comment, .. } if comment == "work laptop"));

    // Nothing named, nothing allowed — the state that reads as `Filtered`.
    let none = agent_allowed(
        vec![AgentIdentity::PublicKey { key: other.public_key().clone(), comment: String::new() }],
        &[named.public_key().clone()],
    );
    assert!(none.is_empty());
}

/// The public half of each configured identity comes from its `.pub` when
/// there is one, as OpenSSH reads it, and from the private file when not. A
/// path that names nothing — `ssh -G` lists the defaults whether or not they
/// exist — contributes nothing.
#[test]
fn the_configured_identities_public_halves_are_read_or_derived() {
    let directory = tempdir::Dir::new("public-halves");
    let with_pub = PrivateKey::random(&mut rand::rng(), russh::keys::Algorithm::Ed25519).unwrap();
    let bare = PrivateKey::random(&mut rand::rng(), russh::keys::Algorithm::Ed25519).unwrap();

    let a = directory.path().join("id_a");
    std::fs::write(&a, with_pub.to_openssh(russh::keys::ssh_key::LineEnding::LF).unwrap()).unwrap();
    std::fs::write(directory.path().join("id_a.pub"), with_pub.public_key().to_openssh().unwrap()).unwrap();
    let b = directory.path().join("id_b");
    std::fs::write(&b, bare.to_openssh(russh::keys::ssh_key::LineEnding::LF).unwrap()).unwrap();
    let missing = directory.path().join("id_missing");

    let keys = configured_public_keys(&[a, b, missing]);
    assert_eq!(keys.len(), 2);
    assert_eq!(keys[0].key_data(), with_pub.public_key().key_data());
    assert_eq!(keys[1].key_data(), bare.public_key().key_data());
}

/// Write `count` usable private keys, and the configuration that names them.
fn identities(count: usize) -> (config::Effective, tempdir::Dir) {
    let directory = tempdir::Dir::new("identities");
    let mut files = Vec::new();
    for index in 0..count {
        let key = PrivateKey::random(&mut rand::rng(), russh::keys::Algorithm::Ed25519).unwrap();
        let path = directory.path().join(format!("id_{index}"));
        std::fs::write(&path, key.to_openssh(russh::keys::ssh_key::LineEnding::LF).unwrap()).unwrap();
        files.push(path);
    }
    (
        config::Effective {
            hostname: "127.0.0.1".into(),
            user: USER.into(),
            port: 22,
            identity_files: files,
            ..Default::default()
        },
        directory,
    )
}


/// Every rejected key counts against the server's `MaxAuthTries`, which
/// defaults to six. A machine with several keys and an agent could spend the
/// whole budget on publickey and be disconnected before the password it was
/// always going to need — on exactly the password-only hosts this transport
/// exists for, and reading as "unreachable" rather than "wants a password".
#[tokio::test]
async fn offering_keys_leaves_room_for_the_password_the_host_actually_wants() {
    let host = start_host().await;
    let (effective, _keys) = identities(6);
    let mut handle = connect(&host, trusting(&host.key)).await.expect("connect");
    let target = Target {
        session: "budget".into(),
        profile: None,
        host: "127.0.0.1".into(),
        user: USER.into(),
        port: host.port,
        config_file: None,
    };

    // Not interactive, so it stops at the point of needing the user.
    let failure = authenticate(
        &mut handle,
        &effective,
        &target,
        &Prompts::new(Box::new(NoPrompts)),
        &no_vault(),
        &fingerprint_of(&host),
        false,
    )
    .await
    .expect_err("a host that wants a password cannot be satisfied without one");

    assert_eq!(failure.cause, Cause::Auth);
    assert!(
        failure.is_interactive_auth(),
        "the interface must be told a password would work: {failure:?}"
    );
    let offered = host.counters.publickey.load(AtomicOrdering::SeqCst);
    assert!(
        offered <= 3,
        "offered {offered} keys, which would exhaust a default MaxAuthTries of 6"
    );
    assert!(offered > 0, "keys that exist should still be tried first");
}

/// A sink that answers the dialog the way a user would.
///
/// `NoPrompts` covers the poll, which must never ask. This covers the other
/// half — opening a tab on a password-only host — which is the whole reason
/// the native transport exists and had no test until one was wanted.
struct Answering {
    asked: Arc<StdMutex<Vec<AuthRequest>>>,
    prompts: Arc<StdMutex<Option<Arc<Prompts>>>>,
    password: String,
}

impl PromptSink for Answering {
    fn request(&self, request: &AuthRequest) {
        self.asked.lock().unwrap().push(request.clone());
        // The handshake is waiting on this, so answer from another task rather
        // than re-entering it from inside its own notification.
        let prompts = self.prompts.lock().unwrap().clone();
        let (id, password) = (request.request.clone(), self.password.clone());
        if let Some(prompts) = prompts {
            tokio::spawn(async move {
                prompts.answer(&id, Some(vec![password]), false);
            });
        }
    }
    fn resolved(&self, _: &str) {}
    fn notice(&self, _: &str, _: &str) {}
}

/// Opening a tab on a password-only host raises the dialog and gets in.
///
/// The non-interactive half is covered above: a poll stops at the point of
/// needing the user. Nothing covered the half the user actually sees, which is
/// that asking produces a prompt at all.
#[tokio::test]
async fn a_password_only_host_is_asked_for_a_password_when_a_tab_opens() {
    let host = start_host().await;
    let mut handle = connect(&host, trusting(&host.key)).await.expect("connect");
    let target = Target {
        session: "tab".into(),
        profile: None,
        host: "127.0.0.1".into(),
        user: USER.into(),
        port: host.port,
        config_file: None,
    };
    let asked = Arc::new(StdMutex::new(Vec::new()));
    let shared: Arc<StdMutex<Option<Arc<Prompts>>>> = Arc::new(StdMutex::new(None));
    let prompts = Arc::new(Prompts::new(Box::new(Answering {
        asked: Arc::clone(&asked),
        prompts: Arc::clone(&shared),
        password: PASSWORD.into(),
    })));
    *shared.lock().unwrap() = Some(Arc::clone(&prompts));

    // No keys offered, so password is the only way in — the host this
    // transport exists for.
    let effective = config::Effective {
        hostname: "127.0.0.1".into(),
        user: USER.into(),
        port: host.port,
        identity_files: Vec::new(),
        ..Default::default()
    };
    let method = authenticate(
        &mut handle,
        &effective,
        &target,
        &prompts,
        &no_vault(),
        &fingerprint_of(&host),
        true,
    )
    .await
    .expect("a password-only host must be reachable by asking for the password");

    assert_eq!(method, "password");
    let asked = asked.lock().unwrap();
    assert_eq!(asked.len(), 1, "exactly one dialog: {asked:?}");
    assert_eq!(asked[0].user, USER);
}

/// Poll a condition for a short while. The alternative is a fixed sleep, which
/// is either slow or flaky and usually both.
async fn wait_for<T>(check: impl FnMut() -> Option<T>) -> Option<T> {
    wait_within(Duration::from_secs(5), check).await
}

/// The same, for a test that has to allow for real work rather than a state
/// change. Five seconds is plenty for something that is either true or wrong;
/// it is not plenty for megabytes crossing a loopback SSH session on a hosted
/// two-core agent, where the difference between slow and stuck is the whole
/// question being asked.
async fn wait_within<T>(patience: Duration, mut check: impl FnMut() -> Option<T>) -> Option<T> {
    let deadline = Instant::now() + patience;
    loop {
        if let Some(value) = check() {
            return Some(value);
        }
        if Instant::now() >= deadline {
            return None;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}
