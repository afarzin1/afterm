//! Copying a whole folder, in either direction.
//!
//! One file at a time, over whichever transport the session already has. The
//! walk is here rather than in either transport because there is nothing
//! transport-specific about it: both of them can list a directory, fetch a
//! file, make a directory and send a file, and a tree is those in a loop.
//! Writing it once also means the two cannot disagree about what a folder
//! transfer does, which is the failure this project keeps finding between
//! them.
//!
//! What it will not do is as much of the design as what it will.
//!
//! **It never follows a link.** A symbolic link is copied as nothing at all
//! and counted as skipped. Following one means the tree can leave the folder
//! the user asked for — `/etc` reached through a link inside a home
//! directory — and it means a loop can make the walk run until the disk is
//! full. OpenSSH's own `get -r` skips non-regular files for the same reason.
//!
//! **It never writes outside the folder it was given.** Coming down, every
//! remote name goes through `localfs::safe_name` and is then created with an
//! exclusive create, which refuses anything already at that name and hands
//! back `name (1)` instead: a host that serves a file called `../../evil`
//! gets a file called `.._.._evil`. Going up, every name is one component of
//! a local directory listing, and each directory is made with `mkdir`, which
//! the protocol refuses on a name that is taken.
//!
//! **One bad file does not lose the rest.** A listing or a transfer that fails
//! for a reason to do with that one path is counted and stepped over;
//! anything that is about the connection rather than the path — dropped,
//! refused, unreachable — ends the walk, because every remaining file would
//! fail the same way.
//!
//! **Nothing already on the far side is replaced without an answer.** A
//! download claims a local folder that did not exist a moment ago. An upload
//! asks the host to make one, which it refuses if the name is taken, and the
//! refusal becomes the pane's question: merge into what is there, keep both,
//! or skip. Only the answer *merge* lets a file be written over, and even
//! then it is the single-file replacement with all of its care — staged,
//! moved into place, wearing the permissions of what it replaced.

use crate::fsmodel::{join, numbered, EntryKind};
use crate::localfs;
use crate::transport::{Cause, Collision, Failure, Transport};
use std::path::Path;

/// How deep the walk will go.
///
/// Nothing in an ordinary tree comes near it. It is here because the depth of
/// the walk is decided by what the *host* says is in each directory, and a
/// server that answers every listing with a directory of the same name would
/// otherwise have afterm descending until it ran out of path. A cap turns
/// that into a number of skipped entries and a folder that finishes.
const MAX_DEPTH: usize = 64;

/// How many files one folder download will fetch before it gives up.
///
/// The same argument as the depth cap, against a host that answers with an
/// endless directory instead of an endless tree. A refusal rather than a
/// silent stop: a download that quietly brought back the first fifty thousand
/// files of a larger folder would look exactly like a complete one.
const MAX_FILES: u32 = 50_000;

/// What a folder download brought back.
#[derive(Debug, Default, Clone, Copy)]
pub struct Tally {
    /// Regular files that arrived.
    pub files: u32,
    /// Entries passed over: links, and directories below the depth cap.
    pub skipped: u32,
    /// Entries that could not be copied — a directory that could not be
    /// listed, a file one end would not give up or the other would not take.
    /// One each, whatever was inside.
    pub failed: u32,
}

/// Copy `remote` into the already-created local directory `into`.
///
/// `reporter` is asked for a progress callback per file, and is given the
/// file's path *within* the folder — `sub/deep.txt`, not the whole remote
/// path — because that is what says where a long transfer has got to without
/// the pane having to elide anything.
pub async fn download<R, P>(
    transport: &Transport,
    remote: &str,
    into: &Path,
    reporter: R,
) -> Result<Tally, Failure>
where
    R: Fn(&str) -> P,
    P: Fn(u64, Option<u64>) + Send + 'static,
{
    let mut tally = Tally::default();
    // Directories still to walk: where each one is on the host, where it is
    // here, and how far below the folder the user asked for it sits. A stack
    // rather than recursion — an async function cannot call itself without
    // being boxed, and the depth is the thing being bounded.
    let mut pending = vec![(remote.to_string(), into.to_path_buf(), 0_usize)];

    while let Some((there, here, depth)) = pending.pop() {
        let listing = match transport.list(&there).await {
            Ok(listing) => listing,
            Err(failure) if failure.cause == Cause::Operation => {
                tally.failed += 1;
                continue;
            }
            Err(failure) => return Err(failure),
        };

        for entry in listing.entries {
            // Both clients drop these already. Said again here because an
            // endless walk is the worst thing this loop could do, and it
            // should not depend on a dependency's filtering to avoid it.
            if entry.name == "." || entry.name == ".." {
                continue;
            }
            let name = localfs::safe_name(&entry.name);
            match entry.kind {
                EntryKind::Directory => {
                    if depth + 1 > MAX_DEPTH {
                        tally.skipped += 1;
                        continue;
                    }
                    // A local directory that cannot be created is not about
                    // this one entry: the disk is full, or the destination
                    // has gone. Nothing else would work either.
                    let made = localfs::claim_dir(&here, &name)?;
                    pending.push((entry.path, made, depth + 1));
                }
                EntryKind::File => {
                    if tally.files >= MAX_FILES {
                        return Err(Failure::operation(format!(
                            "{remote} holds more than {MAX_FILES} files"
                        )));
                    }
                    let local = localfs::claim(&here, &name)?;
                    let label = within(into, &local);
                    match transport
                        .download(&entry.path, &local.to_string_lossy(), reporter(&label))
                        .await
                    {
                        Ok(()) => tally.files += 1,
                        // Whatever did not arrive leaves no empty file behind
                        // to be mistaken for one that did.
                        Err(failure) if failure.cause == Cause::Operation => {
                            localfs::discard(&local);
                            tally.failed += 1;
                        }
                        Err(failure) => {
                            localfs::discard(&local);
                            return Err(failure);
                        }
                    }
                }
                EntryKind::Link => tally.skipped += 1,
            }
        }
    }

    Ok(tally)
}

/// Copy the local folder `local` into the remote directory `into`, and say
/// where it landed and what went.
///
/// `collision` is the answer to a name already taken on the host, and it is
/// the whole of the difference between the two shapes this has:
///
/// `Fail` and `Rename` make a folder nothing was in. Nothing on the host can
/// be overwritten by either, because every file inside goes to a name that
/// did not exist a moment ago — so no file inside is ever asked about.
///
/// `Replace` merges into the folder that is there, and it is the only way a
/// file on the host is written over. One answer covers the tree: a folder of
/// four hundred files must not be four hundred questions, and a user who has
/// said "merge" about the folder has said it about what is in it. Each file
/// still goes through the single-file replacement — staged under a private
/// name, moved into place, wearing the permissions of what it replaced, and
/// refused outright over a directory or anything carrying a setuid bit.
pub async fn upload<R, P>(
    transport: &Transport,
    local: &Path,
    into: &str,
    collision: Collision,
    reporter: R,
) -> Result<(String, Tally), Failure>
where
    R: Fn(&str) -> P,
    P: Fn(u64, Option<u64>) + Send + 'static,
{
    let name = local
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| Failure::internal(format!("{} has no folder name", local.display())))?;

    let root = match collision {
        Collision::Fail => {
            let path = join(into, name);
            if !make(transport, &path).await? {
                return Err(Failure::exists(format!("{name} is already in {into}")));
            }
            path
        }
        Collision::Rename => {
            let mut free = None;
            for attempt in 0..100 {
                let candidate =
                    if attempt == 0 { name.to_string() } else { numbered(name, attempt) };
                let path = join(into, &candidate);
                if make(transport, &path).await? {
                    free = Some(path);
                    break;
                }
            }
            free.ok_or_else(|| {
                Failure::operation(format!("too many folders called {name} in {into}"))
            })?
        }
        // Already there is the point. A name that is a *file* rather than a
        // folder is not made into one here; the files that would go inside it
        // fail one by one, which is what says so.
        Collision::Replace => {
            let path = join(into, name);
            make(transport, &path).await?;
            path
        }
    };

    let mut tally = Tally::default();
    // Inside a folder made a moment ago nothing can be in the way, so the
    // files that go into one are sent with the answer that refuses to
    // overwrite. Only a merge carries the answer that does.
    let inside =
        if collision == Collision::Replace { Collision::Replace } else { Collision::Fail };
    let mut pending = vec![(local.to_path_buf(), root.clone(), 0_usize)];

    while let Some((here, there, depth)) = pending.pop() {
        let reading = match std::fs::read_dir(&here) {
            Ok(reading) => reading,
            // One directory of this machine's that cannot be read — a
            // permission, a device that went away — is one directory's worth
            // of the upload lost, not the upload.
            Err(_) => {
                tally.failed += 1;
                continue;
            }
        };

        for entry in reading {
            let Ok(entry) = entry else {
                tally.failed += 1;
                continue;
            };
            let path = entry.path();
            // `symlink_metadata`, so a link is seen as a link rather than as
            // whatever it points at. Following one is how an upload leaves the
            // folder it was given — a link to `/` inside a home directory is
            // the whole filesystem — and how it runs until the host is full.
            let Ok(metadata) = std::fs::symlink_metadata(&path) else {
                tally.failed += 1;
                continue;
            };
            if metadata.is_symlink() {
                tally.skipped += 1;
                continue;
            }
            // A name this machine holds but cannot spell as text. There is
            // nothing to send it as, and guessing at an encoding would put a
            // file on the host under a name nobody asked for.
            let Some(name) = entry.file_name().to_str().map(|name| name.to_string()) else {
                tally.failed += 1;
                continue;
            };

            if metadata.is_dir() {
                if depth + 1 > MAX_DEPTH {
                    tally.skipped += 1;
                    continue;
                }
                let child = join(&there, &name);
                match make(transport, &child).await {
                    Ok(_) => pending.push((path, child, depth + 1)),
                    Err(failure) if failure.cause == Cause::Operation => tally.failed += 1,
                    Err(failure) => return Err(failure),
                }
            } else if metadata.is_file() {
                if tally.files >= MAX_FILES {
                    return Err(Failure::operation(format!(
                        "{} holds more than {MAX_FILES} files",
                        local.display()
                    )));
                }
                let label = within(local, &path);
                match transport
                    .upload(&path.to_string_lossy(), &there, &name, inside, reporter(&label))
                    .await
                {
                    Ok(_) => tally.files += 1,
                    // `Exists` belongs with the per-entry failures rather than
                    // with the fatal ones: in a merge it is a name the host
                    // would not let this file have, and in a fresh folder it is
                    // something that appeared while the walk was running.
                    Err(failure)
                        if failure.cause == Cause::Operation || failure.cause == Cause::Exists =>
                    {
                        tally.failed += 1
                    }
                    Err(failure) => return Err(failure),
                }
            } else {
                // A socket, a device, a fifo. Nothing a file transfer can
                // carry, and nothing the host would know what to do with.
                tally.skipped += 1;
            }
        }
    }

    Ok((root, tally))
}

/// Delete the remote directory `root` and everything in it.
///
/// The protocol has no recursive remove, so this is `rm -r` spelt out: every
/// file and link is removed as it is listed, and the directories go last,
/// deepest first, once there is nothing left in them. A link is removed as
/// itself and never walked into — removing the contents of what a link points
/// at is how a delete leaves the folder it was asked about.
///
/// An entry that will not go is counted and stepped over, the way a transfer
/// steps over one: its directory then refuses its own `rmdir` in turn, and
/// what is left is exactly what could not be deleted. The first refusal is
/// the one reported, because that is the one that says why.
pub async fn remove(transport: &Transport, root: &str) -> Result<(), Failure> {
    // The pane can only offer a directory it listed inside another one, so
    // neither of these can come from a click. Refused anyway: this is the one
    // function in the project that can empty a filesystem.
    if root.trim_end_matches('/').is_empty() || root == "." || root == ".." {
        return Err(Failure::operation(format!("refusing to delete {root}")));
    }
    let mut failed = 0_u32;
    let mut first: Option<Failure> = None;
    let mut note = |failure: Failure, failed: &mut u32| {
        *failed += 1;
        first.get_or_insert(failure);
    };
    // Every directory found, parents before children; removed in reverse.
    let mut found = Vec::new();
    let mut pending = vec![(root.to_string(), 0_usize)];

    while let Some((there, depth)) = pending.pop() {
        found.push(there.clone());
        let listing = match transport.list(&there).await {
            Ok(listing) => listing,
            Err(failure) if failure.cause == Cause::Operation => {
                note(failure, &mut failed);
                continue;
            }
            Err(failure) => return Err(failure),
        };
        for entry in listing.entries {
            if entry.name == "." || entry.name == ".." {
                continue;
            }
            match entry.kind {
                EntryKind::Directory if depth + 1 <= MAX_DEPTH => {
                    pending.push((entry.path, depth + 1));
                }
                // Too deep to walk is left in place, and its parent with it.
                EntryKind::Directory => note(
                    Failure::operation(format!("{} is nested too deeply", entry.path)),
                    &mut failed,
                ),
                EntryKind::File | EntryKind::Link => {
                    match transport.delete(&entry.path, false).await {
                        Ok(()) => {}
                        Err(failure) if failure.cause == Cause::Operation => {
                            note(failure, &mut failed)
                        }
                        Err(failure) => return Err(failure),
                    }
                }
            }
        }
    }

    for directory in found.iter().rev() {
        match transport.delete(directory, true).await {
            Ok(()) => {}
            // A directory that kept something is already accounted for by
            // whatever it kept; its own refusal would only say "not empty".
            Err(failure) if failure.cause == Cause::Operation => {
                if failed == 0 {
                    note(failure, &mut failed);
                }
            }
            Err(failure) => return Err(failure),
        }
    }

    match first {
        None => Ok(()),
        Some(failure) if failed == 1 => Err(failure),
        Some(failure) => Err(Failure::operation(format!(
            "{} (and {} more could not be deleted)",
            failure.message,
            failed - 1
        ))),
    }
}

/// Make `path` a directory on the host, and say whether this call is what made
/// it.
///
/// `mkdir` is an exclusive create on both transports, and version 3 of the
/// protocol reports a taken name as a plain failure with nothing in it to match
/// on — so a refusal is followed by a look. Something there now means the name
/// was taken, which is an answer rather than an error; nothing there means the
/// refusal was about something else, and it is returned as itself.
async fn make(transport: &Transport, path: &str) -> Result<bool, Failure> {
    match transport.create_directory(path).await {
        Ok(()) => Ok(true),
        Err(failure) => match transport.exists(path).await {
            Ok(true) => Ok(false),
            _ => Err(failure),
        },
    }
}

/// Where a file sits inside the folder being transferred, for the pane to
/// show: `sub/deep.txt` rather than a whole path, going either way. Falls back
/// to the file's own name, which is all a path outside the folder could
/// truthfully be called — and nothing here can produce one.
fn within(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .ok()
        .map(|inside| inside.to_string_lossy().replace('\\', "/"))
        .filter(|inside| !inside.is_empty())
        .unwrap_or_else(|| {
            path.file_name().map(|name| name.to_string_lossy().to_string()).unwrap_or_default()
        })
}

/// A tree for the live tests to delete, made in `root` on this machine (the
/// throwaway server serves its own filesystem): files at two depths, an empty
/// directory, and a link to a directory *beside* the tree. The link is the
/// point — a walk that followed it would empty `keep` — so both live tests
/// check `keep` survives. Returns the tree and the file that must survive.
#[cfg(all(test, unix))]
pub fn doomed_tree(root: &str) -> (String, String) {
    let doomed = format!("{root}/doomed");
    let keep = format!("{root}/keep");
    std::fs::create_dir_all(format!("{doomed}/sub/deeper")).unwrap();
    std::fs::create_dir_all(format!("{doomed}/empty")).unwrap();
    std::fs::create_dir_all(&keep).unwrap();
    std::fs::write(format!("{doomed}/top.txt"), b"top").unwrap();
    std::fs::write(format!("{doomed}/sub/mid.txt"), b"mid").unwrap();
    std::fs::write(format!("{doomed}/sub/deeper/low.txt"), b"low").unwrap();
    std::fs::write(format!("{keep}/keep.txt"), b"keep").unwrap();
    std::os::unix::fs::symlink(&keep, format!("{doomed}/sub/out")).unwrap();
    (doomed, format!("{keep}/keep.txt"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn a_files_label_is_where_it_sits_in_the_folder() {
        let root = PathBuf::from("/home/arya/Downloads/reports");
        assert_eq!(within(&root, &root.join("sub/deep.txt")), "sub/deep.txt");
        assert_eq!(within(&root, &root.join("top.txt")), "top.txt");
    }

    #[test]
    fn a_path_outside_the_folder_still_has_something_to_show() {
        let root = PathBuf::from("/home/arya/Downloads/reports");
        assert_eq!(within(&root, Path::new("/elsewhere/loose.txt")), "loose.txt");
        assert_eq!(within(&root, &root), "reports");
    }
}
