//! Mister Download Manager's remote control (#141): a phone or another computer on the local
//! network adds downloads and torrents, sees the list and controls it, from a small web page
//! MDM serves over HTTPS. Off by default (Options → Remote).
//!
//! * One command path: every request becomes an [`mdm_ipc::Command`] marked `remote`, handed to
//!   the app through its command channel ([`mdm_ipc::Sink`]) and answered by the app as the
//!   automation API is: downloads added this way get the categories, queues, limits, adapter
//!   and kill switch of any other.
//! * Paired devices only: a code shown in MDM pairs a device ([`pairing`]), which gets a token of
//!   its own ([`devices`]), removable in Options.
//! * Local network only: one listener per local network address ([`addrs`]), never all at once.
//! * HTTPS with a certificate made once and kept ([`tls`]); plain `http://` is redirected.
//!
//! The threats and how each is handled: docs/remote-control.md#security.

pub mod addrs;
pub mod devices;
mod http;
pub mod pairing;
pub mod tls;

use std::net::{IpAddr, SocketAddr};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;

pub use devices::Device;

/// Where and whether to listen.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Config {
    pub on: bool,
    /// 0: any free port (tests).
    pub port: u16,
    /// "" every local network address; an address; an adapter's name ([`addrs::chosen`]).
    pub listen: String,
    /// Names besides the addresses a request may be sent to (`Host`): a server's name in DNS,
    /// a reverse proxy's.
    pub hosts: Vec<String>,
}

/// The default port.
pub const PORT: u16 = 8463;

impl Default for Config {
    fn default() -> Self {
        Config {
            on: false,
            port: PORT,
            listen: String::new(),
            hosts: Vec::new(),
        }
    }
}

/// What the remote control is doing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Status {
    Off,
    /// Listening at these addresses.
    Listening(Vec<SocketAddr>),
    /// On, but no address to listen on (no local network now, or the address chosen is gone);
    /// it starts when one comes.
    NoAddress,
    /// The port is taken by another program.
    PortInUse(u16),
    /// Anything else (the certificate could not be made…).
    Failed(String),
}

/// The page's texts in one language: `texts` maps each English text to its translation.
#[derive(Clone, Debug, Default, serde::Serialize)]
pub struct PageTexts {
    pub lang: String,
    pub rtl: bool,
    pub texts: std::collections::BTreeMap<String, String>,
}

/// The app's translations for the page: given the browser's languages (`de-DE,de,en`), the
/// texts in the best one MDM has.
pub type Texts = Arc<dyn Fn(&str) -> PageTexts + Send + Sync>;

/// The remote control: one per app, cheap to clone.
#[derive(Clone)]
pub struct Remote {
    inner: Arc<Inner>,
}

struct Inner {
    rt: tokio::runtime::Handle,
    /// `remote/` in the data folder: certificate, paired devices.
    dir: PathBuf,
    sink: mdm_ipc::Sink,
    texts: Texts,
    state: Mutex<State>,
    devices: Mutex<devices::Devices>,
    pairing: Mutex<pairing::Pairing>,
}

#[derive(Default)]
struct State {
    config: Config,
    status: Option<Status>,
    /// The addresses listened on, and their tasks.
    ips: Vec<IpAddr>,
    tasks: Vec<tokio::task::JoinHandle<()>>,
    fingerprint: Option<String>,
    /// The address watch (every local network address: they come and go).
    watch: Option<tokio::task::JoinHandle<()>>,
    /// Counts the restarts: a connection still open from before one is refused.
    generation: u64,
}

impl Remote {
    /// The remote control of the app whose commands go to `sink`, keeping its files in `dir`;
    /// off until [`Remote::apply`] turns it on.
    pub fn new(
        rt: tokio::runtime::Handle,
        dir: PathBuf,
        sink: mdm_ipc::Sink,
        texts: Texts,
    ) -> Remote {
        let devices = devices::Devices::load(Some(dir.join("devices.json")));
        Remote {
            inner: Arc::new(Inner {
                rt,
                dir,
                sink,
                texts,
                state: Mutex::default(),
                devices: Mutex::new(devices),
                pairing: Mutex::default(),
            }),
        }
    }

    /// Listen as `config` says (started, stopped or moved when it changed).
    pub fn apply(&self, config: Config) {
        let mut st = self.inner.state.lock().unwrap();
        if st.config == config && st.status.is_some() {
            return;
        }
        st.config = config;
        Inner::restart(&self.inner, &mut st);
    }

    pub fn status(&self) -> Status {
        self.inner
            .state
            .lock()
            .unwrap()
            .status
            .clone()
            .unwrap_or(Status::Off)
    }

    /// The page's addresses (`https://192.168.1.20:8463/`) while it listens, the one a phone
    /// most likely reaches first ([`addrs::rank`]).
    pub fn urls(&self) -> Vec<String> {
        match self.status() {
            Status::Listening(mut addrs) => {
                addrs.sort_by_key(|a| addrs::rank(a.ip()));
                addrs.iter().map(|a| addrs::url(a.ip(), a.port())).collect()
            }
            _ => Vec::new(),
        }
    }

    /// The certificate's SHA-256 fingerprint, once it is made.
    pub fn fingerprint(&self) -> Option<String> {
        self.inner.state.lock().unwrap().fingerprint.clone()
    }

    pub fn devices(&self) -> Vec<Device> {
        self.inner.devices.lock().unwrap().list()
    }

    /// Unpair device `id`: refused from its next request on.
    pub fn remove(&self, id: &str) {
        self.inner.devices.lock().unwrap().remove(id);
    }

    /// Show a new pairing code (Pair a device).
    pub fn start_pairing(&self) -> String {
        self.inner.pairing.lock().unwrap().start()
    }

    /// The pairing window closed.
    pub fn stop_pairing(&self) {
        self.inner.pairing.lock().unwrap().stop();
    }

    /// The code shown and its time left; the device it paired, once it has.
    pub fn pairing(&self) -> (Option<(String, Duration)>, Option<String>) {
        let p = self.inner.pairing.lock().unwrap();
        (p.shown(), p.paired())
    }

    /// Stop listening (the app quits).
    pub fn stop(&self) {
        let mut st = self.inner.state.lock().unwrap();
        st.config.on = false;
        Inner::restart(&self.inner, &mut st);
    }
}

impl Inner {
    /// Stop what listens, then listen again as `st.config` says.
    fn restart(this: &Arc<Inner>, st: &mut State) {
        for t in st.tasks.drain(..) {
            t.abort();
        }
        if let Some(w) = st.watch.take() {
            w.abort();
        }
        st.ips.clear();
        st.generation += 1;
        if !st.config.on {
            st.status = Some(Status::Off);
            return;
        }
        let adapters = mdm_engine::net::adapters();
        let ips = addrs::chosen(&st.config.listen, &adapters);
        // Every local network address: watched, as they come and go (Wi-Fi joined, DHCP).
        if st.config.listen.trim().is_empty() {
            let weak = Arc::downgrade(this);
            st.watch = Some(this.rt.spawn(watch(weak)));
        }
        if ips.is_empty() {
            st.status = Some(Status::NoAddress);
            return;
        }
        // The certificate names the computer, `localhost` and the addresses found the first time.
        let mut names = vec!["localhost".to_owned()];
        names.extend(addrs::host_name());
        names.extend(ips.iter().map(IpAddr::to_string));
        let identity = match tls::identity(&this.dir, &names) {
            Ok(i) => i,
            Err(e) => {
                tracing::warn!("remote control: no certificate: {e}");
                st.status = Some(Status::Failed(e));
                return;
            }
        };
        st.fingerprint = Some(identity.fingerprint.clone());
        let mut bound = Vec::new();
        let mut failure = None;
        for ip in &ips {
            match std::net::TcpListener::bind((*ip, st.config.port)) {
                Ok(l) => bound.push(l),
                Err(e) if e.kind() == std::io::ErrorKind::AddrInUse => {
                    failure = Some(Status::PortInUse(st.config.port));
                }
                Err(e) => {
                    failure.get_or_insert(Status::Failed(e.to_string()));
                }
            }
        }
        let mut addrs = Vec::new();
        for l in bound {
            let Ok(addr) = l.local_addr() else { continue };
            if l.set_nonblocking(true).is_err() {
                continue;
            }
            let state = http::State::new(this.clone(), st.config.hosts.clone(), st.generation);
            let tls = identity.config.clone();
            let task = this.rt.spawn(async move {
                let Ok(tcp) = tokio::net::TcpListener::from_std(l) else {
                    return;
                };
                let hosts = state.hosts.clone();
                let host_ok: Arc<dyn Fn(&str) -> bool + Send + Sync> =
                    Arc::new(move |h: &str| addrs::host_allowed(h, &hosts));
                let Ok(listener) = tls::Listener::new(tcp, tls, host_ok) else {
                    return;
                };
                let app = http::router(state).into_make_service_with_connect_info::<http::Peer>();
                let _ = axum::serve(listener, app).await;
            });
            st.tasks.push(task);
            addrs.push(addr);
        }
        tracing::info!("remote control: listening on {addrs:?}");
        st.ips = ips;
        st.status = Some(match (addrs.is_empty(), failure) {
            (true, Some(f)) => f,
            (true, None) => Status::NoAddress,
            (false, _) => Status::Listening(addrs),
        });
    }
}

/// Every 10 s: listen again when the local network addresses changed.
async fn watch(weak: Weak<Inner>) {
    loop {
        tokio::time::sleep(Duration::from_secs(10)).await;
        let Some(this) = weak.upgrade() else { return };
        let ips = tokio::task::spawn_blocking(|| addrs::chosen("", &mdm_engine::net::adapters()))
            .await
            .unwrap_or_default();
        let mut st = this.state.lock().unwrap();
        if st.config.on && st.config.listen.trim().is_empty() && ips != st.ips {
            // The watch is restarted with the rest: this task ends here.
            st.watch = None;
            Inner::restart(&this, &mut st);
            return;
        }
    }
}
