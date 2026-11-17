// SPDX-License-Identifier: GPL-3.0-or-later
// PyO3 bindings for the trimmed MDM engine ported from Mister Download Manager (c) Ahmed
// Nefzaoui, https://github.com/anefzaoui/misterdownloadmanager, GPL-3.0-or-later.
// Written for paradl by Tawsif Rahman, 2026-11-17. No original MDM file is copied here;
// this is the new PyO3 surface described in the migration plan's Task 1 interface contract.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use mdm_engine::download::StartOptions;
use mdm_engine::{Download, Error, Hash, Snapshot, State};
use pyo3::create_exception;
use pyo3::exceptions::{PyException, PyRuntimeError, PyValueError};
use pyo3::prelude::*;
use pyo3::types::{PyDict, PyList};
use tokio::runtime::Runtime;
use url::Url;

create_exception!(paradl._rust_engine, DownloadError, PyException);
create_exception!(paradl._rust_engine, VerificationError, DownloadError);

fn py_error(e: Error) -> PyErr {
    match e {
        Error::ChecksumMismatch | Error::Verification(_) => {
            VerificationError::new_err(e.to_string())
        }
        _ => DownloadError::new_err(e.to_string()),
    }
}

/// Shared runtime for the module-level functions (S2 calls them without constructing an
/// engine). Kept separate from an explicit `RustDownloadEngine` instance's runtime.
static DEFAULT_RT: OnceLock<Arc<Runtime>> = OnceLock::new();

fn default_runtime() -> Arc<Runtime> {
    DEFAULT_RT
        .get_or_init(|| {
            Arc::new(
                tokio::runtime::Builder::new_multi_thread()
                    .worker_threads(8)
                    .thread_name("paradl-rust-engine")
                    .enable_all()
                    .build()
                    .expect("failed to build paradl Rust engine runtime"),
            )
        })
        .clone()
}

/// Defaults shared by module-level functions and explicit engine instances.
#[derive(Clone)]
struct EngineCfg {
    rt: Arc<Runtime>,
    max_connections: u32,
    rate_limit_bps: Option<u64>,
    user_agent: Option<String>,
}

impl EngineCfg {
    fn defaults() -> Self {
        EngineCfg {
            rt: default_runtime(),
            max_connections: 16,
            rate_limit_bps: None,
            user_agent: None,
        }
    }
}

/// Engine holder: owns the Tokio runtime so Python never creates one per download.
#[pyclass]
struct RustDownloadEngine {
    cfg: EngineCfg,
}

#[pymethods]
impl RustDownloadEngine {
    #[new]
    #[pyo3(signature = (max_connections=None, rate_limit_bps=None, global_rate_limit_bps=None, user_agent=None, **_kwargs))]
    fn new(
        max_connections: Option<u32>,
        rate_limit_bps: Option<u64>,
        global_rate_limit_bps: Option<u64>,
        user_agent: Option<String>,
        _kwargs: Option<HashMap<String, Py<PyAny>>>,
    ) -> PyResult<Self> {
        let max_connections = max_connections.unwrap_or(16).clamp(1, 16);
        let rt = Arc::new(
            tokio::runtime::Builder::new_multi_thread()
                .worker_threads(max_connections.clamp(1, 8) as usize)
                .thread_name("paradl-rust-engine")
                .enable_all()
                .build()
                .map_err(|e| PyRuntimeError::new_err(e.to_string()))?,
        );
        Ok(Self {
            cfg: EngineCfg {
                rt,
                max_connections,
                rate_limit_bps: rate_limit_bps.or(global_rate_limit_bps),
                user_agent,
            },
        })
    }

    #[pyo3(signature = (url, headers=None, **_kwargs))]
    fn probe_url(
        &self,
        py: Python<'_>,
        url: String,
        headers: Option<HashMap<String, String>>,
        _kwargs: Option<HashMap<String, Py<PyAny>>>,
    ) -> PyResult<Py<PyAny>> {
        do_probe(&self.cfg, py, url, headers)
    }

    #[allow(clippy::too_many_arguments)]
    #[pyo3(signature = (url, out_path=None, dest=None, filename=None, connections=None, headers=None, cookies=None, referer=None, user_agent=None, proxy=None, ip_family=None, mirrors=None, checksum=None, rate_limit_bps=None, accept_ranges=None, total=None, http3=None, doh_endpoint=None, **_kwargs))]
    fn start_download(
        &self,
        py: Python<'_>,
        url: String,
        out_path: Option<String>,
        dest: Option<String>,
        filename: Option<String>,
        connections: Option<usize>,
        headers: Option<HashMap<String, String>>,
        cookies: Option<String>,
        referer: Option<String>,
        user_agent: Option<String>,
        proxy: Option<String>,
        ip_family: Option<String>,
        mirrors: Option<Vec<String>>,
        checksum: Option<String>,
        rate_limit_bps: Option<u64>,
        accept_ranges: Option<Py<PyAny>>,
        total: Option<Py<PyAny>>,
        http3: Option<Py<PyAny>>,
        doh_endpoint: Option<Py<PyAny>>,
        _kwargs: Option<HashMap<String, Py<PyAny>>>,
    ) -> PyResult<DownloadHandle> {
        let _ = (accept_ranges, total, http3, doh_endpoint); // accepted for adapter parity
        do_start(
            &self.cfg,
            py,
            url,
            out_path,
            dest,
            filename,
            connections,
            headers,
            cookies,
            referer,
            user_agent,
            proxy,
            ip_family,
            mirrors,
            checksum,
            rate_limit_bps,
            false,
        )
    }

    #[allow(clippy::too_many_arguments)]
    #[pyo3(signature = (url, out_path=None, dest=None, filename=None, connections=None, headers=None, cookies=None, referer=None, user_agent=None, proxy=None, ip_family=None, mirrors=None, checksum=None, rate_limit_bps=None, accept_ranges=None, total=None, http3=None, doh_endpoint=None, **_kwargs))]
    fn resume_download(
        &self,
        py: Python<'_>,
        url: String,
        out_path: Option<String>,
        dest: Option<String>,
        filename: Option<String>,
        connections: Option<usize>,
        headers: Option<HashMap<String, String>>,
        cookies: Option<String>,
        referer: Option<String>,
        user_agent: Option<String>,
        proxy: Option<String>,
        ip_family: Option<String>,
        mirrors: Option<Vec<String>>,
        checksum: Option<String>,
        rate_limit_bps: Option<u64>,
        accept_ranges: Option<Py<PyAny>>,
        total: Option<Py<PyAny>>,
        http3: Option<Py<PyAny>>,
        doh_endpoint: Option<Py<PyAny>>,
        _kwargs: Option<HashMap<String, Py<PyAny>>>,
    ) -> PyResult<DownloadHandle> {
        let _ = (accept_ranges, total, http3, doh_endpoint);
        do_start(
            &self.cfg,
            py,
            url,
            out_path,
            dest,
            filename,
            connections,
            headers,
            cookies,
            referer,
            user_agent,
            proxy,
            ip_family,
            mirrors,
            checksum,
            rate_limit_bps,
            true,
        )
    }

    fn __repr__(&self) -> String {
        format!(
            "RustDownloadEngine(max_connections={}, rate_limit_bps={:?})",
            self.cfg.max_connections, self.cfg.rate_limit_bps
        )
    }
}

/// The adapter passes `cookie_file` (an on-disk cookie jar) through the `cookies` argument.
/// Returns the cookie header plus the EXACT domains it was captured for. The engine only
/// sends the jar to those hosts; a 302 to an unrelated host (e.g. 127.0.0.1 -> localhost)
/// never receives it.
fn resolve_cookies(value: Option<String>, host: Option<&str>) -> (Option<String>, Vec<String>) {
    let Some(value) = value else {
        return (None, Vec::new());
    };
    let path = PathBuf::from(&value);
    if !path.is_file() {
        // Raw header value: it belongs to the host being downloaded.
        return (Some(value), host.map(str::to_owned).into_iter().collect());
    }
    let Ok(text) = std::fs::read_to_string(path) else {
        return (None, Vec::new());
    };
    let mut pairs = Vec::new();
    let mut hosts = Vec::new();
    for line in text.lines() {
        if line.starts_with('#') || line.is_empty() {
            continue;
        }
        let fields: Vec<&str> = line.split('\t').collect();
        if fields.len() < 7 {
            continue;
        }
        // Keep the Netscape domain verbatim (including a leading dot for subdomain jars).
        let domain = fields[0].to_ascii_lowercase();
        let bare = domain.trim_start_matches('.');
        let matches_host = host.is_some_and(|h| {
            let h = h.to_ascii_lowercase();
            h == bare || h.ends_with(&format!(".{bare}"))
        });
        if matches_host {
            pairs.push(format!("{}={}", fields[5], fields[6]));
            if !hosts.contains(&domain) {
                hosts.push(domain);
            }
        }
    }
    if pairs.is_empty() {
        (None, Vec::new())
    } else {
        (Some(pairs.join("; ")), hosts)
    }
}

/// One-request URL look: final (post-redirect) URL, size, name, Range support.
fn do_probe(
    cfg: &EngineCfg,
    py: Python<'_>,
    url: String,
    headers: Option<HashMap<String, String>>,
) -> PyResult<Py<PyAny>> {
    let url = Url::parse(&url).map_err(|e| PyValueError::new_err(e.to_string()))?;
    let headers = mdm_engine::probe::headers_from_pairs(headers.as_ref());
    let info = py.detach(|| {
        let client = mdm_engine::probe::build_client(None, None, Duration::from_secs(15))
            .map_err(py_error)?;
        cfg.rt
            .block_on(mdm_engine::probe::probe(&client, url, headers, None))
            .map_err(py_error)
    })?;
    let d = PyDict::new(py);
    d.set_item("url", info.url.as_str())?;
    d.set_item("final_url", info.final_url.as_str())?;
    d.set_item("size", info.size)?;
    d.set_item("file_name", &info.file_name)?;
    d.set_item("filename", &info.file_name)?;
    d.set_item("resumable", info.resumable)?;
    d.set_item("accept_ranges", info.resumable)?;
    d.set_item("content_type", info.content_type)?;
    d.set_item("web_page", false)?;
    Ok(d.unbind().into())
}

#[allow(clippy::too_many_arguments)]
fn do_start(
    cfg: &EngineCfg,
    py: Python<'_>,
    url: String,
    out_path: Option<String>,
    dest: Option<String>,
    filename: Option<String>,
    connections: Option<usize>,
    headers: Option<HashMap<String, String>>,
    cookies: Option<String>,
    referer: Option<String>,
    user_agent: Option<String>,
    proxy: Option<String>,
    ip_family: Option<String>,
    mirrors: Option<Vec<String>>,
    checksum: Option<String>,
    rate_limit_bps: Option<u64>,
    resume: bool,
) -> PyResult<DownloadHandle> {
    let url = Url::parse(&url).map_err(|e| PyValueError::new_err(e.to_string()))?;

    // F3: the Python adapter passes a FULL output file path as the 2nd positional arg.
    // Treat it as the destination itself. Only when it names an existing directory (legacy
    // callers) do we append a file name. `dest` is accepted as an explicit alias.
    let raw = dest.or(out_path).ok_or_else(|| PyValueError::new_err("missing output path"))?;
    let mut dest_path = PathBuf::from(&raw);
    if dest_path.is_dir() {
        let name = filename.unwrap_or_else(|| {
            url.path_segments()
                .and_then(|mut s| s.next_back())
                .filter(|s| !s.is_empty())
                .map(str::to_owned)
                .unwrap_or_else(|| "download".to_owned())
        });
        dest_path = dest_path.join(name);
    }
    if let Some(parent) = dest_path.parent().filter(|p| !p.as_os_str().is_empty()) {
        std::fs::create_dir_all(parent).map_err(|e| PyRuntimeError::new_err(e.to_string()))?;
    }

    let mut hdrs = mdm_engine::probe::headers_from_pairs(headers.as_ref());
    if let Some(ua) = user_agent.or_else(|| cfg.user_agent.clone()) {
        hdrs.insert(
            "user-agent",
            ua.parse().map_err(|_| PyValueError::new_err("bad user_agent"))?,
        );
    }
    if let Some(r) = referer {
        hdrs.insert("referer", r.parse().map_err(|_| PyValueError::new_err("bad referer"))?);
    }
    // Cookies are deliberately NOT copied into the global header map (headers_from_pairs
    // also filters a "cookie" key): they travel in the host-scoped `cookie_header` field.

    let checksum = match checksum {
        Some(s) => Some(Hash::parse(&s).ok_or_else(|| PyValueError::new_err("invalid checksum"))?),
        None => None,
    };
    let mirrors = mirrors
        .unwrap_or_default()
        .into_iter()
        .map(|m| Url::parse(&m).map_err(|e| PyValueError::new_err(e.to_string())))
        .collect::<PyResult<Vec<_>>>()?;

    let (cookie_header, cookie_hosts) = resolve_cookies(cookies, url.host_str());
    let opts = StartOptions {
        url,
        mirrors,
        dest: dest_path,
        connections: connections.unwrap_or(cfg.max_connections as usize) as u32,
        headers: hdrs,
        checksum,
        rate_limit_bps: rate_limit_bps.or(cfg.rate_limit_bps),
        min_split: 64 * 1024,
        resume,
        proxy,
        ip_family,
        cookie_header,
        cookie_hosts,
    };

    let inner = py.detach(|| cfg.rt.block_on(Download::start(opts)).map_err(py_error))?;
    Ok(DownloadHandle {
        inner: Arc::new(inner),
        rt: cfg.rt.clone(),
    })
}

// ---------------------------------------------------------------------------
// Module-level API — S2 (_rust_engine.pyi, rust_engine.py) imports these directly.
// ---------------------------------------------------------------------------

#[pyfunction]
#[pyo3(signature = (url, headers=None, **_kwargs))]
fn probe_url(
    py: Python<'_>,
    url: String,
    headers: Option<HashMap<String, String>>,
    _kwargs: Option<HashMap<String, Py<PyAny>>>,
) -> PyResult<Py<PyAny>> {
    do_probe(&EngineCfg::defaults(), py, url, headers)
}

#[allow(clippy::too_many_arguments)]
#[pyfunction]
#[pyo3(signature = (url, out_path=None, dest=None, filename=None, connections=None, headers=None, cookies=None, referer=None, user_agent=None, proxy=None, ip_family=None, mirrors=None, checksum=None, rate_limit_bps=None, accept_ranges=None, total=None, http3=None, doh_endpoint=None, **_kwargs))]
fn start_download(
    py: Python<'_>,
    url: String,
    out_path: Option<String>,
    dest: Option<String>,
    filename: Option<String>,
    connections: Option<usize>,
    headers: Option<HashMap<String, String>>,
    cookies: Option<String>,
    referer: Option<String>,
    user_agent: Option<String>,
    proxy: Option<String>,
    ip_family: Option<String>,
    mirrors: Option<Vec<String>>,
    checksum: Option<String>,
    rate_limit_bps: Option<u64>,
    accept_ranges: Option<Py<PyAny>>,
    total: Option<Py<PyAny>>,
    http3: Option<Py<PyAny>>,
    doh_endpoint: Option<Py<PyAny>>,
    _kwargs: Option<HashMap<String, Py<PyAny>>>,
) -> PyResult<DownloadHandle> {
    let _ = (accept_ranges, total, http3, doh_endpoint);
    do_start(
        &EngineCfg::defaults(), py, url, out_path, dest, filename, connections, headers,
        cookies, referer, user_agent, proxy, ip_family, mirrors, checksum, rate_limit_bps, false,
    )
}

#[allow(clippy::too_many_arguments)]
#[pyfunction]
#[pyo3(signature = (url, out_path=None, dest=None, filename=None, connections=None, headers=None, cookies=None, referer=None, user_agent=None, proxy=None, ip_family=None, mirrors=None, checksum=None, rate_limit_bps=None, accept_ranges=None, total=None, http3=None, doh_endpoint=None, **_kwargs))]
fn resume_download(
    py: Python<'_>,
    url: String,
    out_path: Option<String>,
    dest: Option<String>,
    filename: Option<String>,
    connections: Option<usize>,
    headers: Option<HashMap<String, String>>,
    cookies: Option<String>,
    referer: Option<String>,
    user_agent: Option<String>,
    proxy: Option<String>,
    ip_family: Option<String>,
    mirrors: Option<Vec<String>>,
    checksum: Option<String>,
    rate_limit_bps: Option<u64>,
    accept_ranges: Option<Py<PyAny>>,
    total: Option<Py<PyAny>>,
    http3: Option<Py<PyAny>>,
    doh_endpoint: Option<Py<PyAny>>,
    _kwargs: Option<HashMap<String, Py<PyAny>>>,
) -> PyResult<DownloadHandle> {
    let _ = (accept_ranges, total, http3, doh_endpoint);
    do_start(
        &EngineCfg::defaults(), py, url, out_path, dest, filename, connections, headers,
        cookies, referer, user_agent, proxy, ip_family, mirrors, checksum, rate_limit_bps, true,
    )
}

#[pyfunction]
fn is_available() -> bool {
    true
}

#[pyfunction]
fn version() -> &'static str {
    "0.1.0"
}

/// A running download. Every potentially long call detaches from the GIL.
#[pyclass]
struct DownloadHandle {
    inner: Arc<Download>,
    rt: Arc<Runtime>,
}

#[pymethods]
impl DownloadHandle {
    /// `{downloaded, total, speed_bps, connections, state}` — exactly the Task 1 contract.
    fn poll_snapshot(&self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        let snap: Snapshot = self.inner.poll_snapshot();
        let d = PyDict::new(py);
        d.set_item("downloaded", snap.downloaded)?;
        d.set_item("total", snap.total)?;
        d.set_item("speed_bps", snap.speed_bps)?;
        d.set_item("connections", snap.connections)?;
        d.set_item("state", snap.state.as_str())?;
        Ok(d.unbind().into())
    }

    /// In-process socket recycle: committed offsets and the URI pool are preserved.
    /// A completed/cancelled download is a no-op — recycling it would turn a
    /// successful transfer into "download cancelled" (exit 130).
    fn recycle_connections(&self, py: Python<'_>) -> PyResult<()> {
        let snapshot = self.inner.poll_snapshot();
        if matches!(snapshot.state, State::Done | State::Failed | State::Cancelled) {
            return Ok(());
        }
        py.detach(|| {
            self.rt
                .block_on(self.inner.recycle_connections())
                .map_err(py_error)
        })
    }

    fn pause(&self, py: Python<'_>) -> PyResult<()> {
        py.detach(|| self.inner.pause().map_err(py_error))
    }

    fn resume(&self, py: Python<'_>) -> PyResult<()> {
        py.detach(|| self.inner.resume().map_err(py_error))
    }

    fn cancel(&self, py: Python<'_>) -> PyResult<()> {
        py.detach(|| self.inner.cancel().map_err(py_error))
    }

    /// Wait for completion; returns a result dict for the Python adapter to map.
    /// Accepts the adapter's optional `timeout=` argument (seconds).
    #[pyo3(signature = (timeout=None))]
    fn wait(&self, py: Python<'_>, timeout: Option<f64>) -> PyResult<Py<PyAny>> {
        let result = py.detach(|| {
            let fut = self.inner.wait();
            match timeout {
                Some(secs) if secs > 0.0 => self
                    .rt
                    .block_on(async {
                        tokio::time::timeout(Duration::from_secs_f64(secs), fut).await
                    })
                    .map_err(|_| DownloadError::new_err("wait timed out"))
                    .and_then(|r| r.map_err(py_error)),
                _ => self.rt.block_on(fut).map_err(py_error),
            }
        })?;
        let d = PyDict::new(py);
        d.set_item("success", result.success)?;
        d.set_item("error", result.error)?;
        d.set_item(
            "path",
            result.path.as_ref().map(|p| p.to_string_lossy().into_owned()),
        )?;
        d.set_item("downloaded", result.downloaded)?;
        d.set_item("total", result.total)?;
        Ok(d.unbind().into())
    }

    /// The stable URI pool (final URL first). Exposed for diagnostics / parity tests.
    fn uri_pool(&self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        let urls = self.inner.uri_pool();
        let list = PyList::new(py, urls)?;
        Ok(list.unbind().into())
    }

    fn __repr__(&self) -> String {
        let s = self.inner.poll_snapshot();
        format!(
            "DownloadHandle(state={}, downloaded={}, connections={})",
            s.state.as_str(),
            s.downloaded,
            s.connections
        )
    }
}

#[pymodule]
fn _rust_engine(m: &Bound<'_, PyModule>) -> PyResult<()> {
    // Module-level API required by S2's shim / adapter.
    m.add_function(wrap_pyfunction!(probe_url, m)?)?;
    m.add_function(wrap_pyfunction!(start_download, m)?)?;
    m.add_function(wrap_pyfunction!(resume_download, m)?)?;
    m.add_function(wrap_pyfunction!(is_available, m)?)?;
    m.add_function(wrap_pyfunction!(version, m)?)?;
    // Backward-compatible classes.
    m.add_class::<RustDownloadEngine>()?;
    m.add_class::<DownloadHandle>()?;
    m.add("DownloadError", m.py().get_type::<DownloadError>())?;
    m.add("VerificationError", m.py().get_type::<VerificationError>())?;
    m.add("__version__", "0.1.0")?;
    for (k, v) in [
        ("QUEUED", State::Queued.as_str()),
        ("DOWNLOADING", State::Downloading.as_str()),
        ("PAUSED", State::Paused.as_str()),
        ("VERIFYING", State::Verifying.as_str()),
        ("DONE", State::Done.as_str()),
        ("FAILED", State::Failed.as_str()),
        ("CANCELLED", State::Cancelled.as_str()),
    ] {
        m.add(k, v)?;
    }
    Ok(())
}
