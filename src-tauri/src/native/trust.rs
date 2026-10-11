//! Which host keys the user already trusts, according to the user's own client.
//!
//! russh ships a `known_hosts` checker, and it is not OpenSSH's. Compiled
//! against 0.63.2 and probed directly, it silently skips `@revoked` and
//! `@cert-authority` lines, does not match wildcard host patterns or
//! tab-separated fields, and reports a key of an algorithm it was not expecting
//! as an *unknown host* rather than a mismatch. Every one of those failures
//! points the same way — toward "unknown" — which is safe while unknown always
//! refuses, and unsafe the moment afterm offers to accept an unknown key: a
//! revoked entry would be offered for acceptance.
//!
//! So afterm asks `ssh-keygen -F` instead. It resolves hashing and wildcards,
//! searches whichever file it is given, exits 1 when nothing matches, and marks
//! what it found:
//!
//! ```text
//! # Host rev.example found: line 2 REVOKED
//! @revoked rev.example ssh-ed25519 AAAA...
//! ```
//!
//! A revoked key or a certificate authority is a refusal with a named reason,
//! not something to work around.

use crate::transport::Failure;
use crate::winproc::hide_console;
use russh::keys::{parse_public_key_base64, Algorithm, HashAlg, PublicKey};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// What the user's known_hosts files say about one host.
#[derive(Debug, Default, Clone)]
pub struct Trusted {
    /// Every key recorded for this host, across every file searched.
    pub keys: Vec<PublicKey>,
    /// The host key algorithms to negotiate, derived from those keys.
    ///
    /// Without this the client may negotiate a key type the user's OpenSSH
    /// never recorded — ed25519 against a host whose entry is the ecdsa key
    /// `ssh.exe` saved — and refuse a host that has been trusted for years.
    pub algorithms: Vec<Algorithm>,
}

impl Trusted {
    pub fn is_empty(&self) -> bool {
        self.keys.is_empty()
    }

    /// Whether this is a key the user has already accepted. Compares key
    /// material rather than whole records, so a comment cannot change the
    /// answer.
    pub fn accepts(&self, offered: &PublicKey) -> bool {
        self.keys.iter().any(|known| known.key_data() == offered.key_data())
    }
}

/// Look this host up in every known_hosts file OpenSSH would search.
pub fn lookup(host: &str, port: u16, files: &[PathBuf]) -> Result<Trusted, Failure> {
    // The host reaches a command line, so it must not be able to look like an
    // option or split into extra arguments. The same rule the profile fields
    // are held to.
    if host.is_empty() || host.starts_with('-') || host.contains(char::is_whitespace) {
        return Err(Failure::host_key("this host name cannot be looked up safely"));
    }
    let target = if port == 22 {
        host.to_string()
    } else {
        format!("[{host}]:{port}")
    };

    let mut trusted = Trusted::default();
    for file in files {
        if !file.exists() {
            continue;
        }
        let found = search(&target, file)?;
        for record in found {
            match record.marker {
                // A distrusted host. Not a fallback case: nothing should
                // connect to it, and saying why is the whole point.
                Some(Marker::Revoked) => {
                    return Err(Failure::host_key(format!(
                        "{host} has a revoked host key recorded in {}; afterm will not \
                         connect to it",
                        file.display()
                    )))
                }
                // Not a distrusted host — a host afterm cannot verify. That is
                // what sends it back to the OpenSSH client, which can.
                Some(Marker::CertAuthority) => {
                    return Err(Failure::unsupported(format!(
                        "{host} is verified through an SSH certificate authority in {}, \
                         which afterm's native transport does not implement, so it stays \
                         on the OpenSSH client",
                        file.display()
                    )))
                }
                None => {}
            }
            trusted.keys.push(record.key);
        }
    }
    trusted.algorithms = negotiable(&trusted.keys);
    Ok(trusted)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Marker {
    Revoked,
    CertAuthority,
}

struct Record {
    key: PublicKey,
    marker: Option<Marker>,
}

fn search(target: &str, file: &Path) -> Result<Vec<Record>, Failure> {
    let mut builder = Command::new(if cfg!(windows) { "ssh-keygen.exe" } else { "ssh-keygen" });
    let output = hide_console(&mut builder)
        .arg("-F")
        .arg(target)
        .arg("-f")
        .arg(file)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        // A tool that will not run is not a verdict about this host's key.
        .map_err(|error| {
            Failure::spawn(if cfg!(windows) { "ssh-keygen.exe" } else { "ssh-keygen" }, &error)
        })?;

    // Exit 1 is "no entry for this host", which is an answer, not a failure.
    if !output.status.success() && output.status.code() != Some(1) {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(Failure::host_key(format!(
            "could not read {}: {}",
            file.display(),
            stderr.trim().lines().next().unwrap_or("ssh-keygen failed")
        )));
    }
    parse(&String::from_utf8_lossy(&output.stdout))
}

fn parse(text: &str) -> Result<Vec<Record>, Failure> {
    let mut records = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let mut fields = line.split_whitespace();
        let mut first = match fields.next() {
            Some(value) => value,
            None => continue,
        };
        let marker = match first {
            "@revoked" => Some(Marker::Revoked),
            "@cert-authority" => Some(Marker::CertAuthority),
            other if other.starts_with('@') => {
                // An unrecognised marker changes the meaning of the line in a
                // way afterm cannot guess at. Refuse rather than treat it as an
                // ordinary entry.
                return Err(Failure::unsupported(format!(
                    "known_hosts uses the marker {other}, which afterm does not understand, \
                     so this host stays on the OpenSSH client"
                )));
            }
            _ => None,
        };
        if marker.is_some() {
            // Step past the marker to the host pattern.
            first = match fields.next() {
                Some(value) => value,
                None => continue,
            };
        }
        let _ = first;
        // ... then the key type and the key itself.
        let (Some(_kind), Some(encoded)) = (fields.next(), fields.next()) else {
            continue;
        };
        match parse_public_key_base64(encoded) {
            Ok(key) => records.push(Record { key, marker }),
            // A line ssh-keygen matched but afterm cannot decode is not
            // something to shrug at: it is a trusted record afterm cannot see.
            Err(error) => {
                return Err(Failure::host_key(format!(
                    "a known_hosts entry for this host could not be read: {error}"
                )))
            }
        }
    }
    Ok(records)
}

/// The host key algorithms to offer, given what the user has recorded.
///
/// RSA is expanded to its SHA-2 signature variants: the recorded key is the
/// same key whether the server signs with `ssh-rsa`, `rsa-sha2-256` or
/// `rsa-sha2-512`, and offering only the bare name would ask a modern server
/// for a SHA-1 signature it refuses to give.
fn negotiable(keys: &[PublicKey]) -> Vec<Algorithm> {
    let mut out: Vec<Algorithm> = Vec::new();
    let mut push = |algorithm: Algorithm| {
        if !out.contains(&algorithm) {
            out.push(algorithm);
        }
    };
    for key in keys {
        match key.algorithm() {
            Algorithm::Rsa { .. } => {
                push(Algorithm::Rsa { hash: Some(HashAlg::Sha512) });
                push(Algorithm::Rsa { hash: Some(HashAlg::Sha256) });
                push(Algorithm::Rsa { hash: None });
            }
            other => push(other),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const ED25519: &str = "AAAAC3NzaC1lZDI1NTE5AAAAIJdD7y3aLq454yWBdwLWbieU1ebz9/cu7/QEXn9OIeZJ";
    const ED25519_OTHER: &str = "AAAAC3NzaC1lZDI1NTE5AAAAIA6rWI3G2sz07DnfFlrouTcysQlj2P+jpNSOEWD9OJ3X";
    const RSA: &str = "AAAAB3NzaC1yc2EAAAADAQABAAABAQCI4Bw7W4uh4QgEM/y2NJdLfs/XePmjyaH52PgaQqLnYYc1Csw5kmxbfp82CCz8dpYPijomgf2DTfiLeyCUkL4pyg9KHuHVTgIxlRzMLeHQ4gzqZ6hUOi7+dzulhLfLhrNCMbTQHoG8VztTM/oQJHc6q84HzopwPkgqNVKDrLN/7DckPeiqA9IO31ddnC4E5WgIi3CWlS0ypIJZqlf3gxtBP+8BEtKEA0nw2vf9MF+ZQ/BYP0w/5dkzNXge4LrB2Izm1J0rZtrCiB3zxF3KpDiolvHkdtIqdxrF5wPtMzf2A1ph0djOjqdUu1kH2L87lwS+y0kJ9xQ8N0lc2ljn96yP";

    #[test]
    fn reads_a_plain_entry() {
        let records = parse(&format!("# Host h found: line 1 \nh ssh-ed25519 {ED25519}\n")).unwrap();
        assert_eq!(records.len(), 1);
        assert!(records[0].marker.is_none());
    }

    #[test]
    fn reads_a_hashed_entry_that_ssh_keygen_resolved() {
        let text = format!(
            "# Host h found: line 1 \n|1|O33ESRMWPVkMYIwJ1Uw+n877jTo=|nuuC5vEqXlEZ/8BXQR7m619W6Ak= ssh-ed25519 {ED25519}\n"
        );
        assert_eq!(parse(&text).unwrap().len(), 1);
    }

    #[test]
    fn sees_the_revoked_marker_that_russh_skips_silently() {
        let records = parse(&format!("@revoked h ssh-ed25519 {ED25519}\n")).unwrap();
        assert_eq!(records[0].marker, Some(Marker::Revoked));
    }

    #[test]
    fn sees_the_cert_authority_marker() {
        let records = parse(&format!("@cert-authority *.h ssh-ed25519 {ED25519}\n")).unwrap();
        assert_eq!(records[0].marker, Some(Marker::CertAuthority));
    }

    #[test]
    fn refuses_a_marker_it_does_not_understand_rather_than_ignoring_it() {
        assert!(parse(&format!("@future h ssh-ed25519 {ED25519}\n")).is_err());
    }

    #[test]
    fn accepts_by_key_material_not_by_record() {
        let trusted = Trusted {
            keys: vec![parse_public_key_base64(ED25519).unwrap()],
            algorithms: Vec::new(),
        };
        assert!(trusted.accepts(&parse_public_key_base64(ED25519).unwrap()));
        assert!(!trusted.accepts(&parse_public_key_base64(ED25519_OTHER).unwrap()));
    }

    #[test]
    fn pins_negotiation_to_the_recorded_algorithms() {
        let keys = vec![parse_public_key_base64(ED25519).unwrap()];
        assert_eq!(negotiable(&keys), vec![Algorithm::Ed25519]);
    }

    #[test]
    fn an_rsa_entry_still_allows_a_sha2_signature() {
        // The recorded key is the same key whichever hash the server signs
        // with; pinning to the bare name alone would ask a modern server for a
        // SHA-1 signature it refuses to give.
        let keys = vec![parse_public_key_base64(RSA).unwrap()];
        let algorithms = negotiable(&keys);
        assert!(algorithms.contains(&Algorithm::Rsa { hash: Some(HashAlg::Sha512) }));
        assert!(algorithms.contains(&Algorithm::Rsa { hash: Some(HashAlg::Sha256) }));
        assert!(algorithms.contains(&Algorithm::Rsa { hash: None }));
    }

    #[test]
    fn two_recorded_algorithms_are_both_offered_once() {
        let keys = vec![
            parse_public_key_base64(ED25519).unwrap(),
            parse_public_key_base64(ED25519_OTHER).unwrap(),
        ];
        // Same algorithm twice collapses to one entry.
        assert_eq!(negotiable(&keys), vec![Algorithm::Ed25519]);
    }

    // ------------------------------------------------ against the real binary
    //
    // Everything above parses fixture text. These run `ssh-keygen -F` itself,
    // which is the point: the reason afterm uses the oracle rather than russh's
    // reader is that the oracle knows things the reader does not, and only the
    // real binary can demonstrate that. On the Windows agent this is also the
    // only check that Win32-OpenSSH's `ssh-keygen` behaves as assumed.

    fn have_ssh_keygen() -> bool {
        Command::new(if cfg!(windows) { "ssh-keygen.exe" } else { "ssh-keygen" })
            .arg("-?")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok()
    }

    /// A file in a fresh directory, so nothing here can touch a real
    /// `~/.ssh/known_hosts`.
    fn known_hosts_file(name: &str, contents: &str) -> PathBuf {
        let directory = std::env::temp_dir().join(format!(
            "afterm-trust-{}-{name}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        std::fs::create_dir_all(&directory).unwrap();
        let path = directory.join("known_hosts");
        std::fs::write(&path, contents).unwrap();
        path
    }

    #[test]
    fn the_real_oracle_finds_a_plain_entry_and_pins_its_algorithm() {
        if !have_ssh_keygen() {
            eprintln!("no ssh-keygen on PATH; skipping");
            return;
        }
        let file = known_hosts_file("plain", &format!("host.example ssh-ed25519 {ED25519}\n"));
        let trusted = lookup("host.example", 22, &[file]).expect("lookup");
        assert_eq!(trusted.keys.len(), 1);
        assert_eq!(trusted.algorithms, vec![Algorithm::Ed25519]);
        assert!(trusted.accepts(&parse_public_key_base64(ED25519).unwrap()));
    }

    /// The shape russh's reader cannot see. `@revoked` must refuse, and refuse
    /// as a host key problem rather than as something to fall back from.
    #[test]
    fn the_real_oracle_refuses_a_revoked_key() {
        if !have_ssh_keygen() {
            return;
        }
        let file = known_hosts_file("revoked", &format!("@revoked host.example ssh-ed25519 {ED25519}\n"));
        let failure = lookup("host.example", 22, &[file]).expect_err("a revoked key must refuse");
        assert_eq!(failure.cause, crate::transport::Cause::HostKey);
        assert!(failure.message.contains("revoked"), "{}", failure.message);
    }

    /// A certificate authority is afterm's limitation, not a distrusted host,
    /// so it falls back to the OpenSSH client instead of refusing outright.
    #[test]
    fn the_real_oracle_sends_a_certificate_host_to_the_openssh_client() {
        if !have_ssh_keygen() {
            return;
        }
        let file = known_hosts_file("ca", &format!("@cert-authority *.example ssh-ed25519 {ED25519}\n"));
        let failure = lookup("host.example", 22, &[file]).expect_err("a CA entry is not usable");
        assert_eq!(failure.cause, crate::transport::Cause::Unsupported);
    }

    /// Two shapes russh's reader misses outright: a hashed entry and a wildcard
    /// pattern. `ssh-keygen -H` rewrites the file the way `HashKnownHosts` does.
    #[test]
    fn the_real_oracle_resolves_hashed_and_wildcard_entries() {
        if !have_ssh_keygen() {
            return;
        }
        let wildcard = known_hosts_file("wildcard", &format!("*.example ssh-ed25519 {ED25519}\n"));
        assert_eq!(
            lookup("host.example", 22, &[wildcard]).expect("lookup").keys.len(),
            1,
            "a wildcard pattern should match"
        );

        let hashed = known_hosts_file("hashed", &format!("host.example ssh-ed25519 {ED25519}\n"));
        let status = Command::new(if cfg!(windows) { "ssh-keygen.exe" } else { "ssh-keygen" })
            .arg("-H")
            .arg("-f")
            .arg(&hashed)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .expect("ssh-keygen -H");
        assert!(status.success());
        assert_eq!(
            lookup("host.example", 22, &[hashed]).expect("lookup").keys.len(),
            1,
            "a hashed entry should match"
        );
    }

    /// A non-default port is a different host, in both directions.
    #[test]
    fn the_real_oracle_keeps_ports_apart() {
        if !have_ssh_keygen() {
            return;
        }
        let file = known_hosts_file("ports", &format!("[host.example]:2222 ssh-ed25519 {ED25519}\n"));
        assert!(lookup("host.example", 2222, &[file.clone()]).unwrap().keys.len() == 1);
        assert!(lookup("host.example", 22, &[file]).unwrap().is_empty());
    }

    /// A host name that could be read as an option never reaches a command line.
    #[test]
    fn a_host_that_could_look_like_an_option_is_refused_before_it_runs() {
        assert!(lookup("-oProxyCommand=evil", 22, &[]).is_err());
        assert!(lookup("host example", 22, &[]).is_err());
    }

    #[test]
    fn nothing_recorded_is_an_empty_answer_not_an_error() {
        let trusted = Trusted::default();
        assert!(trusted.is_empty());
        assert!(!trusted.accepts(&parse_public_key_base64(ED25519).unwrap()));
    }
}
