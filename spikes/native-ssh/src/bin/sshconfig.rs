//! Fourth part of the spike: if afterm is a general-purpose terminal, it cannot
//! ignore ~/.ssh/config. This checks whether a parser gets us the directives
//! that matter, against a config shaped like a real one.
//!
//! The danger is not "unsupported". It is "silently half-supported": a client
//! that quietly ignores ProxyJump reaches an internal host directly, and one
//! that ignores UserKnownHostsFile verifies against the wrong file.

use ssh2_config::{ParseRule, SshConfig};
use std::io::BufReader;

const CONFIG: &str = r#"
# A realistic config: aliases, wildcards, a jump host, an include, and
# directives that change security posture rather than convenience.

Host prod
    HostName prod-01.internal
    User deploy
    Port 2222
    IdentityFile ~/.ssh/id_ed25519_prod
    IdentitiesOnly yes

Host *.internal
    User deploy
    ProxyJump bastion
    UserKnownHostsFile ~/.ssh/known_hosts_work
    StrictHostKeyChecking yes

Host bastion
    HostName 10.0.0.5
    User arya

Host legacy
    HostName 10.0.0.9
    PreferredAuthentications password
    HostKeyAlgorithms +ssh-rsa

Host *
    ServerAliveInterval 30
    ServerAliveCountMax 3
"#;

fn main() {
    let config = SshConfig::default()
        .parse(&mut BufReader::new(CONFIG.as_bytes()), ParseRule::ALLOW_UNKNOWN_FIELDS)
        .expect("parse");

    for target in ["prod", "bastion", "db-01.internal", "legacy", "8.8.8.8"] {
        let params = config.query(target);
        println!("query({target:?})");
        println!("   host_name  : {:?}", params.host_name);
        println!("   user       : {:?}", params.user);
        println!("   port       : {:?}", params.port);
        println!("   identity   : {:?}", params.identity_file);
        println!("   keep-alive : interval {:?} count {:?}", params.server_alive_interval, params.connect_timeout);
        println!("   proxy_jump : {:?}", params.proxy_jump);
        println!("   host_key_algorithms: {:?}", params.host_key_algorithms);
        // Anything the parser did not recognise at all.
        println!("   unknown fields: {:?}", params.ignored_fields.keys().collect::<Vec<_>>());
        println!();
    }

    // The check that decides the design: does the parser tell us a host is
    // behind a jump box? If afterm cannot see this, it connects directly.
    let internal = config.query("db-01.internal");
    println!("--- the decisive question ---");
    println!("db-01.internal resolves to host_name {:?}, user {:?}", internal.host_name, internal.user);
    println!("ProxyJump           -> {:?}   (a typed field: afterm CAN see this)", internal.proxy_jump);
    // There is no HostParams field for UserKnownHostsFile or
    // StrictHostKeyChecking. The parser recognises them, so they are not
    // reported as unknown either: they are simply gone.
    println!("UserKnownHostsFile  -> not exposed by HostParams; unknown-fields says {:?}", internal.ignored_fields.get("UserKnownHostsFile"));
    println!("StrictHostKeyChecking -> not exposed either;      unknown-fields says {:?}", internal.ignored_fields.get("StrictHostKeyChecking"));
    println!("PreferredAuthentications (legacy host) -> {:?}", config.query("legacy").ignored_fields.get("PreferredAuthentications"));
}
