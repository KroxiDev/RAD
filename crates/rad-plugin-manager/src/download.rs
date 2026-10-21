#[derive(Clone, Default, Debug)]
pub struct Cancellation(std::sync::Arc<std::sync::atomic::AtomicBool>);
impl Cancellation {
    pub fn cancel(&self) {
        self.0.store(true, std::sync::atomic::Ordering::Release);
    }
    pub fn is_cancelled(&self) -> bool {
        self.0.load(std::sync::atomic::Ordering::Acquire)
    }
}
/// How much of the package has arrived, in whole percent, for whoever shows it; `notify`
/// hears of each new percent. Unknown until the release says the package's size.
#[derive(Clone, Default)]
pub struct Progress(std::sync::Arc<ProgressState>);
#[derive(Default)]
struct ProgressState {
    /// `u8::MAX` while unknown.
    percent: std::sync::atomic::AtomicU8,
    known: std::sync::atomic::AtomicBool,
    notify: Option<Box<dyn Fn() + Send + Sync>>,
}
impl Progress {
    pub fn new(notify: impl Fn() + Send + Sync + 'static) -> Self {
        Self(std::sync::Arc::new(ProgressState {
            notify: Some(Box::new(notify)),
            ..Default::default()
        }))
    }
    pub fn percent(&self) -> Option<u8> {
        use std::sync::atomic::Ordering;
        self.0
            .known
            .load(Ordering::Acquire)
            .then(|| self.0.percent.load(Ordering::Acquire))
    }
    /// `received` of `total` bytes; a total of zero says nothing.
    pub fn report(&self, received: u64, total: u64) {
        use std::sync::atomic::Ordering;
        if total == 0 {
            return;
        }
        let percent = (received.min(total) * 100 / total) as u8;
        let was_known = self.0.known.swap(true, Ordering::AcqRel);
        let changed = self.0.percent.swap(percent, Ordering::AcqRel) != percent || !was_known;
        if changed && let Some(notify) = &self.0.notify {
            notify();
        }
    }
}
use crate::{Error, GithubSource};
#[derive(Default)]
pub struct Downloader;
impl Downloader {
    pub fn new() -> Self {
        Self
    }
    pub fn download(
        &self,
        source: &GithubSource,
        target: &str,
        cancel: Cancellation,
        progress: Progress,
    ) -> Result<Vec<u8>, Error> {
        let source = source.clone();
        let target = target.to_owned();
        let timeout = std::time::Duration::from_secs(600);
        let deadline = std::time::Instant::now() + timeout;
        let worker_cancel = cancel.clone();
        bounded_job_controlled(timeout, &cancel, move || {
            let mut hops = 0;
            let mut transport = Https;
            let metadata = fetch_controlled(
                &mut transport,
                &source.metadata_url(),
                1048576,
                std::time::Instant::now() + std::time::Duration::from_secs(15),
                &mut hops,
                &worker_cancel,
                None,
            )?;
            let (url, size) = asset_for_target(&metadata, &target)?;
            progress.report(0, size);
            let bytes = fetch_controlled(
                &mut transport,
                &url,
                67174420,
                deadline,
                &mut hops,
                &worker_cancel,
                Some((&progress, size)),
            )?;
            rad_plugin_package::Candidate::parse(&bytes, rad_plugin_contract::HOST_API, &target)
                .map_err(|_| Error::InvalidPackage)?;
            if worker_cancel.is_cancelled() {
                return Err(Error::Cancelled);
            }
            if std::time::Instant::now() >= deadline {
                return Err(Error::Deadline);
            }
            Ok(bytes)
        })
    }
}
#[cfg(test)]
fn asset(bytes: &[u8]) -> Result<String, Error> {
    asset_for_target(bytes, "x86_64-unknown-linux-gnu").map(|(url, _)| url)
}
/// The package for `target` and its size as the release declares it (zero when it does not).
fn asset_for_target(bytes: &[u8], target: &str) -> Result<(String, u64), Error> {
    if bytes.len() > 1048576 {
        return Err(Error::LimitExceeded);
    }
    #[derive(serde::Deserialize)]
    struct Release {
        assets: Vec<Asset>,
    }
    #[derive(serde::Deserialize)]
    struct Asset {
        name: String,
        browser_download_url: String,
        #[serde(default)]
        size: u64,
    }
    let release: Release = serde_json::from_slice(bytes).map_err(|_| Error::InvalidPackage)?;
    if release.assets.len() > 128 {
        return Err(Error::LimitExceeded);
    }
    let mut matches = release
        .assets
        .iter()
        // `rad-plugin-<target>.radp`, or `<id>-<version>-<target>.radp` as the official
        // releases publish it: exactly one package for this target.
        .filter(|a| a.name.ends_with(&format!("-{target}.radp")));
    let found = matches.next().ok_or(Error::AmbiguousAsset)?;
    if matches.next().is_some() {
        return Err(Error::AmbiguousAsset);
    }
    let url = found.browser_download_url.as_str();
    if url.len() > 8192 {
        return Err(Error::LimitExceeded);
    }
    Ok((url.into(), found.size))
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn release_asset_selection_is_exact_and_unambiguous() {
        let valid=br#"{"assets":[{"name":"rad-plugin-x86_64-unknown-linux-gnu.radp","browser_download_url":"https://github.com/o/r/releases/download/v1/a"}]}"#;
        assert_eq!(
            asset(valid).unwrap(),
            "https://github.com/o/r/releases/download/v1/a"
        );
        assert_eq!(asset(br#"{"assets":[]}"#), Err(Error::AmbiguousAsset));
        let duplicate=br#"{"assets":[{"name":"rad-plugin-x86_64-unknown-linux-gnu.radp","browser_download_url":"https://github.com/a"},{"name":"rad-plugin-x86_64-unknown-linux-gnu.radp","browser_download_url":"https://github.com/b"}]}"#;
        assert_eq!(asset(duplicate), Err(Error::AmbiguousAsset));
        // The official releases name the package after the plugin and its version.
        let published=br#"{"assets":[{"name":"manifest.json","browser_download_url":"https://github.com/m"},{"name":"org.rad.audio-0.2.0-x86_64-unknown-linux-gnu.radp","browser_download_url":"https://github.com/p"},{"name":"org.rad.audio-0.2.0-aarch64-unknown-linux-gnu.radp","browser_download_url":"https://github.com/q"}]}"#;
        assert_eq!(asset(published).unwrap(), "https://github.com/p");
        let two=br#"{"assets":[{"name":"rad-plugin-x86_64-unknown-linux-gnu.radp","browser_download_url":"https://github.com/a"},{"name":"org.rad.audio-0.2.0-x86_64-unknown-linux-gnu.radp","browser_download_url":"https://github.com/b"}]}"#;
        assert_eq!(
            asset(two),
            Err(Error::AmbiguousAsset),
            "two packages for the target"
        );
        let glued=br#"{"assets":[{"name":"plugin_x86_64-unknown-linux-gnu.radp","browser_download_url":"https://github.com/a"}]}"#;
        assert_eq!(
            asset(glued),
            Err(Error::AmbiguousAsset),
            "the target is a whole name part"
        );
    }
}

fn destination(url: &str) -> Result<ureq::http::Uri, Error> {
    if url.len() > 8192 || url.contains(['#', '\\']) {
        return Err(Error::InvalidSource);
    }
    let uri: ureq::http::Uri = url.parse().map_err(|_| Error::InvalidSource)?;
    if uri.scheme_str() != Some("https")
        || !matches!(
            uri.host(),
            Some("github.com" | "api.github.com" | "release-assets.githubusercontent.com")
        )
        || uri.port_u16().is_some_and(|p| p != 443)
        || uri.authority().is_none_or(|a| a.as_str().contains('@'))
    {
        return Err(Error::InvalidSource);
    }
    Ok(uri)
}
#[cfg(test)]
mod destination_tests {
    use super::*;
    #[test]
    fn only_exact_https_public_hosts_and_standard_port_are_allowed() {
        assert!(destination("https://api.github.com/repos/o/r/releases/latest").is_ok());
        for url in [
            "http://github.com/a",
            "https://github.com:444/a",
            "https://github.com.evil/a",
            "https://user@github.com/a",
            "https://127.0.0.1/a",
            "https://[::1]/a",
            "https://github.com/a#fragment",
        ] {
            assert!(destination(url).is_err(), "{url}");
        }
    }
}

struct Response {
    status: u16,
    location: Option<String>,
    encoding: Option<String>,
    body: Box<dyn std::io::Read>,
}
trait Transport {
    fn get(&mut self, url: &str, remaining: std::time::Duration) -> Result<Response, Error>;
}
#[cfg(test)]
fn fetch(
    transport: &mut impl Transport,
    url: &str,
    max: usize,
    deadline: std::time::Instant,
    hops: &mut usize,
) -> Result<Vec<u8>, Error> {
    fetch_controlled(
        transport,
        url,
        max,
        deadline,
        hops,
        &Cancellation::default(),
        None,
    )
}
fn fetch_controlled(
    transport: &mut impl Transport,
    url: &str,
    max: usize,
    deadline: std::time::Instant,
    hops: &mut usize,
    cancel: &Cancellation,
    progress: Option<(&Progress, u64)>,
) -> Result<Vec<u8>, Error> {
    let mut url = url.to_owned();
    loop {
        if cancel.is_cancelled() {
            return Err(Error::Cancelled);
        }
        destination(&url)?;
        let remaining = deadline
            .checked_duration_since(std::time::Instant::now())
            .filter(|d| !d.is_zero())
            .ok_or(Error::Deadline)?;
        let mut response = transport.get(&url, remaining)?;
        if matches!(response.status, 301 | 302 | 303 | 307 | 308) {
            if *hops >= 5 {
                return Err(Error::LimitExceeded);
            }
            *hops += 1;
            url = response.location.ok_or(Error::InvalidSource)?;
            continue;
        }
        if response.status != 200 {
            return Err(Error::Network);
        }
        if response
            .encoding
            .as_deref()
            .is_some_and(|s| s != "identity")
        {
            return Err(Error::InvalidPackage);
        }
        let mut bytes = Vec::new();
        let mut chunk = [0; 16384];
        loop {
            if cancel.is_cancelled() {
                return Err(Error::Cancelled);
            }
            if std::time::Instant::now() >= deadline {
                return Err(Error::Deadline);
            }
            let n = response.body.read(&mut chunk).map_err(|_| Error::Network)?;
            if cancel.is_cancelled() {
                return Err(Error::Cancelled);
            }
            if std::time::Instant::now() >= deadline {
                return Err(Error::Deadline);
            }
            if n == 0 {
                return Ok(bytes);
            }
            if n > max.saturating_sub(bytes.len()) {
                return Err(Error::LimitExceeded);
            }
            bytes.extend_from_slice(&chunk[..n]);
            if let Some((progress, total)) = progress {
                progress.report(bytes.len() as u64, total);
            }
        }
    }
}
#[cfg(test)]
mod transport_tests {
    use super::*;
    use std::{
        collections::VecDeque,
        io::Cursor,
        time::{Duration, Instant},
    };
    struct Fake(VecDeque<Response>);
    impl Transport for Fake {
        fn get(&mut self, _: &str, _: Duration) -> Result<Response, Error> {
            self.0.pop_front().ok_or(Error::Network)
        }
    }
    fn response(status: u16, location: Option<&str>, body: &[u8]) -> Response {
        Response {
            status,
            location: location.map(str::to_string),
            encoding: None,
            body: Box::new(Cursor::new(body.to_vec())),
        }
    }
    #[test]
    fn progress_rises_with_each_chunk_to_one_hundred_and_notifies_only_changes() {
        let heard = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let progress = Progress::new({
            let heard = heard.clone();
            move || heard.lock().unwrap().push(())
        });
        assert_eq!(progress.percent(), None, "unknown before the size is");
        let body = vec![7; 40_000];
        let mut transport = Fake([response(200, None, &body)].into());
        let fetched = fetch_controlled(
            &mut transport,
            "https://github.com/o/r",
            50_000,
            Instant::now() + Duration::from_secs(2),
            &mut 0,
            &Cancellation::default(),
            Some((&progress, 40_000)),
        )
        .unwrap();
        assert_eq!(fetched.len(), 40_000);
        assert_eq!(progress.percent(), Some(100));
        let notified = heard.lock().unwrap().len();
        assert_eq!(notified, 3, "16 KiB chunks: 40 %, 81 %, 100 %");
        progress.report(40_000, 40_000);
        assert_eq!(heard.lock().unwrap().len(), notified, "no news, no notice");
        progress.report(1, 0);
        assert_eq!(
            progress.percent(),
            Some(100),
            "an unknown total says nothing"
        );
    }
    #[test]
    fn bounded_transport_does_not_truncate_or_follow_untrusted_redirects() {
        let deadline = Instant::now() + Duration::from_secs(3);
        let mut f = Fake([response(200, None, b"abc")].into());
        assert_eq!(
            fetch(&mut f, "https://github.com/a", 3, deadline, &mut 0).unwrap(),
            b"abc"
        );
        let mut f = Fake([response(200, None, b"abcd")].into());
        assert_eq!(
            fetch(&mut f, "https://github.com/a", 3, deadline, &mut 0),
            Err(Error::LimitExceeded)
        );
        let mut f = Fake([response(302, Some("http://127.0.0.1/"), b"")].into());
        assert_eq!(
            fetch(&mut f, "https://github.com/a", 3, deadline, &mut 0),
            Err(Error::InvalidSource)
        );
        let mut f = Fake([response(302, Some("https://github.com/b"), b"")].into());
        assert_eq!(
            fetch(&mut f, "https://github.com/a", 3, deadline, &mut 5),
            Err(Error::LimitExceeded)
        );
        let mut f = Fake([response(200, None, b"abc")].into());
        assert_eq!(
            fetch(&mut f, "https://github.com/a", 3, Instant::now(), &mut 0),
            Err(Error::Deadline)
        );
    }
}

fn public_ip(ip: std::net::IpAddr) -> bool {
    match ip {
        std::net::IpAddr::V4(ip) => {
            let [a, b, c, _] = ip.octets();
            !(a == 0
                || a == 10
                || a == 127
                || a >= 224
                || a == 100 && (64..=127).contains(&b)
                || a == 169 && b == 254
                || a == 172 && (16..=31).contains(&b)
                || a == 192 && (b == 168 || b == 0 || b == 2 || b == 88 && c == 99)
                || a == 198 && (b == 18 || b == 19 || b == 51 && c == 100)
                || a == 203 && b == 0 && c == 113)
        }
        std::net::IpAddr::V6(ip) => {
            let s = ip.segments();
            // Conservative global unicast only; reject special-use 2001::/23,
            // documentation, 6to4 and IPv4-mapped/translation ranges.
            (s[0] & 0xe000) == 0x2000
                && !(s[0] == 0x2001 && (s[1] < 0x200 || s[1] == 0xdb8)
                    || s[0] == 0x2002
                    || s[0] == 0x3fff)
        }
    }
}
#[cfg(test)]
mod resolver_tests {
    use super::*;
    #[test]
    fn resolver_rejects_non_global_addresses_including_mapped_ipv4() {
        for ip in [
            "127.0.0.1",
            "10.0.0.1",
            "172.16.1.1",
            "192.168.1.1",
            "169.254.1.1",
            "100.64.0.1",
            "0.0.0.0",
            "192.0.2.1",
            "198.51.100.1",
            "203.0.113.1",
            "224.0.0.1",
            "240.0.0.1",
            "::1",
            "::",
            "::ffff:127.0.0.1",
            "fc00::1",
            "fe80::1",
            "ff00::1",
            "2001:db8::1",
        ] {
            assert!(!public_ip(ip.parse().unwrap()), "{ip}");
        }
        for ip in ["140.82.112.3", "2606:50c0:8000::154"] {
            assert!(public_ip(ip.parse().unwrap()));
        }
    }
}

static ACTIVE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
struct Lease;
impl Drop for Lease {
    fn drop(&mut self) {
        ACTIVE.store(false, std::sync::atomic::Ordering::Release);
    }
}
#[cfg(test)]
fn bounded_job<T: Send + 'static>(
    timeout: std::time::Duration,
    job: impl FnOnce() -> Result<T, Error> + Send + 'static,
) -> Result<T, Error> {
    bounded_job_controlled(timeout, &Cancellation::default(), job)
}
fn bounded_job_controlled<T: Send + 'static>(
    timeout: std::time::Duration,
    cancel: &Cancellation,
    job: impl FnOnce() -> Result<T, Error> + Send + 'static,
) -> Result<T, Error> {
    if ACTIVE
        .compare_exchange(
            false,
            true,
            std::sync::atomic::Ordering::AcqRel,
            std::sync::atomic::Ordering::Acquire,
        )
        .is_err()
    {
        return Err(Error::Busy);
    }
    let lease = Lease;
    let (tx, rx) = std::sync::mpsc::sync_channel(1);
    std::thread::Builder::new()
        .name("rad-plugin-https".into())
        .spawn(move || {
            let _lease = lease;
            let _ = tx.send(job());
        })
        .map_err(|_| Error::Io)?;
    let deadline = std::time::Instant::now() + timeout;
    loop {
        if cancel.is_cancelled() {
            return Err(Error::Cancelled);
        }
        let remaining = deadline.saturating_duration_since(std::time::Instant::now());
        if remaining.is_zero() {
            return Err(Error::Deadline);
        }
        match rx.recv_timeout(remaining.min(std::time::Duration::from_millis(100))) {
            Ok(result) => return result,
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
            Err(_) => return Err(Error::Network),
        }
    }
}
#[cfg(test)]
mod worker_tests {
    use super::*;
    use std::time::Duration;
    #[test]
    fn timed_out_worker_retains_global_slot_until_it_really_exits() {
        let (tx, rx) = std::sync::mpsc::channel();
        assert_eq!(
            bounded_job(Duration::from_millis(10), move || {
                rx.recv().unwrap();
                Ok(())
            }),
            Err(Error::Deadline)
        );
        assert_eq!(
            bounded_job(Duration::from_secs(1), || Ok(())),
            Err(Error::Busy)
        );
        tx.send(()).unwrap();
    }
}

#[derive(Debug)]
struct PublicResolver;
impl ureq::unversioned::resolver::Resolver for PublicResolver {
    fn resolve(
        &self,
        uri: &ureq::http::Uri,
        _: &ureq::config::Config,
        _: ureq::unversioned::transport::NextTimeout,
    ) -> Result<ureq::unversioned::resolver::ResolvedSocketAddrs, ureq::Error> {
        use std::net::ToSocketAddrs;
        let host = uri.host().ok_or(ureq::Error::HostNotFound)?;
        let mut result = self.empty();
        // Runs on our sole bounded worker, not ureq's detached DNS threads.
        for address in (host, 443)
            .to_socket_addrs()
            .map_err(|_| ureq::Error::HostNotFound)?
        {
            if !public_ip(address.ip()) || result.len() >= 16 {
                return Err(ureq::Error::HostNotFound);
            }
            result.push(address);
        }
        if result.is_empty() {
            return Err(ureq::Error::HostNotFound);
        }
        Ok(result)
    }
}
struct Https;
impl Transport for Https {
    fn get(&mut self, url: &str, remaining: std::time::Duration) -> Result<Response, Error> {
        destination(url)?;
        let tls = ureq::tls::TlsConfig::builder()
            .provider(ureq::tls::TlsProvider::Rustls)
            .root_certs(ureq::tls::RootCerts::WebPki)
            .build();
        let config = ureq::Agent::config_builder()
            .https_only(true)
            .proxy(None)
            .max_redirects(0)
            .max_redirects_will_error(false)
            .http_status_as_error(false)
            .max_response_header_size(65536)
            .timeout_global(Some(remaining))
            .timeout_connect(Some(std::time::Duration::from_secs(10)))
            .timeout_recv_response(Some(std::time::Duration::from_secs(10)))
            .tls_config(tls)
            .build();
        let agent = ureq::Agent::with_parts(config, inactivity_connector(), PublicResolver);
        let response = agent
            .get(url)
            .header("User-Agent", "RAD-plugin-manager/1")
            .header("Accept-Encoding", "identity")
            .call()
            .map_err(|_| Error::Network)?;
        let status = response.status().as_u16();
        let location = response
            .headers()
            .get("location")
            .map(|h| {
                h.to_str()
                    .map(str::to_string)
                    .map_err(|_| Error::InvalidSource)
            })
            .transpose()?;
        let encoding = response
            .headers()
            .get("content-encoding")
            .map(|h| {
                h.to_str()
                    .map(str::to_string)
                    .map_err(|_| Error::InvalidPackage)
            })
            .transpose()?;
        Ok(Response {
            status,
            location,
            encoding,
            body: Box::new(response.into_body().into_reader()),
        })
    }
}

#[cfg(test)]
mod ambiguous_json_tests {
    use super::*;
    #[test]
    fn duplicate_metadata_keys_are_not_silently_overwritten() {
        let json=br#"{"assets":[],"assets":[{"name":"rad-plugin-x86_64-unknown-linux-gnu.radp","browser_download_url":"https://github.com/a"}]}"#;
        assert_eq!(asset(json), Err(Error::InvalidPackage));
    }
}

#[cfg(test)]
mod limits_tests {
    use super::*;
    #[test]
    fn metadata_size_and_asset_count_are_bounded() {
        assert_eq!(asset(&vec![b' '; 1048577]), Err(Error::LimitExceeded));
        let item =
            serde_json::json!({"name":"irrelevant","browser_download_url":"https://github.com/a"});
        let bytes = serde_json::to_vec(&serde_json::json!({"assets":vec![item;129]})).unwrap();
        assert_eq!(asset(&bytes), Err(Error::LimitExceeded));
    }
    #[test]
    fn compressed_body_and_total_deadline_during_read_are_rejected() {
        struct Slow;
        impl std::io::Read for Slow {
            fn read(&mut self, _: &mut [u8]) -> std::io::Result<usize> {
                std::thread::sleep(std::time::Duration::from_millis(5));
                Ok(0)
            }
        }
        struct One(Option<Response>);
        impl Transport for One {
            fn get(&mut self, _: &str, _: std::time::Duration) -> Result<Response, Error> {
                Ok(self.0.take().unwrap())
            }
        }
        let mut compressed = One(Some(Response {
            status: 200,
            location: None,
            encoding: Some("gzip".into()),
            body: Box::new(std::io::empty()),
        }));
        assert_eq!(
            fetch(
                &mut compressed,
                "https://github.com/a",
                10,
                std::time::Instant::now() + std::time::Duration::from_secs(1),
                &mut 0
            ),
            Err(Error::InvalidPackage)
        );
        let mut slow = One(Some(Response {
            status: 200,
            location: None,
            encoding: None,
            body: Box::new(Slow),
        }));
        assert_eq!(
            fetch(
                &mut slow,
                "https://github.com/a",
                10,
                std::time::Instant::now() + std::time::Duration::from_millis(1),
                &mut 0
            ),
            Err(Error::Deadline)
        );
    }
}

#[cfg(test)]
mod logging_tests {
    #[test]
    fn third_party_http_trace_cannot_expose_signed_urls() {
        assert!(log::STATIC_MAX_LEVEL <= log::LevelFilter::Info);
    }
}

#[cfg(test)]
mod trace_logger_tests {
    use super::*;
    static MESSAGES: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());
    struct Capture;
    impl log::Log for Capture {
        fn enabled(&self, _: &log::Metadata<'_>) -> bool {
            true
        }
        fn log(&self, record: &log::Record<'_>) {
            MESSAGES.lock().unwrap().push(record.args().to_string());
        }
        fn flush(&self) {}
    }
    static LOGGER: Capture = Capture;
    #[derive(Debug)]
    struct NoNetwork;
    impl ureq::unversioned::resolver::Resolver for NoNetwork {
        fn resolve(
            &self,
            _: &ureq::http::Uri,
            _: &ureq::config::Config,
            _: ureq::unversioned::transport::NextTimeout,
        ) -> Result<ureq::unversioned::resolver::ResolvedSocketAddrs, ureq::Error> {
            Err(ureq::Error::HostNotFound)
        }
    }
    #[test]
    fn enabled_trace_logger_never_receives_signed_uri_or_secret_parameters() {
        log::set_logger(&LOGGER).unwrap();
        log::set_max_level(log::LevelFilter::Trace);
        log::info!("safe logger sentinel");
        let uri = "https://release-assets.githubusercontent.com/synthetic?signature=NOT_A_REAL_SECRET&parameter=PRIVATE_MARKER";
        assert!(destination(uri).is_ok());
        let config = ureq::Agent::config_builder()
            .https_only(true)
            .proxy(None)
            .max_redirects(0)
            .build();
        let agent = ureq::Agent::with_parts(
            config,
            ureq::unversioned::transport::DefaultConnector::default(),
            NoNetwork,
        );
        assert!(agent.get(uri).call().is_err());
        let messages = MESSAGES.lock().unwrap();
        assert!(messages.iter().any(|m| m == "safe logger sentinel"));
        assert!(messages.iter().all(|m| !m.contains("NOT_A_REAL_SECRET")
            && !m.contains("PRIVATE_MARKER")
            && !m.contains("signature=")));
    }
}

#[cfg(test)]
mod cancellation_tests {
    use super::*;
    #[test]
    fn shutdown_cancels_body_between_blocks_without_reading_next_block() {
        struct Body {
            cancel: Cancellation,
            reads: usize,
        }
        impl std::io::Read for Body {
            fn read(&mut self, b: &mut [u8]) -> std::io::Result<usize> {
                self.reads += 1;
                assert_eq!(self.reads, 1, "shutdown must prevent next block");
                b[0] = 1;
                self.cancel.cancel();
                Ok(1)
            }
        }
        struct One(Option<Response>);
        impl Transport for One {
            fn get(&mut self, _: &str, _: std::time::Duration) -> Result<Response, Error> {
                Ok(self.0.take().unwrap())
            }
        }
        let cancel = Cancellation::default();
        let mut transport = One(Some(Response {
            status: 200,
            location: None,
            encoding: None,
            body: Box::new(Body {
                cancel: cancel.clone(),
                reads: 0,
            }),
        }));
        assert!(
            fetch_controlled(
                &mut transport,
                "https://github.com/o/r",
                10,
                std::time::Instant::now() + std::time::Duration::from_secs(2),
                &mut 0,
                &cancel,
                None
            )
            .is_err()
        );
    }
}

// Bound each socket read, not the total response body: progressing downloads retain
// the broad global budget. Installed before TLS so every network wait is bounded.
fn inactivity_connector() -> impl ureq::unversioned::transport::Connector {
    use ureq::unversioned::transport::{Connector, RustlsConnector, TcpConnector};
    ().chain(TcpConnector::default())
        .chain(Inactivity)
        .chain(RustlsConnector::default())
}
#[derive(Debug)]
struct Inactivity;
#[derive(Debug)]
struct InactivityTransport<T>(T);
impl<T: ureq::unversioned::transport::Transport> ureq::unversioned::transport::Connector<T>
    for Inactivity
{
    type Out = InactivityTransport<T>;
    fn connect(
        &self,
        _: &ureq::unversioned::transport::ConnectionDetails,
        chained: Option<T>,
    ) -> Result<Option<Self::Out>, ureq::Error> {
        Ok(chained.map(InactivityTransport))
    }
}
impl<T: ureq::unversioned::transport::Transport> ureq::unversioned::transport::Transport
    for InactivityTransport<T>
{
    fn buffers(&mut self) -> &mut dyn ureq::unversioned::transport::Buffers {
        self.0.buffers()
    }
    fn transmit_output(
        &mut self,
        n: usize,
        timeout: ureq::unversioned::transport::NextTimeout,
    ) -> Result<(), ureq::Error> {
        self.0.transmit_output(n, timeout)
    }
    fn await_input(
        &mut self,
        mut timeout: ureq::unversioned::transport::NextTimeout,
    ) -> Result<bool, ureq::Error> {
        timeout.after = timeout
            .after
            .min(ureq::unversioned::transport::time::Duration::from_secs(10));
        self.0.await_input(timeout)
    }
    fn is_open(&mut self) -> bool {
        self.0.is_open()
    }
}
