//! Options → Proxy / Socks → Network adapter (#35): the chosen adapter, watched. Once a second
//! a thread of its own reads the computer's adapters and works out the route connections take:
//! through the chosen adapter while it has an address, none at all while it doesn't (a VPN
//! switched off), and the system's choice when no adapter is chosen. The binding itself is the
//! kill switch (`mdm_engine::net`); this watch is what stops the downloads at once and starts
//! them again when the adapter is back (`App::route_changed`).

use std::sync::{Arc, Mutex};
use std::time::Duration;

use mdm_engine::Route;

/// How often the adapters are read (a few milliseconds each time).
const EVERY: Duration = Duration::from_secs(1);

#[derive(Clone, Default)]
pub struct NetWatch {
    shared: Arc<Mutex<Shared>>,
}

#[derive(Default)]
struct Shared {
    /// The adapter chosen (Settings::net_adapter); "" none.
    wanted: String,
    route: Route,
    /// Woken when the route changes.
    ctx: Option<egui::Context>,
    started: bool,
}

impl NetWatch {
    /// The route for `wanted` now, the watch started if needed (and told what is wanted). The
    /// first answer is worked out at once, so nothing starts on a stale route.
    pub fn route(&self, wanted: &str, ctx: &egui::Context) -> Route {
        let mut s = self.shared.lock().unwrap();
        if s.wanted != wanted || !s.started {
            s.wanted = wanted.to_owned();
            s.route = compute(wanted);
        }
        if s.ctx.is_none() {
            s.ctx = Some(ctx.clone());
        }
        if !s.started {
            s.started = true;
            let shared = self.shared.clone();
            let _ = std::thread::Builder::new()
                .name("netwatch".into())
                .spawn(move || watch(shared));
        }
        s.route.clone()
    }
}

fn compute(wanted: &str) -> Route {
    if wanted.trim().is_empty() {
        return Route::Any;
    }
    #[allow(unused_mut)]
    let mut route = mdm_engine::net::route_for(wanted, &adapters());
    // The test adapter (debug builds, below) is on loopback, where the lab server is too:
    // connections to this computer are bound to it as well, so the lab sees them come from the
    // adapter's address (#35, the helpers' relay; unbound, nothing would show it).
    #[cfg(debug_assertions)]
    if let Route::Bound(b) = &mut route
        && test_adapter().is_some_and(|(name, _)| name == b.name)
    {
        std::sync::Arc::make_mut(b).loopback_too = true;
    }
    route
}

/// Debug builds: the test adapter `MDM_TEST_ADAPTER` names (see [`adapters`]), as (name, address).
#[cfg(debug_assertions)]
fn test_adapter() -> Option<(String, std::net::IpAddr)> {
    std::env::var("MDM_TEST_ADAPTER")
        .ok()
        .and_then(|v| match v.strip_prefix('@') {
            Some(file) => std::fs::read_to_string(file).ok(),
            None => Some(v),
        })
        .and_then(|v| {
            let (n, a) = v.trim().split_once('=')?;
            Some((n.trim().to_owned(), a.trim().parse().ok()?))
        })
}

/// The computer's network adapters, as Options lists them and the watch reads them. Debug
/// builds only: MDM_TEST_ADAPTER="Lab adapter=127.0.0.2" adds an adapter with that address (one
/// on loopback, which the system's list leaves out), for tests by hand that must not reach the
/// network or listen beyond this computer (#143); "@file" reads that line from a file each
/// time, so deleting the file disconnects the adapter.
pub fn adapters() -> Vec<mdm_engine::net::Adapter> {
    #[allow(unused_mut)]
    let mut all = mdm_engine::net::adapters();
    #[cfg(debug_assertions)]
    if let Some((name, ip)) = test_adapter() {
        let (mut v4, mut v6) = (Vec::new(), Vec::new());
        match ip {
            std::net::IpAddr::V4(a) => v4.push(a),
            std::net::IpAddr::V6(a) => v6.push(a),
        }
        all.push(mdm_engine::net::Adapter {
            name,
            index: 0,
            v4,
            v6,
            local_only: Vec::new(),
        });
    }
    all
}

fn watch(shared: Arc<Mutex<Shared>>) {
    loop {
        std::thread::sleep(EVERY);
        let wanted = shared.lock().unwrap().wanted.clone();
        let route = compute(&wanted);
        let mut s = shared.lock().unwrap();
        if s.wanted == wanted && s.route != route {
            tracing::info!(
                "network adapter: {}",
                match &route {
                    Route::Any => "any".to_owned(),
                    Route::Down { label } => format!("{label} down"),
                    Route::Bound(b) => format!("{} {:?} {:?}", b.label, b.v4, b.v6),
                }
            );
            s.route = route;
            if let Some(ctx) = &s.ctx {
                ctx.request_repaint();
            }
        }
    }
}
