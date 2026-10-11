use serde::Serialize;
use std::{fs, path::PathBuf};

/// A public key found on this machine, ready to install on a remote host.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DiscoveredKey {
    /// Absolute path of the .pub file it came from.
    pub path: String,
    /// File name, for labelling the choice.
    pub name: String,
    /// Key type, e.g. `ssh-ed25519`.
    pub kind: String,
    /// Trailing comment, usually `user@host`. Empty when the key has none.
    pub comment: String,
    /// The single-line key itself, exactly as it must be installed.
    pub content: String,
}

/// Key types afterm will offer to install, in the order they are preferred.
/// DSA and bare ECDSA-SK are deliberately absent: OpenSSH no longer accepts the
/// former by default, and afterm's installer does not validate the latter.
const ACCEPTED: [&str; 4] = [
    "ssh-ed25519",
    "sk-ssh-ed25519@openssh.com",
    "ecdsa-sha2-nistp256",
    "ssh-rsa",
];

fn rank(kind: &str) -> usize {
    ACCEPTED.iter().position(|value| *value == kind).unwrap_or(usize::MAX)
}

/// Parse one line of a `.pub` file into a key, or `None` if it is not one.
pub fn parse(path: &str, name: &str, line: &str) -> Option<DiscoveredKey> {
    let trimmed = line.trim();
    let mut parts = trimmed.splitn(3, ' ');
    let kind = parts.next()?;
    let body = parts.next()?;
    let comment = parts.next().unwrap_or("").trim();

    if !ACCEPTED.contains(&kind) || body.is_empty() {
        return None;
    }
    // A public key body is base64. Anything else is not a key, and must not be
    // fed to the installer.
    if !body.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'+' || b == b'/' || b == b'=') {
        return None;
    }
    Some(DiscoveredKey {
        path: path.to_string(),
        name: name.to_string(),
        kind: kind.to_string(),
        comment: comment.to_string(),
        content: format!("{kind} {body}{}", if comment.is_empty() { String::new() } else { format!(" {comment}") }),
    })
}

fn search_directories() -> Vec<PathBuf> {
    let mut directories = Vec::new();
    if let Some(home) = dirs::home_dir() {
        directories.push(home.join(".ssh"));
    }
    directories
}

/// Find the public keys already on this machine, so installing one is a click
/// rather than a copy-paste. Only `.pub` files are read; a private key is never
/// opened, let alone transferred.
pub fn discover() -> Vec<DiscoveredKey> {
    let mut found: Vec<DiscoveredKey> = Vec::new();
    for directory in search_directories() {
        let Ok(entries) = fs::read_dir(&directory) else { continue };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|value| value.to_str()) != Some("pub") {
                continue;
            }
            let Ok(contents) = fs::read_to_string(&path) else { continue };
            let name = path.file_name().unwrap_or_default().to_string_lossy().to_string();
            let display = path.to_string_lossy().to_string();
            // A .pub file holds one key, but tolerate stray blank lines.
            if let Some(key) = contents.lines().find_map(|line| parse(&display, &name, line)) {
                found.push(key);
            }
        }
    }
    // Strongest key type first, then by name so the order is stable.
    found.sort_by(|a, b| rank(&a.kind).cmp(&rank(&b.kind)).then_with(|| a.name.cmp(&b.name)));
    found.dedup_by(|a, b| a.content == b.content);
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_key_with_a_comment() {
        let key = parse("/h/.ssh/id_ed25519.pub", "id_ed25519.pub", "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5 arya@desk").unwrap();
        assert_eq!(key.kind, "ssh-ed25519");
        assert_eq!(key.comment, "arya@desk");
        assert_eq!(key.content, "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5 arya@desk");
        assert_eq!(key.name, "id_ed25519.pub");
    }

    #[test]
    fn parses_a_key_with_no_comment() {
        let key = parse("p", "n", "ssh-rsa AAAAB3NzaC1yc2E=").unwrap();
        assert_eq!(key.comment, "");
        assert_eq!(key.content, "ssh-rsa AAAAB3NzaC1yc2E=");
    }

    #[test]
    fn rejects_private_keys_and_junk() {
        assert!(parse("p", "n", "-----BEGIN OPENSSH PRIVATE KEY-----").is_none());
        assert!(parse("p", "n", "ssh-ed25519").is_none());
        assert!(parse("p", "n", "").is_none());
        // An unsupported type must not be offered, since the installer would reject it.
        assert!(parse("p", "n", "ssh-dss AAAAB3Nza comment").is_none());
        // A body that is not base64 is not a key.
        assert!(parse("p", "n", "ssh-ed25519 not$valid$base64 c").is_none());
    }

    #[test]
    fn prefers_ed25519_over_rsa() {
        assert!(rank("ssh-ed25519") < rank("ssh-rsa"));
        assert!(rank("ssh-rsa") < rank("ssh-dss"));
    }
}
