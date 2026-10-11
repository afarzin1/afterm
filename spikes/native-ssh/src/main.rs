//! Spike: can afterm speak SSH natively, the way MobaXterm does?
//!
//! One authenticated connection, then every feature afterm needs as a separate
//! channel on it: an exec channel for telemetry, an SFTP subsystem for files,
//! and a PTY channel for the terminal. Plus the part that has to be right —
//! host key verification against known_hosts, including refusing a changed key.
//!
//! Nothing here is integrated into afterm. It exists to find out whether the
//! rewrite is worth commissioning.

use russh::client::{self, AuthResult, Handle, Handler, Msg};
use russh::keys::{known_hosts, PrivateKeyWithHashAlg};
use russh::{Channel, ChannelMsg, Disconnect};
use russh_sftp::client::SftpSession;
use std::path::PathBuf;
use std::sync::Arc;

/// What the host key check decided, so the spike can report it.
#[derive(Debug, PartialEq)]
enum HostKeyVerdict {
    Known,
    Unknown,
    Changed { line: usize },
}

struct Client {
    known_hosts: PathBuf,
    host: String,
    port: u16,
    /// Set by check_server_key so the caller can see what happened.
    verdict: Arc<std::sync::Mutex<Option<HostKeyVerdict>>>,
    /// When false, an unknown or changed key aborts the connection.
    trust_anything: bool,
}

impl Handler for Client {
    type Error = russh::Error;

    /// The hook that makes or breaks the security of a native client. russh's
    /// default implementation returns Ok(false) — it rejects every key — so a
    /// client that forgets to override this connects to nothing. Overriding it
    /// wrongly is the silent failure; forgetting it is a loud one.
    async fn check_server_key(
        &mut self,
        key: &russh::keys::PublicKeyOrCertificate,
    ) -> Result<bool, Self::Error> {
        let public = match key {
            russh::keys::PublicKeyOrCertificate::PublicKey { key, .. } => key.clone(),
            russh::keys::PublicKeyOrCertificate::Certificate(certificate) => {
                certificate.public_key().clone().into()
            }
        };
        let verdict = match known_hosts::check_known_hosts_path(
            &self.host,
            self.port,
            &public,
            &self.known_hosts,
        ) {
            Ok(true) => HostKeyVerdict::Known,
            Ok(false) => HostKeyVerdict::Unknown,
            Err(russh::keys::Error::KeyChanged { line }) => HostKeyVerdict::Changed { line },
            Err(error) => {
                println!("  known_hosts read error: {error}");
                HostKeyVerdict::Unknown
            }
        };
        let accept = match &verdict {
            HostKeyVerdict::Known => true,
            // A real client prompts here on first sight, and never on a change.
            _ => self.trust_anything,
        };
        *self.verdict.lock().unwrap() = Some(verdict);
        Ok(accept)
    }
}

async fn connect(
    host: &str,
    port: u16,
    known_hosts: PathBuf,
    trust_anything: bool,
) -> Result<(Handle<Client>, Option<HostKeyVerdict>), Box<dyn std::error::Error>> {
    let verdict = Arc::new(std::sync::Mutex::new(None));
    let handler = Client {
        known_hosts,
        host: host.to_string(),
        port,
        verdict: Arc::clone(&verdict),
        trust_anything,
    };
    let config = Arc::new(client::Config::default());
    let handle = client::connect(config, (host, port), handler).await?;
    let seen = verdict.lock().unwrap().take();
    Ok((handle, seen))
}

/// Run a command on its own exec channel — the telemetry path.
async fn exec(session: &Handle<Client>, command: &str) -> Result<String, Box<dyn std::error::Error>> {
    let mut channel: Channel<Msg> = session.channel_open_session().await?;
    channel.exec(true, command).await?;
    let mut output = Vec::new();
    while let Some(message) = channel.wait().await {
        match message {
            ChannelMsg::Data { ref data } => output.extend_from_slice(data),
            ChannelMsg::Eof | ChannelMsg::ExitStatus { .. } => break,
            _ => {}
        }
    }
    Ok(String::from_utf8_lossy(&output).trim().to_string())
}

/// Open an interactive shell on a PTY channel — the terminal path.
async fn pty_shell(session: &Handle<Client>) -> Result<String, Box<dyn std::error::Error>> {
    let mut channel: Channel<Msg> = session.channel_open_session().await?;
    channel
        .request_pty(true, "xterm-256color", 80, 24, 0, 0, &[])
        .await?;
    channel.request_shell(true).await?;
    channel.data(&b"echo pty-channel-works\nexit\n"[..]).await?;

    let mut output = Vec::new();
    while let Some(message) = channel.wait().await {
        match message {
            ChannelMsg::Data { ref data } => output.extend_from_slice(data),
            ChannelMsg::Eof | ChannelMsg::ExitStatus { .. } => break,
            _ => {}
        }
    }
    Ok(String::from_utf8_lossy(&output).replace('\r', ""))
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let host = std::env::var("SPIKE_HOST").unwrap_or_else(|_| "127.0.0.1".into());
    let port: u16 = std::env::var("SPIKE_PORT").unwrap_or_else(|_| "2222".into()).parse()?;
    let user = std::env::var("SPIKE_USER")?;
    let key_path = std::env::var("SPIKE_KEY")?;
    let known = PathBuf::from(std::env::var("SPIKE_KNOWN_HOSTS")?);
    let bad_known = PathBuf::from(std::env::var("SPIKE_BAD_KNOWN_HOSTS")?);
    let dir = std::env::var("SPIKE_DIR")?;

    println!("== 1. host key: a known_hosts entry that MATCHES");
    let (mut session, verdict) = connect(&host, port, known.clone(), false).await?;
    println!("   verdict: {verdict:?}  (connection allowed)");
    assert_eq!(verdict, Some(HostKeyVerdict::Known));

    println!("== 2. auth: public key");
    let key = russh::keys::load_secret_key(&key_path, None)?;
    let result = session
        .authenticate_publickey(
            &user,
            PrivateKeyWithHashAlg::new(Arc::new(key), None),
        )
        .await?;
    println!("   {result:?}");
    assert!(matches!(result, AuthResult::Success));

    println!("== 3. exec channel (telemetry path)");
    let uptime = exec(&session, "head -1 /proc/uptime").await?;
    println!("   /proc/uptime -> {uptime}");
    assert!(!uptime.is_empty());
    let mem = exec(&session, "grep -E '^MemTotal:' /proc/meminfo").await?;
    println!("   /proc/meminfo -> {mem}");

    println!("== 4. SFTP subsystem on the SAME connection (files path)");
    let channel = session.channel_open_session().await?;
    channel.request_subsystem(true, "sftp").await?;
    let sftp = SftpSession::new(channel.into_stream()).await?;
    println!("   remote cwd -> {}", sftp.canonicalize(".").await?);
    let mut names: Vec<String> = Vec::new();
    for entry in sftp.read_dir(&dir).await? {
        names.push(format!("{}{}", entry.file_name(), if entry.metadata().is_dir() { "/" } else { "" }));
    }
    names.sort();
    println!("   read_dir({dir}) -> {names:?}");
    assert!(names.iter().any(|n| n.starts_with("afile.txt")));

    println!("== 5. PTY channel on the SAME connection (terminal path)");
    let shell = pty_shell(&session).await?;
    println!("   shell said: {:?}", shell.lines().find(|l| l.contains("pty-channel-works")).unwrap_or("<not found>"));
    assert!(shell.contains("pty-channel-works"));

    session.disconnect(Disconnect::ByApplication, "", "English").await?;

    println!("== 6. host key: an entry that does NOT match (rotated key)");
    match connect(&host, port, bad_known.clone(), false).await {
        Ok((_, verdict)) => println!("   !! CONNECTED ANYWAY, verdict {verdict:?} — this would be the bug"),
        Err(error) => {
            // The connection must fail. Report what the check decided first.
            let (_, verdict) = connect(&host, port, bad_known, true).await?;
            println!("   verdict: {verdict:?}");
            println!("   refused with: {error}");
            assert!(matches!(verdict, Some(HostKeyVerdict::Changed { .. })));
        }
    }

    println!("== 7. host key: host absent from known_hosts");
    let empty = std::env::temp_dir().join(format!("spike-empty-{}", std::process::id()));
    std::fs::write(&empty, "")?;
    match connect(&host, port, empty.clone(), false).await {
        Ok((_, verdict)) => println!("   !! CONNECTED ANYWAY, verdict {verdict:?} — this would be the bug"),
        Err(error) => {
            let (_, verdict) = connect(&host, port, empty, true).await?;
            println!("   verdict: {verdict:?}");
            println!("   refused with: {error}");
            assert!(matches!(verdict, Some(HostKeyVerdict::Unknown)));
        }
    }

    println!("\nALL SPIKE CHECKS PASSED");
    Ok(())
}
