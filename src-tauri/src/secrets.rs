//! Remembered passwords, in whatever credential store the operating system
//! keeps.
//!
//! Three rules shape this module, and they are worth stating because they are
//! what keep the feature honest.
//!
//! **A secret never reaches JavaScript.** A newly typed password crosses the
//! IPC once, into the handshake that asked for it. A remembered one is read
//! here and handed to `russh` without leaving Rust, so there is no command
//! that returns one and nothing for the webview to leak.
//!
//! **No fallback.** Where there is no operating-system store there is no
//! feature: the interface says so and the checkbox is not offered. An
//! encrypted file with a key in the executable is not a credential store, it
//! is an obfuscated one, and shipping it would make the promise this module
//! exists to keep untrue.
//!
//! **The platform stops here.** Nothing above this module knows which store is
//! underneath, or that Windows is the only one implemented today. The
//! interface answers *can this be saved, and if not why* — never *is this
//! Windows* — so a second platform is a new `Store` in this file and a
//! dependency in one `cfg` block, and no change anywhere else.

use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::sync::{Arc, Mutex};
use zeroize::Zeroizing;

/// The format of the stored blob. Read back before anything else in it is
/// trusted, so a future version can change the shape without a stored
/// credential from an older afterm being mistaken for the new one.
const FORMAT: u8 = 1;

/// What afterm calls itself in the credential store. Every entry it writes is
/// under this name, which is what makes them afterm's to find and to delete.
const SERVICE: &str = "afterm";

/// What a remembered password is bound to.
///
/// Not the display name and not the tab: a saved host's own identity, plus the
/// destination as it resolved. The whole binding is checked before a stored
/// password is used, so moving a saved host to another machine or account
/// asks for the password again rather than sending the old one somewhere new.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Binding {
    /// The saved profile's stable id, which is the key the entry is stored
    /// under. A rename does not touch it; deleting the host deletes it.
    pub profile: String,
    /// The account, after `ssh_config` has had its say.
    pub user: String,
    /// The host, likewise resolved — a `Host` alias saves under where it
    /// actually goes, not under what it is called.
    pub hostname: String,
    pub port: u16,
    /// The host key afterm accepted for this connection. A host that presents
    /// a different key is not the host the password was saved for.
    pub fingerprint: String,
}

/// The blob that goes into the store: the binding, and the secret it is bound
/// to.
///
/// The binding travels *with* the secret rather than beside it in afterm's own
/// files, which is what makes it worth checking. A binding in `profiles.json`
/// could be edited by anything that can write the file; this one is only
/// readable by whoever can already read the password.
#[derive(Serialize, Deserialize)]
struct Stored {
    version: u8,
    user: String,
    hostname: String,
    port: u16,
    fingerprint: String,
    password: String,
}

/// What came back from a read.
#[derive(Debug, PartialEq, Eq)]
pub enum Remembered {
    /// Nothing is saved for this profile.
    Missing,
    /// Something is saved, and it was saved for somewhere else — a different
    /// account, host, port, or a host presenting a different key. The user is
    /// asked again; the entry is left alone, because it is still correct for
    /// the destination it was saved for.
    Elsewhere,
    /// A password saved for exactly this destination.
    Found(Zeroizing<String>),
}

/// Why a store operation could not be done.
#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("this build of afterm has no credential store to save passwords in")]
    Unsupported,
    #[error("the credential store could not be reached: {0}")]
    Unreachable(String),
    #[error("the credential store refused the entry: {0}")]
    Refused(String),
}

/// Whether saving can be offered at all, and what to say when it cannot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Availability {
    Ready,
    /// Phrased for the interface to show as it stands. It says what is missing
    /// rather than which platform is missing it.
    Unavailable(String),
}

/// One credential store.
///
/// Every entry is addressed by the profile alone, which is what makes
/// forgetting possible without knowing where the host has moved to since. The
/// rest of the binding rides inside the entry and is checked on the way out.
pub trait Store: Send + Sync {
    fn availability(&self) -> Availability;
    fn load(&self, binding: &Binding) -> Result<Remembered, StoreError>;
    fn save(&self, binding: &Binding, password: &str) -> Result<(), StoreError>;
    /// Remove the entry for one profile. Removing what is not there succeeds:
    /// the caller wanted it gone, and it is.
    fn forget(&self, profile: &str) -> Result<(), StoreError>;
    /// Whether an entry exists, whatever it is bound to. For the interface,
    /// which offers *Forget password* on the strength of it and must not need
    /// the secret to decide.
    fn holds(&self, profile: &str) -> Result<bool, StoreError>;
}

/// The store, plus the one piece of state that is policy rather than storage.
///
/// A password the host has just rejected must not be sent again on its own —
/// not by this tab retrying and not by the four other tabs on the same saved
/// host, which is why the record of it lives here, once, and not in a
/// connection.
pub struct Vault {
    store: Arc<dyn Store>,
    /// Profiles whose stored password has been refused since afterm started.
    /// Cleared for a profile when the user supplies a new password for it, and
    /// on nothing else: an entry the server said no to is not worth offering
    /// again until something has changed.
    rejected: Mutex<HashSet<String>>,
}

impl Vault {
    pub fn new(store: Arc<dyn Store>) -> Self {
        Self { store, rejected: Mutex::new(HashSet::new()) }
    }

    /// The store this vault is built on, for the interface's own questions.
    pub fn store(&self) -> &dyn Store {
        self.store.as_ref()
    }

    /// Whether the dialog should offer to remember. Both halves have to hold:
    /// a store to put it in, and a saved host to key it to.
    pub fn may_remember(&self, profile: Option<&str>) -> bool {
        profile.is_some() && self.store.availability() == Availability::Ready
    }

    /// A password worth trying before the user is asked for one.
    ///
    /// `None` covers every reason not to: no profile, no store, nothing saved,
    /// something saved for somewhere else, or something saved that this host
    /// has already refused. None of those is an error the user needs to see —
    /// each one ends in the same place, which is the dialog they would have
    /// got anyway.
    pub fn remembered(&self, binding: &Binding) -> Option<Zeroizing<String>> {
        if self.rejected.lock().unwrap().contains(&binding.profile) {
            return None;
        }
        match self.store.load(binding) {
            Ok(Remembered::Found(password)) => Some(password),
            _ => None,
        }
    }

    /// Note that a stored password was refused, so nothing offers it again.
    pub fn reject(&self, profile: &str) {
        self.rejected.lock().unwrap().insert(profile.to_string());
    }

    /// Save what the user has just authenticated with, having asked us to.
    ///
    /// Clears any rejection first: this is the replacement that makes the
    /// entry worth trying again.
    pub fn remember(&self, binding: &Binding, password: &str) -> Result<(), StoreError> {
        self.rejected.lock().unwrap().remove(&binding.profile);
        self.store.save(binding, password)
    }

    pub fn forget(&self, profile: &str) -> Result<(), StoreError> {
        self.rejected.lock().unwrap().remove(profile);
        self.store.forget(profile)
    }
}

/// A store that cannot store anything, and says so.
///
/// What every platform without an implementation gets. It is not a fallback —
/// it holds nothing and refuses every write — it is the shape of "not here",
/// so the rest of afterm has one path rather than two.
pub struct Absent {
    reason: String,
}

impl Absent {
    pub fn new(reason: impl Into<String>) -> Self {
        Self { reason: reason.into() }
    }
}

impl Store for Absent {
    fn availability(&self) -> Availability {
        Availability::Unavailable(self.reason.clone())
    }

    fn load(&self, _: &Binding) -> Result<Remembered, StoreError> {
        Ok(Remembered::Missing)
    }

    fn save(&self, _: &Binding, _: &str) -> Result<(), StoreError> {
        Err(StoreError::Unsupported)
    }

    fn forget(&self, _: &str) -> Result<(), StoreError> {
        Ok(())
    }

    fn holds(&self, _: &str) -> Result<bool, StoreError> {
        Ok(false)
    }
}

/// Read a blob back and decide what it is worth.
///
/// Split out from the platform code because it is the part with the rules in
/// it, and the part a test can drive without a credential store.
fn interpret(blob: &[u8], binding: &Binding) -> Remembered {
    let Ok(stored) = serde_json::from_slice::<Stored>(blob) else {
        // Not afterm's, or not a shape this version understands. Treated as
        // nothing rather than as an error: the user is asked for a password,
        // and saving again overwrites whatever this was.
        return Remembered::Missing;
    };
    if stored.version != FORMAT {
        return Remembered::Missing;
    }
    let bound = stored.user == binding.user
        && stored.hostname == binding.hostname
        && stored.port == binding.port
        && stored.fingerprint == binding.fingerprint;
    if !bound {
        return Remembered::Elsewhere;
    }
    Remembered::Found(Zeroizing::new(stored.password))
}

/// The blob to store for one binding.
fn compose(binding: &Binding, password: &str) -> Result<Zeroizing<Vec<u8>>, StoreError> {
    let stored = Stored {
        version: FORMAT,
        user: binding.user.clone(),
        hostname: binding.hostname.clone(),
        port: binding.port,
        fingerprint: binding.fingerprint.clone(),
        password: password.to_string(),
    };
    serde_json::to_vec(&stored)
        .map(Zeroizing::new)
        .map_err(|error| StoreError::Refused(error.to_string()))
}

// ------------------------------------------------------- the real store

/// The operating system's own credential store, where there is one.
///
/// One `cfg` block, because this is the file the platform is allowed to reach.
#[cfg(windows)]
mod platform {
    use super::{compose, interpret, Availability, Binding, Remembered, Store, StoreError};
    use keyring_core::{set_default_store, Entry, Error as KeyringError};
    use std::collections::HashMap;

    /// Windows Credential Manager, through `keyring-core`.
    pub struct Native;

    impl Native {
        /// Install the platform store as the process default, and answer
        /// whether it took.
        pub fn open() -> Result<Self, StoreError> {
            let store = windows_native_keyring_store::Store::new()
                .map_err(|error| StoreError::Unreachable(error.to_string()))?;
            set_default_store(store);
            Ok(Self)
        }

        /// One entry, keyed on the profile.
        ///
        /// `Local` is `CRED_PERSIST_LOCAL_MACHINE`: the credential is kept for
        /// this Windows user on this machine across logins, and is not offered
        /// to a roaming profile to carry elsewhere. The store crate's own
        /// default is `Enterprise`, which does roam, so this is passed on
        /// every entry rather than left to the default.
        fn entry(&self, profile: &str) -> Result<Entry, StoreError> {
            let mut modifiers = HashMap::new();
            modifiers.insert("persistence", "Local");
            Entry::new_with_modifiers(super::SERVICE, profile, &modifiers)
                .map_err(|error| StoreError::Refused(error.to_string()))
        }
    }

    impl Store for Native {
        fn availability(&self) -> Availability {
            Availability::Ready
        }

        fn load(&self, binding: &Binding) -> Result<Remembered, StoreError> {
            match self.entry(&binding.profile)?.get_secret() {
                Ok(blob) => Ok(interpret(&blob, binding)),
                Err(KeyringError::NoEntry) => Ok(Remembered::Missing),
                Err(error) => Err(StoreError::Unreachable(error.to_string())),
            }
        }

        fn save(&self, binding: &Binding, password: &str) -> Result<(), StoreError> {
            let blob = compose(binding, password)?;
            self.entry(&binding.profile)?
                .set_secret(&blob)
                .map_err(|error| StoreError::Refused(error.to_string()))
        }

        fn forget(&self, profile: &str) -> Result<(), StoreError> {
            match self.entry(profile)?.delete_credential() {
                Ok(()) | Err(KeyringError::NoEntry) => Ok(()),
                Err(error) => Err(StoreError::Refused(error.to_string())),
            }
        }

        fn holds(&self, profile: &str) -> Result<bool, StoreError> {
            match self.entry(profile)?.get_secret() {
                Ok(_) => Ok(true),
                Err(KeyringError::NoEntry) => Ok(false),
                Err(error) => Err(StoreError::Unreachable(error.to_string())),
            }
        }
    }
}

/// The store this build has, whatever it is.
///
/// Nothing above this function chooses a platform, and nothing above it needs
/// to know one was chosen. A store that cannot be reached is not an error to
/// report at startup — a machine with a credential store afterm cannot open is
/// a machine that does not save passwords, and the dialog says so when the
/// question comes up.
pub fn open() -> Vault {
    // Written as one binding per platform rather than an early return, so the
    // shape of the function is the same on every target and neither branch
    // depends on the other having been compiled away.
    #[cfg(windows)]
    let store: Arc<dyn Store> = match platform::Native::open() {
        Ok(store) => Arc::new(store),
        Err(error) => Arc::new(Absent::new(format!(
            "this machine's credential store could not be opened ({error})"
        ))),
    };
    #[cfg(not(windows))]
    let store: Arc<dyn Store> = Arc::new(Absent::new(
        "saving passwords needs an operating-system credential store, \
         which this build of afterm does not have",
    ));
    Vault::new(store)
}

// ------------------------------------------------------------------ tests

#[cfg(test)]
pub(crate) mod fake {
    use super::*;

    /// A store held in memory, and told to fail on demand.
    ///
    /// Every rule in this module is a rule about what happens around the
    /// store — bindings that no longer match, passwords the host refused,
    /// writes that did not land — and none of them needs a real credential
    /// store to exercise. This is what the connection path is driven against.
    #[derive(Default)]
    pub struct Fake {
        entries: Mutex<Vec<(String, Vec<u8>)>>,
        /// When set, every write fails with this reason.
        pub refuse_writes: Mutex<Option<String>>,
        pub unavailable: Mutex<Option<String>>,
    }

    impl Fake {
        pub fn stored(&self, profile: &str) -> Option<Vec<u8>> {
            self.entries
                .lock()
                .unwrap()
                .iter()
                .find(|(key, _)| key == profile)
                .map(|(_, blob)| blob.clone())
        }

        pub fn count(&self) -> usize {
            self.entries.lock().unwrap().len()
        }
    }

    impl Store for Fake {
        fn availability(&self) -> Availability {
            match self.unavailable.lock().unwrap().clone() {
                Some(reason) => Availability::Unavailable(reason),
                None => Availability::Ready,
            }
        }

        fn load(&self, binding: &Binding) -> Result<Remembered, StoreError> {
            Ok(match self.stored(&binding.profile) {
                Some(blob) => interpret(&blob, binding),
                None => Remembered::Missing,
            })
        }

        fn save(&self, binding: &Binding, password: &str) -> Result<(), StoreError> {
            if let Some(reason) = self.refuse_writes.lock().unwrap().clone() {
                return Err(StoreError::Refused(reason));
            }
            let blob = compose(binding, password)?;
            let mut entries = self.entries.lock().unwrap();
            entries.retain(|(key, _)| key != &binding.profile);
            entries.push((binding.profile.clone(), blob.to_vec()));
            Ok(())
        }

        fn forget(&self, profile: &str) -> Result<(), StoreError> {
            self.entries.lock().unwrap().retain(|(key, _)| key != profile);
            Ok(())
        }

        fn holds(&self, profile: &str) -> Result<bool, StoreError> {
            Ok(self.stored(profile).is_some())
        }
    }
}

/// The real credential store, exercised against real entries.
///
/// Everything in `tests` below proves the rules around the store, against one
/// held in memory. That is most of the feature and none of the platform: it
/// cannot say whether `CredWrite` works, whether a blob survives the round
/// trip, or whether this machine has a credential store reachable from a
/// process at all. This module is the part that only a real one can answer,
/// and it runs wherever the tests run on that platform — which for afterm is
/// the hosted Windows agent, the only Windows this project executes on.
///
/// Every entry it writes is uniquely named and removed again on the way out,
/// including when an assertion fails: a test that leaves credentials behind in
/// the store of whoever ran it is not a test worth having.
///
/// One thing it still cannot answer, because no single process can: whether
/// the credential is there after afterm closes and after a Windows login.
/// `CRED_PERSIST_LOCAL_MACHINE` is what asks for that, and walking it is in
/// `ROADMAP.md` as a manual check.
#[cfg(all(test, windows))]
mod real_store {
    use super::*;

    /// A name nothing else will collide with: this machine's clock and this
    /// process, so parallel tests and leftovers from an interrupted run cannot
    /// stand in for each other.
    fn disposable_profile() -> String {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|since| since.as_nanos())
            .unwrap_or(0);
        format!("afterm-test-{}-{nanos}", std::process::id())
    }

    /// Removes the entry however the test ends, panic included.
    struct Disposable<'a> {
        store: &'a dyn Store,
        profile: String,
    }

    impl Drop for Disposable<'_> {
        fn drop(&mut self) {
            let _ = self.store.forget(&self.profile);
        }
    }

    fn binding(profile: &str) -> Binding {
        Binding {
            profile: profile.to_string(),
            user: "deploy".into(),
            hostname: "build.example.com".into(),
            port: 22,
            fingerprint: "SHA256:aaaa".into(),
        }
    }

    /// Write a credential, read it back, prove the binding is enforced through
    /// a real round trip, and delete it.
    ///
    /// A store this machine cannot open is a failure here rather than a skip.
    /// The whole point of this test is to find out, and a skip that prints a
    /// note is a test that passes while proving nothing — which is how a
    /// feature ends up believed to work because the suite is green.
    #[test]
    fn a_credential_survives_a_round_trip_through_the_real_store() {
        let vault = open();
        let store = vault.store();
        assert_eq!(
            store.availability(),
            Availability::Ready,
            "this machine's credential store could not be opened, so nothing here was tested"
        );

        let profile = disposable_profile();
        let _cleanup = Disposable { store, profile: profile.clone() };
        let bound = binding(&profile);

        // Nothing there to start with, which is also a read of an absent entry
        // going through the store's own "no such credential" path.
        assert_eq!(store.load(&bound).unwrap(), Remembered::Missing);
        assert!(!store.holds(&profile).unwrap());

        store.save(&bound, "hunter2").expect("the store should take the credential");
        assert!(store.holds(&profile).unwrap(), "the credential was written and cannot be found");
        assert_eq!(
            store.load(&bound).unwrap(),
            Remembered::Found(Zeroizing::new("hunter2".into())),
            "what came back is not what went in"
        );

        // The binding, through the real store rather than through `interpret`
        // alone: the blob has been written to the operating system and read
        // back out, and the fingerprint in it still decides.
        let elsewhere = Binding { fingerprint: "SHA256:bbbb".into(), ..bound.clone() };
        assert_eq!(store.load(&elsewhere).unwrap(), Remembered::Elsewhere);

        // Replacing an entry rather than adding a second one under the same
        // name, which is what `save` on an existing credential has to mean.
        store.save(&bound, "correct-horse").expect("the store should replace the credential");
        assert_eq!(
            store.load(&bound).unwrap(),
            Remembered::Found(Zeroizing::new("correct-horse".into()))
        );

        store.forget(&profile).expect("the store should delete the credential");
        assert!(!store.holds(&profile).unwrap(), "the credential outlived being forgotten");
        assert_eq!(store.load(&bound).unwrap(), Remembered::Missing);
        // Deleting what is already gone is what the caller asked for, and the
        // real store must agree with the fake about that.
        store.forget(&profile).expect("forgetting twice is not a failure");
    }
}

#[cfg(test)]
mod tests {
    use super::fake::Fake;
    use super::*;

    fn binding() -> Binding {
        Binding {
            profile: "profile-1".into(),
            user: "deploy".into(),
            hostname: "build.example.com".into(),
            port: 22,
            fingerprint: "SHA256:aaaa".into(),
        }
    }

    fn vault() -> (Arc<Fake>, Vault) {
        let store = Arc::new(Fake::default());
        (Arc::clone(&store), Vault::new(store))
    }

    #[test]
    fn a_saved_password_comes_back_for_the_destination_it_was_saved_for() {
        let (_store, vault) = vault();
        vault.remember(&binding(), "hunter2").unwrap();
        assert_eq!(vault.remembered(&binding()).as_deref().map(String::as_str), Some("hunter2"));
    }

    #[test]
    fn nothing_comes_back_when_nothing_was_saved() {
        let (_store, vault) = vault();
        assert!(vault.remembered(&binding()).is_none());
    }

    /// Each of these is a different destination, and none of them is the one
    /// the password was saved for. A password is not sent to a host because
    /// the profile it was saved under happens to point there now.
    #[test]
    fn a_moved_destination_is_asked_about_again() {
        for changed in [
            Binding { user: "root".into(), ..binding() },
            Binding { hostname: "elsewhere.example.com".into(), ..binding() },
            Binding { port: 2222, ..binding() },
            Binding { fingerprint: "SHA256:bbbb".into(), ..binding() },
        ] {
            let (store, vault) = vault();
            vault.remember(&binding(), "hunter2").unwrap();
            assert!(vault.remembered(&changed).is_none(), "{changed:?} was offered a password");
            // Still there: it is correct for where it was saved, and only the
            // user may throw it away.
            assert!(store.holds("profile-1").unwrap());
        }
    }

    /// The rule that keeps four tabs on one host from walking a refused
    /// password past the server four times.
    #[test]
    fn a_refused_password_is_not_offered_again() {
        let (_store, vault) = vault();
        vault.remember(&binding(), "hunter2").unwrap();
        vault.reject("profile-1");
        assert!(vault.remembered(&binding()).is_none());
    }

    #[test]
    fn a_replacement_password_is_offered_again() {
        let (_store, vault) = vault();
        vault.remember(&binding(), "hunter2").unwrap();
        vault.reject("profile-1");
        vault.remember(&binding(), "correct-horse").unwrap();
        assert_eq!(
            vault.remembered(&binding()).as_deref().map(String::as_str),
            Some("correct-horse")
        );
    }

    #[test]
    fn forgetting_removes_the_entry() {
        let (store, vault) = vault();
        vault.remember(&binding(), "hunter2").unwrap();
        vault.forget("profile-1").unwrap();
        assert_eq!(store.count(), 0);
        assert!(!store.holds("profile-1").unwrap());
    }

    #[test]
    fn forgetting_what_was_never_saved_is_not_a_failure() {
        let (_store, vault) = vault();
        assert!(vault.forget("profile-1").is_ok());
    }

    /// Saving is offered only where both halves are in place: a store to write
    /// to, and a saved host to key the entry to. An ad-hoc connection has no
    /// stable identity, so it has nowhere to save.
    #[test]
    fn remembering_needs_a_store_and_a_saved_host() {
        let (store, vault) = vault();
        assert!(vault.may_remember(Some("profile-1")));
        assert!(!vault.may_remember(None));
        *store.unavailable.lock().unwrap() = Some("no store here".into());
        assert!(!vault.may_remember(Some("profile-1")));
    }

    #[test]
    fn a_store_that_holds_nothing_offers_nothing_and_refuses_writes() {
        let absent = Absent::new("nothing here");
        assert_eq!(absent.availability(), Availability::Unavailable("nothing here".into()));
        assert_eq!(absent.load(&binding()).unwrap(), Remembered::Missing);
        assert!(matches!(absent.save(&binding(), "hunter2"), Err(StoreError::Unsupported)));
        assert!(!absent.holds("profile-1").unwrap());
    }

    /// A blob from a future afterm is not read as one from this one.
    #[test]
    fn an_unknown_format_is_treated_as_nothing_saved() {
        let future = br#"{"version":99,"user":"deploy","hostname":"build.example.com","port":22,"fingerprint":"SHA256:aaaa","password":"hunter2"}"#;
        assert_eq!(interpret(future, &binding()), Remembered::Missing);
        assert_eq!(interpret(b"not json at all", &binding()), Remembered::Missing);
    }

    /// The blob is the binding and the secret together, which is what makes
    /// the binding worth checking.
    #[test]
    fn the_stored_blob_carries_its_binding() {
        let (store, vault) = vault();
        vault.remember(&binding(), "hunter2").unwrap();
        let blob = store.stored("profile-1").unwrap();
        let stored: Stored = serde_json::from_slice(&blob).unwrap();
        assert_eq!(stored.version, FORMAT);
        assert_eq!(stored.fingerprint, "SHA256:aaaa");
        assert_eq!(stored.hostname, "build.example.com");
    }
}
