//! Whether the OpenSSH command-line tools afterm shells out to are here at all.
//!
//! The SSH protocol itself is compiled in — `russh` carries every session. But
//! two jobs are deliberately handed to the user's own client, because doing
//! them a second way would mean quietly disagreeing with it: `ssh -G` resolves
//! the configuration, and `ssh-keygen -F` says which host keys are trusted.
//! Both run on every native connection, and the OpenSSH-client transport is
//! those same binaries again.
//!
//! So a machine without them cannot connect to anything, and until this check
//! existed it said so as `could not ask the ssh client to resolve its
//! configuration: program not found` — the truth, in the wrong vocabulary, at
//! the wrong moment. The interface asks once at startup instead and says what
//! to install.
//!
//! Windows 10 1809 and later and Windows 11 install the OpenSSH client by
//! default, so this is the uncommon case: a feature someone removed, a
//! hardened image, or a Server SKU.

use crate::winproc::hide_console;
use serde::Serialize;
use std::process::{Command, Stdio};

/// The tools afterm cannot work without, in the order they are reported.
const REQUIRED: [&str; 2] = ["ssh", "ssh-keygen"];

fn binary(tool: &str) -> String {
    if cfg!(windows) { format!("{tool}.exe") } else { tool.to_string() }
}

/// What this machine has.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Presence {
    /// Tools that could not be run, by the name the user would install them
    /// under. Empty is the good answer, and the only one afterm can work with.
    pub missing: Vec<String>,
    /// What `ssh -V` said, when there was a client to ask. Shown so a report
    /// about client-specific behaviour carries the client's own version.
    pub version: Option<String>,
}

/// Try to run each tool. Nothing is asked of a host and nothing is parsed: the
/// question is only whether the process starts.
pub fn check() -> Presence {
    let missing = REQUIRED
        .iter()
        .filter(|tool| !runs(&binary(tool)))
        .map(|tool| binary(tool))
        .collect();
    Presence { missing, version: version() }
}

/// `-V` on the client, an unrecognised option on the rest: either way the
/// process starting is the whole of the answer, and neither touches a host.
fn runs(binary: &str) -> bool {
    let mut builder = Command::new(binary);
    hide_console(&mut builder)
        .arg("-V")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok()
}

/// OpenSSH writes its version to stderr and exits 0.
fn version() -> Option<String> {
    let mut builder = Command::new(binary("ssh"));
    let output = hide_console(&mut builder)
        .arg("-V")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .output()
        .ok()?;
    let text = String::from_utf8_lossy(&output.stderr);
    let line = text.lines().next()?.trim();
    (!line.is_empty()).then(|| line.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The names are what the user would go and install, so on Windows they
    /// carry the extension and elsewhere they do not.
    #[test]
    fn names_the_binary_for_this_platform() {
        assert_eq!(binary("ssh"), if cfg!(windows) { "ssh.exe" } else { "ssh" });
    }

    /// Whatever this machine has, the answer is well formed: every reported
    /// name is one of the required tools, and none is reported twice.
    #[test]
    fn reports_only_required_tools_and_never_twice() {
        let presence = check();
        let expected: Vec<String> = REQUIRED.iter().map(|tool| binary(tool)).collect();
        for name in &presence.missing {
            assert!(expected.contains(name), "unexpected tool reported: {name}");
        }
        let mut sorted = presence.missing.clone();
        sorted.sort();
        sorted.dedup();
        assert_eq!(sorted.len(), presence.missing.len());
    }

    /// A version string only ever accompanies a client that ran. This is the
    /// pair the interface relies on to decide it has nothing to say.
    #[test]
    fn a_version_means_the_client_ran() {
        let presence = check();
        if presence.version.is_some() {
            assert!(!presence.missing.contains(&binary("ssh")));
        }
    }
}
