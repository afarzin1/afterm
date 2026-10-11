//! Third part of the spike: can the whole security battery run with no external
//! server at all?
//!
//! This matters because the Azure agent is a plain hosted Windows box with no
//! OpenSSH server and no `sftp-server` binary. If the tests need one, they only
//! ever run on a developer's Linux machine, which is exactly where a Windows
//! security regression would hide.
//!
//! Everything here is in-process: an SSH server on loopback with a host key the
//! test controls, an SFTP subsystem served by russh-sftp, and the real client
//! driven against it. That is a plain `cargo test` on any platform.

use russh::keys::ssh_key::PublicKey;
use russh::keys::PrivateKey;
use russh::server::{self, Auth, Msg, Server as _, Session};
use russh::{client, Channel, ChannelId, MethodKind, MethodSet};
use russh_sftp::protocol::{File, FileAttributes, Handle, Name, Status, StatusCode, Version};
use std::collections::HashMap;
use std::sync::Arc;

const USER: &str = "arya";
const PASSWORD: &str = "hunter2";

/// How many times the server was asked to verify a credential. The point of
/// host key verification is that a host failing it never receives one.
static AUTH_ATTEMPTS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

fn auth_attempts() -> usize {
    AUTH_ATTEMPTS.load(std::sync::atomic::Ordering::SeqCst)
}

// ------------------------------------------------------- in-process SSH server

#[derive(Clone)]
struct Server;

impl server::Server for Server {
    type Handler = Handler_;
    fn new_client(&mut self, _: Option<std::net::SocketAddr>) -> Handler_ {
        Handler_::default()
    }
}

/// The subsystem request arrives after the channel is open, and the session
/// does not hand channels back out, so the handler keeps them.
#[derive(Default)]
struct Handler_ {
    channels: HashMap<ChannelId, Channel<Msg>>,
}

impl server::Handler for Handler_ {
    type Error = russh::Error;

    async fn auth_none(&mut self, _: &str) -> Result<Auth, Self::Error> {
        Ok(Auth::Reject {
            proceed_with_methods: Some(MethodSet::from(&[MethodKind::Password][..])),
            partial_success: false,
        })
    }

    async fn auth_password(&mut self, user: &str, password: &str) -> Result<Auth, Self::Error> {
        AUTH_ATTEMPTS.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        if user == USER && password == PASSWORD {
            Ok(Auth::Accept)
        } else {
            Ok(Auth::Reject { proceed_with_methods: None, partial_success: false })
        }
    }

    async fn channel_open_session(
        &mut self,
        channel: Channel<Msg>,
        reply: server::ChannelOpenHandle,
        _: &mut Session,
    ) -> Result<(), Self::Error> {
        reply.accept().await;
        self.channels.insert(channel.id(), channel);
        Ok(())
    }

    /// Serve the SFTP subsystem from inside the test process.
    async fn subsystem_request(
        &mut self,
        channel_id: ChannelId,
        name: &str,
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        if name != "sftp" {
            session.channel_failure(channel_id)?;
            return Ok(());
        }
        let channel = self.channels.remove(&channel_id).ok_or(russh::Error::WrongChannel)?;
        session.channel_success(channel_id)?;
        tokio::spawn(async move {
            russh_sftp::server::run(channel.into_stream(), Sftp::default()).await;
        });
        Ok(())
    }
}

// ------------------------------------------------------ in-process SFTP server

#[derive(Default)]
struct Sftp {
    /// Directory handles already drained, so readdir terminates.
    drained: HashMap<String, bool>,
}

impl russh_sftp::server::Handler for Sftp {
    type Error = StatusCode;

    fn unimplemented(&self) -> Self::Error {
        StatusCode::OpUnsupported
    }

    async fn init(&mut self, version: u32, _: HashMap<String, String>) -> Result<Version, Self::Error> {
        let _ = version;
        Ok(Version::new())
    }

    async fn realpath(&mut self, id: u32, path: String) -> Result<Name, Self::Error> {
        Ok(Name {
            id,
            files: vec![File::dummy(if path == "." { "/home/arya" } else { &path })],
        })
    }

    async fn opendir(&mut self, id: u32, path: String) -> Result<Handle, Self::Error> {
        self.drained.insert(path.clone(), false);
        Ok(Handle { id, handle: path })
    }

    async fn readdir(&mut self, id: u32, handle: String) -> Result<Name, Self::Error> {
        // Second call must report EOF or the client loops forever.
        if self.drained.get(&handle).copied().unwrap_or(true) {
            return Err(StatusCode::Eof);
        }
        self.drained.insert(handle, true);
        let mut directory = FileAttributes::default();
        directory.set_dir(true);
        let mut regular = FileAttributes::default();
        regular.set_regular(true);
        regular.size = Some(6);
        Ok(Name {
            id,
            files: vec![
                File { filename: "afile.txt".into(), longname: String::new(), attrs: regular },
                File { filename: "file with space.txt".into(), longname: String::new(), attrs: FileAttributes::default() },
                File { filename: "sub".into(), longname: String::new(), attrs: directory },
            ],
        })
    }

    async fn close(&mut self, id: u32, _: String) -> Result<Status, Self::Error> {
        Ok(Status { id, status_code: StatusCode::Ok, error_message: "Ok".into(), language_tag: "en-US".into() })
    }
}

// ------------------------------------------------------------------ the client

struct Client {
    /// The key the test pretends is recorded in known_hosts, if any.
    expected: Option<PublicKey>,
    /// Records what the check decided.
    saw: Arc<std::sync::Mutex<Option<String>>>,
}

impl client::Handler for Client {
    type Error = russh::Error;

    async fn check_server_key(
        &mut self,
        key: &russh::keys::PublicKeyOrCertificate,
    ) -> Result<bool, Self::Error> {
        let offered = match key {
            russh::keys::PublicKeyOrCertificate::PublicKey { key, .. } => key.clone(),
            russh::keys::PublicKeyOrCertificate::Certificate(c) => c.public_key().clone().into(),
        };
        let (verdict, accept) = match &self.expected {
            None => ("unknown".to_string(), false),
            Some(expected) if *expected == offered => ("known".to_string(), true),
            Some(_) => ("changed".to_string(), false),
        };
        *self.saw.lock().unwrap() = Some(verdict);
        Ok(accept)
    }
}

async fn start_server() -> Result<(u16, PublicKey), Box<dyn std::error::Error>> {
    let key = PrivateKey::random(&mut rand::rng(), russh::keys::Algorithm::Ed25519)?;
    let public = key.public_key().clone();
    let config = Arc::new(server::Config { keys: vec![key], ..Default::default() });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let port = listener.local_addr()?.port();
    tokio::spawn(async move {
        let _ = Server.run_on_socket(config, &listener).await;
    });
    tokio::time::sleep(std::time::Duration::from_millis(150)).await;
    Ok((port, public))
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let (port, host_key) = start_server().await?;
    let config = Arc::new(client::Config::default());
    println!("in-process SSH+SFTP server on 127.0.0.1:{port}\n");

    println!("== 1. matching host key -> allowed, password auth, SFTP over it");
    let saw = Arc::new(std::sync::Mutex::new(None));
    let mut session = client::connect(
        Arc::clone(&config),
        ("127.0.0.1", port),
        Client { expected: Some(host_key.clone()), saw: Arc::clone(&saw) },
    ).await?;
    println!("   host key verdict: {:?}", saw.lock().unwrap().clone());
    assert!(matches!(session.authenticate_password(USER, PASSWORD).await?, client::AuthResult::Success));

    let channel = session.channel_open_session().await?;
    channel.request_subsystem(true, "sftp").await?;
    let sftp = russh_sftp::client::SftpSession::new(channel.into_stream()).await?;
    println!("   realpath(.) -> {}", sftp.canonicalize(".").await?);
    let mut names: Vec<String> = sftp.read_dir("/srv").await?.map(|e| e.file_name()).collect();
    names.sort();
    println!("   read_dir(/srv) -> {names:?}");
    assert_eq!(names, vec!["afile.txt", "file with space.txt", "sub"]);

    let attempts_before = auth_attempts();
    println!("   credential checks so far: {attempts_before}");

    println!("== 2. CHANGED host key -> refused, and no password is ever sent");
    let other = PrivateKey::random(&mut rand::rng(), russh::keys::Algorithm::Ed25519)?
        .public_key().clone();
    let saw = Arc::new(std::sync::Mutex::new(None));
    let attempt = client::connect(
        Arc::clone(&config),
        ("127.0.0.1", port),
        Client { expected: Some(other), saw: Arc::clone(&saw) },
    ).await;
    println!("   host key verdict: {:?}", saw.lock().unwrap().clone());
    assert_eq!(saw.lock().unwrap().clone(), Some("changed".into()));
    assert!(attempt.is_err(), "a changed host key must abort the connection");
    println!("   refused: {}", attempt.err().unwrap());
    assert_eq!(
        auth_attempts(), attempts_before,
        "the server must not have been asked to check any credential"
    );
    println!("   credential checks during refusal: 0  <-- the property that matters");

    println!("== 3. UNKNOWN host key -> refused");
    let saw = Arc::new(std::sync::Mutex::new(None));
    let attempt = client::connect(
        Arc::clone(&config),
        ("127.0.0.1", port),
        Client { expected: None, saw: Arc::clone(&saw) },
    ).await;
    assert_eq!(saw.lock().unwrap().clone(), Some("unknown".into()));
    assert!(attempt.is_err());
    println!("   refused: {}", attempt.err().unwrap());
    assert_eq!(auth_attempts(), attempts_before, "no credential may be offered to an unknown host");
    println!("   credential checks during refusal: 0");

    println!("\nIN-PROCESS BATTERY PASSED (no external sshd, no sftp-server binary)");
    Ok(())
}
