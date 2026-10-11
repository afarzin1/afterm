use serde::Serialize;

/// One entry in a directory listing. Shared by the local filesystem and SFTP
/// backends so the frontend renders both through the same component.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FileEntry {
    pub name: String,
    pub path: String,
    pub kind: EntryKind,
    pub size: Option<u64>,
    pub modified: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum EntryKind {
    Directory,
    File,
    Link,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Listing {
    /// Absolute path that was listed, as the backend resolved it.
    pub path: String,
    /// Parent directory, or `None` at a filesystem root.
    pub parent: Option<String>,
    pub entries: Vec<FileEntry>,
}

/// Sort directories first, then by case-insensitive name. Applied to both
/// backends so listings do not reorder when switching session kinds.
pub fn sort_entries(entries: &mut [FileEntry]) {
    entries.sort_by(|a, b| {
        let rank = |entry: &FileEntry| match entry.kind {
            EntryKind::Directory => 0,
            _ => 1,
        };
        rank(a)
            .cmp(&rank(b))
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
}

/// A remote directory and a name in it, joined. Remote paths are POSIX
/// whatever this machine runs, so the separator is not the local one.
///
/// Shared rather than written per transport: the folder walk joins paths for
/// both of them, and a tree that nested differently depending on which
/// transport carried it would be a difference nobody could see until it
/// mattered.
pub fn join(directory: &str, name: &str) -> String {
    if directory.ends_with('/') {
        format!("{directory}{name}")
    } else {
        format!("{directory}/{name}")
    }
}

/// `name`, with a counter worked in ahead of its extension: `report (1).txt`.
///
/// The shape every browser and Explorer use for a second copy, so a file that
/// arrives under it reads as what it is. The extension is everything after the
/// last dot, except for a name that *starts* with its only dot: `.bashrc` has
/// no extension to keep, and `.bashrc (1)` is the name that says so.
pub fn numbered(name: &str, count: usize) -> String {
    match name.rfind('.') {
        Some(dot) if dot > 0 => format!("{} ({count}){}", &name[..dot], &name[dot..]),
        _ => format!("{name} ({count})"),
    }
}

// ------------------------------------------------------------- permissions

/// Permissions a staging file wears while an upload is in flight, so a partly
/// written file is never readable to anyone but its owner.
///
/// afterm's own transport puts it in the `open` request, before a single byte
/// has reached the file. The OpenSSH-client transport cannot — the batch
/// language has no way to give `put` a mode — so there the file is protected
/// by the private directory it is created in, and this is applied to the file
/// straight afterwards as well. See `sftp::deliver`.
pub const STAGING_MODE: u32 = 0o600;

/// What a brand-new upload lands as when the local file has no Unix mode to
/// copy, which is every file on Windows.
///
/// `0644` rather than `STAGING_MODE`: a file uploaded to a server is
/// ordinarily meant to be readable, and a mode nobody chose should be the
/// unsurprising one rather than the strictest one. A replacement never reaches
/// this — it wears what it replaced.
pub const NEW_FILE_MODE: u32 = 0o644;

/// The permission bits a replacement carries across.
///
/// Setuid, setgid and the sticky bit are deliberately not among them: they are
/// a grant of privilege attached to particular content, and re-applying one to
/// content that arrived from somewhere else is how a setuid binary becomes
/// somebody else's. `carried` refuses rather than dropping them quietly, so
/// nobody finds out by noticing a program stopped working.
///
/// Ownership and ACLs are outside what any of this can promise. The protocol's
/// basic attributes carry uid and gid, which afterm does not set — a rename
/// keeps the staging file's owner, which is the connected user, and that is
/// what the OpenSSH client does too — and POSIX ACLs are not in the protocol
/// at all. A file whose protection lives in an ACL rather than in its mode is
/// not one afterm can promise to preserve.
pub const CARRIED_MODE: u32 = 0o777;

/// The mode a replacement must wear, given the mode of what it replaces.
///
/// `Err` carries the offending bits, and means the replacement must not happen.
pub fn carried(existing: u32) -> Result<u32, String> {
    if existing & 0o7000 != 0 {
        return Err(format!("{:04o}", existing & 0o7777));
    }
    Ok(existing & CARRIED_MODE)
}

/// The mode a new upload should land under, from the local file it came from.
///
/// On Unix this is the local file's own permissions, which is what the OpenSSH
/// client's `put` sends. A local file with no permission bits at all is not a
/// mode worth copying onto a remote host — it would arrive unreadable to its
/// own owner — so that falls back with everything else.
#[cfg(unix)]
pub fn local_mode(local: Option<&std::fs::Metadata>) -> u32 {
    use std::os::unix::fs::PermissionsExt;
    match local.map(|metadata| metadata.permissions().mode() & CARRIED_MODE) {
        Some(0) | None => NEW_FILE_MODE,
        Some(mode) => mode,
    }
}

/// Windows has no Unix mode to copy, so every new upload lands the same way.
#[cfg(not(unix))]
pub fn local_mode(_: Option<&std::fs::Metadata>) -> u32 {
    NEW_FILE_MODE
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::numbered;

    /// A replacement keeps the ordinary bits and is refused outright over the
    /// privileged ones, rather than quietly dropping them.
    #[test]
    fn a_replacement_carries_ordinary_bits_and_refuses_privileged_ones() {
        assert_eq!(carried(0o600), Ok(0o600));
        assert_eq!(carried(0o755), Ok(0o755));
        for special in [0o4755, 0o2755, 0o1777] {
            assert!(carried(special).is_err(), "{special:o} must be refused");
        }
    }

    #[test]
    fn numbers_a_name_ahead_of_its_extension() {
        assert_eq!(numbered("report.txt", 1), "report (1).txt");
        assert_eq!(numbered("archive.tar.gz", 2), "archive.tar (2).gz");
        assert_eq!(numbered("Makefile", 1), "Makefile (1)");
        assert_eq!(numbered(".bashrc", 1), ".bashrc (1)");
    }
}
