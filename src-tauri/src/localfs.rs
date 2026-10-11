//! What afterm still needs from the machine it is running on: the Downloads
//! folder, and a way to put a file or a folder there that cannot land anywhere
//! else and cannot replace anything already there.
//!
//! This used to browse, create and rename here too, for the PowerShell and CMD
//! sessions that had a local file pane. That half did not come back with the
//! shells: a shell on this machine is already in front of the filesystem the
//! pane would show. The pane is always SFTP, and this is what is left.
//!
//! The names handled here came from another machine. A remote host can call a
//! file `..\Documents\report.txt`, and on Windows that is a path, not a name.
//! So the boundary is enforced on this side, in Rust, and it is enforced twice:
//! the name is reduced to one plain component, and the file is then created
//! with `create_new`, which the operating system refuses if anything already
//! exists there. Nothing the interface sends can widen either check.

use crate::fsmodel::numbered;
use crate::transport::Failure;
use std::fs::OpenOptions;
use std::path::{Component, Path, PathBuf};

/// The platform's Downloads directory, used as the destination for files
/// copied from a remote host. Windows resolves this through its known-folder
/// API; the explicit `Downloads` fallback covers profiles where that lookup is
/// unavailable without dropping the file loose in the user directory.
pub fn downloads() -> String {
    downloads_dir().to_string_lossy().to_string()
}

pub fn downloads_dir() -> PathBuf {
    download_directory(dirs::download_dir(), dirs::home_dir())
}

fn download_directory(platform: Option<PathBuf>, home: Option<PathBuf>) -> PathBuf {
    platform
        .or_else(|| home.map(|path| path.join("Downloads")))
        .unwrap_or_else(|| PathBuf::from("Downloads"))
}

/// A remote entry's name, made safe to use as one component of a local path.
///
/// The rules applied are Windows', on every platform: Windows is where afterm
/// runs, and a rule that only applied there could only be tested there. So
/// `/` and `\` cannot appear, nor `< > : " | ? *` or a control character; a
/// name cannot end in a dot or a space, because Windows drops those itself and
/// would create the file under a different name from the one that was checked;
/// and `CON`, `COM1` and the other device names get a leading underscore,
/// because a file cannot be called that whatever its extension.
///
/// Characters that cannot appear are replaced rather than refused: a file
/// called `what?.txt` on a Linux host should still be downloadable, as
/// `what_.txt`. Only a name that is nothing at all after that — `..`, or a
/// name made entirely of dots — falls back to `download`.
pub fn safe_name(remote: &str) -> String {
    const FORBIDDEN: [char; 9] = ['/', '\\', '<', '>', ':', '"', '|', '?', '*'];
    let replaced: String = remote
        .chars()
        .map(|c| if FORBIDDEN.contains(&c) || c.is_control() { '_' } else { c })
        .collect();
    let mut name = replaced.trim().trim_end_matches(['.', ' ']).to_string();
    if name.is_empty() {
        return "download".into();
    }
    if is_reserved_device(&name) {
        name.insert(0, '_');
    }
    name
}

/// `CON`, `PRN`, `AUX`, `NUL`, `COM1`–`COM9` and `LPT1`–`LPT9`, in any case and
/// with any extension: `con.txt` is as unusable on Windows as `CON`.
fn is_reserved_device(name: &str) -> bool {
    let stem = name.split('.').next().unwrap_or_default().to_ascii_uppercase();
    match stem.as_str() {
        "CON" | "PRN" | "AUX" | "NUL" => true,
        _ => {
            (stem.starts_with("COM") || stem.starts_with("LPT"))
                && stem.len() == 4
                && stem.as_bytes()[3].is_ascii_digit()
                && stem.as_bytes()[3] != b'0'
        }
    }
}

/// Whether `name` is exactly one ordinary path component, on this platform.
///
/// This is the check that does not depend on `safe_name` having thought of
/// everything: the standard library's own parser says whether the name is a
/// parent reference, a root, or — on Windows — a drive or UNC prefix.
fn is_one_component(name: &str) -> bool {
    if name.contains(['/', '\\']) {
        return false;
    }
    let mut components = Path::new(name).components();
    matches!(
        (components.next(), components.next()),
        (Some(Component::Normal(_)), None)
    )
}

/// The name a transfer is written under until it is whole.
pub fn part_name(name: &str) -> String {
    format!("{name}.part")
}

/// Create `name` in `directory`, or the first `name (n)` that is free, and
/// return its path.
///
/// `create_new` is what makes this safe: the operating system either creates
/// the file, atomically and only if nothing was there, or refuses. Nothing that
/// already exists is ever opened, so nothing is ever truncated, and two
/// transfers racing for the same name get two names.
pub fn claim(directory: &Path, name: &str) -> Result<PathBuf, Failure> {
    if !is_one_component(name) {
        return Err(Failure::internal(format!("'{name}' is not a plain file name")));
    }
    std::fs::create_dir_all(directory).map_err(|error| {
        Failure::operation(format!("could not create {}: {error}", directory.display()))
    })?;
    for attempt in 0..1000 {
        let candidate = if attempt == 0 { name.to_string() } else { numbered(name, attempt) };
        let path = directory.join(&candidate);
        // Belt and braces: joining one plain component cannot leave the
        // directory, and this says so where a test can read it.
        if path.parent() != Some(directory) {
            return Err(Failure::internal(format!("'{name}' would leave {}", directory.display())));
        }
        match OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(_) => return Ok(path),
            // Taken, or something else. Which one is decided by looking rather
            // than by the error kind, because the kinds disagree across
            // platforms: a *directory* in the way answers `AlreadyExists` on
            // Unix and `PermissionDenied` on Windows, where the underlying
            // error is `ERROR_ACCESS_DENIED` and not `ERROR_ALREADY_EXISTS`.
            // Trusting the kind meant a download whose name matched a folder
            // in Downloads failed with "Access is denied" on Windows instead
            // of landing as `name (1)`. Found by the pipeline, which is the
            // only Windows this project runs on.
            Err(error) => {
                if path.symlink_metadata().is_ok() {
                    continue;
                }
                return Err(Failure::operation(format!(
                    "could not create {}: {error}",
                    path.display()
                )));
            }
        }
    }
    Err(Failure::operation(format!(
        "too many files called {name} in {}",
        directory.display()
    )))
}

/// Create `name` in `directory` as a new directory, or the first `name (n)`
/// that is free, and return its path.
///
/// `create_dir` is the directory-shaped `create_new`: the operating system
/// either makes the directory, atomically and only if nothing was there, or
/// refuses. So a folder arriving from a host can never be written *into* a
/// folder that was already in Downloads, which is the same promise `claim`
/// makes for a file.
///
/// A folder lands under its real name rather than travelling under a `.part`
/// name that is moved into place at the end, which is what `claim` and
/// `commit` do for a file. Renaming a *directory* onto a name that already
/// exists is not portable: POSIX replaces an empty directory and refuses one
/// with anything in it, Windows refuses both, so the two-step would behave
/// differently on the platform afterm ships on from the one it is developed
/// on. Claiming the final name up front keeps the promise that matters —
/// nothing already in Downloads is touched — and the cost is that a folder
/// which fails half way leaves the part of it that arrived. That is a
/// deliberate trade: throwing away gigabytes that did arrive because one file
/// in the tree could not be copied would be worse, and the status says how
/// many files came and how many did not.
pub fn claim_dir(directory: &Path, name: &str) -> Result<PathBuf, Failure> {
    if !is_one_component(name) {
        return Err(Failure::internal(format!("'{name}' is not a plain file name")));
    }
    std::fs::create_dir_all(directory).map_err(|error| {
        Failure::operation(format!("could not create {}: {error}", directory.display()))
    })?;
    for attempt in 0..1000 {
        let candidate = if attempt == 0 { name.to_string() } else { numbered(name, attempt) };
        let path = directory.join(&candidate);
        if path.parent() != Some(directory) {
            return Err(Failure::internal(format!("'{name}' would leave {}", directory.display())));
        }
        match std::fs::create_dir(&path) {
            Ok(()) => return Ok(path),
            // Taken or not is decided by looking, for the reason `claim`
            // gives: the error kinds for "something is in the way" disagree
            // between platforms.
            Err(error) => {
                if path.symlink_metadata().is_ok() {
                    continue;
                }
                return Err(Failure::operation(format!(
                    "could not create {}: {error}",
                    path.display()
                )));
            }
        }
    }
    Err(Failure::operation(format!(
        "too many folders called {name} in {}",
        directory.display()
    )))
}

/// Move a finished `.part` file to its real name, or the first free variant
/// of it. Returns where it ended up.
///
/// The final name is claimed with `create_new` first, so the rename only ever
/// replaces a file this function created a moment earlier. A rename that fails
/// removes both, so nothing half-done is left under either name.
pub fn commit(part: &Path, directory: &Path, name: &str) -> Result<PathBuf, Failure> {
    let destination = match claim(directory, name) {
        Ok(path) => path,
        Err(failure) => {
            discard(part);
            return Err(failure);
        }
    };
    match std::fs::rename(part, &destination) {
        Ok(()) => Ok(destination),
        Err(error) => {
            let _ = std::fs::remove_file(&destination);
            discard(part);
            Err(Failure::operation(format!(
                "could not move the download to {}: {error}",
                destination.display()
            )))
        }
    }
}

/// Remove a transfer that did not finish. Nothing to report: the failure that
/// stopped it is the one worth showing.
pub fn discard(part: &Path) {
    let _ = std::fs::remove_file(part);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn download_destination_prefers_the_platform_known_folder() {
        let redirected = PathBuf::from("D:/Incoming");
        assert_eq!(
            download_directory(Some(redirected.clone()), Some(PathBuf::from("C:/Users/arya"))),
            redirected
        );
    }

    #[test]
    fn download_destination_falls_back_beneath_the_user_directory() {
        assert_eq!(
            download_directory(None, Some(PathBuf::from("C:/Users/arya"))),
            PathBuf::from("C:/Users/arya/Downloads")
        );
    }

    #[test]
    fn a_name_that_is_a_path_becomes_a_name() {
        // Backslashes are a path on Windows whatever the remote host thought.
        assert_eq!(safe_name("..\\Documents\\report.txt"), ".._Documents_report.txt");
        assert_eq!(safe_name("../etc/passwd"), ".._etc_passwd");
        assert_eq!(safe_name("\\\\server\\share\\x"), "__server_share_x");
        assert_eq!(safe_name("C:report.txt"), "C_report.txt");
        assert_eq!(safe_name("dir/"), "dir_");
    }

    #[test]
    fn dots_and_nothing_fall_back_to_a_real_name() {
        assert_eq!(safe_name("."), "download");
        assert_eq!(safe_name(".."), "download");
        assert_eq!(safe_name("..."), "download");
        assert_eq!(safe_name(""), "download");
        assert_eq!(safe_name("   "), "download");
        // A dotfile keeps its dot; only trailing ones go.
        assert_eq!(safe_name(".bashrc"), ".bashrc");
        assert_eq!(safe_name("report.txt."), "report.txt");
        assert_eq!(safe_name("report.txt "), "report.txt");
    }

    #[test]
    fn windows_forbidden_characters_are_replaced_not_refused() {
        assert_eq!(safe_name("what?.txt"), "what_.txt");
        assert_eq!(safe_name("a<b>c:d\"e|f*g"), "a_b_c_d_e_f_g");
        assert_eq!(safe_name("tab\there"), "tab_here");
        assert_eq!(safe_name("nul\0byte"), "nul_byte");
        // Ordinary names, including spaces and unicode, come through untouched.
        assert_eq!(safe_name("file with space.txt"), "file with space.txt");
        assert_eq!(safe_name("résumé.pdf"), "résumé.pdf");
    }

    #[test]
    fn reserved_device_names_are_prefixed() {
        assert_eq!(safe_name("CON"), "_CON");
        assert_eq!(safe_name("con.txt"), "_con.txt");
        assert_eq!(safe_name("Com1"), "_Com1");
        assert_eq!(safe_name("LPT9.log"), "_LPT9.log");
        assert_eq!(safe_name("NUL"), "_NUL");
        // Near misses are not reserved.
        assert_eq!(safe_name("COM10"), "COM10");
        assert_eq!(safe_name("COM0"), "COM0");
        assert_eq!(safe_name("console.log"), "console.log");
        assert_eq!(safe_name("aux2"), "aux2");
    }

    #[test]
    fn only_a_single_ordinary_component_may_be_claimed() {
        let directory = scratch("claim-guard");
        for bad in ["..", ".", "a/b", "a\\b", "/etc/passwd", "", "..\\x"] {
            assert!(claim(directory.path(), bad).is_err(), "{bad:?} must be refused");
        }
        // Whatever the guard thinks of them, the sanitised versions are fine.
        for bad in ["..", "a/b", "a\\b", "/etc/passwd", "..\\x"] {
            let path = claim(directory.path(), &safe_name(bad)).unwrap();
            assert_eq!(path.parent(), Some(directory.path()));
        }
    }

    #[test]
    fn a_name_already_taken_gets_a_number_rather_than_a_truncation() {
        let directory = scratch("claim-numbered");
        std::fs::write(directory.path().join("report.txt"), "original").unwrap();

        let first = claim(directory.path(), "report.txt").unwrap();
        assert_eq!(first.file_name().unwrap(), "report (1).txt");
        let second = claim(directory.path(), "report.txt").unwrap();
        assert_eq!(second.file_name().unwrap(), "report (2).txt");
        // The original was never opened.
        assert_eq!(std::fs::read_to_string(directory.path().join("report.txt")).unwrap(), "original");
    }

    /// This passed on Linux and failed on Windows, which is the shape every
    /// bug in this project has had: `create_new` onto a directory answers
    /// `AlreadyExists` on one and `PermissionDenied` on the other, so the
    /// claim has to look at the path rather than read the error kind.
    #[test]
    fn a_directory_in_the_way_is_a_collision_not_a_crash() {
        let directory = scratch("claim-dir");
        std::fs::create_dir(directory.path().join("logs")).unwrap();
        let path = claim(directory.path(), "logs").expect("a directory in the way is not a crash");
        assert_eq!(path.file_name().unwrap(), "logs (1)");
    }

    #[test]
    fn a_folder_never_lands_inside_one_that_was_already_there() {
        let directory = scratch("claim-folder");
        std::fs::create_dir(directory.path().join("reports")).unwrap();
        std::fs::write(directory.path().join("reports/kept.txt"), "mine").unwrap();

        let landed = claim_dir(directory.path(), "reports").expect("a second folder gets a name");
        assert_eq!(landed.file_name().unwrap(), "reports (1)");
        // The point of the exclusive create: the folder that was there keeps
        // everything in it, and the download writes somewhere else entirely.
        assert_eq!(
            std::fs::read_to_string(directory.path().join("reports/kept.txt")).unwrap(),
            "mine"
        );
    }

    #[test]
    fn a_file_in_the_way_of_a_folder_is_a_collision_not_a_crash() {
        let directory = scratch("claim-folder-file");
        std::fs::write(directory.path().join("reports"), "a file with the folder's name").unwrap();
        let landed = claim_dir(directory.path(), "reports").expect("a file in the way is not a crash");
        assert_eq!(landed.file_name().unwrap(), "reports (1)");
        assert!(landed.is_dir());
    }

    #[test]
    fn a_folder_name_that_is_a_path_is_refused_outright() {
        let directory = scratch("claim-folder-path");
        // `safe_name` is what callers pass through; this is the second check,
        // which does not depend on that one having thought of everything.
        assert!(claim_dir(directory.path(), "../elsewhere").is_err());
        assert!(claim_dir(directory.path(), "sub/deep").is_err());
    }

    #[test]
    fn a_finished_part_lands_under_its_real_name_and_replaces_nothing() {
        let directory = scratch("commit");
        std::fs::write(directory.path().join("report.txt"), "the first one").unwrap();

        let part = claim(directory.path(), &part_name("report.txt")).unwrap();
        assert_eq!(part.file_name().unwrap(), "report.txt.part");
        std::fs::write(&part, "the second one").unwrap();

        let landed = commit(&part, directory.path(), "report.txt").unwrap();
        assert_eq!(landed.file_name().unwrap(), "report (1).txt");
        assert_eq!(std::fs::read_to_string(&landed).unwrap(), "the second one");
        assert_eq!(
            std::fs::read_to_string(directory.path().join("report.txt")).unwrap(),
            "the first one"
        );
        assert!(!part.exists(), "the part file is gone once it has landed");
    }

    #[test]
    fn a_failed_transfer_leaves_nothing_behind() {
        let directory = scratch("discard");
        let part = claim(directory.path(), &part_name("big.iso")).unwrap();
        std::fs::write(&part, "half of it").unwrap();
        discard(&part);
        assert!(!part.exists());
        assert!(!directory.path().join("big.iso").exists());
    }

    #[test]
    fn a_commit_that_cannot_move_the_file_cleans_up_both_names() {
        let directory = scratch("commit-fails");
        // A part path that does not exist: the rename has nothing to move.
        let ghost = directory.path().join("ghost.part");
        assert!(commit(&ghost, directory.path(), "ghost").is_err());
        assert!(!directory.path().join("ghost").exists(), "the claimed name was released");
    }

    /// A directory that removes itself, so a failed test leaves nothing behind.
    struct Scratch(PathBuf);

    impl Scratch {
        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn scratch(name: &str) -> Scratch {
        let path = std::env::temp_dir().join(format!(
            "afterm-localfs-{name}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).unwrap();
        Scratch(path)
    }
}
