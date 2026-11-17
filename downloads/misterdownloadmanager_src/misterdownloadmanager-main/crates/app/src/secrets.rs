//! Sign-in secrets in the system's keychain (#44): the debrid accounts' credentials, Sites
//! Logins' passwords and the proxy's password. The settings keep a reference in a secret's
//! place, `keychain:<service>/<scope>/<key>`, and the secret lives in Windows Credential
//! Manager (win32.rs), the Secret Service keyring on Linux or the macOS keychain (unix.rs).
//!
//! - In memory a secret is the secret itself (`settings::sealed` opens a reference when the
//!   settings are read), so nothing that reads the settings changes.
//! - Writes happen off the window's thread (the worker here). Until the keychain confirmed one
//!   (written, then read back the same), the settings keep that secret as before (sealed with
//!   DPAPI on Windows, as it is in the owner-only data folder elsewhere): a secret is always in
//!   one place at least. Once confirmed, the next save writes the reference.
//! - A reference that can't be read (a locked keyring, an entry removed meanwhile) is written
//!   back as it was, so the secret comes back once the keyring opens again.
//! - Entries this data folder no longer refers to (an account removed) are deleted after the
//!   list without them is on the disk.
//! - Where no keychain answers (no Secret Service running, a locked keyring: never unlocked
//!   from here, that would prompt) secrets stay in the settings. A portable copy writes nothing
//!   outside its folder: it keeps them there too.
//!
//! Nothing here writes a secret to the log: only keys, references and error codes.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Condvar, Mutex, OnceLock};

#[cfg(unix)]
use crate::unix as os;
#[cfg(windows)]
use crate::win32 as os;

/// The keychain's name for MDM's entries (Credential Manager's targets start with it; Secret
/// Service's `service` attribute; the macOS keychain's service).
pub const SERVICE: &str = "Mister Download Manager";
/// What the settings hold in a secret's place.
pub const REF: &str = "keychain:";

/// Where the secrets are kept, for Options → Accounts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kept {
    /// Not known yet: the first look at the keychain is under way.
    Looking,
    /// In the system's keychain.
    Keychain,
    /// A portable copy: in its own settings.
    Portable,
    /// The keyring is locked (MDM never unlocks it): in the settings.
    Locked,
    /// No keychain answered: in the settings.
    Missing,
}

/// Why the keychain didn't do something. Never holds a secret: an error code at most.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Fault {
    /// No keychain here (no Secret Service running, no session).
    Missing,
    /// It is there but locked; opening it would prompt the user.
    Locked,
    /// Anything else, as the system said it (a code).
    Failed(String),
}

impl std::fmt::Display for Fault {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Fault::Missing => f.write_str("no keychain"),
            Fault::Locked => f.write_str("the keychain is locked"),
            Fault::Failed(why) => f.write_str(why),
        }
    }
}

/// A keychain: entries by service and account, each with a label for the system's own UI.
pub trait Store: Send + Sync {
    /// Whether it can be used now, without asking the user anything.
    fn probe(&self, service: &str) -> Result<(), Fault>;
    /// None: no such entry.
    fn read(&self, service: &str, account: &str) -> Result<Option<String>, Fault>;
    /// Create or replace.
    fn write(&self, service: &str, account: &str, label: &str, secret: &str) -> Result<(), Fault>;
    /// Deleting what isn't there is no fault.
    fn delete(&self, service: &str, account: &str) -> Result<(), Fault>;
    /// The accounts of this service's entries (only these: never another program's).
    fn list(&self, service: &str) -> Result<Vec<String>, Fault>;
}

/// The system's keychain (win32.rs, unix.rs).
pub struct System;

impl Store for System {
    fn probe(&self, service: &str) -> Result<(), Fault> {
        os::keychain_probe(service)
    }
    fn read(&self, service: &str, account: &str) -> Result<Option<String>, Fault> {
        os::keychain_read(service, account)
    }
    fn write(&self, service: &str, account: &str, label: &str, secret: &str) -> Result<(), Fault> {
        os::keychain_write(service, account, label, secret)
    }
    fn delete(&self, service: &str, account: &str) -> Result<(), Fault> {
        os::keychain_delete(service, account)
    }
    fn list(&self, service: &str) -> Result<Vec<String>, Fault> {
        os::keychain_list(service)
    }
}

/// A secret's place in the keychain.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct Target {
    service: String,
    /// `<scope>/<key>`.
    account: String,
}

impl Target {
    fn reference(&self) -> String {
        format!("{REF}{}/{}", self.service, self.account)
    }

    fn parse(stored: &str) -> Option<Target> {
        let (service, account) = stored.strip_prefix(REF)?.split_once('/')?;
        (!service.is_empty() && account.contains('/')).then(|| Target {
            service: service.to_owned(),
            account: account.to_owned(),
        })
    }

    /// The setting it holds (`debrid/3`), whichever data folder wrote it.
    fn key(&self) -> &str {
        self.account
            .split_once('/')
            .map_or(&*self.account, |(_, key)| key)
    }
}

#[derive(Default)]
struct State {
    kept: Option<Kept>,
    /// What the keychain holds, as read or written and read back: the reference can be saved.
    held: HashMap<Target, String>,
    /// References that couldn't be read, by key: written back as they were.
    unread: HashMap<String, String>,
    /// Writes for the worker: the newest secret for each place, and its label.
    queued: HashMap<Target, (String, String)>,
    /// Writes that failed this session: not tried again for the same secret.
    failed: HashMap<Target, String>,
    /// The places the newest settings use (written or waiting): never deleted.
    wanted: HashSet<Target>,
    /// `wanted` as it was at the last sweep.
    swept: Option<HashSet<Target>>,
    sweep_due: bool,
    /// Writes confirmed so far: the app saves again when it grows.
    confirmed: u64,
    worker: bool,
}

/// The settings' secrets and the keychain, for one data folder.
pub struct Vault {
    store: Box<dyn Store>,
    service: String,
    /// Keeps one data folder's entries apart from another's.
    scope: String,
    /// False for a portable copy: nothing written outside its folder.
    writes: bool,
    /// The worker runs on a thread of its own (tests run it by hand).
    background: bool,
    state: Mutex<State>,
    wake: Condvar,
}

impl Vault {
    pub fn new(
        store: Box<dyn Store>,
        service: &str,
        scope: &str,
        writes: bool,
        background: bool,
    ) -> Vault {
        Vault {
            store,
            service: service.to_owned(),
            scope: scope.to_owned(),
            writes,
            background,
            state: Mutex::default(),
            wake: Condvar::new(),
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn target(&self, key: &str) -> Target {
        Target {
            service: self.service.clone(),
            account: format!("{}/{key}", self.scope),
        }
    }

    /// A reference read from the settings: its secret, or "" when it can't be read now (the
    /// reference is then kept for the next save). The list is read before the window opens:
    /// once the keychain didn't answer (none, or none within the Secret Service's 5 s), the
    /// other references aren't asked this session, so MDM waits for it once, not once each.
    pub fn open(&self, stored: &str) -> String {
        let Some(target) = Target::parse(stored) else {
            tracing::warn!("sign-ins: a keychain reference that doesn't read");
            return String::new();
        };
        let outcome = if self.lock().kept == Some(Kept::Missing) {
            Err(Fault::Missing)
        } else {
            self.store.read(&target.service, &target.account)
        };
        if outcome == Err(Fault::Missing) {
            self.lock().kept.get_or_insert(Kept::Missing);
        }
        match outcome {
            Ok(Some(secret)) => {
                let mut s = self.lock();
                s.unread.remove(target.key());
                s.held.insert(target, secret.clone());
                secret
            }
            outcome => {
                let why = match outcome {
                    Err(f) => f.to_string(),
                    Ok(_) => "no such entry".to_owned(),
                };
                tracing::warn!(
                    "sign-ins: {} can't be read from the keychain ({why}); its reference is kept",
                    target.key()
                );
                let key = target.key().to_owned();
                self.lock().unread.insert(key, stored.to_owned());
                String::new()
            }
        }
    }

    /// A save's look at the secrets: [`Pass::secret`] for each, then [`Pass::finish`].
    pub fn pass(&self) -> Pass<'_> {
        Pass {
            vault: self,
            wanted: HashSet::new(),
            work: false,
        }
    }

    /// The list was written: entries it no longer refers to can go.
    pub fn after_write(&self) {
        if !self.writes {
            return;
        }
        let mut s = self.lock();
        if s.swept.as_ref() != Some(&s.wanted) {
            s.sweep_due = true;
            drop(s);
            self.start();
        }
    }

    /// Where the secrets are kept (the first call starts looking).
    pub fn kept(&self) -> Kept {
        if !self.writes {
            return Kept::Portable;
        }
        let kept = self.lock().kept;
        kept.unwrap_or_else(|| {
            self.start();
            Kept::Looking
        })
    }

    /// The key's reference couldn't be read this session (a locked keyring).
    pub fn unread(&self, key: &str) -> bool {
        self.lock().unread.contains_key(key)
    }

    /// Writes confirmed so far.
    pub fn confirmed(&self) -> u64 {
        self.lock().confirmed
    }

    /// Work still waiting for the worker.
    pub fn pending(&self) -> bool {
        let s = self.lock();
        !s.queued.is_empty() || s.sweep_due
    }

    /// Wake the worker, starting it the first time.
    fn start(&self) {
        let mut s = self.lock();
        if self.background && !s.worker {
            s.worker = true;
            drop(s);
            let spawned = std::thread::Builder::new()
                .name("sign-ins".into())
                .spawn(|| {
                    let v = global();
                    loop {
                        v.wait();
                        v.run_once();
                    }
                });
            if spawned.is_err() {
                self.lock().worker = false;
            }
            return;
        }
        drop(s);
        self.wake.notify_all();
    }

    /// Until there is something to do.
    fn wait(&self) {
        let mut s = self.lock();
        while s.kept.is_some() && s.queued.is_empty() && !s.sweep_due {
            s = self.wake.wait(s).unwrap_or_else(|e| e.into_inner());
        }
    }

    /// The worker's round: look at the keychain once, then the waiting writes, then the sweep.
    pub fn run_once(&self) {
        if self.lock().kept.is_none() {
            let kept = match self.store.probe(&self.service) {
                Ok(()) => {
                    tracing::info!("sign-ins: kept in the system's keychain");
                    Kept::Keychain
                }
                Err(Fault::Locked) => {
                    tracing::info!("sign-ins: the keychain is locked; kept in the settings");
                    Kept::Locked
                }
                Err(f) => {
                    tracing::info!("sign-ins: no keychain ({f}); kept in the settings");
                    Kept::Missing
                }
            };
            self.lock().kept = Some(kept);
        }
        let usable = self.lock().kept == Some(Kept::Keychain);
        let queued: Vec<_> = self.lock().queued.drain().collect();
        for (target, (label, secret)) in queued {
            if !usable {
                self.lock().failed.insert(target, secret);
                continue;
            }
            let written = self
                .store
                .write(&target.service, &target.account, &label, &secret)
                .and_then(
                    |()| match self.store.read(&target.service, &target.account)? {
                        Some(back) if back == secret => Ok(()),
                        _ => Err(Fault::Failed("it reads back otherwise".into())),
                    },
                );
            let mut s = self.lock();
            match written {
                Ok(()) => {
                    tracing::info!("sign-ins: {} kept in the keychain", target.key());
                    s.failed.remove(&target);
                    s.held.insert(target, secret);
                    s.confirmed += 1;
                }
                Err(f) => {
                    tracing::warn!(
                        "sign-ins: {} couldn't be written to the keychain ({f}); kept in the settings",
                        target.key()
                    );
                    if f == Fault::Locked {
                        s.kept = Some(Kept::Locked);
                    }
                    s.failed.insert(target, secret);
                }
            }
        }
        let sweep = {
            let mut s = self.lock();
            std::mem::take(&mut s.sweep_due).then(|| s.wanted.clone())
        };
        if let Some(wanted) = sweep
            && usable
        {
            self.sweep(&wanted);
            self.lock().swept = Some(wanted);
        }
    }

    /// Delete this data folder's entries that `wanted` doesn't name.
    fn sweep(&self, wanted: &HashSet<Target>) {
        let prefix = format!("{}/", self.scope);
        let listed = match self.store.list(&self.service) {
            Ok(l) => l,
            Err(f) => {
                tracing::warn!("sign-ins: the keychain's entries can't be listed ({f})");
                return;
            }
        };
        for account in listed.into_iter().filter(|a| a.starts_with(&prefix)) {
            let target = Target {
                service: self.service.clone(),
                account,
            };
            if wanted.contains(&target) {
                continue;
            }
            match self.store.delete(&target.service, &target.account) {
                Ok(()) => {
                    tracing::info!("sign-ins: {} removed from the keychain", target.key());
                    self.lock().held.remove(&target);
                }
                Err(f) => tracing::warn!(
                    "sign-ins: {} couldn't be removed from the keychain ({f})",
                    target.key()
                ),
            }
        }
    }

    /// Every entry of the service, whichever data folder wrote it: how many went.
    pub fn forget_all(&self) -> Result<usize, Fault> {
        let mut n = 0;
        for account in self.store.list(&self.service)? {
            self.store.delete(&self.service, &account)?;
            n += 1;
        }
        Ok(n)
    }
}

/// One save's secrets.
pub struct Pass<'a> {
    vault: &'a Vault,
    wanted: HashSet<Target>,
    work: bool,
}

impl Pass<'_> {
    /// What the settings write for the secret `key` (`debrid/3`): its reference once the
    /// keychain holds it, else None (the caller seals it as before) and the keychain is asked
    /// to take it. `label` names it in the system's own UI.
    pub fn secret(
        &mut self,
        key: &str,
        label: &dyn Fn() -> String,
        secret: &str,
    ) -> Option<String> {
        let v = self.vault;
        let mut s = v.lock();
        if secret.is_empty() {
            // Unread: the reference stays, and the secret comes back once the keyring opens.
            let kept = s.unread.get(key).cloned()?;
            if let Some(t) = Target::parse(&kept) {
                self.wanted.insert(t);
            }
            return Some(kept);
        }
        // A portable copy keeps them in its settings (a reference read from elsewhere moves
        // there; its entry stays where it is).
        if !v.writes {
            return None;
        }
        // This folder's own place (a reference from another folder's settings, copied here,
        // moves to it).
        let target = v.target(key);
        self.wanted.insert(target.clone());
        if s.held.get(&target).is_some_and(|h| h == secret) {
            return Some(target.reference());
        }
        let failed = s.failed.get(&target).is_some_and(|f| f == secret);
        let refused = matches!(s.kept, Some(Kept::Locked | Kept::Missing));
        if !failed && !refused {
            s.queued.insert(target, (label(), secret.to_owned()));
            self.work = true;
        }
        None
    }

    /// The places these settings use: kept; the others may go after the next write.
    pub fn finish(self) {
        let v = self.vault;
        v.lock().wanted = self.wanted;
        if self.work {
            v.start();
        }
    }
}

/// Keeps one data folder's entries apart from another's: a short hash of its path, so a test
/// copy beside the real data never writes over the real entries.
fn scope() -> String {
    let path = mdm_ipc::data_dir()
        .map(|d| d.to_string_lossy().into_owned())
        .unwrap_or_default();
    let path = if cfg!(windows) {
        path.to_lowercase()
    } else {
        path
    };
    let h = crate::settings::fnv(&path);
    format!("{:08x}", (h ^ (h >> 32)) as u32)
}

/// The service's name: `SERVICE`, or a test's own (`MDM_KEYCHAIN_SERVICE=MDM-test-…`; only names
/// of that form, so a test never writes MDM's real entries).
fn service() -> String {
    std::env::var("MDM_KEYCHAIN_SERVICE")
        .ok()
        .filter(|s| s.starts_with("MDM-test-") && !s.contains('/'))
        .unwrap_or_else(|| SERVICE.to_owned())
}

fn global() -> &'static Arc<Vault> {
    static VAULT: OnceLock<Arc<Vault>> = OnceLock::new();
    VAULT.get_or_init(|| {
        // Unit tests never touch the system's keychain through the settings.
        #[cfg(test)]
        let store: Box<dyn Store> = Box::new(Arc::new(Memory::default()));
        #[cfg(not(test))]
        let store: Box<dyn Store> = Box::new(System);
        let portable =
            std::env::var_os("MDM_DATA_DIR").is_none() && mdm_ipc::portable_dir().is_some();
        Arc::new(Vault::new(
            store,
            &service(),
            &scope(),
            !portable,
            !cfg!(test),
        ))
    })
}

#[cfg(test)]
thread_local! {
    static TEST_VAULT: std::cell::RefCell<Option<Arc<Vault>>> = const { std::cell::RefCell::new(None) };
}

/// The vault the settings use (in a test, the one it set on its thread).
pub fn vault() -> Arc<Vault> {
    #[cfg(test)]
    if let Some(v) = TEST_VAULT.with(|t| t.borrow().clone()) {
        return v;
    }
    global().clone()
}

/// Run `f` with `v` as this thread's vault.
#[cfg(test)]
pub fn with_vault<R>(v: &Arc<Vault>, f: impl FnOnce() -> R) -> R {
    let before = TEST_VAULT.with(|t| t.replace(Some(v.clone())));
    let r = f();
    TEST_VAULT.with(|t| *t.borrow_mut() = before);
    r
}

thread_local! {
    static PREPARED: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Serialize settings made ready for the disk ([`crate::settings::Settings::for_disk`]): their
/// secret fields already hold what is written (a reference or a sealed secret) and go as they
/// are. Settings read inside it are read as the disk has them, the same way: nothing is opened
/// nor asked of the keychain (a check that a list reads).
pub fn prepared<R>(f: impl FnOnce() -> R) -> R {
    struct Reset;
    impl Drop for Reset {
        fn drop(&mut self) {
            PREPARED.with(|p| p.set(false));
        }
    }
    PREPARED.with(|p| p.set(true));
    let _reset = Reset;
    f()
}

pub fn is_prepared() -> bool {
    PREPARED.with(std::cell::Cell::get)
}

/// `--forget-sign-ins` (the Windows uninstaller runs it when the user chose to remove the data
/// too): every entry of MDM's service goes, whichever data folder wrote it. Nothing else is
/// touched.
pub fn forget_all() -> std::process::ExitCode {
    match Vault::new(Box::new(System), &service(), "", true, false).forget_all() {
        Ok(n) => {
            println!("{n} sign-in(s) removed from the keychain");
            std::process::ExitCode::SUCCESS
        }
        // Nothing to remove.
        Err(Fault::Missing) => std::process::ExitCode::SUCCESS,
        Err(f) => {
            eprintln!("Error: {f}");
            std::process::ExitCode::FAILURE
        }
    }
}

/// A keychain in memory, for tests.
#[cfg(test)]
#[derive(Default)]
pub struct Memory {
    pub entries: Mutex<HashMap<(String, String), (String, String)>>,
    /// Every call fails so.
    pub fault: Mutex<Option<Fault>>,
}

#[cfg(test)]
impl Memory {
    fn check(&self) -> Result<(), Fault> {
        self.fault.lock().unwrap().clone().map_or(Ok(()), Err)
    }
}

/// A keychain that takes `delay` to answer each call, and answers as `inner` does: one that
/// hangs (a stopped gnome-keyring) until the Secret Service's timeout, for tests.
#[cfg(test)]
pub struct Slow {
    pub inner: Arc<Memory>,
    pub delay: std::time::Duration,
    pub calls: std::sync::atomic::AtomicU32,
}

#[cfg(test)]
impl Slow {
    fn call(&self) {
        self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        std::thread::sleep(self.delay);
    }
}

#[cfg(test)]
impl Store for Arc<Slow> {
    fn probe(&self, service: &str) -> Result<(), Fault> {
        self.call();
        self.inner.probe(service)
    }
    fn read(&self, service: &str, account: &str) -> Result<Option<String>, Fault> {
        self.call();
        self.inner.read(service, account)
    }
    fn write(&self, service: &str, account: &str, label: &str, secret: &str) -> Result<(), Fault> {
        self.call();
        self.inner.write(service, account, label, secret)
    }
    fn delete(&self, service: &str, account: &str) -> Result<(), Fault> {
        self.call();
        self.inner.delete(service, account)
    }
    fn list(&self, service: &str) -> Result<Vec<String>, Fault> {
        self.call();
        self.inner.list(service)
    }
}

#[cfg(test)]
impl Store for Arc<Memory> {
    fn probe(&self, _: &str) -> Result<(), Fault> {
        self.check()
    }
    fn read(&self, service: &str, account: &str) -> Result<Option<String>, Fault> {
        self.check()?;
        let e = self.entries.lock().unwrap();
        Ok(e.get(&(service.into(), account.into()))
            .map(|(_, s)| s.clone()))
    }
    fn write(&self, service: &str, account: &str, label: &str, secret: &str) -> Result<(), Fault> {
        self.check()?;
        let mut e = self.entries.lock().unwrap();
        e.insert(
            (service.into(), account.into()),
            (label.into(), secret.into()),
        );
        Ok(())
    }
    fn delete(&self, service: &str, account: &str) -> Result<(), Fault> {
        self.check()?;
        self.entries
            .lock()
            .unwrap()
            .remove(&(service.into(), account.into()));
        Ok(())
    }
    fn list(&self, service: &str) -> Result<Vec<String>, Fault> {
        self.check()?;
        let e = self.entries.lock().unwrap();
        Ok(e.keys()
            .filter(|(s, _)| s == service)
            .map(|(_, a)| a.clone())
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::{DebridAccount, Settings};

    const PROXY: &str = "proxy-s3cret";
    const TOKEN: &str = "rd-s3cret";
    const SITE: &str = "site-s3cret";

    fn vault_on(mem: &Arc<Memory>, scope: &str) -> Arc<Vault> {
        Arc::new(Vault::new(
            Box::new(mem.clone()),
            "MDM-test-unit",
            scope,
            true,
            false,
        ))
    }

    fn sample() -> Settings {
        Settings {
            proxy_host: "10.0.0.1".into(),
            proxy_port: "3128".into(),
            proxy_pass: PROXY.into(),
            debrid_accounts: vec![DebridAccount {
                id: 1,
                user: "me".into(),
                secret: format!(r#"{{"kind":"token","token":"{TOKEN}"}}"#),
                ..Default::default()
            }],
            site_logins: vec![("example.com".into(), "me".into(), SITE.into())],
            ..Default::default()
        }
    }

    /// The settings as the list file writes them.
    fn on_disk(s: &Settings) -> String {
        let disk = s.for_disk();
        prepared(|| serde_json::to_string(&disk).unwrap())
    }

    fn read(json: &str) -> Settings {
        serde_json::from_str(json).unwrap()
    }

    #[test]
    fn secrets_from_before_move_to_the_keychain_and_read_back() {
        let mem = Arc::new(Memory::default());
        let v = vault_on(&mem, "aaaa0001");
        with_vault(&v, || {
            // A list from before: sealed with DPAPI on Windows, as they are elsewhere.
            let before = serde_json::to_string(&sample()).unwrap();
            assert!(!before.contains(REF));
            let loaded = read(&before);
            assert_eq!(loaded, sample());
            // Not confirmed yet: the list keeps them as before (nothing lost if MDM ends now).
            let first = on_disk(&loaded);
            assert!(!first.contains(REF));
            assert_eq!(read(&first), sample());
            assert!(v.pending());
            v.run_once();
            assert_eq!(v.confirmed(), 3);
            assert_eq!(mem.entries.lock().unwrap().len(), 3);
            // Now references only.
            let second = on_disk(&loaded);
            for secret in [PROXY, TOKEN, SITE] {
                assert!(!second.contains(secret), "{second}");
            }
            assert_eq!(
                second.matches("keychain:MDM-test-unit/aaaa0001/").count(),
                3,
                "{second}"
            );
            assert!(second.contains("keychain:MDM-test-unit/aaaa0001/debrid/1"));
            assert!(!v.pending());
            // The next session reads them back from the keychain.
            let next = vault_on(&mem, "aaaa0001");
            let again = with_vault(&next, || read(&second));
            assert_eq!(again, sample());
            assert!(again.debrid_accounts[0].account().is_some());
            // And writes the same references without asking the keychain again.
            let third = with_vault(&next, || on_disk(&again));
            assert_eq!(third, second);
            assert!(!next.pending());
            // The labels name them in the system's UI.
            let labels: Vec<String> = mem
                .entries
                .lock()
                .unwrap()
                .values()
                .map(|(l, _)| l.clone())
                .collect();
            assert!(labels.iter().any(|l| l.contains("Real-Debrid (me)")));
            assert!(labels.iter().any(|l| l.contains("example.com (me)")));
            assert!(labels.iter().any(|l| l.contains("10.0.0.1:3128")));
        });
    }

    #[test]
    fn a_keychain_that_fails_keeps_the_secrets_in_the_settings() {
        // None at all (no Secret Service).
        let mem = Arc::new(Memory::default());
        *mem.fault.lock().unwrap() = Some(Fault::Missing);
        let v = vault_on(&mem, "aaaa0002");
        with_vault(&v, || {
            let _ = on_disk(&sample());
            v.run_once();
            assert_eq!(v.kept(), Kept::Missing);
            let json = on_disk(&sample());
            assert!(!json.contains(REF));
            assert_eq!(read(&json), sample());
            // Not asked again and again.
            assert!(!v.pending());
        });
        // There, but every write fails: never a reference to what isn't written.
        let mem = Arc::new(Memory::default());
        let v = vault_on(&mem, "aaaa0003");
        with_vault(&v, || {
            v.run_once();
            assert_eq!(v.kept(), Kept::Keychain);
            *mem.fault.lock().unwrap() = Some(Fault::Failed("Windows error 5".into()));
            let _ = on_disk(&sample());
            v.run_once();
            let json = on_disk(&sample());
            assert!(!json.contains(REF));
            assert_eq!(read(&json), sample());
            assert!(!v.pending());
            // A new secret is tried (the old one failed, not this one).
            let mut changed = sample();
            changed.proxy_pass = "another".into();
            let _ = on_disk(&changed);
            assert!(v.pending());
        });
    }

    #[test]
    fn a_reference_that_cant_be_read_is_written_back_as_it_was() {
        let mem = Arc::new(Memory::default());
        let v = vault_on(&mem, "aaaa0004");
        let saved = with_vault(&v, || {
            let _ = on_disk(&sample());
            v.run_once();
            on_disk(&sample())
        });
        assert!(saved.contains(REF));
        // The next session: the keyring is locked.
        *mem.fault.lock().unwrap() = Some(Fault::Locked);
        let next = vault_on(&mem, "aaaa0004");
        with_vault(&next, || {
            let loaded = read(&saved);
            assert!(loaded.proxy_pass.is_empty());
            assert!(loaded.debrid_accounts[0].account().is_none());
            assert!(next.unread("debrid/1"));
            next.run_once();
            assert_eq!(next.kept(), Kept::Locked);
            // Saved again: the same references, and the entries stay.
            let json = on_disk(&loaded);
            assert_eq!(json, saved);
            next.after_write();
            next.run_once();
        });
        *mem.fault.lock().unwrap() = None;
        assert_eq!(mem.entries.lock().unwrap().len(), 3);
        // Unlocked later: back.
        let later = vault_on(&mem, "aaaa0004");
        assert_eq!(with_vault(&later, || read(&saved)), sample());
    }

    #[test]
    fn entries_no_longer_named_go_after_the_write_and_only_this_folders() {
        let mem = Arc::new(Memory::default());
        {
            let mut e = mem.entries.lock().unwrap();
            // Another data folder's (a test copy), and another service's.
            e.insert(
                ("MDM-test-unit".into(), "bbbb0001/debrid/1".into()),
                ("x".into(), "other".into()),
            );
            e.insert(
                ("MDM-test-other".into(), "aaaa0005/debrid/1".into()),
                ("x".into(), "other".into()),
            );
        }
        let v = vault_on(&mem, "aaaa0005");
        with_vault(&v, || {
            let _ = on_disk(&sample());
            v.run_once();
            let _ = on_disk(&sample());
            v.after_write();
            v.run_once();
            assert_eq!(mem.entries.lock().unwrap().len(), 5);
            // The account removed: its entry stays until the list without it is written.
            let mut less = sample();
            less.debrid_accounts.clear();
            let json = on_disk(&less);
            assert!(!json.contains("debrid/1"));
            v.run_once();
            assert_eq!(mem.entries.lock().unwrap().len(), 5);
            v.after_write();
            v.run_once();
            let e = mem.entries.lock().unwrap();
            assert_eq!(e.len(), 4);
            assert!(!e.contains_key(&("MDM-test-unit".into(), "aaaa0005/debrid/1".into())));
            assert!(e.contains_key(&("MDM-test-unit".into(), "bbbb0001/debrid/1".into())));
            assert!(e.contains_key(&("MDM-test-other".into(), "aaaa0005/debrid/1".into())));
        });
    }

    #[test]
    fn a_copied_data_folder_moves_the_secrets_to_its_own_entries() {
        let mem = Arc::new(Memory::default());
        let v = vault_on(&mem, "aaaa0006");
        let saved = with_vault(&v, || {
            let _ = on_disk(&sample());
            v.run_once();
            on_disk(&sample())
        });
        let copy = vault_on(&mem, "cccc0006");
        with_vault(&copy, || {
            let loaded = read(&saved);
            assert_eq!(loaded, sample());
            let _ = on_disk(&loaded);
            copy.run_once();
            let json = on_disk(&loaded);
            assert_eq!(json.matches("keychain:MDM-test-unit/cccc0006/").count(), 3);
            copy.after_write();
            copy.run_once();
        });
        // The original's entries are untouched.
        assert_eq!(mem.entries.lock().unwrap().len(), 6);
    }

    #[test]
    fn a_portable_copy_keeps_its_secrets_in_its_settings() {
        let mem = Arc::new(Memory::default());
        let v = Arc::new(Vault::new(
            Box::new(mem.clone()),
            "MDM-test-unit",
            "aaaa0007",
            false,
            false,
        ));
        with_vault(&v, || {
            assert_eq!(v.kept(), Kept::Portable);
            let json = on_disk(&sample());
            assert!(!json.contains(REF));
            assert!(!v.pending());
            v.after_write();
            v.run_once();
        });
        assert!(mem.entries.lock().unwrap().is_empty());
    }

    #[test]
    fn a_plain_secret_that_looks_like_a_reference_stays_itself() {
        let mem = Arc::new(Memory::default());
        *mem.fault.lock().unwrap() = Some(Fault::Missing);
        let v = vault_on(&mem, "aaaa0008");
        with_vault(&v, || {
            v.run_once();
            for odd in [
                "keychain:MDM-test-unit/aaaa0008/proxy",
                "plain:keychain:x/y/z",
                "plain:x",
            ] {
                let s = Settings {
                    proxy_pass: odd.into(),
                    ..Default::default()
                };
                assert_eq!(read(&on_disk(&s)).proxy_pass, odd);
                assert_eq!(read(&serde_json::to_string(&s).unwrap()).proxy_pass, odd);
            }
        });
    }

    /// The keychain hangs at start (a stopped gnome-keyring: each call ends at the Secret
    /// Service's timeout): the list's references wait for it once, not once each, and the
    /// secrets come back once it answers again.
    #[test]
    fn a_keychain_that_doesnt_answer_is_waited_for_once() {
        use std::sync::atomic::Ordering;
        let mem = Arc::new(Memory::default());
        let first = vault_on(&mem, "aaaa0011");
        let saved = with_vault(&first, || {
            let _ = on_disk(&sample());
            first.run_once();
            on_disk(&sample())
        });
        assert_eq!(saved.matches(REF).count(), 3);
        *mem.fault.lock().unwrap() = Some(Fault::Missing);
        let slow = Arc::new(Slow {
            inner: mem.clone(),
            delay: std::time::Duration::from_millis(400),
            calls: Default::default(),
        });
        let next = Arc::new(Vault::new(
            Box::new(slow.clone()),
            "MDM-test-unit",
            "aaaa0011",
            true,
            false,
        ));
        let start = std::time::Instant::now();
        let loaded = with_vault(&next, || read(&saved));
        let took = start.elapsed();
        assert_eq!(slow.calls.load(Ordering::SeqCst), 1, "asked once");
        assert!(took < std::time::Duration::from_millis(1000), "{took:?}");
        assert!(loaded.proxy_pass.is_empty());
        assert_eq!(next.kept(), Kept::Missing);
        // Saved meanwhile: the references as they were, nothing asked of the keychain.
        assert_eq!(with_vault(&next, || on_disk(&loaded)), saved);
        next.run_once();
        assert_eq!(slow.calls.load(Ordering::SeqCst), 1);
        // The next start, with a keychain that answers: back.
        *mem.fault.lock().unwrap() = None;
        assert_eq!(
            with_vault(&vault_on(&mem, "aaaa0011"), || read(&saved)),
            sample()
        );
    }

    /// Checking that the list on the disk reads (before a save keeps it as a generation) opens
    /// none of its secrets: no keychain call on every save, nor on the window's last save.
    #[test]
    fn checking_a_list_asks_no_keychain() {
        use std::sync::atomic::Ordering;
        let mem = Arc::new(Memory::default());
        let slow = Arc::new(Slow {
            inner: mem.clone(),
            delay: std::time::Duration::ZERO,
            calls: Default::default(),
        });
        let v = Arc::new(Vault::new(
            Box::new(slow.clone()),
            "MDM-test-unit",
            "aaaa0012",
            true,
            false,
        ));
        with_vault(&v, || {
            let _ = on_disk(&sample());
            v.run_once();
            let saved = on_disk(&sample());
            assert!(saved.contains(REF));
            let before = slow.calls.load(Ordering::SeqCst);
            let checked: Settings = prepared(|| serde_json::from_str(&saved).unwrap());
            assert_eq!(slow.calls.load(Ordering::SeqCst), before);
            // As on the disk: the references themselves.
            assert!(checked.proxy_pass.starts_with(REF));
        });
    }

    /// Everything the log gets while secrets go every way: none of them.
    #[test]
    fn no_secret_reaches_the_log() {
        #[derive(Clone, Default)]
        struct Log(Arc<Mutex<Vec<u8>>>);
        impl std::io::Write for Log {
            fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
                self.0.lock().unwrap().extend_from_slice(b);
                Ok(b.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        let log = Log::default();
        let writer = log.clone();
        let subscriber = tracing_subscriber::fmt()
            .with_max_level(tracing::Level::TRACE)
            .with_ansi(false)
            .with_writer(move || writer.clone())
            .finish();
        tracing::subscriber::with_default(subscriber, || {
            let mem = Arc::new(Memory::default());
            let v = vault_on(&mem, "aaaa0009");
            with_vault(&v, || {
                let _ = on_disk(&sample());
                v.run_once();
                let saved = on_disk(&sample());
                let mut less = sample();
                less.debrid_accounts.clear();
                let _ = on_disk(&less);
                v.after_write();
                v.run_once();
                // Locked, then failing writes, then none at all.
                *mem.fault.lock().unwrap() = Some(Fault::Locked);
                let next = vault_on(&mem, "aaaa0009");
                with_vault(&next, || {
                    let loaded = read(&saved);
                    let _ = on_disk(&loaded);
                    next.run_once();
                });
                *mem.fault.lock().unwrap() = Some(Fault::Failed("Windows error 1312".into()));
                let other = vault_on(&mem, "aaaa0010");
                with_vault(&other, || {
                    let _ = on_disk(&sample());
                    other.run_once();
                });
                let _ = read(r#"{"proxy_pass":"keychain:broken"}"#);
            });
        });
        let text = String::from_utf8(log.0.lock().unwrap().clone()).unwrap();
        assert!(text.contains("sign-ins:"), "the log was caught: {text}");
        for secret in [PROXY, TOKEN, SITE] {
            assert!(!text.contains(secret), "{secret} in the log: {text}");
        }
    }

    /// The system's own keychain (Credential Manager, the Secret Service, the macOS keychain),
    /// under a service of its own (`MDM-test-<random>`): only its entries are listed, and every
    /// one is deleted again. Skipped where there is none (a CI runner without a keyring).
    #[test]
    fn the_system_keychain_keeps_and_forgets() {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos()) as u64
            ^ u64::from(std::process::id());
        let service = format!(
            "MDM-test-{:08x}",
            crate::settings::fnv(&nonce.to_string()) as u32
        );
        let sys = System;
        if let Err(f) = sys.probe(&service) {
            eprintln!("no system keychain here ({f}): skipped");
            return;
        }
        /// Every entry of the test's service goes, whatever happens.
        struct Clean(String);
        impl Drop for Clean {
            fn drop(&mut self) {
                if let Ok(left) = System.list(&self.0) {
                    for a in left {
                        let _ = System.delete(&self.0, &a);
                    }
                }
            }
        }
        let _clean = Clean(service.clone());
        assert_eq!(sys.list(&service).unwrap(), Vec::<String>::new());
        let v = Arc::new(Vault::new(
            Box::new(System),
            &service,
            "unit0001",
            true,
            false,
        ));
        let secret = format!("s3cret-{nonce:x}");
        let s = Settings {
            proxy_pass: secret.clone(),
            ..Default::default()
        };
        let saved = with_vault(&v, || {
            let _ = on_disk(&s);
            v.run_once();
            assert_eq!(v.kept(), Kept::Keychain);
            assert_eq!(v.confirmed(), 1);
            on_disk(&s)
        });
        assert!(saved.contains(&format!("keychain:{service}/unit0001/proxy")));
        assert!(!saved.contains(&secret));
        assert_eq!(
            sys.list(&service).unwrap(),
            vec!["unit0001/proxy".to_owned()]
        );
        let next = Arc::new(Vault::new(
            Box::new(System),
            &service,
            "unit0001",
            true,
            false,
        ));
        assert_eq!(with_vault(&next, || read(&saved)).proxy_pass, secret);
        // Cleared: the entry goes once the list without it is written.
        with_vault(&next, || {
            let _ = on_disk(&Settings::default());
            next.after_write();
            next.run_once();
        });
        assert_eq!(sys.list(&service).unwrap(), Vec::<String>::new());
        assert_eq!(sys.read(&service, "unit0001/proxy").unwrap(), None);
    }
}
