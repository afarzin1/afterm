//! A shell that outlives the connection, through tmux on the host.
//!
//! Opt in per saved host. With it on, the tab's terminal runs
//! `tmux new-session -A` under a name that belongs to the tab rather than a
//! login shell, so a connection that drops takes the tmux client with it and
//! leaves the session, and the reconnect — by hand or on its own — attaches to
//! it again: the same shell, the same working directory, a build still running.
//!
//! tmux rather than anything of afterm's own, because afterm installs nothing
//! on the far end. A host without tmux gets its ordinary login shell and a line
//! saying why the shell will not survive, rather than a failure.
//!
//! The name is per tab, and the tab keeps its id across reconnects, so a
//! reconnect finds its own session and two tabs on one host never share one.
//! Closing the tab is what ends it — the one time afterm runs `kill-session`
//! — because a session nothing will ever attach to again is a shell left
//! running on someone's server for nothing. A connection that has already gone
//! when the tab is closed cannot be asked to do that, and the session stays
//! until tmux or the host is restarted; `tmux ls` shows it by the same name.

/// The tmux session for a tab: `afterm-` and the first eight letters and
/// digits of the tab's id. Only those characters, so the name can go into a
/// shell command without quoting and into a tmux target without being read as
/// a pattern.
pub fn session_name(tab: &str) -> String {
    let tail: String = tab
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .map(|c| c.to_ascii_lowercase())
        .take(8)
        .collect();
    format!("afterm-{tail}")
}

/// What the host runs in place of a login shell.
///
/// Through `sh -c` because the command is handed to the user's own login
/// shell, which may be fish or something else that does not read `if … fi`.
/// `-A` attaches when the session exists and makes it when it does not; `-D`
/// detaches any other client first, which after a drop is the dead one the
/// host has not noticed yet — left attached, its old size would hold the new
/// window to it until the server's keepalive gave up.
pub fn attach_command(name: &str) -> String {
    format!(
        "sh -c 'if command -v tmux >/dev/null 2>&1; then exec tmux new-session -A -D -s {name}; fi; \
         echo \"afterm: tmux is not installed on this host, so this shell will not outlive the connection.\"; \
         exec \"${{SHELL:-/bin/sh}}\" -l'"
    )
}

/// What ends the session when its tab is closed. `=` makes the target an
/// exact name rather than a prefix, and a session that is already gone is not
/// an error worth reporting.
pub fn end_command(name: &str) -> String {
    format!("tmux kill-session -t ={name} 2>/dev/null; true")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_name_is_the_tabs_own_and_safe_to_put_in_a_command() {
        assert_eq!(session_name("3F2A9C81-b7de-4c1a-9e2f-0a1b2c3d4e5f"), "afterm-3f2a9c81");
        // Whatever an id turns out to hold, nothing but letters and digits
        // reaches the command line.
        assert_eq!(session_name("a'; rm -rf ~; '"), "afterm-armrf");
        assert_eq!(session_name("tab-1"), "afterm-tab1");
    }

    #[test]
    fn the_command_attaches_or_makes_the_session_and_falls_back_to_a_shell() {
        let command = attach_command("afterm-3f2a9c81");
        assert!(command.starts_with("sh -c '"));
        assert!(command.contains("exec tmux new-session -A -D -s afterm-3f2a9c81"));
        assert!(command.contains("exec \"${SHELL:-/bin/sh}\" -l"));
        // One level of quoting, closed where it opened: nothing inside may end it.
        assert_eq!(command.matches('\'').count(), 2);
    }

    #[test]
    fn ending_names_the_session_exactly() {
        assert_eq!(end_command("afterm-3f2a9c81"), "tmux kill-session -t =afterm-3f2a9c81 2>/dev/null; true");
    }
}
