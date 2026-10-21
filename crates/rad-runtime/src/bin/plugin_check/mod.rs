mod host;
mod raw;
mod scenario;
use rad_plugin_package::Candidate;
use scenario::Scenario;
use serde::Serialize;
use std::{
    fs,
    io::Read,
    os::unix::fs::{DirBuilderExt, OpenOptionsExt},
    path::{Path, PathBuf},
    time::Instant,
};

pub(super) const SECRET: &str = "rad-check-synthetic-credential-7f301b8c";
pub(super) type Result<T = ()> = std::result::Result<T, &'static str>;
#[derive(Serialize)]
struct Report {
    version: u32,
    status: &'static str,
    checks: Vec<&'static str>,
    failure: Option<&'static str>,
}
pub fn main() {
    eprintln!(
        "WARNING: executes trusted native plugin code. This checker is not a sandbox or signature verification."
    );
    let mut report = Report {
        version: 1,
        status: "NO_VERIFICADO",
        checks: vec![],
        failure: None,
    };
    match run(&mut report) {
        Ok(()) => report.status = "PASS",
        Err(reason) => report.failure = Some(reason),
    }
    println!(
        "{}",
        serde_json::to_string(&report).expect("bounded report")
    );
    if report.failure.is_some() {
        std::process::exit(1);
    }
}
fn read_bounded(path: &Path, limit: usize) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    fs::File::open(path)
        .map_err(|_| "input_open")?
        .take(limit as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| "input_read")?;
    if bytes.len() > limit {
        return Err("input_too_large");
    }
    Ok(bytes)
}
fn run(report: &mut Report) -> Result {
    if unsafe { libc::prctl(libc::PR_SET_CHILD_SUBREAPER, 1, 0, 0, 0) } != 0 {
        return Err("subreaper_unavailable");
    }
    let _descendants = Descendants;
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    if args.len() != 2 {
        return Err("usage_rad_plugin_check_package_radp_scenario_json");
    }
    let bytes = read_bounded(
        Path::new(&args[0]),
        rad_plugin_package::MAX_EXECUTABLE_BYTES + rad_plugin_package::MAX_MANIFEST_BYTES + 20,
    )?;
    let package = Candidate::parse(
        &bytes,
        rad_plugin_contract::HOST_API,
        rad_plugin_host::TARGET,
    )
    .map_err(|_| "invalid_package_manifest_target_contract")?;
    report
        .checks
        .push("package_manifest_target_contract_capabilities");
    let scenario: Scenario = serde_json::from_slice(&read_bounded(Path::new(&args[1]), 262144)?)
        .map_err(|_| "invalid_scenario")?;
    scenario.validate(package.manifest())?;
    report.checks.push("complete_explicit_scenario");
    let dir = Scratch::new()?;
    let executable = dir.0.join("plugin");
    use std::io::Write;
    fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o700)
        .open(&executable)
        .map_err(|_| "extract_create")?
        .write_all(package.executable())
        .map_err(|_| "extract_write")?;
    let start = Instant::now();
    START.set(start).map_err(|_| "checker_already_started")?;
    host::check(&executable, package.manifest(), &scenario)?;
    report.checks.extend([
        "host_identity_capabilities",
        "host_actions_options",
        "host_visuals_payloadless_freshness_unsubscribe",
        if package.manifest().credential_slots().is_empty() {
            "credentials_no_declared_slots_not_applicable"
        } else {
            "synthetic_credentials_no_leak"
        },
    ]);
    raw::unsubscribe(&executable, package.manifest(), &scenario)?;
    report.checks.push("unsubscribe_source_quiescence");
    raw::eof(&executable, package.manifest(), &scenario)?;
    report.checks.push("eof_exits_without_children");
    raw::pressure(&executable, package.manifest(), &scenario)?;
    report.checks.push("stdout_blocked_budget_terminal_prefix");
    if start.elapsed().as_secs() > 180 {
        return Err("total_budget");
    }
    Ok(())
}
struct Scratch(PathBuf);
impl Scratch {
    fn new() -> Result<Self> {
        for n in 0..100 {
            let p =
                std::env::temp_dir().join(format!("rad-plugin-check-{}-{n}", std::process::id()));
            match fs::DirBuilder::new().mode(0o700).create(&p) {
                Ok(()) => return Ok(Self(p)),
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(_) => return Err("scratch_create"),
            }
        }
        Err("scratch_exhausted")
    }
}
impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// Subreaper ownership is local to this short-lived checker, not runtime lifecycle policy.
struct Descendants;
pub(super) fn children() -> Result<Vec<i32>> {
    let path = format!("/proc/self/task/{}/children", std::process::id());
    fs::read_to_string(path)
        .map_err(|_| "children_unobservable")?
        .split_whitespace()
        .map(|s| s.parse().map_err(|_| "children_unobservable"))
        .collect()
}
/// Reaps, for at most one second, the already killed members of process group `pgid` that
/// are this checker's children, so a later check sees only its own descendants.
pub(super) fn reap_group(pgid: i32) {
    let end = Instant::now() + std::time::Duration::from_secs(1);
    loop {
        let result = unsafe { libc::waitpid(-pgid, std::ptr::null_mut(), libc::WNOHANG) };
        if result < 0 || Instant::now() >= end {
            break;
        };
        if result == 0 {
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
    }
}
impl Drop for Descendants {
    fn drop(&mut self) {
        let end = Instant::now() + std::time::Duration::from_secs(2);
        while Instant::now() < end {
            let Ok(children) = children() else { break };
            if children.is_empty() {
                break;
            }
            for pid in children {
                unsafe {
                    libc::kill(pid, libc::SIGKILL);
                    libc::waitpid(pid, std::ptr::null_mut(), libc::WNOHANG);
                }
            }
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
    }
}

static START: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();
pub(super) fn budget() -> Result {
    if START
        .get()
        .is_some_and(|start| start.elapsed().as_secs() >= 180)
    {
        Err("total_budget")
    } else {
        Ok(())
    }
}
