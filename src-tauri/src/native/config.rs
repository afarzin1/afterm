//! What the user's `ssh_config` actually says, according to the user's own
//! client.
//!
//! afterm does not parse `~/.ssh/config`. `ssh -G` resolves it — aliases,
//! wildcards, `Include`, `Match`, the system file, the lot — and prints the
//! effective settings as `key value` lines before exiting. afterm reads the
//! answer rather than reimplementing the question.
//!
//! This is the difference between "unsupported" and "silently half-supported",
//! which is the failure mode that matters: a client that quietly ignores
//! `ProxyJump` reaches an internal host directly, and one that ignores
//! `UserKnownHostsFile` verifies against the wrong file. Anything the native
//! transport cannot honour faithfully sends the host back to the OpenSSH client
//! with a named reason.
//!
//! Note what this is not: it is a run-to-completion capture of a
//! non-interactive, machine-readable output. Nothing here waits on a prompt,
//! matches an echo, or depends on flush behaviour — the failure modes that broke
//! the SFTP client four times on Windows.

use crate::transport::Failure;
use crate::winproc::hide_console;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// The effective configuration for one host, reduced to what afterm uses.
#[derive(Debug, Clone)]
pub struct Effective {
    pub hostname: String,
    pub user: String,
    pub port: u16,
    pub identity_files: Vec<PathBuf>,
    /// `UserKnownHostsFile` and `GlobalKnownHostsFile`, in the order OpenSSH
    /// would search them.
    pub known_hosts: Vec<PathBuf>,
    /// `StrictHostKeyChecking`, verbatim, so a difference can be named.
    pub strict_host_key_checking: String,
    /// `PreferredAuthentications`, in order.
    pub preferred_authentications: Vec<String>,
    /// `IdentitiesOnly`: the agent may sign only for a key the configuration
    /// names, however many more it holds.
    pub identities_only: bool,
    /// `IdentityAgent`: where the agent is, or that there is not to be one.
    pub identity_agent: Agent,
    /// `PubkeyAuthentication`, `PasswordAuthentication` and
    /// `KbdInteractiveAuthentication`: a switch on each method.
    pub pubkey_authentication: bool,
    pub password_authentication: bool,
    pub kbd_interactive_authentication: bool,
    /// `BatchMode`: never ask the user anything.
    pub batch_mode: bool,
    /// The cryptographic policy, as the client resolved it. Every list is
    /// already the effective one — `ssh -G` applies `+`, `-` and `^` before
    /// printing — so these are the algorithms the user's own client would
    /// negotiate with, in the order it would prefer them.
    pub crypto: Crypto,
}

/// `Ciphers`, `MACs`, `KexAlgorithms`, `HostKeyAlgorithms` and
/// `PubkeyAcceptedAlgorithms`, verbatim and in order.
///
/// These used to be read and deliberately ignored, on the reasoning that russh
/// negotiates from a modern set and the outcome would differ only in which
/// strong algorithm was picked. That reasoning was wrong in one direction that
/// matters: a configuration that *narrows* the set is a policy, and honouring
/// a host key while negotiating an algorithm the policy forbids is still a
/// silent difference from what the user's client would have done. So the
/// policy is applied, and a policy afterm cannot satisfy sends the host to the
/// OpenSSH client by name rather than being quietly widened.
///
/// Empty means the directive did not appear, which `ssh -G` does only for a
/// client that does not print it at all; every current OpenSSH prints all five
/// whether or not the user set them.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Crypto {
    pub ciphers: Vec<String>,
    pub macs: Vec<String>,
    pub kex_algorithms: Vec<String>,
    pub host_key_algorithms: Vec<String>,
    pub pubkey_accepted_algorithms: Vec<String>,
}

impl Crypto {
    /// Whether a public-key signature algorithm may be offered.
    ///
    /// An empty list is the directive being absent, not a refusal of
    /// everything: a client too old to print it has no opinion to honour.
    pub fn allows_signature(&self, algorithm: &str) -> bool {
        self.pubkey_accepted_algorithms.is_empty()
            || self
                .pubkey_accepted_algorithms
                .iter()
                .any(|allowed| allowed == algorithm)
    }
}

/// What OpenSSH would do with no configuration at all, which is what a test
/// that builds an `Effective` by hand should start from.
impl Default for Effective {
    fn default() -> Self {
        Self {
            hostname: String::new(),
            user: String::new(),
            port: 22,
            identity_files: Vec::new(),
            known_hosts: Vec::new(),
            strict_host_key_checking: String::new(),
            preferred_authentications: Vec::new(),
            identities_only: false,
            identity_agent: Agent::Default,
            pubkey_authentication: true,
            password_authentication: true,
            kbd_interactive_authentication: true,
            batch_mode: false,
            crypto: Crypto::default(),
        }
    }
}

/// Where `IdentityAgent` says the agent is.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum Agent {
    /// Unset, or the literal `SSH_AUTH_SOCK`: the platform's usual agent.
    #[default]
    Default,
    /// `none`: do not consult one.
    None,
    /// A socket path on Unix, a named pipe on Windows.
    At(PathBuf),
}

impl Effective {
    /// Whether the configuration lets afterm try `method`: the method's own
    /// switch is on, and `PreferredAuthentications`, if set, lists it.
    pub fn allows(&self, method: &str) -> bool {
        let switched_on = match method {
            "publickey" => self.pubkey_authentication,
            "password" => self.password_authentication,
            "keyboard-interactive" => self.kbd_interactive_authentication,
            _ => true,
        };
        switched_on
            && (self.preferred_authentications.is_empty()
                || self.preferred_authentications.iter().any(|m| m == method))
    }
}

/// How a directive's appearance in `ssh -G` output is judged.
enum Rule {
    /// Any value refuses. The client prints these only when they are set.
    IfPresent,
    /// Refuse unless the value is one of these, which are what the client
    /// prints for a directive left at its default.
    Unless(&'static [&'static str]),
}

/// Directives the native transport cannot honour. Each one changes where the
/// connection goes, what it trusts, how it authenticates or what it carries,
/// so ignoring it would be a silent difference rather than a missing
/// convenience. A host that sets one stays on the OpenSSH client, which
/// honours all of them, and the refusal names the directive.
///
/// Every effective directive falls into one of three buckets, and this table
/// is the middle one. The first is what afterm implements: `HostName`, `User`,
/// `Port`, `IdentityFile`, `IdentitiesOnly`, `IdentityAgent`, the known-hosts
/// files, `StrictHostKeyChecking` (verify-only, named when it differs),
/// `PreferredAuthentications`, the three method switches, `BatchMode` and the
/// cryptographic policy — `Ciphers`, `MACs`, `KexAlgorithms`,
/// `HostKeyAlgorithms` and `PubkeyAcceptedAlgorithms`, intersected with what
/// russh can negotiate and refused by name when the intersection is empty.
///
/// The third is what is read and deliberately not acted on, recorded here so
/// nobody re-derives it. `ConnectTimeout` and `ServerAlive*`, because afterm
/// has its own. `Compression`.
/// `ControlMaster` and `ControlPath`, which Win32-OpenSSH does not implement
/// either. `HashKnownHosts`, `UpdateHostKeys` and `CheckHostIP`, because
/// afterm never writes `known_hosts`. `GSSAPIAuthentication`, which is on by
/// default on some distributions and not implemented here: a host that accepts
/// nothing else fails with its methods named, rather than every host on such a
/// machine being sent to the client.
const REFUSED: &[(&str, &str, Rule)] = &[
    // Routing: the connection would go somewhere else, or from somewhere else.
    ("proxyjump", "ProxyJump", Rule::Unless(&["none"])),
    ("proxycommand", "ProxyCommand", Rule::Unless(&["none"])),
    ("proxyusefdpass", "ProxyUseFdpass", Rule::Unless(&["no", "false"])),
    ("bindaddress", "BindAddress", Rule::IfPresent),
    ("bindinterface", "BindInterface", Rule::IfPresent),
    ("addressfamily", "AddressFamily", Rule::Unless(&["any"])),
    // Trust: the host key would be checked against something else.
    ("hostkeyalias", "HostKeyAlias", Rule::IfPresent),
    ("certificatefile", "CertificateFile", Rule::IfPresent),
    ("revokedhostkeys", "RevokedHostKeys", Rule::IfPresent),
    ("knownhostscommand", "KnownHostsCommand", Rule::IfPresent),
    // Authentication afterm does not implement, that the user turned on.
    ("hostbasedauthentication", "HostbasedAuthentication", Rule::Unless(&["no", "false"])),
    ("pkcs11provider", "PKCS11Provider", Rule::Unless(&["none"])),
    ("securitykeyprovider", "SecurityKeyProvider", Rule::Unless(&["internal"])),
    // Forwarding: the session would carry things this one does not.
    ("forwardagent", "ForwardAgent", Rule::Unless(&["no", "false"])),
    ("forwardx11", "ForwardX11", Rule::Unless(&["no", "false"])),
    ("localforward", "LocalForward", Rule::IfPresent),
    ("remoteforward", "RemoteForward", Rule::IfPresent),
    ("dynamicforward", "DynamicForward", Rule::IfPresent),
    ("tunnel", "Tunnel", Rule::Unless(&["no", "false"])),
    // The session itself would be a different shape.
    ("remotecommand", "RemoteCommand", Rule::Unless(&["none"])),
    ("permitlocalcommand", "PermitLocalCommand", Rule::Unless(&["no", "false"])),
    ("sessiontype", "SessionType", Rule::Unless(&["default"])),
    ("requesttty", "RequestTTY", Rule::Unless(&["auto", "yes", "true"])),
    ("stdinnull", "StdinNull", Rule::Unless(&["no", "false"])),
    ("forkafterauthentication", "ForkAfterAuthentication", Rule::Unless(&["no", "false"])),
];

/// Ask the installed OpenSSH client what it would do for this target.
///
/// `config_file` is `None` everywhere in the application: the whole point is
/// that the client reads the user's own configuration. The live test passes a
/// throwaway one, because OpenSSH resolves `~` through the password database
/// rather than `$HOME`, so a test cannot redirect it any other way.
pub fn resolve(
    host: &str,
    user: &str,
    port: u16,
    config_file: Option<&Path>,
) -> Result<Effective, Failure> {
    let text = run_ssh_g(host, user, port, config_file)?;
    parse(&text, host, user, port)
}

fn run_ssh_g(
    host: &str,
    user: &str,
    port: u16,
    config_file: Option<&Path>,
) -> Result<String, Failure> {
    let mut builder = Command::new(if cfg!(windows) { "ssh.exe" } else { "ssh" });
    if let Some(path) = config_file {
        builder.arg("-F").arg(path);
    }
    let output = hide_console(&mut builder)
        .args(["-G", "-p", &port.to_string(), "-l", user, host])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        // Not `Unsupported`: that cause pins the session to the OpenSSH-client
        // transport, and a client that will not run cannot carry it either.
        .map_err(|error| Failure::spawn(if cfg!(windows) { "ssh.exe" } else { "ssh" }, &error))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(Failure::unsupported(format!(
            "ssh -G could not resolve the configuration for this host: {}",
            stderr.trim().lines().next().unwrap_or("no reason given")
        )));
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// `ssh -G` prints one `key value` line per directive, keys already lowercased
/// and values already resolved. Repeated keys accumulate, which is how
/// `IdentityFile` reports more than one.
fn parse(text: &str, host: &str, user: &str, port: u16) -> Result<Effective, Failure> {
    let mut fields: HashMap<&str, Vec<&str>> = HashMap::new();
    for line in text.lines() {
        let line = line.trim();
        let Some((key, value)) = line.split_once(char::is_whitespace) else {
            continue;
        };
        let value = value.trim();
        if value.is_empty() {
            continue;
        }
        fields.entry(key).or_default().push(value);
    }

    let first = |key: &str| -> Option<String> {
        fields.get(key).and_then(|values| values.first()).map(|value| value.to_string())
    };

    for (key, name, rule) in REFUSED {
        let Some(value) = first(key) else { continue };
        let refused = match rule {
            Rule::IfPresent => true,
            Rule::Unless(defaults) => !defaults.iter().any(|ok| value.eq_ignore_ascii_case(ok)),
        };
        if refused {
            return Err(Failure::unsupported(format!(
                "this host's ssh_config sets {name}, which afterm's native \
                 transport cannot honour, so it stays on the OpenSSH client"
            )));
        }
    }

    // `yes`/`no` in the file, but `true`/`false` for some directives in the
    // client's own output; both spellings are read.
    let flag = |key: &str, default: bool| -> bool {
        match first(key) {
            Some(value) => !matches!(value.to_ascii_lowercase().as_str(), "no" | "false"),
            None => default,
        }
    };
    let identity_agent = match first("identityagent") {
        None => Agent::Default,
        Some(value) if value.eq_ignore_ascii_case("none") => Agent::None,
        Some(value) if value == "SSH_AUTH_SOCK" => Agent::Default,
        Some(value) => Agent::At(expand(&expand_tokens(&value))),
    };

    // Paths can be space-separated within one value as well as repeated, and
    // they arrive with the client's own placeholders still in them.
    let paths = |key: &str| -> Vec<PathBuf> {
        fields
            .get(key)
            .map(|values| values.iter().flat_map(|value| one_or_many(value)).collect())
            .unwrap_or_default()
    };

    let names = |key: &str| -> Vec<String> { algorithm_list(fields.get(key)) };

    let mut known_hosts = paths("userknownhostsfile");
    known_hosts.extend(paths("globalknownhostsfile"));

    Ok(Effective {
        hostname: first("hostname").unwrap_or_else(|| host.to_string()),
        user: first("user").unwrap_or_else(|| user.to_string()),
        port: first("port").and_then(|value| value.parse().ok()).unwrap_or(port),
        identity_files: paths("identityfile"),
        known_hosts,
        strict_host_key_checking: first("stricthostkeychecking").unwrap_or_default(),
        preferred_authentications: first("preferredauthentications")
            .map(|value| value.split(',').map(|m| m.trim().to_string()).collect())
            .unwrap_or_default(),
        identities_only: flag("identitiesonly", false),
        identity_agent,
        pubkey_authentication: flag("pubkeyauthentication", true),
        password_authentication: flag("passwordauthentication", true),
        kbd_interactive_authentication: flag("kbdinteractiveauthentication", true),
        batch_mode: flag("batchmode", false),
        crypto: Crypto {
            ciphers: names("ciphers"),
            macs: names("macs"),
            kex_algorithms: names("kexalgorithms"),
            host_key_algorithms: names("hostkeyalgorithms"),
            pubkey_accepted_algorithms: names("pubkeyacceptedalgorithms"),
        },
    })
}

/// One comma-separated algorithm list from `ssh -G`, in the order it was given.
///
/// A leading `+`, `-` or `^` should never reach this: those modify the default
/// list and the client resolves them before printing. One that does anyway is
/// dropped rather than matched literally, so a modifier afterm did not expect
/// narrows the intersection instead of widening it.
fn algorithm_list(values: Option<&Vec<&str>>) -> Vec<String> {
    values
        .map(|values| {
            values
                .iter()
                .flat_map(|value| value.split(','))
                .map(str::trim)
                .filter(|name| !name.is_empty() && !name.starts_with(['+', '-', '^']))
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

/// The substitutions `IdentityAgent` allows that `ssh -G` leaves in place:
/// environment variables as `${NAME}` or `$NAME`, and the tokens for the home
/// directory and the local user. Anything else is left as written.
fn expand_tokens(value: &str) -> String {
    let mut out = String::new();
    let mut rest = value;
    while let Some(index) = rest.find(['$', '%']) {
        out.push_str(&rest[..index]);
        let tail = &rest[index..];
        let consumed = if let Some(after) = tail.strip_prefix("${") {
            match after.find('}') {
                Some(end) => {
                    out.push_str(&std::env::var(&after[..end]).unwrap_or_default());
                    end + 3
                }
                None => {
                    out.push('$');
                    1
                }
            }
        } else if let Some(after) = tail.strip_prefix('$') {
            let end = after
                .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
                .unwrap_or(after.len());
            if end == 0 {
                out.push('$');
                1
            } else {
                out.push_str(&std::env::var(&after[..end]).unwrap_or_default());
                end + 1
            }
        } else {
            match tail.chars().nth(1) {
                Some('d') => {
                    if let Some(home) = dirs::home_dir() {
                        out.push_str(&home.to_string_lossy());
                    }
                    2
                }
                Some('u') => {
                    out.push_str(
                        &std::env::var("USER")
                            .or_else(|_| std::env::var("USERNAME"))
                            .unwrap_or_default(),
                    );
                    2
                }
                Some('%') => {
                    out.push('%');
                    2
                }
                Some(other) => {
                    out.push('%');
                    out.push(other);
                    1 + other.len_utf8()
                }
                None => {
                    out.push('%');
                    1
                }
            }
        };
        rest = &tail[consumed..];
    }
    out.push_str(rest);
    out
}

/// One value from `ssh -G`, as the list of paths it names.
///
/// `GlobalKnownHostsFile` really does hold two paths separated by a space, and
/// `C:\Users\Arya Farzin\.ssh\known_hosts` really is one path containing one.
/// `ssh -G` prints the parsed value, so the quoting that told them apart in the
/// configuration is gone by the time afterm sees it.
///
/// The tiebreak is the filesystem: if the whole value names something that
/// exists, it is one path. Otherwise it is a list. That gets the space-in-a-
/// username case right without guessing, and costs one `stat`.
fn one_or_many(value: &str) -> Vec<PathBuf> {
    let whole = expand(value);
    if whole.exists() {
        return vec![whole];
    }
    split_paths(value).iter().map(|part| expand(part)).collect()
}

/// Resolve the placeholders `ssh -G` leaves in a path.
///
/// Win32-OpenSSH prints its system paths with a literal `__PROGRAMDATA__`
/// token, which it substitutes itself at the point of use:
///
/// ```text
/// globalknownhostsfile __PROGRAMDATA__\ssh/ssh_known_hosts …
/// ```
///
/// Left alone, that path never exists, so afterm quietly skipped the
/// system-wide `known_hosts` on Windows — which in a managed environment is
/// exactly where host keys are provisioned, and the failure would have been a
/// trusted host reading as unknown. Found by the test that runs the real
/// client, on the Windows agent, which is the only place it could have been.
///
/// `~` is expanded here too, so every consumer sees a real path.
fn expand(value: &str) -> PathBuf {
    if let Some(rest) = value
        .strip_prefix("__PROGRAMDATA__/")
        .or_else(|| value.strip_prefix("__PROGRAMDATA__\\"))
    {
        let root = std::env::var("ProgramData").unwrap_or_else(|_| "C:\\ProgramData".into());
        return PathBuf::from(root).join(rest);
    }
    if let Some(rest) = value.strip_prefix("~/").or_else(|| value.strip_prefix("~\\")) {
        if let Some(home) = dirs::home_dir() {
            return home.join(rest);
        }
    }
    PathBuf::from(value)
}

/// Split a value that may hold several paths. Quoted paths keep their spaces;
/// unquoted ones are separated by them, which is how OpenSSH writes the
/// known-hosts lists.
fn split_paths(value: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut current = String::new();
    let mut quoted = false;
    for character in value.chars() {
        match character {
            '"' => quoted = !quoted,
            c if c.is_whitespace() && !quoted => {
                if !current.is_empty() {
                    out.push(std::mem::take(&mut current));
                }
            }
            c => current.push(c),
        }
    }
    if !current.is_empty() {
        out.push(current);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "\
host prod
user deploy
hostname prod-01.internal
port 2222
identityfile ~/.ssh/id_ed25519_prod
identityfile ~/.ssh/id_rsa
stricthostkeychecking true
preferredauthentications publickey,password
userknownhostsfile /home/arya/.ssh/known_hosts /home/arya/.ssh/known_hosts2
globalknownhostsfile /etc/ssh/ssh_known_hosts /etc/ssh/ssh_known_hosts2
proxyjump none
proxycommand none
";

    #[test]
    fn reads_the_effective_target_rather_than_the_typed_one() {
        let effective = parse(SAMPLE, "prod", "arya", 22).unwrap();
        assert_eq!(effective.hostname, "prod-01.internal");
        assert_eq!(effective.user, "deploy");
        assert_eq!(effective.port, 2222);
    }

    #[test]
    fn collects_every_identity_file_and_known_hosts_file() {
        let effective = parse(SAMPLE, "prod", "arya", 22).unwrap();
        assert_eq!(effective.identity_files.len(), 2);
        // User files first, then the system ones, in OpenSSH's search order.
        assert_eq!(
            effective.known_hosts,
            vec![
                PathBuf::from("/home/arya/.ssh/known_hosts"),
                PathBuf::from("/home/arya/.ssh/known_hosts2"),
                PathBuf::from("/etc/ssh/ssh_known_hosts"),
                PathBuf::from("/etc/ssh/ssh_known_hosts2"),
            ]
        );
    }

    #[test]
    fn an_unset_proxy_is_reported_as_none_and_must_not_refuse() {
        assert!(parse(SAMPLE, "prod", "arya", 22).is_ok());
    }

    #[test]
    fn refuses_a_host_whose_routing_afterm_cannot_reproduce() {
        let text = SAMPLE.replace("proxyjump none", "proxyjump bastion");
        let failure = parse(&text, "prod", "arya", 22).unwrap_err();
        assert_eq!(failure.cause, crate::transport::Cause::Unsupported);
        assert!(failure.message.contains("ProxyJump"), "{}", failure.message);
    }

    #[test]
    fn refuses_a_host_key_alias_because_trust_would_be_checked_under_the_wrong_name() {
        let text = format!("{SAMPLE}hostkeyalias other-name\n");
        let failure = parse(&text, "prod", "arya", 22).unwrap_err();
        assert!(failure.message.contains("HostKeyAlias"));
    }

    #[test]
    fn a_harmless_global_block_does_not_refuse_anything() {
        // The three-directive allowlist an early design proposed would have
        // rejected this; `Host *` keep-alives are on almost every real config.
        let text = format!("{SAMPLE}serveraliveinterval 30\nserveralivecountmax 3\ncompression no\n");
        assert!(parse(&text, "prod", "arya", 22).is_ok());
    }

    /// Every line here is what `ssh -G` prints for a directive nobody set.
    /// Refusing any of them would send every host to the OpenSSH client.
    #[test]
    fn the_defaults_the_client_prints_refuse_nothing() {
        let text = format!(
            "{SAMPLE}forwardagent no\nforwardx11 no\ntunnel false\naddressfamily any\n\
             requesttty auto\nsessiontype default\npermitlocalcommand no\n\
             securitykeyprovider internal\nproxyusefdpass no\nstdinnull no\n\
             forkafterauthentication no\nhostbasedauthentication no\nidentitiesonly no\n\
             pubkeyauthentication true\npasswordauthentication yes\n\
             kbdinteractiveauthentication yes\nbatchmode no\ngssapiauthentication yes\n\
             ciphers aes256-gcm@openssh.com\nupdatehostkeys true\ncontrolmaster false\n"
        );
        let effective = parse(&text, "prod", "arya", 22).expect("defaults must not refuse");
        assert!(!effective.identities_only);
        assert_eq!(effective.identity_agent, Agent::Default);
        assert!(effective.pubkey_authentication);
        assert!(effective.password_authentication);
        assert!(effective.kbd_interactive_authentication);
        assert!(!effective.batch_mode);
    }

    /// Forwarding and remote commands are things the user's session would do
    /// that afterm's would not. Each is refused by name.
    #[test]
    fn refuses_what_the_session_would_otherwise_silently_not_carry() {
        for (line, name) in [
            ("forwardagent yes", "ForwardAgent"),
            ("forwardx11 yes", "ForwardX11"),
            ("localforward 8080 [localhost]:80", "LocalForward"),
            ("remoteforward 2222 [localhost]:22", "RemoteForward"),
            ("dynamicforward 1080", "DynamicForward"),
            ("remotecommand tmux", "RemoteCommand"),
            ("permitlocalcommand yes", "PermitLocalCommand"),
            ("tunnel point-to-point", "Tunnel"),
            ("bindaddress 10.0.0.2", "BindAddress"),
            ("addressfamily inet", "AddressFamily"),
            ("sessiontype none", "SessionType"),
            ("requesttty no", "RequestTTY"),
            ("knownhostscommand /usr/bin/fetch-keys", "KnownHostsCommand"),
            ("hostbasedauthentication yes", "HostbasedAuthentication"),
            ("pkcs11provider /usr/lib/opensc-pkcs11.so", "PKCS11Provider"),
            ("certificatefile ~/.ssh/id_ed25519-cert.pub", "CertificateFile"),
        ] {
            let text = format!("{SAMPLE}{line}\n");
            let failure = parse(&text, "prod", "arya", 22)
                .err()
                .unwrap_or_else(|| panic!("{line:?} must be refused"));
            assert_eq!(failure.cause, crate::transport::Cause::Unsupported, "{line}");
            assert!(failure.message.contains(name), "{line}: {}", failure.message);
        }
    }

    /// The switches that shape authentication are read, so afterm neither
    /// offers a method the configuration turned off nor prompts under
    /// `BatchMode`.
    #[test]
    fn reads_the_switches_that_shape_authentication() {
        let text = format!(
            "{SAMPLE}identitiesonly yes\nidentityagent none\npubkeyauthentication false\n\
             passwordauthentication no\nkbdinteractiveauthentication no\nbatchmode yes\n"
        );
        let effective = parse(&text, "prod", "arya", 22).unwrap();
        assert!(effective.identities_only);
        assert_eq!(effective.identity_agent, Agent::None);
        assert!(effective.batch_mode);
        // `PreferredAuthentications` in the sample lists publickey and password;
        // the switches take both away.
        assert!(!effective.allows("publickey"));
        assert!(!effective.allows("password"));
        assert!(!effective.allows("keyboard-interactive"));
    }

    #[test]
    fn resolves_where_the_agent_is() {
        let at = |line: &str| {
            parse(&format!("{SAMPLE}{line}\n"), "prod", "arya", 22).unwrap().identity_agent
        };
        assert_eq!(at("identityagent SSH_AUTH_SOCK"), Agent::Default);
        assert_eq!(at("identityagent none"), Agent::None);
        assert_eq!(at("identityagent NONE"), Agent::None);
        match at("identityagent ~/.1password/agent.sock") {
            Agent::At(path) => assert!(!path.to_string_lossy().starts_with('~'), "{path:?}"),
            other => panic!("expected a path, got {other:?}"),
        }
        std::env::set_var("AFTERM_TEST_SOCK", "/run/user/1000/agent");
        assert_eq!(
            at("identityagent ${AFTERM_TEST_SOCK}/S.sock"),
            Agent::At(PathBuf::from("/run/user/1000/agent/S.sock"))
        );
        assert_eq!(
            at("identityagent $AFTERM_TEST_SOCK"),
            Agent::At(PathBuf::from("/run/user/1000/agent"))
        );
        // `%%` is a literal percent; an unknown token is left as written.
        assert_eq!(expand_tokens("100%% %z"), "100% %z");
    }

    /// The five crypto directives are read as ordered lists, and a value the
    /// client would never print — a `+` modifier that reached here unresolved —
    /// is dropped rather than matched literally.
    #[test]
    fn reads_the_cryptographic_policy_as_ordered_lists() {
        let text = format!(
            "{SAMPLE}\
             ciphers aes256-gcm@openssh.com,aes256-ctr\n\
             macs hmac-sha2-512-etm@openssh.com\n\
             kexalgorithms curve25519-sha256,+diffie-hellman-group14-sha1\n\
             hostkeyalgorithms ssh-ed25519,rsa-sha2-512\n\
             pubkeyacceptedalgorithms rsa-sha2-512,rsa-sha2-256\n"
        );
        let crypto = parse(&text, "prod", "arya", 22).unwrap().crypto;
        assert_eq!(crypto.ciphers, vec!["aes256-gcm@openssh.com", "aes256-ctr"]);
        assert_eq!(crypto.macs, vec!["hmac-sha2-512-etm@openssh.com"]);
        assert_eq!(crypto.kex_algorithms, vec!["curve25519-sha256"]);
        assert_eq!(crypto.host_key_algorithms, vec!["ssh-ed25519", "rsa-sha2-512"]);
        assert!(crypto.allows_signature("rsa-sha2-256"));
        assert!(!crypto.allows_signature("ssh-rsa"));
    }

    /// A restriction is a policy, not a preference: it must survive into the
    /// `Effective` rather than being read and dropped, which is what the
    /// earlier version did.
    #[test]
    fn a_configuration_with_no_crypto_directives_states_no_policy() {
        let crypto = parse(SAMPLE, "prod", "arya", 22).unwrap().crypto;
        assert_eq!(crypto, Crypto::default());
        assert!(crypto.allows_signature("ssh-ed25519"));
    }

    #[test]
    fn honours_preferred_authentications_as_a_filter() {
        let effective = parse(SAMPLE, "prod", "arya", 22).unwrap();
        assert!(effective.allows("password"));
        assert!(!effective.allows("keyboard-interactive"));
        // An unset directive allows everything rather than nothing.
        let empty = parse("hostname h\n", "h", "u", 22).unwrap();
        assert!(empty.allows("keyboard-interactive"));
    }

    /// Everything above parses a fixture. This one runs the real client.
    ///
    /// It is the check that matters most on Windows, where this project's
    /// assumptions have been wrong four times: `ssh -G` is a Win32-OpenSSH code
    /// path afterm now depends on, and the hosted agent is the only Windows
    /// afterm ever executes on. Skipped rather than failed where there is no
    /// client, so a machine without OpenSSH does not fail the build.
    #[test]
    fn the_installed_client_answers_and_afterm_understands_the_answer() {
        if !have_openssh() {
            eprintln!("no ssh client on PATH; skipping");
            return;
        }
        let effective = resolve("afterm-test.invalid", "someone", 2222, None)
            .expect("the installed ssh client should resolve an unknown host");

        assert_eq!(effective.hostname, "afterm-test.invalid");
        assert_eq!(effective.user, "someone");
        assert_eq!(effective.port, 2222);
        // Both the user and the system known-hosts files, absolute. russh's own
        // reader takes one path; this is why afterm searches the list itself.
        assert!(
            !effective.known_hosts.is_empty(),
            "ssh -G always names its known-hosts files"
        );
        assert!(
            effective.known_hosts.iter().all(|path| path.is_absolute()),
            "paths should come back resolved: {:?}",
            effective.known_hosts
        );
        // Win32-OpenSSH prints its system paths with a placeholder still in
        // them. A path afterm cannot resolve is one it silently skips.
        assert!(
            !effective
                .known_hosts
                .iter()
                .any(|path| path.to_string_lossy().contains("__")),
            "a placeholder survived: {:?}",
            effective.known_hosts
        );
        // A host with no configuration must not be refused, and reads as the
        // defaults: every method on, the usual agent, prompting allowed.
        assert!(effective.identity_files.iter().all(|path| !path.as_os_str().is_empty()));
        assert!(!effective.identities_only);
        assert_eq!(effective.identity_agent, Agent::Default);
        assert!(effective.allows("publickey"));
        assert!(effective.allows("password"));
        assert!(!effective.batch_mode);
    }

    /// The same, with a configuration written for the occasion, so the real
    /// client's spelling of each directive is what is parsed — on Windows,
    /// Win32-OpenSSH's spelling, which is the one that matters.
    #[test]
    fn the_installed_client_reports_what_a_written_config_sets() {
        if !have_openssh() {
            eprintln!("no ssh client on PATH; skipping");
            return;
        }
        let directory = std::env::temp_dir().join(format!("afterm-ssh-g-{}", std::process::id()));
        std::fs::create_dir_all(&directory).unwrap();
        let written = |name: &str, body: &str| -> PathBuf {
            let path = directory.join(name);
            std::fs::write(&path, format!("Host afterm-test.invalid\n{body}")).unwrap();
            path
        };

        // The real client resolves `Ciphers -aes256-ctr` and friends before it
        // prints them, which is the whole reason afterm reads the output
        // rather than the file. Check that what comes back is a narrowed list
        // afterm can act on, on the machine actually running the test.
        let narrowed = written(
            "crypto",
            "  Ciphers aes256-ctr\n  MACs hmac-sha2-256\n\
             \n  KexAlgorithms curve25519-sha256\n\
             \n  PubkeyAcceptedAlgorithms ssh-ed25519\n",
        );
        let effective = resolve("afterm-test.invalid", "someone", 22, Some(&narrowed))
            .expect("a narrowed crypto policy must not refuse the host by itself");
        assert_eq!(effective.crypto.ciphers, vec!["aes256-ctr"]);
        assert_eq!(effective.crypto.macs, vec!["hmac-sha2-256"]);
        assert_eq!(effective.crypto.kex_algorithms, vec!["curve25519-sha256"]);
        assert!(effective.crypto.allows_signature("ssh-ed25519"));
        assert!(!effective.crypto.allows_signature("rsa-sha2-512"));
        // And the policy narrows the negotiation rather than being recorded
        // and ignored.
        let preferred = crate::native::crypto::preferences(
            &effective.crypto,
            &[russh::keys::Algorithm::Ed25519],
        )
        .expect("a supported narrowing is not a refusal");
        assert_eq!(
            preferred.cipher.iter().map(|n| n.as_ref()).collect::<Vec<_>>(),
            vec!["aes256-ctr"]
        );

        let switches = written(
            "switches",
            "  IdentitiesOnly yes\n  IdentityAgent none\n  PubkeyAuthentication no\n  BatchMode yes\n",
        );
        let effective = resolve("afterm-test.invalid", "someone", 22, Some(&switches))
            .expect("switches alone must not refuse the host");
        assert!(effective.identities_only);
        assert_eq!(effective.identity_agent, Agent::None);
        assert!(!effective.allows("publickey"));
        assert!(effective.allows("password"));
        assert!(effective.batch_mode);

        for (body, name) in [
            ("  ForwardAgent yes\n", "ForwardAgent"),
            ("  LocalForward 8080 localhost:80\n", "LocalForward"),
            ("  RemoteCommand tmux\n", "RemoteCommand"),
            ("  ProxyJump bastion\n", "ProxyJump"),
        ] {
            let config = written(name, body);
            let failure = resolve("afterm-test.invalid", "someone", 22, Some(&config))
                .err()
                .unwrap_or_else(|| panic!("{name} must be refused"));
            assert!(failure.message.contains(name), "{}", failure.message);
        }

        let _ = std::fs::remove_dir_all(&directory);
    }

    pub(super) fn have_openssh() -> bool {
        Command::new(if cfg!(windows) { "ssh.exe" } else { "ssh" })
            .arg("-V")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok()
    }

    #[test]
    fn expands_the_placeholder_win32_openssh_prints() {
        std::env::set_var("ProgramData", "C:\\ProgramData");
        let expanded = expand("__PROGRAMDATA__\\ssh/ssh_known_hosts");
        let text = expanded.to_string_lossy().replace('\\', "/");
        assert_eq!(text, "C:/ProgramData/ssh/ssh_known_hosts");
        // Forward slashes too: the client mixes them freely.
        assert_eq!(
            expand("__PROGRAMDATA__/ssh/ssh_known_hosts").to_string_lossy().replace('\\', "/"),
            "C:/ProgramData/ssh/ssh_known_hosts"
        );
    }

    #[test]
    fn expands_a_home_relative_path_so_every_consumer_sees_a_real_one() {
        let expanded = expand("~/.ssh/known_hosts");
        assert!(!expanded.to_string_lossy().starts_with('~'), "{expanded:?}");
        // An already-absolute path is left exactly as it was.
        let absolute = if cfg!(windows) { "C:\\keys\\known_hosts" } else { "/etc/ssh/known_hosts" };
        assert_eq!(expand(absolute), PathBuf::from(absolute));
    }

    #[test]
    fn a_path_that_exists_is_one_path_even_with_a_space_in_it() {
        let directory = std::env::temp_dir().join(format!("afterm cfg {}", std::process::id()));
        std::fs::create_dir_all(&directory).unwrap();
        let spaced = directory.join("known_hosts");
        std::fs::write(&spaced, "").unwrap();

        // A Windows profile with a space in it arrives unquoted, and splitting
        // it would produce two paths that name nothing.
        let value = spaced.to_string_lossy().to_string();
        assert!(value.contains(' '), "the fixture needs a space in it");
        assert_eq!(one_or_many(&value), vec![spaced]);

        // The ordinary two-path value still comes back as two.
        assert_eq!(
            one_or_many("/etc/ssh/ssh_known_hosts /etc/ssh/ssh_known_hosts2").len(),
            2
        );

        let _ = std::fs::remove_dir_all(&directory);
    }

    #[test]
    fn keeps_spaces_inside_a_quoted_path() {
        assert_eq!(
            split_paths("\"/home/a b/known_hosts\" /etc/ssh/known_hosts"),
            vec!["/home/a b/known_hosts", "/etc/ssh/known_hosts"]
        );
    }
}
