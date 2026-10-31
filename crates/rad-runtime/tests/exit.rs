//! Exit classes (ADR 0011): codes, the last unexpected exit, and the unit file that depends on them.
use rad_runtime::exit::{self};
use std::{fs, path::Path};

#[test]
fn the_unit_restarts_only_unexpected_failures_with_a_cap_and_never_the_blocking_code() {
    let unit = fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../packaging/rad-runtime.service"),
    )
    .unwrap();
    // Directives by section: the start limit is only honored in [Unit].
    let mut section = "";
    let mut directives = Vec::new();
    for line in unit.lines().map(str::trim) {
        if line.starts_with('[') {
            section = line;
        } else if !line.starts_with('#') && !line.is_empty() {
            directives.push((section, line));
        }
    }
    let has = |section: &str, directive: &str| directives.contains(&(section, directive));
    for directive in ["StartLimitBurst=5", "StartLimitIntervalSec=300"] {
        assert!(has("[Unit]", directive), "{directive}");
    }
    for directive in ["Restart=on-failure", "KillMode=mixed", "RestartSec=1"] {
        assert!(has("[Service]", directive), "{directive}");
    }
    assert!(
        has(
            "[Service]",
            &format!("RestartPreventExitStatus={}", exit::BLOCKING)
        ),
        "the blocking code in the unit must be the one the runtime exits with"
    );
    for growing in ["RestartSteps=", "RestartMaxDelaySec="] {
        assert!(
            directives
                .iter()
                .any(|(section, line)| *section == "[Service]" && line.starts_with(growing)),
            "{growing}"
        );
    }
}
