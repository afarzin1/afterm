//! The user's cryptographic policy, applied to what afterm negotiates.
//!
//! `Ciphers`, `MACs`, `KexAlgorithms`, `HostKeyAlgorithms` and
//! `PubkeyAcceptedAlgorithms` are the five directives that say which
//! primitives the user's client is willing to use. afterm used to read them and
//! act on none of them, on the reasoning that russh negotiates from a modern
//! set anyway. That is true of what afterm *would* pick and beside the point of
//! what the user *forbade*: a configuration that narrows the set is a policy,
//! and a known host key does not license negotiating an algorithm the policy
//! rules out.
//!
//! So the effective lists from `ssh -G` are intersected with what russh can
//! actually do, in the configuration's own order of preference. An empty
//! intersection is never widened back to russh's defaults — it is a refusal by
//! name, which sends the host to the OpenSSH client, the one transport that
//! honours the policy in full.
//!
//! The result is handed to russh as `client::Config::preferred`, which is the
//! same list a rekey negotiates from, so the policy holds for the life of the
//! connection rather than only its first handshake.

use super::config::Crypto;
use crate::transport::Failure;
use russh::keys::Algorithm;
use russh::{cipher, kex, mac, Preferred};
use std::borrow::Cow;
use std::str::FromStr;

/// Kex names that are not algorithms.
///
/// `ext-info-c` and the strict-kex markers are how a client advertises that it
/// understands extension negotiation; OpenSSH sends them and does not list them
/// in `KexAlgorithms`, so intersecting with the configuration would drop them.
/// Losing them costs the server-sig-algs extension, which is how afterm learns
/// which RSA signature a host will accept — an omission whose symptom is a
/// password prompt on a host where the user's RSA key works perfectly.
const KEX_MARKERS: [kex::Name; 4] = [
    kex::EXTENSION_SUPPORT_AS_CLIENT,
    kex::EXTENSION_SUPPORT_AS_SERVER,
    kex::EXTENSION_OPENSSH_STRICT_KEX_AS_CLIENT,
    kex::EXTENSION_OPENSSH_STRICT_KEX_AS_SERVER,
];

/// Narrow russh's negotiation to what the configuration permits.
///
/// `recorded` is the host key algorithms `known_hosts` already has for this
/// host, which is a separate and equally binding restriction: afterm offers
/// only the types the user has actually recorded, so that a host does not
/// present a key OpenSSH never saved and read as unknown.
pub fn preferences(policy: &Crypto, recorded: &[Algorithm]) -> Result<Preferred, Failure> {
    let defaults = Preferred::DEFAULT;

    let kex = if policy.kex_algorithms.is_empty() {
        defaults.kex.to_vec()
    } else {
        let mut chosen = keep::<kex::Name>(&policy.kex_algorithms);
        if chosen.is_empty() {
            return Err(refused("KexAlgorithms", &policy.kex_algorithms));
        }
        chosen.extend(KEX_MARKERS);
        chosen
    };

    let cipher = if policy.ciphers.is_empty() {
        defaults.cipher.to_vec()
    } else {
        let chosen = keep::<cipher::Name>(&policy.ciphers);
        if chosen.is_empty() {
            return Err(refused("Ciphers", &policy.ciphers));
        }
        chosen
    };

    let mac = if policy.macs.is_empty() {
        defaults.mac.to_vec()
    } else {
        let chosen = keep::<mac::Name>(&policy.macs);
        if chosen.is_empty() {
            return Err(refused("MACs", &policy.macs));
        }
        chosen
    };

    let key = host_keys(policy, recorded)?;

    Ok(Preferred {
        kex: Cow::Owned(kex),
        key: Cow::Owned(key),
        cipher: Cow::Owned(cipher),
        mac: Cow::Owned(mac),
        ..Preferred::DEFAULT
    })
}

/// The host key algorithms to offer: recorded in `known_hosts`, and permitted
/// by `HostKeyAlgorithms`, in the configuration's order of preference.
///
/// Both restrictions apply. A host whose recorded key is of a type the
/// configuration forbids is not one afterm can verify the way the user's own
/// client would, so it goes to that client rather than being verified against a
/// key the policy says not to accept.
fn host_keys(policy: &Crypto, recorded: &[Algorithm]) -> Result<Vec<Algorithm>, Failure> {
    if policy.host_key_algorithms.is_empty() {
        return Ok(recorded.to_vec());
    }
    let permitted: Vec<Algorithm> = policy
        .host_key_algorithms
        .iter()
        // Certificate names — `ssh-ed25519-cert-v01@openssh.com` and the rest —
        // do not parse as a bare algorithm, and afterm does not offer
        // certificates anyway. Dropping them narrows, which is the safe way to
        // be wrong here.
        .filter_map(|name| Algorithm::from_str(name).ok())
        .filter(|algorithm| recorded.contains(algorithm))
        .collect();
    if permitted.is_empty() {
        return Err(Failure::unsupported(format!(
            "this host's ssh_config restricts HostKeyAlgorithms to {}, and none of them \
             is a key type recorded for it in known_hosts, so it stays on the OpenSSH client",
            policy.host_key_algorithms.join(",")
        )));
    }
    Ok(permitted)
}

/// The configured names russh knows, in the configuration's order.
fn keep<T>(configured: &[String]) -> Vec<T>
where
    for<'a> T: TryFrom<&'a str>,
{
    configured
        .iter()
        .filter_map(|name| T::try_from(name.as_str()).ok())
        .collect()
}

fn refused(directive: &str, configured: &[String]) -> Failure {
    Failure::unsupported(format!(
        "this host's ssh_config restricts {directive} to {}, none of which afterm's native \
         transport can negotiate, so it stays on the OpenSSH client",
        configured.join(",")
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn policy(kex: &[&str], ciphers: &[&str], macs: &[&str], keys: &[&str]) -> Crypto {
        let list = |values: &[&str]| values.iter().map(|v| v.to_string()).collect::<Vec<_>>();
        Crypto {
            ciphers: list(ciphers),
            macs: list(macs),
            kex_algorithms: list(kex),
            host_key_algorithms: list(keys),
            pubkey_accepted_algorithms: Vec::new(),
        }
    }

    fn names<T: AsRef<str>>(values: &[T]) -> Vec<String> {
        values.iter().map(|v| v.as_ref().to_string()).collect()
    }

    /// The whole point: a restricted list is what gets negotiated, in the
    /// order the configuration asked for, and nothing outside it is offered.
    #[test]
    fn a_restricted_list_is_the_list_that_is_offered() {
        let restricted = policy(
            &["curve25519-sha256"],
            &["aes256-ctr", "aes128-ctr"],
            &["hmac-sha2-512-etm@openssh.com"],
            &[],
        );
        let preferred = preferences(&restricted, &[Algorithm::Ed25519]).unwrap();

        assert_eq!(names(&preferred.cipher), vec!["aes256-ctr", "aes128-ctr"]);
        assert_eq!(names(&preferred.mac), vec!["hmac-sha2-512-etm@openssh.com"]);
        // The markers are appended, and nothing else is.
        assert_eq!(names(&preferred.kex)[0], "curve25519-sha256");
        assert!(!names(&preferred.kex).contains(&"diffie-hellman-group14-sha256".to_string()));
    }

    /// Extension negotiation survives a narrowed `KexAlgorithms`. Without this
    /// afterm stops being told which RSA signature the host accepts.
    #[test]
    fn narrowing_the_kex_list_keeps_the_extension_markers() {
        let restricted = policy(&["curve25519-sha256"], &[], &[], &[]);
        let preferred = preferences(&restricted, &[Algorithm::Ed25519]).unwrap();
        assert!(names(&preferred.kex).contains(&"ext-info-c".to_string()));
        assert!(names(&preferred.kex).contains(&"kex-strict-c-v00@openssh.com".to_string()));
    }

    /// An intersection of nothing is a refusal, never a quiet return to
    /// russh's own defaults. Each directive says which one it was.
    #[test]
    fn an_empty_intersection_refuses_by_name_rather_than_widening() {
        for (directive, restricted) in [
            ("KexAlgorithms", policy(&["gss-group14-sha256-"], &[], &[], &[])),
            ("Ciphers", policy(&[], &["blowfish-cbc"], &[], &[])),
            ("MACs", policy(&[], &[], &["umac-64@openssh.com"], &[])),
        ] {
            let failure = preferences(&restricted, &[Algorithm::Ed25519]).unwrap_err();
            assert_eq!(failure.cause, crate::transport::Cause::Unsupported, "{directive}");
            assert!(failure.message.contains(directive), "{}", failure.message);
        }
    }

    /// A configuration that permits only key types this host has not recorded
    /// cannot be honoured, and that is a refusal too.
    #[test]
    fn host_key_algorithms_are_intersected_with_what_known_hosts_recorded() {
        let both = policy(&[], &[], &[], &["ssh-ed25519", "ecdsa-sha2-nistp256"]);
        let preferred = preferences(&both, &[Algorithm::Ed25519]).unwrap();
        assert_eq!(preferred.key.to_vec(), vec![Algorithm::Ed25519]);

        let neither = policy(&[], &[], &[], &["ecdsa-sha2-nistp384"]);
        let failure = preferences(&neither, &[Algorithm::Ed25519]).unwrap_err();
        assert!(failure.message.contains("HostKeyAlgorithms"), "{}", failure.message);
    }

    /// Certificate names in `HostKeyAlgorithms` are dropped rather than
    /// mistaken for their bare equivalents: afterm offers no certificates.
    #[test]
    fn certificate_host_key_names_do_not_stand_in_for_bare_ones() {
        let certificates = policy(&[], &[], &[], &["ssh-ed25519-cert-v01@openssh.com"]);
        assert!(preferences(&certificates, &[Algorithm::Ed25519]).is_err());
    }

    /// No policy at all leaves russh's defaults exactly as they were, so a host
    /// with an ordinary configuration negotiates what it always did.
    #[test]
    fn an_unset_policy_changes_nothing() {
        let preferred = preferences(&Crypto::default(), &[Algorithm::Ed25519]).unwrap();
        assert_eq!(names(&preferred.kex), names(&Preferred::DEFAULT.kex));
        assert_eq!(names(&preferred.cipher), names(&Preferred::DEFAULT.cipher));
        assert_eq!(names(&preferred.mac), names(&Preferred::DEFAULT.mac));
        assert_eq!(preferred.key.to_vec(), vec![Algorithm::Ed25519]);
    }

    /// `PubkeyAcceptedAlgorithms` restricts signatures rather than the
    /// handshake, so it is enforced where keys are offered — but the rule
    /// itself lives with the rest of the policy.
    #[test]
    fn signature_restrictions_are_read_as_a_list_not_as_silence() {
        let mut restricted = Crypto::default();
        assert!(restricted.allows_signature("ssh-ed25519"), "an absent directive forbids nothing");

        restricted.pubkey_accepted_algorithms =
            names(&["rsa-sha2-512", "rsa-sha2-256"]);
        assert!(restricted.allows_signature("rsa-sha2-512"));
        assert!(!restricted.allows_signature("ssh-rsa"));
        assert!(!restricted.allows_signature("ssh-ed25519"));
    }
}
