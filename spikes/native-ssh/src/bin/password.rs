//! Second half of the spike: does password authentication actually work?
//!
//! This is the whole motivation for going native, and it could not be tested
//! against the throwaway OpenSSH server, because verifying a password needs
//! root. So this stands up a minimal russh *server* that accepts a known
//! password, and drives the real russh client against it over a real SSH wire.
//!
//! It also exercises keyboard-interactive, which is what a host with 2FA or a
//! PAM-driven prompt will actually offer.

use russh::client::{self, AuthResult};
use russh::keys::PrivateKey;
use russh::server::{self, Auth, Msg, Server as _, Session};
use russh::{Channel, ChannelId, ChannelMsg, MethodKind, MethodSet};
use std::sync::Arc;

const USER: &str = "arya";
const PASSWORD: &str = "correct horse battery staple";

// ----------------------------------------------------------------- server

#[derive(Clone)]
struct TestServer;

impl server::Server for TestServer {
    type Handler = TestHandler;
    fn new_client(&mut self, _: Option<std::net::SocketAddr>) -> TestHandler {
        TestHandler
    }
}

struct TestHandler;

impl server::Handler for TestHandler {
    type Error = russh::Error;

    /// Refuse `none` but advertise password and keyboard-interactive, which is
    /// how a password-only host behaves.
    async fn auth_none(&mut self, _: &str) -> Result<Auth, Self::Error> {
        Ok(Auth::Reject {
            proceed_with_methods: Some(MethodSet::from(
                &[MethodKind::Password, MethodKind::KeyboardInteractive][..],
            )),
            partial_success: false,
        })
    }

    async fn auth_password(&mut self, user: &str, password: &str) -> Result<Auth, Self::Error> {
        if user == USER && password == PASSWORD {
            Ok(Auth::Accept)
        } else {
            Ok(Auth::Reject { proceed_with_methods: None, partial_success: false })
        }
    }

    async fn auth_keyboard_interactive<'a>(
        &'a mut self,
        user: &str,
        _: &str,
        response: Option<server::Response<'a>>,
    ) -> Result<Auth, Self::Error> {
        // First call has no response: issue the prompt.
        let Some(mut response) = response else {
            return Ok(Auth::Partial {
                name: "Password".into(),
                instructions: "".into(),
                prompts: vec![("Password: ".into(), false)].into(),
            });
        };
        let answer = response.next().map(|value| String::from_utf8_lossy(&value).to_string());
        if user == USER && answer.as_deref() == Some(PASSWORD) {
            Ok(Auth::Accept)
        } else {
            Ok(Auth::Reject { proceed_with_methods: None, partial_success: false })
        }
    }

    async fn channel_open_session(
        &mut self,
        _: Channel<Msg>,
        reply: server::ChannelOpenHandle,
        _: &mut Session,
    ) -> Result<(), Self::Error> {
        // 0.63 hands the handler an explicit accept/reject handle; returning
        // Ok without accepting leaves the client with
        // ChannelOpenFailure(AdministrativelyProhibited).
        reply.accept().await;
        Ok(())
    }

    async fn exec_request(
        &mut self,
        channel: ChannelId,
        data: &[u8],
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        let command = String::from_utf8_lossy(data).to_string();
        let reply = russh::keys::ssh_encoding::bytes::Bytes::from(
            format!("ran: {command}\n").into_bytes(),
        );
        session.data(channel, reply)?;
        session.channel_success(channel)?;
        session.eof(channel)?;
        Ok(())
    }
}

// ----------------------------------------------------------------- client

struct TestClient;

impl client::Handler for TestClient {
    type Error = russh::Error;
    async fn check_server_key(
        &mut self,
        _: &russh::keys::PublicKeyOrCertificate,
    ) -> Result<bool, Self::Error> {
        // The real client checks known_hosts; that is covered by the other
        // half of the spike. Here the server key is ephemeral.
        Ok(true)
    }
}

async fn exec(session: &client::Handle<TestClient>, command: &str) -> Result<String, Box<dyn std::error::Error>> {
    let mut channel = session.channel_open_session().await?;
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

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let key = PrivateKey::random(&mut rand::rng(), russh::keys::Algorithm::Ed25519)?;
    let config = Arc::new(server::Config {
        keys: vec![key],
        ..Default::default()
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let port = listener.local_addr()?.port();
    tokio::spawn(async move {
        let _ = TestServer.run_on_socket(config, &listener).await;
    });
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    println!("test server listening on 127.0.0.1:{port}\n");

    let client_config = Arc::new(client::Config::default());

    println!("== A. password auth, correct password");
    let mut session = client::connect(Arc::clone(&client_config), ("127.0.0.1", port), TestClient).await?;
    let result = session.authenticate_password(USER, PASSWORD).await?;
    println!("   {result:?}");
    assert!(matches!(result, AuthResult::Success));
    println!("   exec over it -> {:?}", exec(&session, "uptime").await?);

    println!("== B. password auth, WRONG password");
    let mut session = client::connect(Arc::clone(&client_config), ("127.0.0.1", port), TestClient).await?;
    let result = session.authenticate_password(USER, "wrong").await?;
    println!("   {result:?}");
    assert!(!matches!(result, AuthResult::Success));

    println!("== C. keyboard-interactive (what a PAM or 2FA host offers)");
    let mut session = client::connect(Arc::clone(&client_config), ("127.0.0.1", port), TestClient).await?;
    let started = session.authenticate_keyboard_interactive_start(USER, None).await?;
    match started {
        client::KeyboardInteractiveAuthResponse::InfoRequest { prompts, .. } => {
            println!("   server prompted: {:?}", prompts.iter().map(|p| &p.prompt).collect::<Vec<_>>());
            let next = session
                .authenticate_keyboard_interactive_respond(vec![PASSWORD.to_string()])
                .await?;
            println!("   {next:?}");
            assert!(matches!(next, client::KeyboardInteractiveAuthResponse::Success));
        }
        other => panic!("expected a prompt, got {other:?}"),
    }

    println!("== D. what methods does the server advertise after `none`?");
    let mut session = client::connect(Arc::clone(&client_config), ("127.0.0.1", port), TestClient).await?;
    let result = session.authenticate_none(USER).await?;
    if let AuthResult::Failure { remaining_methods, .. } = &result {
        println!("   remaining_methods -> {remaining_methods:?}");
        println!("   (this is how afterm knows whether to show a password box)");
    }

    println!("\nPASSWORD SPIKE CHECKS PASSED");
    Ok(())
}
