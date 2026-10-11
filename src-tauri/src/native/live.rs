//! The native transport against a real OpenSSH server.
//!
//! The battery proves the protocol against an in-process server, which is what
//! makes it runnable on the Windows agent. It cannot prove the parts that talk
//! to the *installed* OpenSSH: `ssh -G` resolving a real config, `ssh-keygen -F`
//! reading a real known_hosts, and an exec channel against a real sshd running
//! a real shell. Those are exactly the seams this project has been wrong about
//! before, so they get a test too — one that needs a server, and so is ignored
//! by default.
//!
//! `scripts/live-native-test.sh` stands up a throwaway sshd and a throwaway
//! HOME, and runs this.
//!
//! The harness points the client at a throwaway `ssh_config` with `-F`, because
//! OpenSSH resolves `~` through the password database rather than `$HOME`:
//! setting `HOME` does not redirect it, which is worth knowing before trying.

use super::*;
use std::borrow::Cow;
use crate::transport::Cause;

fn from_env() -> (Target, String) {
    (
        Target {
            // The live test connects to an address, not a saved host.
            profile: None,
            session: "live".into(),
            host: std::env::var("AFTERM_LIVE_HOST").expect("AFTERM_LIVE_HOST"),
            user: std::env::var("AFTERM_LIVE_USER").expect("AFTERM_LIVE_USER"),
            port: std::env::var("AFTERM_LIVE_PORT").unwrap().parse().unwrap(),
            config_file: Some(std::path::PathBuf::from(
                std::env::var("AFTERM_LIVE_CONFIG").expect("AFTERM_LIVE_CONFIG"),
            )),
        },
        std::env::var("AFTERM_LIVE_DIR").expect("AFTERM_LIVE_DIR"),
    )
}

fn quiet() -> Prompts {
    // Key authentication needs no dialog, and anything that asked for one here
    // would be a failure worth seeing rather than answering.
    Prompts::new(Box::new(NoPrompts))
}

/// No credential store: the live test authenticates with a key, so there is
/// nothing here to remember and nothing that would look one up.
fn no_store() -> crate::secrets::Vault {
    crate::secrets::Vault::new(std::sync::Arc::new(crate::secrets::Absent::new(
        "the live test saves no passwords",
    )))
}

async fn wait_for<T>(mut check: impl FnMut() -> Option<T>) -> Option<T> {
    for _ in 0..240 {
        if let Some(value) = check() {
            return Some(value);
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    None
}

/// The whole path, in the order the application walks it.
#[tokio::test]
#[ignore = "requires the sshd fixture; run scripts/live-native-test.sh"]
async fn the_native_transport_works_against_a_real_server() {
    let (target, root) = from_env();

    // 1. The installed client resolves the alias the harness wrote.
    let effective = config::resolve(
        &target.host,
        &target.user,
        target.port,
        target.config_file.as_deref(),
    )
    .expect("ssh -G");
    assert_eq!(effective.hostname, "127.0.0.1", "the alias should resolve");
    assert_eq!(effective.port, target.port);

    // 2. The installed ssh-keygen finds the key the harness recorded.
    let trusted = trust::lookup(&effective.hostname, effective.port, &effective.known_hosts)
        .expect("ssh-keygen -F");
    assert!(!trusted.is_empty(), "the harness recorded this host");

    // 3. Connect and authenticate with the key the config names.
    let connection = Connection::open(&target, &quiet(), &no_store(), false)
        .await
        .expect("the native transport should connect");
    assert!(connection.method.starts_with("publickey"), "method: {}", connection.method);
    assert!(connection.alive());

    // 4. SFTP on that connection.
    let home = connection.home().await.expect("home");
    assert!(home.starts_with('/'), "home should be absolute, got {home}");

    let listing = connection.list(&root).await.expect("list");
    let names: Vec<&str> = listing.entries.iter().map(|e| e.name.as_str()).collect();
    assert!(names.contains(&"afile.txt"), "missing file in {names:?}");
    assert!(names.contains(&"sub"), "missing directory in {names:?}");
    assert!(names.contains(&"file with space.txt"), "missing spaced name in {names:?}");
    assert_eq!(listing.entries[0].kind, crate::fsmodel::EntryKind::Directory);
    let file = listing.entries.iter().find(|e| e.name == "afile.txt").unwrap();
    assert_eq!(file.size, Some(6));

    assert_eq!(
        connection.list("/__afterm_definitely_missing__").await.unwrap_err().cause,
        Cause::Operation,
        "a missing path must be an error, not an empty listing"
    );

    // A folder made private is `0700` on the host, whatever the umask made it.
    // The server serves this machine's own filesystem, so the mode is read
    // back directly.
    let private = format!("{root}/private");
    connection.create_directory(&private).await.expect("mkdir");
    connection.make_private(&private).await.expect("make private");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&private).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o700);
    }

    // 5. Telemetry, on a channel of that same connection, from a real shell
    //    reading real /proc. The status bar is the surface this feeds, and it
    //    is the one that has been reported not working.
    let metrics = wait_for(|| connection.telemetry().0)
        .await
        .unwrap_or_else(|| panic!("no telemetry sample: {:?}", connection.telemetry().1));
    assert!(metrics.total_memory.unwrap_or(0) > 0, "memory should be read from /proc");
    assert!(metrics.uptime.unwrap_or(0) > 0, "uptime should be read from /proc");

    // The second sample is the one that can report a rate.
    let rated = wait_for(|| connection.telemetry().0.filter(|m| m.cpu.is_some()))
        .await
        .expect("a second telemetry sample");
    assert!(rated.cpu.unwrap() >= 0.0);

    // 6. A transfer, streamed rather than shelled out to.
    let target_file = format!("{root}/downloaded.txt");
    connection
        .download(&format!("{root}/afile.txt"), &target_file, |_, _| {})
        .await
        .expect("download");
    assert_eq!(std::fs::read_to_string(&target_file).unwrap(), "hello\n");

    connection.close().await;
}

/// A folder with things in it is deleted whole over the native transport, and
/// the link inside it is removed as a link rather than walked into.
#[cfg(unix)]
#[tokio::test]
#[ignore = "requires the sshd fixture; run scripts/live-native-test.sh"]
async fn a_folder_is_deleted_with_everything_in_it() {
    let (target, root) = from_env();
    let connection = Connection::open(&target, &quiet(), &no_store(), false)
        .await
        .expect("connect");
    let transport = crate::transport::Transport::Native(std::sync::Arc::new(connection));
    let (doomed, kept) = crate::folder::doomed_tree(&root);

    crate::folder::remove(&transport, &doomed).await.expect("recursive delete");

    assert!(!std::path::Path::new(&doomed).exists(), "the folder is gone");
    assert_eq!(std::fs::read_to_string(&kept).unwrap(), "keep", "the link was not followed");
    transport.close().await;
}

/// A pasted image lands in a private folder made for it, under a name a shell
/// can take as it is, and the folder is kept to a week of them — without
/// anything else in it being touched.
#[cfg(unix)]
#[tokio::test]
#[ignore = "requires the sshd fixture; run scripts/live-native-test.sh"]
async fn a_pasted_image_lands_in_a_private_folder_that_is_kept_tidy() {
    use std::os::unix::fs::PermissionsExt;
    let (target, root) = from_env();
    let connection = Connection::open(&target, &quiet(), &no_store(), false)
        .await
        .expect("connect");
    let transport = crate::transport::Transport::Native(std::sync::Arc::new(connection));
    let now = 1_790_346_612; // 2026-09-25 14:30:12 UTC
    let png = b"\x89PNG\r\n\x1a\nnot really".to_vec();

    let (landed, folder) = crate::clipimage::send(&transport, &root, png.clone(), now)
        .await
        .expect("send");
    assert_eq!(folder, format!("{root}/.cache/afterm/paste"));
    assert!(landed.starts_with(&format!("{folder}/paste-20260925-143012-")), "landed at {landed}");
    assert!(!landed.contains(' '));
    assert_eq!(std::fs::read(&landed).unwrap(), png);
    let mode = std::fs::metadata(&folder).unwrap().permissions().mode();
    assert_eq!(mode & 0o777, 0o700, "the folder is the owner's alone");

    // A second paste into the folder that is now there, and an old one and a
    // stranger's file beside them for the tidy to tell apart.
    let (second, _) = crate::clipimage::send(&transport, &root, png, now).await.expect("again");
    assert_ne!(second, landed);
    let old = format!("{folder}/paste-20260901-000000-0000.png");
    let other = format!("{folder}/paste-notes.png");
    std::fs::write(&old, b"old").unwrap();
    std::fs::write(&other, b"mine").unwrap();

    crate::clipimage::tidy(&transport, &folder, now).await;
    assert!(!std::path::Path::new(&old).exists(), "a paste past a week is cleared");
    assert!(std::path::Path::new(&landed).exists() && std::path::Path::new(&second).exists());
    assert!(std::path::Path::new(&other).exists(), "anything else is left alone");
    transport.close().await;
}

/// The whole of the persistent-shell promise, against a real sshd and a real
/// tmux: a shell started for a tab survives its connection going, the next
/// connection for that tab lands in the same shell — its variables, its
/// directory — and ending the tab ends it.
#[tokio::test]
#[ignore = "requires the sshd fixture and tmux; run scripts/live-native-test.sh"]
async fn a_persistent_shell_outlives_its_connection_and_ends_with_its_tab() {
    let (target, root) = from_env();
    let name = crate::persist::session_name(&format!("live{}", std::process::id()));
    let command = crate::persist::attach_command(&name);
    async fn exists(connection: &Connection, name: &str) -> bool {
        let check = format!("tmux has-session -t ={name} 2>/dev/null && echo yes || echo no");
        connection.run(&check).await.expect("has-session").stdout.trim() == "yes"
    }
    let capture = || {
        let seen = Arc::new(StdMutex::new(String::new()));
        let into = Arc::clone(&seen);
        (seen, move |data: &[u8]| into.lock().unwrap().push_str(&String::from_utf8_lossy(data)))
    };

    // The first connection makes the session and leaves something in it.
    let first = Connection::open(&target, &quiet(), &no_store(), false).await.expect("connect");
    let (_, output) = capture();
    let shell = first
        .open_terminal(Some(command.clone()), output, |_| {}, 100, 30, Arc::new(crate::flow::FlowGate::new()))
        .await
        .expect("attach");
    tokio::time::sleep(Duration::from_millis(1500)).await;
    shell.write(&format!("cd {root} && export AFTERM_MARK=still-here\n")).expect("write");
    tokio::time::sleep(Duration::from_millis(500)).await;

    // The connection goes the way a dropped one does, with nobody ending tmux.
    first.close().await;

    let second = Connection::open(&target, &quiet(), &no_store(), false).await.expect("reconnect");
    assert!(exists(&second, &name).await, "the session outlived the connection");
    let (seen, output) = capture();
    let shell = second
        .open_terminal(Some(command.clone()), output, |_| {}, 100, 30, Arc::new(crate::flow::FlowGate::new()))
        .await
        .expect("reattach");
    tokio::time::sleep(Duration::from_millis(1000)).await;
    shell.write("echo \"mark=$AFTERM_MARK in=$(pwd)\"\n").expect("write");
    let expected = format!("mark=still-here in={root}");
    let arrived = wait_for(|| seen.lock().unwrap().contains(&expected).then_some(())).await;
    assert!(arrived.is_some(), "the same shell, not a new one: {:?}", seen.lock().unwrap());

    // Closing the tab is what ends it.
    second.run(&crate::persist::end_command(&name)).await.expect("end");
    assert!(!exists(&second, &name).await, "the session ended with its tab");
    second.close().await;
}

/// An RSA key, against a server that refuses the legacy signature.
///
/// russh signs an RSA key with SHA-1 unless told otherwise, and OpenSSH has
/// refused `ssh-rsa` by default since 8.8. Left alone, every RSA key user would
/// be turned away by a current server and handed a password prompt — on a host
/// where `ssh.exe`, which negotiates `rsa-sha2-*`, works perfectly. Nothing in
/// the in-process battery notices, because its server accepts whatever it is
/// given.
#[tokio::test]
#[ignore = "requires the sshd fixture; run scripts/live-native-test.sh"]
async fn an_rsa_key_authenticates_against_a_server_that_refuses_sha1() {
    let (mut target, _) = from_env();
    target.host = "afterm-live-rsa".into();

    let connection = Connection::open(&target, &quiet(), &no_store(), false)
        .await
        .expect("an RSA key should authenticate against a current OpenSSH");
    assert!(connection.method.contains("id_rsa"), "method: {}", connection.method);
    connection.close().await;
}

/// The refusal that matters, against a real server: a host whose key is not the
/// one recorded gets no credential.
#[tokio::test]
#[ignore = "requires the sshd fixture; run scripts/live-native-test.sh"]
async fn a_real_server_with_an_unrecorded_key_is_refused() {
    let (target, _) = from_env();
    let known = std::path::PathBuf::from(
        std::env::var("AFTERM_LIVE_WRONG_KNOWN_HOSTS").expect("AFTERM_LIVE_WRONG_KNOWN_HOSTS"),
    );

    let trusted = trust::lookup("127.0.0.1", target.port, &[known]).expect("lookup");
    assert!(!trusted.is_empty(), "the harness recorded a different key for this host");

    // The verifier is the whole defence; drive it exactly as `open` does.
    let config = Arc::new(client::Config {
        preferred: russh::Preferred {
            key: Cow::Owned(trusted.algorithms.clone()),
            ..Default::default()
        },
        ..Default::default()
    });
    let offered = Arc::new(StdMutex::new(None));
    let attempt = client::connect(
        config,
        ("127.0.0.1", target.port),
        Verifier { trusted, offered: Arc::clone(&offered) },
    )
    .await;

    assert!(attempt.is_err(), "a key that is not the recorded one must abort");
    assert!(offered.lock().unwrap().is_some(), "the verifier should have seen the key");
}
