use crate::fsmodel::{
    carried, join, local_mode, numbered, sort_entries, EntryKind, FileEntry, Listing,
    STAGING_MODE,
};
use crate::remote::read_capped;
use crate::transport::{classify_openssh, Collision, Failure};
use crate::winproc::hide_console;
use std::{
    io::Write,
    process::{Command, Stdio},
    time::{Duration, Instant},
};

/// How long a listing or metadata request may take before the client is killed.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(20);

/// Transfers get far longer: a large file legitimately takes minutes.
const TRANSFER_TIMEOUT: Duration = Duration::from_secs(60 * 60);

/// The most of one run's output afterm will hold.
///
/// `read_to_string` on a pipe the far end can keep writing to is an
/// out-of-memory waiting for a broken or hostile server. Both are generous
/// against what the client actually produces: stdout is a listing, at roughly a
/// hundred bytes a row, so eight megabytes is a directory of some eighty
/// thousand entries; stderr is a handful of diagnostic lines. Reading does not
/// stop at the limit — see `read_capped` — because a pipe nobody drains is a
/// client that never exits.
const MAX_STDOUT: usize = 8 * 1024 * 1024;
const MAX_STDERR: usize = 256 * 1024;

/// Remote files over the OpenSSH client, one invocation per operation.
///
/// An earlier version kept a single `sftp -b -` process alive and framed
/// responses within its output stream. That could not be made to work across
/// platforms: batch mode enforces quiet mode, so the `sftp> ` command echo is
/// documented to be suppressed; the client's stdout is block-buffered when it
/// is not a console; and neither behaviour matches between builds. Three
/// separate Windows-only failures came out of assumptions verified only on
/// Linux.
///
/// Running the client once per operation removes the entire class of problem.
/// Closing stdin makes sftp process the batch and exit, and a process that
/// exits flushes its buffers, so afterm can simply read everything the client
/// wrote and parse it whole. Nothing depends on prompts, echoes, ordering or
/// flush behaviour.
///
/// The cost is a connection per operation. For a file pane that is a login per
/// navigation, not per keystroke, and it is invisible with key or agent
/// authentication.
pub struct RemoteFiles {
    host: String,
    user: String,
    port: u16,
    /// Extra client arguments. Tests use this to aim at a throwaway server;
    /// production passes nothing and gets stock OpenSSH behaviour.
    extra: Vec<String>,
    /// Which client to run. A field rather than a constant so a test can point
    /// one `RemoteFiles` at a stub without touching `PATH`, which is process
    /// wide and would reach every other test running at the same time.
    client: String,
    timeout: Duration,
    /// Remembered so opening the pane costs one round trip rather than two:
    /// the reachability check and the home directory are the same question.
    cwd: std::sync::Mutex<Option<String>>,
}

/// Everything the client wrote, once it had finished and exited — or as much of
/// it as afterm was willing to hold.
struct Output {
    stdout: String,
    stderr: String,
    /// Whether either capture hit its limit. A short answer that is not known
    /// to be short is the dangerous kind: an incomplete listing looks exactly
    /// like a directory with fewer files in it.
    truncated: bool,
}

fn quote(path: &str) -> Result<String, Failure> {
    if path.contains(['"', '\n', '\r']) {
        return Err(Failure::operation(
            "paths containing quotes or newlines are not supported",
        ));
    }
    Ok(format!("\"{}\"", path.replace('\\', "\\\\")))
}

impl RemoteFiles {
    pub fn new(host: &str, user: &str, port: u16) -> Self {
        Self {
            host: host.to_string(),
            user: user.to_string(),
            port,
            extra: Vec::new(),
            client: if cfg!(windows) { "sftp.exe" } else { "sftp" }.to_string(),
            timeout: REQUEST_TIMEOUT,
            cwd: std::sync::Mutex::new(None),
        }
    }

    #[cfg(test)]
    fn with_args(mut self, extra: &[&str], timeout: Duration) -> Self {
        self.extra = extra.iter().map(|value| value.to_string()).collect();
        self.timeout = timeout;
        self
    }

    #[cfg(test)]
    fn with_client(mut self, client: &str) -> Self {
        self.client = client.to_string();
        self
    }

    /// Run one batch of sftp commands to completion and return its output.
    ///
    /// `BatchMode=yes` means a host that only accepts an interactive password
    /// fails immediately and visibly rather than waiting on a prompt nobody can
    /// see. Key or agent authentication is what makes the file pane work.
    fn run(&self, script: &str, timeout: Duration) -> Result<Output, Failure> {
        let mut builder = Command::new(&self.client);
        let mut child = hide_console(&mut builder)
            .args(&self.extra)
            .args([
                "-o",
                "BatchMode=yes",
                "-o",
                "ConnectTimeout=10",
                "-P",
                &self.port.to_string(),
                "-b",
                "-",
                &format!("{}@{}", self.user, self.host),
            ])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|error| Failure::spawn(&self.client, &error))?;

        // Writing the script and then dropping stdin is what makes the client
        // run the batch and exit, which in turn flushes everything it wrote.
        {
            let mut stdin = child
                .stdin
                .take()
                .ok_or_else(|| Failure::internal("sftp stdin unavailable"))?;
            stdin
                .write_all(script.as_bytes())
                .and_then(|_| stdin.flush())
                .map_err(|error| {
                    Failure::internal(format!("could not send commands to sftp: {error}"))
                })?;
        }

        // Drain both pipes on their own threads. A full pipe buffer would
        // otherwise block the client before it could exit.
        let mut out_pipe = child
            .stdout
            .take()
            .ok_or_else(|| Failure::internal("sftp stdout unavailable"))?;
        let mut err_pipe = child
            .stderr
            .take()
            .ok_or_else(|| Failure::internal("sftp stderr unavailable"))?;
        let out_reader = std::thread::spawn(move || read_capped(&mut out_pipe, MAX_STDOUT));
        let err_reader = std::thread::spawn(move || read_capped(&mut err_pipe, MAX_STDERR));

        // Kill rather than wait forever. A stalled client would otherwise hold
        // a Tauri worker thread, and enough of those take the application down.
        let deadline = Instant::now() + timeout;
        loop {
            match child.try_wait() {
                Ok(Some(_)) => break,
                Ok(None) => {
                    if Instant::now() >= deadline {
                        let _ = child.kill();
                        let _ = child.wait();
                        return Err(Failure::unreachable(format!(
                            "sftp did not finish within {}s",
                            timeout.as_secs()
                        )));
                    }
                    std::thread::sleep(Duration::from_millis(25));
                }
                Err(error) => return Err(Failure::internal(format!("sftp failed: {error}"))),
            }
        }

        let (stdout, out_full) = out_reader.join().unwrap_or_default();
        let (stderr, err_full) = err_reader.join().unwrap_or_default();
        Ok(Output { stdout, stderr, truncated: out_full || err_full })
    }

    /// Prove the host is reachable and authentication works. This is the same
    /// question as "where is home", so it answers both and caches the result.
    pub fn check(&self) -> Result<(), Failure> {
        self.working_directory().map(|_| ())
    }

    /// Absolute path of the remote home directory.
    pub fn working_directory(&self) -> Result<String, Failure> {
        if let Some(path) = self.cwd.lock().unwrap().clone() {
            return Ok(path);
        }
        let output = self.run("pwd\n", self.timeout)?;
        let path = output
            .stdout
            .lines()
            .find_map(|line| {
                line.trim()
                    .strip_prefix("Remote working directory:")
                    .map(|value| value.trim().to_string())
            })
            .ok_or_else(|| describe_failure(&output))?;
        *self.cwd.lock().unwrap() = Some(path.clone());
        Ok(path)
    }

    pub fn list(&self, path: &str) -> Result<Listing, Failure> {
        let output = self.run(&format!("ls -l {}\n", quote(path)?), self.timeout)?;
        // A listing afterm had to cut short is not a listing. Showing the part
        // that fit would be a directory silently missing files, which is worse
        // than the error.
        if output.truncated {
            return Err(Failure::operation(format!(
                "{path} holds more than afterm will read in one listing"
            )));
        }
        let mut entries: Vec<FileEntry> =
            output.stdout.lines().filter_map(parse_entry).collect();

        // An unreadable or missing directory produces no rows at all, so an
        // empty listing plus a diagnostic is a real failure rather than an
        // empty directory.
        if entries.is_empty() && !output.stderr.trim().is_empty() {
            return Err(describe_failure(&output));
        }
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

    pub fn create_directory(&self, path: &str) -> Result<(), Failure> {
        self.expect_silence(&format!("mkdir {}\n", quote(path)?))
    }

    /// Narrow a directory to its owner: `0700`.
    pub fn make_private(&self, path: &str) -> Result<(), Failure> {
        self.expect_silence(&format!("chmod 700 {}\n", quote(path)?))
    }

    /// Rename one entry, refusing to replace anything already at `to`.
    ///
    /// The client's `rename` does not refuse by itself: against an OpenSSH
    /// server it uses the `posix-rename@openssh.com` extension, which is
    /// `rename(2)` and replaces the destination. (An earlier comment here said
    /// the opposite; it was describing the legacy protocol request, which the
    /// client only sends to servers without the extension.) Checking first and
    /// renaming second is therefore check-then-act: a file that appears at `to`
    /// in between is overwritten by a rename that was told not to overwrite
    /// anything.
    ///
    /// So a file is moved by a hard link instead, which the server performs
    /// with `link(2)` and which refuses an existing destination atomically.
    /// A directory cannot be hard-linked on any POSIX filesystem, so that one
    /// case keeps the old sequence; `rename(2)` still refuses to put a
    /// directory over a file or over a directory with anything in it, which
    /// leaves the narrow case of an *empty* directory appearing at `to` between
    /// the check and the move. That gap is named here rather than left
    /// unexamined — the native transport has none, because its `rename` refuses
    /// an existing target outright.
    pub fn rename(&self, from: &str, to: &str) -> Result<(), Failure> {
        if self.names_a_file(from)? {
            return self.link_into(from, to).map(|_| ()).and_then(|()| self.unlink_source(from, to));
        }
        if self.exists(to)? {
            return Err(Failure::exists(format!("{to} already exists")));
        }
        self.rename_replacing(from, to)
    }

    /// Whether `path` names something that is not a directory.
    ///
    /// `ls -l` on a file prints one row for the path it was given; on a
    /// directory it prints the contents by their own names, and nothing at all
    /// when there are none. So the answer is whether any row is about the path
    /// that was asked about.
    fn names_a_file(&self, path: &str) -> Result<bool, Failure> {
        let output = self.run(&format!("ls -l {}\n", quote(path)?), self.timeout)?;
        Ok(output
            .stdout
            .lines()
            .filter_map(parse_entry)
            .any(|entry| entry.path == path))
    }

    /// Give `from` a second name at `to`, atomically and without replacing
    /// anything.
    ///
    /// `ln` without `-s` is `hardlink@openssh.com`, which the server performs
    /// with `link(2)`: it fails with `EEXIST` rather than replacing, and there
    /// is no window between deciding and doing. A server that does not offer
    /// the extension cannot give that guarantee, and says so rather than
    /// falling back to a check and a replacing rename.
    fn link_into(&self, from: &str, to: &str) -> Result<(), Failure> {
        let Err(refused) = self.expect_silence(&format!("ln {} {}\n", quote(from)?, quote(to)?))
        else {
            return Ok(());
        };
        if self.exists(to)? {
            return Err(Failure::exists(format!("{to} already exists")));
        }
        Err(Failure::new(
            refused.cause,
            format!(
                "this server would not put {to} in place without a chance of replacing whatever \
                 arrived there first, which needs hardlink@openssh.com: {}",
                refused.message
            ),
        ))
    }

    /// Drop the name a hard link was made from, which is what turns the link
    /// into a move.
    fn unlink_source(&self, from: &str, to: &str) -> Result<(), Failure> {
        self.delete(from, false).map_err(|failure| {
            Failure::new(
                failure.cause,
                format!(
                    "{to} is in place, but {from} is still there and could not be removed: {}",
                    failure.message
                ),
            )
        })
    }

    /// The permission bits at `path`, if anything is there.
    fn mode_of(&self, path: &str) -> Result<Option<u32>, Failure> {
        let output = self.run(&format!("ls -l {}\n", quote(path)?), self.timeout)?;
        Ok(output
            .stdout
            .lines()
            .find(|line| parse_entry(line).is_some_and(|entry| entry.path == path))
            .and_then(parse_mode))
    }

    /// The client's own `rename`: atomic, and it replaces the destination.
    fn rename_replacing(&self, from: &str, to: &str) -> Result<(), Failure> {
        self.expect_silence(&format!("rename {} {}\n", quote(from)?, quote(to)?))
    }

    /// Whether anything is at `path`. Asked with `ls -l`, which prints a row
    /// for a file and a diagnostic for nothing; an empty directory prints
    /// neither and reads as absent, which the rename that follows then
    /// refuses on its own.
    pub fn exists(&self, path: &str) -> Result<bool, Failure> {
        let output = self.run(&format!("ls -l {}\n", quote(path)?), self.timeout)?;
        if output.stdout.lines().any(|line| parse_entry(line).is_some()) {
            return Ok(true);
        }
        let stderr = output.stderr.trim();
        if stderr.is_empty() || stderr.contains("not found") || stderr.contains("No such file") {
            return Ok(false);
        }
        Err(describe_failure(&output))
    }

    /// Remove one entry. `rmdir` refuses a directory with anything in it;
    /// emptying one first is `folder::remove`, which calls this per entry.
    pub fn delete(&self, path: &str, directory: bool) -> Result<(), Failure> {
        let verb = if directory { "rmdir" } else { "rm" };
        self.expect_silence(&format!("{verb} {}\n", quote(path)?))
    }

    /// Run a batch that is expected to say nothing, and turn anything it did
    /// say into a failure. The same rule `transfer` uses, at the request
    /// timeout rather than the transfer one.
    fn expect_silence(&self, script: &str) -> Result<(), Failure> {
        let output = self.run(script, self.timeout)?;
        if output.stderr.trim().is_empty() {
            Ok(())
        } else {
            Err(describe_failure(&output))
        }
    }

    pub fn download(&self, remote: &str, local: &str) -> Result<(), Failure> {
        self.transfer(&format!("get {} {}\n", quote(remote)?, quote(local)?))
    }

    /// Send `local` into `directory` as `name`, through a private staging
    /// directory, and say where it landed.
    ///
    /// The staging directory is `staging_name`: unguessable, and narrowed to
    /// `0700` before any content is sent. That is what protects the bytes.
    /// `put` creates its file with the *local* file's mode, and the batch
    /// language cannot pass attributes to a create — so on this transport the
    /// staging file wears `0644` for the whole of a transfer whenever the
    /// local file does, and a `chmod` afterwards is far too late. It was: a
    /// `0600` remote file replaced from a `0644` local one left 40 KB of it
    /// readable to anyone who could reach the destination directory. An
    /// unguessable *name* does not help once a file exists, because a name is
    /// discoverable by listing; an unguessable directory that nobody but its
    /// owner may enter does, because there is nothing to list.
    ///
    /// It also buys an exclusive create, which the old single staging file
    /// never had: `mkdir` fails when anything is already at the name, and
    /// `put` did not.
    ///
    /// Every path out of here — a refused transfer, a refused chmod, a taken
    /// name, a commit that would not go — leaves through the same cleanup. It
    /// used to be only the last of those: a `put` that failed returned before
    /// the cleanup the commit failures went through, so a transfer interrupted
    /// halfway left its staging file on the host under a name nobody would ever
    /// look for.
    pub fn upload(
        &self,
        local: &str,
        directory: &str,
        name: &str,
        collision: Collision,
    ) -> Result<String, Failure> {
        let stage = join(directory, &staging_name());
        let part = join(&stage, STAGING_FILE);
        match self.deliver(local, directory, name, collision, &stage, &part) {
            // The commit is the install; dropping what is left of the staging
            // directory is what turns it into a move. Best effort, and
            // unreported: the commit succeeding has already proved this
            // directory is writable by this user and that the file is theirs,
            // so a `rm` of it failing is not a case that arises — and if it
            // somehow did, the file is correctly in place and what remains is
            // a hidden directory beside it. Failing the upload over that would
            // be reporting a success as a failure. The pane's own `rename`
            // does report it, because there the leftover is a name the user
            // can see.
            Ok(Landed { path, linked }) => {
                let _ = self.sweep(&stage, &part, linked);
                Ok(path)
            }
            Err(failure) => Err(self.discard(&stage, &part, failure)),
        }
    }

    /// Take the staging directory away once the upload is over.
    ///
    /// One batch, because both commands have to succeed for the directory to
    /// go and the second is meaningless without the first. `linked` says
    /// whether the staging file is still there: a hard link left it in place
    /// under two names, and a replacing rename moved it out.
    fn sweep(&self, stage: &str, part: &str, linked: bool) -> Result<(), Failure> {
        let remove = if linked { format!("rm {}\n", quote(part)?) } else { String::new() };
        self.expect_silence(&format!("{remove}rmdir {}\n", quote(stage)?))
    }

    /// The upload itself, in one batch per attempt.
    ///
    /// The staging directory, `put`, `chmod` and the commit go to the client
    /// together: batch mode stops at the first command that fails, so the
    /// sequence is all-or-part with the part cleaned up, and the whole upload
    /// costs one login rather than one per step.
    fn deliver(
        &self,
        local: &str,
        directory: &str,
        name: &str,
        collision: Collision,
        stage: &str,
        part: &str,
    ) -> Result<Landed, Failure> {
        // Decided before anything is sent, so a destination afterm will not
        // replace is refused before the transfer rather than after it.
        let mode = self.intended_mode(local, directory, name, collision)?;
        // The order is the point. `mkdir` and `chmod 700` come before `put`,
        // so the containing directory refuses everyone but this account before
        // a byte of content exists; `put` then creates the staging file under
        // whatever mode it likes, somewhere nobody else can reach it.
        //
        // The `chmod` after the `put` is no longer what protects anything —
        // the directory is — and it stays because it costs one command to know
        // that the staging file is never wider than the strictest thing afterm
        // would install, whatever happens between here and the commit. The
        // destination's own mode goes on later, in the commit, so the file
        // never exists under its real name wearing the wrong one.
        //
        // The one window left is between `mkdir` and `chmod`, and it holds an
        // empty directory under a 128-bit name. There is no way to close it
        // from here — the batch language cannot pass a mode to `mkdir` — and
        // nothing is in it to read.
        let send = format!(
            "mkdir {}\nchmod 700 {}\nput {} {}\nchmod {:03o} {}\n",
            quote(stage)?,
            quote(stage)?,
            quote(local)?,
            quote(part)?,
            STAGING_MODE,
            quote(part)?
        );

        match collision {
            Collision::Replace => {
                let destination = join(directory, name);
                // The client's own `rename` is `posix-rename@openssh.com`,
                // which is `rename(2)`: atomic, and it replaces. That is
                // exactly what Replace asked for.
                self.transfer(&format!(
                    "{send}chmod {mode:03o} {}\nrename {} {}\n",
                    quote(part)?,
                    quote(part)?,
                    quote(&destination)?
                ))?;
                // A rename leaves no second name to tidy up.
                Ok(Landed { path: destination, linked: false })
            }
            Collision::Fail => {
                let destination = join(directory, name);
                self.commit(&send, part, &destination, mode)?;
                Ok(Landed { path: destination, linked: true })
            }
            Collision::Rename => {
                let mut sent = false;
                for attempt in 0..100 {
                    let candidate =
                        if attempt == 0 { name.to_string() } else { numbered(name, attempt) };
                    let path = join(directory, &candidate);
                    // The transfer happens once; only the commit is retried.
                    let script = if sent { String::new() } else { send.clone() };
                    sent = true;
                    match self.commit(&script, part, &path, mode) {
                        Ok(()) => return Ok(Landed { path, linked: true }),
                        Err(failure) if failure.cause == crate::transport::Cause::Exists => continue,
                        Err(failure) => return Err(failure),
                    }
                }
                Err(Failure::operation(format!("too many files called {name} in {directory}")))
            }
        }
    }

    /// Put the staging file at `destination` without replacing anything there.
    ///
    /// The link is the last command in the batch, and deliberately: a batch
    /// that also dropped the staging name could fail at either step, and the
    /// two are indistinguishable afterwards — both leave the destination
    /// occupied and the staging name present, so a `rm` that failed after a
    /// successful `ln` would be reported as "already exists" for an upload
    /// that had in fact just worked. Ending here makes a failure unambiguous.
    fn commit(
        &self,
        send: &str,
        part: &str,
        destination: &str,
        mode: u32,
    ) -> Result<(), Failure> {
        let script = format!(
            "{send}chmod {mode:03o} {}\nln {} {}\n",
            quote(part)?,
            quote(part)?,
            quote(destination)?
        );
        let Err(refused) = self.transfer(&script) else {
            return Ok(());
        };
        // The batch stopped somewhere. Where matters: a taken name is the one
        // failure the pane offers a choice about. A host that will not answer
        // the question keeps the original failure, which is the more useful of
        // the two — asking again is how a disconnection turns into "already
        // exists".
        if self.exists(destination).unwrap_or(false) {
            return Err(Failure::exists(format!("{destination} already exists")));
        }
        // Nothing is at the destination, so the batch stopped earlier — or the
        // server has no `hardlink@openssh.com` to commit with. Either way the
        // upload is reported rather than retried with a rename that would
        // replace whatever appeared next.
        Err(refused)
    }

    /// What the uploaded file must end up as: what it replaces, or the local
    /// file's own mode where there is one to copy.
    fn intended_mode(
        &self,
        local: &str,
        directory: &str,
        name: &str,
        collision: Collision,
    ) -> Result<u32, Failure> {
        if collision == Collision::Replace {
            if let Some(existing) = self.mode_of(&join(directory, name))? {
                return carried(existing).map_err(|bits| {
                    Failure::operation(format!(
                        "{name} has special permission bits ({bits}), which afterm will not \
                         re-apply to a replacement; change them on the host first"
                    ))
                });
            }
        }
        Ok(local_mode(std::fs::metadata(local).ok().as_ref()))
    }

    /// Remove a staging directory that should not be there, and say so when it
    /// cannot be.
    ///
    /// Best effort, and never by pattern: the two exact paths afterm created
    /// are the only things it will delete. The batch may have stopped anywhere
    /// — before the `mkdir`, after it, part way through the transfer — so the
    /// file goes first and the directory second, and each is allowed to be
    /// missing. A host that has gone away cannot be asked to tidy up, so the
    /// failure carries the path instead: a name nobody could guess is also a
    /// name nobody would find.
    ///
    /// "May be" rather than "is", deliberately. When the removal fails because
    /// the host is unreachable, afterm does not know whether the transfer got
    /// as far as creating anything, and saying so is more use than guessing
    /// either way.
    fn discard(&self, stage: &str, part: &str, failure: Failure) -> Failure {
        // The same batch the successful path uses, tried first: one login
        // clears the usual case, which is a transfer that stopped part way
        // through. Batch mode stops at its first failure, so a staging file
        // that was never created leaves the `rmdir` unrun — and that is the
        // only reason for a second ask, rather than one being made every time
        // on a path where the host may already be answering with timeouts.
        if self.sweep(stage, part, true).is_ok() {
            return failure;
        }
        match self.delete(stage, true) {
            Ok(()) => failure,
            // Never created, or already gone. Nothing to report.
            Err(removal) if absent(&removal) => failure,
            Err(_) => Failure::new(
                failure.cause,
                format!("{}; a partial upload may be left on the host at {stage}", failure.message),
            ),
        }
    }

    fn transfer(&self, script: &str) -> Result<(), Failure> {
        let output = self.run(script, TRANSFER_TIMEOUT)?;
        if output.stderr.trim().is_empty() {
            Ok(())
        } else {
            Err(describe_failure(&output))
        }
    }
}


/// Where an upload ended up, and whether the staging name is still a second
/// name for it.
struct Landed {
    path: String,
    linked: bool,
}

/// The name of a staging directory nobody can have prepared in advance.
///
/// A *predictable* staging name was a way for anyone who could write to the
/// destination directory to redirect the upload onto a file of their choosing:
/// `put` opens the remote name with create-and-truncate, an OpenSSH server
/// follows a symlink it finds there, so a symlink planted at
/// `.name.afterm-part` had the next upload of `name` write through it.
/// Reproduced against a local `sftp -D sftp-server`, which overwrote the
/// symlink's target and reported success; the rename that followed would then
/// have installed the symlink itself under the destination name.
///
/// So the remedy is `mkstemp`'s: a name that cannot be guessed cannot be
/// prepared. 128 bits from `rand`'s thread generator, which is seeded from the
/// operating system and is the one russh already draws its own randomness
/// from. Naming a directory rather than a file also gets what the batch
/// language would not otherwise give — `mkdir` refuses a name that is already
/// taken, where `put` happily wrote through whatever it found — and it is what
/// lets the content be protected by a mode rather than only by a name, which
/// is the whole of `upload`'s note above.
///
/// One thing this does not do: it carries no part of `name`, which is
/// deliberate. A 250-character name plus a suffix is a name the server
/// rejects, and a staging name that is always the same length cannot do that.
///
/// A staging directory is left behind only if the connection dies between the
/// transfer and the commit. It is a dotfile, and unlike the old fixed name it
/// is not reused by the next attempt.
fn staging_name() -> String {
    format!(".afterm-{:032x}", rand::random::<u128>())
}

/// What the staging file inside it is called. Fixed, and it can be: nothing
/// but this upload may enter the directory it is in, so nothing can collide
/// with it, and a name that carries no part of the destination's cannot push
/// a 250-character upload past what the server will accept.
const STAGING_FILE: &str = "part";

/// Whether a failed removal is a path that was not there. Both spellings,
/// because the client's wording differs between the server's own message and
/// its own.
fn absent(failure: &Failure) -> bool {
    failure.message.contains("No such file") || failure.message.contains("not found")
}

/// Turn whatever the client wrote into a typed failure. The stderr text is the
/// useful part, and `classify_openssh` decides what kind of failure it
/// describes: a refused credential, a rejected host key, an unreachable host,
/// or an operation that simply did not work.
fn describe_failure(output: &Output) -> Failure {
    let stderr = output.stderr.trim();
    if !stderr.is_empty() {
        return classify_openssh(stderr);
    }
    let stdout = output.stdout.trim();
    if !stdout.is_empty() {
        return Failure::operation(format!(
            "unexpected sftp output: {}",
            stdout.lines().take(2).collect::<Vec<_>>().join("; ")
        ));
    }
    Failure::operation("sftp produced no output")
}

/// Parse one `sftp ls -l` row. The format is produced by the OpenSSH client
/// from SFTP protocol attributes, not by a remote `ls`, so it is stable across
/// servers: permissions, links, owner, group, size, month, day, time, path.
pub fn parse_entry(line: &str) -> Option<FileEntry> {
    let (fields, path) = split_fields(line, 8)?;
    let permissions = fields[0];
    let kind = match permissions.chars().next()? {
        'd' => EntryKind::Directory,
        'l' => EntryKind::Link,
        '-' => EntryKind::File,
        _ => return None,
    };
    let size = fields[4].parse::<u64>().ok();
    let name = path.rsplit('/').next().unwrap_or(path).to_string();
    if name.is_empty() || name == "." || name == ".." {
        return None;
    }
    Some(FileEntry {
        name,
        path: path.to_string(),
        kind,
        size: (kind == EntryKind::File).then_some(size).flatten(),
        modified: Some(format!("{} {} {}", fields[5], fields[6], fields[7])),
    })
}

/// The mode bits of one `sftp ls -l` row.
///
/// The client renders the protocol's `permissions` attribute the way `ls`
/// does, so reading it back is the only way to learn a remote file's mode
/// through the batch language — there is no `stat`. `s`, `S`, `t` and `T`
/// stand where the execute bits go and carry setuid, setgid and the sticky bit
/// with them; they are read here so that `carried` can refuse them rather than
/// having them silently rounded off.
fn parse_mode(line: &str) -> Option<u32> {
    let bits: Vec<char> = line.split_whitespace().next()?.chars().collect();
    if bits.len() < 10 {
        return None;
    }
    let mut mode = 0_u32;
    for (index, position) in [1_usize, 2, 3, 4, 5, 6, 7, 8, 9].into_iter().enumerate() {
        let character = *bits.get(position)?;
        let value = 1 << (8 - index);
        match character {
            'r' | 'w' | 'x' => mode |= value,
            // The three special bits live in the execute column of their triple,
            // upper case when the execute bit itself is off.
            's' | 't' => mode |= value | (0o4000 >> (index / 3)),
            'S' | 'T' => mode |= 0o4000 >> (index / 3),
            '-' => {}
            _ => return None,
        }
    }
    Some(mode)
}

/// Split `line` into `count` whitespace-delimited fields plus the untouched
/// remainder, so a filename containing spaces survives intact.
fn split_fields(line: &str, count: usize) -> Option<(Vec<&str>, &str)> {
    let mut fields = Vec::with_capacity(count);
    let mut rest = line;
    for _ in 0..count {
        rest = rest.trim_start();
        let end = rest.find(char::is_whitespace)?;
        fields.push(&rest[..end]);
        rest = &rest[end..];
    }
    let remainder = rest.trim_start();
    (!remainder.is_empty()).then_some((fields, remainder))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_file_row() {
        let entry = parse_entry("-rw-rw-r--    ? arya     arya            6 Sep  4 01:50 /home/arya/afile.txt").unwrap();
        assert_eq!(entry.name, "afile.txt");
        assert_eq!(entry.path, "/home/arya/afile.txt");
        assert_eq!(entry.kind, EntryKind::File);
        assert_eq!(entry.size, Some(6));
        assert_eq!(entry.modified.as_deref(), Some("Sep 4 01:50"));
    }

    #[test]
    fn parses_directories_and_links() {
        let dir = parse_entry("drwxrwxr-x    ? arya     arya         4096 Sep  4 01:51 /srv/logs").unwrap();
        assert_eq!(dir.kind, EntryKind::Directory);
        // Directories report no size; 4096 is the inode size, not content.
        assert_eq!(dir.size, None);

        let link = parse_entry("lrwxrwxrwx    ? arya     arya            9 Sep  4 01:50 /srv/current").unwrap();
        assert_eq!(link.kind, EntryKind::Link);
    }

    #[test]
    fn keeps_spaces_inside_filenames() {
        let entry = parse_entry("-rw-rw-r--    ? arya     arya            2 Sep  4 01:51 /srv/file with space.txt").unwrap();
        assert_eq!(entry.name, "file with space.txt");
        assert_eq!(entry.path, "/srv/file with space.txt");
    }

    #[test]
    fn rejects_noise_that_is_not_a_listing_row() {
        assert!(parse_entry("Can't ls: \"/nope\" not found").is_none());
        assert!(parse_entry("").is_none());
        assert!(parse_entry("sftp> -ls -l /tmp").is_none());
        assert!(parse_entry("Remote working directory: /home/arya").is_none());
    }

    #[test]
    fn refuses_paths_that_could_break_out_of_quoting() {
        assert!(quote("/tmp/ok").is_ok());
        assert!(quote("/tmp/\"evil").is_err());
        assert!(quote("/tmp/evil\nrm -rf /").is_err());
    }

    #[test]
    fn reports_stderr_rather_than_a_bare_failure() {
        let output = Output {
            stdout: String::new(),
            stderr: "arya@host: Permission denied (publickey).\n".into(),
            truncated: false,
        };
        let failure = describe_failure(&output);
        assert!(failure.message.contains("Permission denied"));
        assert_eq!(failure.cause, crate::transport::Cause::Auth);

        let silent =
            Output { stdout: String::new(), stderr: String::new(), truncated: false };
        assert_eq!(describe_failure(&silent).message, "sftp produced no output");
    }

    /// A client that accepts the connection and never exits must be killed, not
    /// waited on. A stalled read previously held a Tauri worker thread, and
    /// because the status bar polled the same path the pool filled up and the
    /// whole application stopped responding.
    ///
    /// The stub is named directly rather than put on `PATH`. `PATH` belongs to
    /// the whole process, and the tests below run at the same time as this one
    /// and spawn the real client: they took the stub instead, waited on its
    /// `sleep`, and failed on files it had never written.
    ///
    /// Unix-only for the stub, which is a shell script. What it proves — that
    /// a client which never exits is killed rather than waited on — is about
    /// afterm's own timeout loop, not about the client.
    #[cfg(unix)]
    #[test]
    fn kills_a_client_that_never_finishes() {
        use std::os::unix::fs::PermissionsExt;

        let stub_dir = crate::tempdir::Dir::new("sftp-stub");
        let stub = stub_dir.path().join("sftp");
        std::fs::write(&stub, "#!/bin/sh\nsleep 300\n").unwrap();
        std::fs::set_permissions(&stub, std::fs::Permissions::from_mode(0o755)).unwrap();

        let started = Instant::now();
        let outcome = RemoteFiles::new("example.invalid", "nobody", 22)
            .with_args(&[], Duration::from_millis(300))
            .with_client(&stub.to_string_lossy())
            .check();

        let error = outcome.err().expect("a client that never finishes must not succeed");
        assert!(error.message.contains("did not finish"), "unexpected error: {error}");
        assert!(started.elapsed() < Duration::from_secs(5), "took too long: {:?}", started.elapsed());
    }

    #[test]
    fn a_temporary_name_is_unguessable_and_always_the_same_length() {
        let one = staging_name();
        let two = staging_name();
        // The whole defence: a name nobody can prepare anything at, because
        // nobody knows it until `mkdir` has already claimed it.
        assert_ne!(one, two);
        assert!(one.starts_with(".afterm-"), "{one}");
        assert_eq!(one.len(), ".afterm-".len() + 32);
        assert_eq!(one.len(), two.len(), "a long filename must not lengthen the temporary");
        // Hex and one dot: nothing in it reads as a path, a pattern, or a
        // quote the batch language would take apart.
        assert!(one[8..40].chars().all(|c| c.is_ascii_hexdigit()), "{one}");
    }

    /// The order the finding is about: the staging directory exists and is
    /// `0700` before `put` is allowed to create anything inside it.
    ///
    /// Read off the batch itself rather than inferred from the end state,
    /// because the end state was already right when the bug was live — the
    /// old sequence narrowed the staging file the moment the transfer
    /// finished, and the exposure was everything before that moment. A stub
    /// client that keeps what it was sent is the only way to assert on an
    /// ordering rather than on a result.
    ///
    /// Unix-only for the stub, which is a shell script; the ordering it checks
    /// is in a `format!` and is the same on every platform.
    #[cfg(unix)]
    #[test]
    fn the_staging_directory_is_private_before_put_is_asked_for_anything() {
        use std::os::unix::fs::PermissionsExt;

        let dir = crate::tempdir::Dir::new("sftp-order");
        let log = dir.path().join("batch.txt");
        let stub = dir.path().join("sftp");
        // Quoted: the temporary directory's name carries the thread id, and
        // an unquoted `ThreadId(1)` is a shell syntax error.
        std::fs::write(&stub, format!("#!/bin/sh\ncat >> \"{}\"\n", log.display())).unwrap();
        std::fs::set_permissions(&stub, std::fs::Permissions::from_mode(0o755)).unwrap();
        let local = dir.path().join("payload.txt");
        std::fs::write(&local, b"payload").unwrap();

        RemoteFiles::new("example.invalid", "nobody", 22)
            .with_args(&[], Duration::from_secs(10))
            .with_client(&stub.to_string_lossy())
            .upload(&local.to_string_lossy(), "/srv", "target.txt", Collision::Fail)
            .expect("a silent client is a client that agreed to everything");

        let script = std::fs::read_to_string(&log).expect("the stub kept the batch");
        let at = |needle: &str| script.find(needle).unwrap_or_else(|| panic!("no {needle} in {script}"));
        assert!(at("mkdir ") < at("chmod 700 "), "{script}");
        assert!(at("chmod 700 ") < at("put "), "{script}");
        // And the file goes inside the directory that was just narrowed,
        // rather than beside it where the mode would protect nothing.
        let stage = script.lines().next().unwrap().trim_start_matches("mkdir ").trim_matches('"');
        let put = script.lines().find(|line| line.starts_with("put ")).unwrap();
        assert!(put.ends_with(&format!("\"{stage}/{STAGING_FILE}\"")), "{put}");
    }

    /// The same property watched from outside, against the real client and
    /// server: through a whole replacement, from the first thing that appears
    /// in the destination directory to the last thing that leaves it, nothing
    /// afterm staged is reachable by anyone else.
    ///
    /// The file is large so that the transfer is a window rather than an
    /// instant, and the watcher samples it throughout. What it requires is the
    /// two halves of the finding: the staging directory is never anything but
    /// `0700`, and no staging *file* is ever visible in the destination
    /// directory at all — which is what a `0644` staging file next to the
    /// destination was.
    ///
    /// Unix-only: it is an assertion about Unix mode bits, and about a
    /// traversal check NTFS does not have.
    #[cfg(unix)]
    #[test]
    fn nothing_staged_is_reachable_by_anyone_else_while_a_replacement_runs() {
        use std::os::unix::fs::PermissionsExt;
        use std::sync::atomic::{AtomicBool, Ordering};
        use std::sync::Arc;

        let dir = crate::tempdir::Dir::new("sftp-window");
        let (here, there) = (dir.path().join("local"), dir.path().join("remote"));
        std::fs::create_dir_all(&here).unwrap();
        std::fs::create_dir_all(&there).unwrap();
        let Some(files) = fixture(&there) else { return };
        let root = sftp_path(&there);

        // The exact shape of the reproduction: a private file on the host,
        // replaced from a world-readable local one, so the staging file wears
        // `0644` for the whole transfer unless something else protects it.
        let private = there.join("secret.txt");
        std::fs::write(&private, b"original").unwrap();
        std::fs::set_permissions(&private, std::fs::Permissions::from_mode(0o600)).unwrap();
        let local = here.join("secret.txt");
        std::fs::write(&local, vec![b'x'; 64 * 1024 * 1024]).unwrap();
        std::fs::set_permissions(&local, std::fs::Permissions::from_mode(0o644)).unwrap();

        let watching = Arc::new(AtomicBool::new(true));
        let stop = Arc::clone(&watching);
        let scene = there.clone();
        let watcher = std::thread::spawn(move || {
            // Sightings of the staging directory with something actually in
            // it, which is the only moment there is anything to protect.
            let mut staged = 0_usize;
            let mut wrong: Vec<String> = Vec::new();
            while stop.load(Ordering::SeqCst) {
                for entry in std::fs::read_dir(&scene).into_iter().flatten().flatten() {
                    let name = entry.file_name().to_string_lossy().to_string();
                    if !name.starts_with(".afterm-") {
                        continue;
                    }
                    let path = entry.path();
                    let mode = || {
                        std::fs::metadata(&path).ok().map(|m| m.permissions().mode() & 0o777)
                    };
                    if !entry.metadata().map(|m| m.is_dir()).unwrap_or(true) {
                        wrong.push(format!("{name} is a staging file, mode {:?}", mode()));
                        continue;
                    }
                    let before = mode();
                    let inside = std::fs::read_dir(&path)
                        .map(|entries| entries.flatten().count())
                        .unwrap_or(0);
                    if inside == 0 {
                        // The one window there is: `mkdir` has run and
                        // `chmod 700` has not. An empty directory discloses
                        // nothing, which is why the order is mkdir, chmod,
                        // put and not mkdir, put, chmod.
                        continue;
                    }
                    staged += 1;
                    // Read again, because the first reading and the listing
                    // are not one operation: a mode seen as wide just before
                    // the `chmod` landed is not a mode that ever held content.
                    // A mode that is still wide now is.
                    // Capped: the poll is fast and a real failure lasts for
                    // the whole transfer, so a handful of them says everything
                    // a thousand identical lines would.
                    if before != Some(0o700) && mode() != Some(0o700) && wrong.len() < 3 {
                        wrong.push(format!("{name} held content at mode {before:?}"));
                    }
                }
                std::thread::sleep(Duration::from_micros(200));
            }
            (staged, wrong)
        });

        files
            .upload(&sftp_path(&local), &root, "secret.txt", Collision::Replace)
            .expect("replace");
        watching.store(false, Ordering::SeqCst);
        let (staged, wrong) = watcher.join().expect("the watcher");

        assert!(wrong.is_empty(), "reachable while the transfer ran: {wrong:?}");
        assert!(staged > 0, "the transfer finished before its staging file could be watched");
        // And the replacement still did its job, under the mode it replaced.
        assert_eq!(std::fs::metadata(&private).unwrap().permissions().mode() & 0o777, 0o600);
        assert_eq!(std::fs::metadata(&private).unwrap().len(), 64 * 1024 * 1024);
        assert!(leftovers(&there).is_empty(), "{:?}", leftovers(&there));
    }

    /// Where each platform keeps the server binary, and whether this machine
    /// has one at all.
    fn sftp_server() -> Option<String> {
        let mut places: Vec<String> = Vec::new();
        if cfg!(windows) {
            // Win32-OpenSSH, as the optional feature and as the separate
            // install. Both are the same binary; which one a machine has says
            // how OpenSSH got onto it, not what it can do.
            let root = std::env::var("SystemRoot").unwrap_or_else(|_| "C:\\Windows".into());
            places.push(format!("{root}\\System32\\OpenSSH\\sftp-server.exe"));
            if let Ok(programs) = std::env::var("ProgramFiles") {
                places.push(format!("{programs}\\OpenSSH\\sftp-server.exe"));
            }
        } else {
            for path in [
                "/usr/lib/openssh/sftp-server",
                "/usr/libexec/openssh/sftp-server",
                "/usr/libexec/sftp-server",
                "/usr/lib/ssh/sftp-server",
            ] {
                places.push(path.to_string());
            }
        }
        places.into_iter().find(|path| std::path::Path::new(path).exists())
    }

    /// A path as the fixture's server will read it back.
    ///
    /// Forward slashes on both platforms. Win32-OpenSSH's server accepts a
    /// Windows path and prints a POSIX one, and afterm matches a listing row
    /// against the path it asked about — so handing it backslashes would make
    /// every row look like a different file. It also keeps `quote`'s backslash
    /// escaping out of the picture entirely.
    fn sftp_path(path: &std::path::Path) -> String {
        path.to_string_lossy().replace('\\', "/")
    }

    /// A client talking to a real `sftp-server` started as a subprocess of
    /// this test: the client's own `-D` mode, no ssh, no network, no
    /// credentials, and the destination argument ignored.
    ///
    /// It is the same client binary and the same server implementation the
    /// fallback transport meets in production, which is what makes the cases
    /// below demonstrations rather than arguments. Everything it touches is
    /// under a temporary directory this process owns.
    ///
    /// `None` is a machine that cannot host the fixture, and the test skips.
    /// Two ways that happens: no server binary, and — the reason this is a
    /// probe rather than a file-exists check — a client whose `-D` does not
    /// start one here. `-D` forks the server as a subprocess, and Win32-OpenSSH
    /// has no `fork`, so whether it works there is a question only a run can
    /// answer. Skipping rather than failing keeps that from breaking a build
    /// over something no host is affected by.
    fn fixture(root: &std::path::Path) -> Option<RemoteFiles> {
        let Some(server) = sftp_server() else {
            eprintln!("no sftp-server on this machine; skipping");
            return None;
        };
        let files = RemoteFiles::new("localhost", "nobody", 22)
            .with_args(&["-D", &server], TRANSFER_TIMEOUT);
        match files.list(&sftp_path(root)) {
            Ok(_) => Some(files),
            Err(reason) => {
                eprintln!("`sftp -D {server}` does not work here ({reason}); skipping");
                None
            }
        }
    }

    /// The finding this exists for: with a *predictable* temporary name,
    /// anyone who could write to the destination directory could plant a
    /// symlink at it and have the next upload write through it, over a file of
    /// their choosing. `put` follows the link — proved by running this test
    /// against the old name — and the collision checks never see it, because
    /// they look at the destination rather than at the temporary.
    ///
    /// Unix-only: it turns on a symlink being followed by the server's `open`,
    /// and a Windows fixture would be answering a question about Windows
    /// symlink semantics rather than about the temporary name.
    #[cfg(unix)]
    #[test]
    fn an_upload_does_not_write_through_a_symlink_left_at_the_old_temporary_name() {
        let dir = crate::tempdir::Dir::new("sftp-symlink");
        // Two sides, as in life: what is being sent, and where it is going.
        let (here, there) = (dir.path().join("local"), dir.path().join("remote"));
        std::fs::create_dir_all(&here).unwrap();
        std::fs::create_dir_all(&there).unwrap();
        let Some(files) = fixture(&there) else { return };
        let root = sftp_path(&there);
        let victim = there.join("victim.txt");
        std::fs::write(&victim, b"precious").unwrap();
        let local = here.join("payload.txt");
        std::fs::write(&local, b"payload").unwrap();
        std::os::unix::fs::symlink(&victim, there.join(".target.txt.afterm-part")).unwrap();

        let landed = files
            .upload(&sftp_path(&local), &root, "target.txt", Collision::Fail)
            .expect("upload");

        assert_eq!(landed, format!("{root}/target.txt"));
        assert_eq!(std::fs::read(&victim).unwrap(), b"precious", "written through the symlink");
        assert_eq!(std::fs::read(there.join("target.txt")).unwrap(), b"payload");
        // The plant is untouched: not written through, and not renamed into
        // place either, which is the other half of what it was for.
        let planted = std::fs::symlink_metadata(there.join(".target.txt.afterm-part")).unwrap();
        assert!(planted.file_type().is_symlink());
        assert!(leftovers(&there).is_empty(), "{:?}", leftovers(&there));
    }

    /// The transfer path end to end against the real client and server: a free
    /// name, a taken one, and both answers to a taken one. The temporary is
    /// gone every time.
    #[test]
    fn the_fallback_transport_commits_uploads_the_way_the_pane_expects() {
        let dir = crate::tempdir::Dir::new("sftp-upload");
        let (here, there) = (dir.path().join("local"), dir.path().join("remote"));
        std::fs::create_dir_all(&here).unwrap();
        std::fs::create_dir_all(&there).unwrap();
        let Some(files) = fixture(&there) else { return };
        let root = sftp_path(&there);
        let local = here.join("report.txt");
        std::fs::write(&local, b"first").unwrap();
        let send = |collision| files.upload(&sftp_path(&local), &root, "report.txt", collision);

        assert_eq!(send(Collision::Fail).expect("a free name"), format!("{root}/report.txt"));
        assert_eq!(std::fs::read(there.join("report.txt")).unwrap(), b"first");

        std::fs::write(&local, b"second").unwrap();
        let refused = send(Collision::Fail).expect_err("a taken name must be refused");
        assert_eq!(refused.cause, crate::transport::Cause::Exists);
        assert_eq!(std::fs::read(there.join("report.txt")).unwrap(), b"first", "untouched");

        assert_eq!(send(Collision::Rename).expect("keep both"), format!("{root}/report (1).txt"));
        assert_eq!(std::fs::read(there.join("report (1).txt")).unwrap(), b"second");

        assert_eq!(send(Collision::Replace).expect("replace"), format!("{root}/report.txt"));
        assert_eq!(std::fs::read(there.join("report.txt")).unwrap(), b"second");
        assert!(leftovers(&there).is_empty(), "{:?}", leftovers(&there));
    }

    /// The permissions of what a transfer leaves behind, against the real
    /// client and server.
    ///
    /// A replacement keeps what it replaced — the finding this exists for is a
    /// `0600` file coming back `0644` — and a new file lands under a mode
    /// somebody chose rather than under the server's umask.
    ///
    /// Unix-only, and not for want of trying: the assertions are Unix mode
    /// bits, which NTFS does not have and Win32-OpenSSH synthesises. The rule
    /// the modes come from is `fsmodel::carried`, which is tested on every
    /// platform, and the native transport's half of it is tested against the
    /// in-process host, which runs on the Windows agent.
    #[cfg(unix)]
    #[test]
    fn the_fallback_transport_keeps_the_permissions_a_file_already_had() {
        use std::os::unix::fs::PermissionsExt;
        let dir = crate::tempdir::Dir::new("sftp-modes");
        let (here, there) = (dir.path().join("local"), dir.path().join("remote"));
        std::fs::create_dir_all(&here).unwrap();
        std::fs::create_dir_all(&there).unwrap();
        let Some(files) = fixture(&there) else { return };
        let root = sftp_path(&there);
        let local = here.join("secret.txt");
        std::fs::write(&local, b"replacement").unwrap();
        std::fs::set_permissions(&local, std::fs::Permissions::from_mode(0o644)).unwrap();

        let private = there.join("secret.txt");
        std::fs::write(&private, b"original").unwrap();
        std::fs::set_permissions(&private, std::fs::Permissions::from_mode(0o600)).unwrap();

        files
            .upload(&sftp_path(&local), &root, "secret.txt", Collision::Replace)
            .expect("replace");

        let mode = std::fs::metadata(&private).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "a private file must not come back world-readable");
        assert_eq!(std::fs::read(&private).unwrap(), b"replacement");

        // An executable stays one.
        let script = there.join("run.sh");
        std::fs::write(&script, b"#!/bin/sh\n").unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        let source = here.join("run.sh");
        std::fs::write(&source, b"#!/bin/sh\necho hi\n").unwrap();
        std::fs::set_permissions(&source, std::fs::Permissions::from_mode(0o644)).unwrap();
        files
            .upload(&sftp_path(&source), &root, "run.sh", Collision::Replace)
            .expect("replace the script");
        assert_eq!(
            std::fs::metadata(&script).unwrap().permissions().mode() & 0o777,
            0o755,
            "an executable must stay executable"
        );

        // And a setuid destination is refused outright rather than re-granted.
        let privileged = there.join("suid.bin");
        std::fs::write(&privileged, b"privileged").unwrap();
        std::fs::set_permissions(&privileged, std::fs::Permissions::from_mode(0o4755)).unwrap();
        let failure = files
            .upload(&sftp_path(&local), &root, "suid.bin", Collision::Replace)
            .expect_err("a setuid destination must be refused");
        assert!(failure.message.contains("special permission"), "{}", failure.message);
        assert_eq!(std::fs::read(&privileged).unwrap(), b"privileged", "untouched");
        assert!(leftovers(&there).is_empty(), "{:?}", leftovers(&there));
    }

    /// The commit for `Fail` and `Rename` is a hard link, which the server
    /// performs with `link(2)`: a name taken between the decision and the move
    /// is refused rather than overwritten.
    ///
    /// The old sequence checked with `ls` and committed with a replacing
    /// rename, so a writer that appeared in between lost their file to an
    /// upload that had been told not to replace anything. This drives that
    /// exact race: the destination is created *after* afterm has looked and
    /// before it commits, by planting it while the transfer is in flight is
    /// not reproducible from here — so the property is checked directly, by
    /// asking the commit to run against a name that is already taken and
    /// requiring that the contents survive.
    #[test]
    fn a_no_overwrite_commit_cannot_replace_a_file_that_appeared_first() {
        let dir = crate::tempdir::Dir::new("sftp-race");
        let (here, there) = (dir.path().join("local"), dir.path().join("remote"));
        std::fs::create_dir_all(&here).unwrap();
        std::fs::create_dir_all(&there).unwrap();
        let Some(files) = fixture(&there) else { return };

        // The staging file exists and is finished; the destination is taken by
        // somebody else. This is the moment the commit has to get right.
        let part = there.join(".afterm-0123456789abcdef0123456789abcdef.part");
        std::fs::write(&part, b"mine").unwrap();
        let contested = there.join("contested.txt");
        std::fs::write(&contested, b"theirs").unwrap();

        let failure = files
            .link_into(&sftp_path(&part), &sftp_path(&contested))
            .expect_err("a taken name must be refused");
        assert_eq!(failure.cause, crate::transport::Cause::Exists);
        assert_eq!(std::fs::read(&contested).unwrap(), b"theirs", "not overwritten");

        // And onto a free name it is a move, leaving one file rather than two.
        let free = there.join("free.txt");
        files.link_into(&sftp_path(&part), &sftp_path(&free)).expect("a free name");
        files
            .unlink_source(&sftp_path(&part), &sftp_path(&free))
            .expect("the staging name goes");
        assert_eq!(std::fs::read(&free).unwrap(), b"mine");
        assert!(!part.exists(), "the staging name is gone");
    }

    /// The pane's own rename gets the same guarantee for a file, and the
    /// directory case still refuses a taken name.
    #[test]
    fn renaming_onto_a_taken_name_refuses_for_files_and_directories_alike() {
        let dir = crate::tempdir::Dir::new("sftp-rename");
        let there = dir.path().join("remote");
        std::fs::create_dir_all(&there).unwrap();
        let Some(files) = fixture(&there) else { return };
        let path = |name: &str| sftp_path(&there.join(name));

        std::fs::write(there.join("a.txt"), b"a").unwrap();
        std::fs::write(there.join("b.txt"), b"b").unwrap();
        let refused = files.rename(&path("a.txt"), &path("b.txt")).expect_err("taken");
        assert_eq!(refused.cause, crate::transport::Cause::Exists);
        assert_eq!(std::fs::read(there.join("b.txt")).unwrap(), b"b", "not overwritten");
        assert_eq!(std::fs::read(there.join("a.txt")).unwrap(), b"a", "and still there");

        files.rename(&path("a.txt"), &path("c.txt")).expect("a free name");
        assert_eq!(std::fs::read(there.join("c.txt")).unwrap(), b"a");
        assert!(!there.join("a.txt").exists());

        // A directory cannot be hard-linked, so it takes the other route.
        std::fs::create_dir(there.join("folder")).unwrap();
        std::fs::write(there.join("folder/inside.txt"), b"x").unwrap();
        let refused = files.rename(&path("folder"), &path("b.txt")).expect_err("taken");
        assert_eq!(refused.cause, crate::transport::Cause::Exists);
        files.rename(&path("folder"), &path("moved")).expect("a free name");
        assert!(there.join("moved/inside.txt").exists());
    }

    /// A transfer that fails partway leaves nothing behind, which is what the
    /// early return used to break: only commit failures reached the cleanup.
    #[test]
    fn a_failed_transfer_takes_its_staging_file_with_it() {
        let dir = crate::tempdir::Dir::new("sftp-abandoned");
        let (here, there) = (dir.path().join("local"), dir.path().join("remote"));
        std::fs::create_dir_all(&here).unwrap();
        std::fs::create_dir_all(&there).unwrap();
        let Some(files) = fixture(&there) else { return };

        // A source that is not there: `put` fails after the client has already
        // been told where to write.
        let failure = files
            .upload(
                &sftp_path(&here.join("not-there.txt")),
                &sftp_path(&there),
                "target.txt",
                Collision::Fail,
            )
            .expect_err("a source that is not there must fail");
        assert!(!failure.message.is_empty());
        assert!(leftovers(&there).is_empty(), "{:?}", leftovers(&there));
        assert!(!there.join("target.txt").exists(), "and nothing under the real name");
    }

    /// The other half of the same rule, and the one that used to leak: the
    /// transfer fails *after* the staging file has been created, so the early
    /// return skipped the cleanup that only commit failures went through.
    ///
    /// Unix-only for the way it makes the write fail. A directory whose mode
    /// forbids writing is one line here and an ACL edit on Windows, and the
    /// property under test is afterm's cleanup rather than the platform's
    /// permission model.
    #[cfg(unix)]
    #[test]
    fn a_transfer_that_cannot_be_written_leaves_no_staging_file() {
        use std::os::unix::fs::PermissionsExt;
        let dir = crate::tempdir::Dir::new("sftp-readonly");
        let (here, there) = (dir.path().join("local"), dir.path().join("remote"));
        std::fs::create_dir_all(&here).unwrap();
        std::fs::create_dir_all(&there).unwrap();
        let Some(files) = fixture(&there) else { return };

        let closed = there.join("closed");
        std::fs::create_dir(&closed).unwrap();
        let local = here.join("payload.txt");
        std::fs::write(&local, b"payload").unwrap();
        std::fs::set_permissions(&closed, std::fs::Permissions::from_mode(0o500)).unwrap();

        let failure = files
            .upload(&sftp_path(&local), &sftp_path(&closed), "target.txt", Collision::Fail)
            .expect_err("a read-only directory must fail");

        assert!(!failure.message.is_empty(), "{failure}");
        std::fs::set_permissions(&closed, std::fs::Permissions::from_mode(0o700)).unwrap();
        assert!(leftovers(&closed).is_empty(), "{:?}", leftovers(&closed));
    }

    #[test]
    fn reads_the_mode_off_a_listing_row() {
        assert_eq!(parse_mode("-rw-------  ? a a 6 Sep 4 01:50 /x"), Some(0o600));
        assert_eq!(parse_mode("-rwxr-xr-x  ? a a 6 Sep 4 01:50 /x"), Some(0o755));
        assert_eq!(parse_mode("-rwsr-xr-x  ? a a 6 Sep 4 01:50 /x"), Some(0o4755));
        assert_eq!(parse_mode("-rwSr-xr-x  ? a a 6 Sep 4 01:50 /x"), Some(0o4655));
        assert_eq!(parse_mode("drwxrwxrwt  ? a a 6 Sep 4 01:50 /x"), Some(0o1777));
        assert_eq!(parse_mode("Remote working directory: /home"), None);
        // And what it reads is what decides a replacement.
        assert!(carried(parse_mode("-rwsr-xr-x  ? a a 6 Sep 4 01:50 /x").unwrap()).is_err());
    }

    /// Anything afterm left behind in `directory`, by the only name it ever
    /// writes under.
    fn leftovers(directory: &std::path::Path) -> Vec<String> {
        std::fs::read_dir(directory)
            .unwrap()
            .filter_map(|entry| entry.ok())
            .map(|entry| entry.file_name().to_string_lossy().to_string())
            .filter(|name| name.starts_with(".afterm-"))
            .collect()
    }

    /// End-to-end against a real OpenSSH server. Ignored by default; the
    /// harness in `scripts/live-sftp-test.sh` starts a throwaway sshd.
    #[test]
    #[ignore = "requires the sshd fixture; run scripts/live-sftp-test.sh"]
    fn talks_to_a_real_sftp_server() {
        let identity = std::env::var("AFTERM_TEST_KEY").expect("AFTERM_TEST_KEY");
        let known = std::env::var("AFTERM_TEST_KNOWN_HOSTS").expect("AFTERM_TEST_KNOWN_HOSTS");
        let host = std::env::var("AFTERM_TEST_HOST").expect("AFTERM_TEST_HOST");
        let user = std::env::var("AFTERM_TEST_USER").expect("AFTERM_TEST_USER");
        let port: u16 = std::env::var("AFTERM_TEST_PORT").unwrap().parse().unwrap();
        let root = std::env::var("AFTERM_TEST_DIR").expect("AFTERM_TEST_DIR");

        let known_option = format!("UserKnownHostsFile={known}");
        let extra = ["-i", &identity, "-o", &known_option, "-o", "IdentitiesOnly=yes"];
        let files = RemoteFiles::new(&host, &user, port).with_args(&extra, REQUEST_TIMEOUT);

        files.check().expect("check");

        let home = files.working_directory().expect("pwd");
        assert!(home.starts_with('/'), "home should be absolute, got {home}");

        let listing = files.list(&root).expect("list");
        let names: Vec<&str> = listing.entries.iter().map(|e| e.name.as_str()).collect();
        assert!(names.contains(&"afile.txt"), "missing file in {names:?}");
        assert!(names.contains(&"sub"), "missing directory in {names:?}");
        assert!(names.contains(&"file with space.txt"), "missing spaced name in {names:?}");
        assert_eq!(listing.entries[0].kind, EntryKind::Directory);

        let file = listing.entries.iter().find(|e| e.name == "afile.txt").unwrap();
        assert_eq!(file.size, Some(6));
        let link = listing.entries.iter().find(|e| e.name == "alink").unwrap();
        assert_eq!(link.kind, EntryKind::Link);

        // A bad path must surface an error, not an empty directory.
        assert!(files.list("/__afterm_definitely_missing__").is_err());

        // A folder made private is `0700`, whatever the umask made it.
        let private = format!("{root}/private");
        files.create_directory(&private).expect("mkdir");
        files.make_private(&private).expect("make private");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&private).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o700);
        }

        let target = format!("{root}/downloaded.txt");
        files.download(&format!("{root}/afile.txt"), &target).expect("download");
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "hello\n");

        // And the whole folder, through the transport rather than the client,
        // because a tree is where this transport and the native one could
        // differ: both walk the same code, and only a real server says
        // whether `ls -l` and `get` carry a nested tree the way it expects.
        let transport = crate::transport::Transport::Cli(std::sync::Arc::new(
            crate::transport::CliTransport::around(
                RemoteFiles::new(&host, &user, port).with_args(&extra, REQUEST_TIMEOUT),
                &host,
                &user,
                port,
            ),
        ));
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let directory = crate::tempdir::Dir::new("live-folder");
        let into = crate::localfs::claim_dir(directory.path(), "data").expect("claim");
        let tally = runtime
            .block_on(crate::folder::download(&transport, &root, &into, |_| |_, _| {}))
            .expect("folder download");

        assert_eq!(std::fs::read_to_string(into.join("afile.txt")).unwrap(), "hello\n");
        assert_eq!(std::fs::read_to_string(into.join("sub/deep.txt")).unwrap(), "deeper\n");
        assert!(!into.join("alink").exists(), "a link is skipped, not followed");
        // afile.txt, the spaced name, the one this test just downloaded, and
        // the nested one. The link is the skip.
        assert_eq!((tally.files, tally.skipped, tally.failed), (4, 1, 0));

        // And the other way: a tree made on the far end with `mkdir` and
        // filled with `put`, which is the half a stub server cannot vouch for.
        let source = crate::tempdir::Dir::new("live-upload");
        let outbound = source.path().join("outbound");
        std::fs::create_dir_all(outbound.join("sub")).unwrap();
        std::fs::write(outbound.join("top.txt"), b"at the top").unwrap();
        std::fs::write(outbound.join("sub/deep.txt"), b"further in").unwrap();
        // Linux-only, like the rest of this test's fixture — but the file it
        // lives in is compiled for Windows too, where `std::os::unix` is not a
        // module at all.
        #[cfg(unix)]
        std::os::unix::fs::symlink(source.path(), outbound.join("out")).unwrap();

        let (landed, sent) = runtime
            .block_on(crate::folder::upload(
                &transport,
                &outbound,
                &root,
                Collision::Fail,
                |_| |_, _| {},
            ))
            .expect("folder upload");

        // The throwaway server serves this machine's own filesystem, so what
        // arrived can be read back directly.
        assert_eq!(landed, format!("{root}/outbound"));
        assert_eq!(std::fs::read_to_string(format!("{root}/outbound/top.txt")).unwrap(), "at the top");
        assert_eq!(
            std::fs::read_to_string(format!("{root}/outbound/sub/deep.txt")).unwrap(),
            "further in"
        );
        assert!(
            !std::path::Path::new(&format!("{root}/outbound/out")).exists(),
            "the link stayed here"
        );
        assert_eq!((sent.files, sent.skipped, sent.failed), (2, u32::from(cfg!(unix)), 0));

        // The same folder again is a question rather than a decision.
        let again = runtime.block_on(crate::folder::upload(
            &transport,
            &outbound,
            &root,
            Collision::Fail,
            |_| |_, _| {},
        ));
        assert_eq!(
            again.expect_err("a taken name is refused").cause,
            crate::transport::Cause::Exists
        );

        // A folder with things in it goes, contents and all, and the link
        // inside it goes as a link: what it pointed at is still there.
        #[cfg(unix)]
        {
            let (doomed, kept) = crate::folder::doomed_tree(&root);
            runtime
                .block_on(crate::folder::remove(&transport, &doomed))
                .expect("recursive delete");
            assert!(!std::path::Path::new(&doomed).exists(), "the folder is gone");
            assert_eq!(std::fs::read_to_string(&kept).unwrap(), "keep", "the link was not followed");
        }
    }
}
