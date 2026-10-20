use rad_plugin_package::{Candidate, Error};

#[test]
fn rejects_truncated_header() {
    // RADP header is exactly 20 bytes, independently of its payload lengths.
    let header = [
        b'R', b'A', b'D', b'P', 0, 0, 0, 1, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 1,
    ];
    for length in 0..20 {
        assert!(
            matches!(
                Candidate::parse(&header[..length], 1, "x86_64-unknown-linux-gnu"),
                Err(Error::InvalidHeader)
            ),
            "truncated header length {length} must be rejected as InvalidHeader"
        );
    }
}

const TARGET: &str = "x86_64-unknown-linux-gnu";
const MANIFEST: &[u8] = br#"{"contract":1,"capabilities":[],"id":"fixture.package","version":{"major":1,"minor":0,"patch":0},"host_min":1,"host_max":1,"targets":["x86_64-unknown-linux-gnu"],"actions":[{"id":"run","label":"Run","schema":{"version":1,"fields":[]},"credential_slots":[]}],"visuals":[]}"#;

fn container(manifest: &[u8], executable: &[u8]) -> Vec<u8> {
    let mut bytes = b"RADP".to_vec();
    bytes.extend_from_slice(&1u32.to_be_bytes());
    bytes.extend_from_slice(&(manifest.len() as u32).to_be_bytes());
    bytes.extend_from_slice(&(executable.len() as u64).to_be_bytes());
    bytes.extend_from_slice(manifest);
    bytes.extend_from_slice(executable);
    bytes
}

fn native_package() -> Vec<u8> {
    // Read a real linked ELF fixture, never launch it to validate the package.
    let executable = std::fs::read(std::env::current_exe().unwrap()).unwrap();
    container(MANIFEST, &executable)
}

#[test]
fn accepts_real_native_executable_without_running_it() {
    let bytes = native_package();
    assert!(Candidate::parse(&bytes, 1, TARGET).is_ok());
}

#[test]
fn enforces_exact_header_and_bounded_payload_before_contract_parsing() {
    let valid = native_package();
    for offset in 0..8 {
        let mut bytes = valid.clone();
        bytes[offset] ^= 0xff;
        assert!(matches!(
            Candidate::parse(&bytes, 1, TARGET),
            Err(Error::InvalidHeader)
        ));
    }
    for size in [0, 65537, u32::MAX] {
        let mut bytes = valid.clone();
        bytes[8..12].copy_from_slice(&size.to_be_bytes());
        assert!(matches!(
            Candidate::parse(&bytes, 1, TARGET),
            Err(Error::InvalidSize)
        ));
    }
    for size in [0, 67108865, u64::MAX] {
        let mut bytes = valid.clone();
        bytes[12..20].copy_from_slice(&size.to_be_bytes());
        assert!(matches!(
            Candidate::parse(&bytes, 1, TARGET),
            Err(Error::InvalidSize)
        ));
    }
    for length in [20, 21, valid.len() - 1] {
        assert!(matches!(
            Candidate::parse(&valid[..length], 1, TARGET),
            Err(Error::InvalidSize)
        ));
    }
    let mut extra = valid.clone();
    extra.push(0);
    assert!(matches!(
        Candidate::parse(&extra, 1, TARGET),
        Err(Error::InvalidSize)
    ));
    // Contract major 0 shares no supported version with this major-1 manifest.
    assert!(matches!(
        Candidate::parse(&valid, 0, TARGET),
        Err(Error::Contract(
            rad_plugin_contract::Error::IncompatibleHost
        ))
    ));
    assert!(matches!(
        Candidate::parse(&valid, 1, "wrong-target"),
        Err(Error::Contract(
            rad_plugin_contract::Error::IncompatibleTarget
        ))
    ));
}

#[test]
fn rejects_non_native_or_malformed_elf_without_execution() {
    let native = std::fs::read(std::env::current_exe().unwrap()).unwrap();
    let mut cases = vec![
        vec![0],
        b"#!/bin/sh\nexit 0\n".to_vec(),
        native[..63].to_vec(),
    ];
    // ELF64 LE current version, Linux/System V ABI, ET_EXEC/ET_DYN, x86-64.
    for (offset, value) in [
        (0, 0),
        (4, 1),
        (5, 2),
        (6, 0),
        (7, 9),
        (16, 1),
        (18, 183),
        (20, 0),
        (52, 0),
        (54, 0),
        (56, 0),
    ] {
        let mut broken = native.clone();
        broken[offset] = value;
        cases.push(broken);
    }
    let mut outside = native.clone();
    outside[32..40].copy_from_slice(&u64::MAX.to_le_bytes());
    cases.push(outside);
    for executable in cases {
        assert!(
            Candidate::parse(&container(MANIFEST, &executable), 1, TARGET).is_err(),
            "invalid ELF must never produce a Candidate"
        );
    }
}

#[test]
fn rejects_elf_segments_outside_file_or_without_executable_entry() {
    let native = std::fs::read(std::env::current_exe().unwrap()).unwrap();
    let phoff = u64::from_le_bytes(native[32..40].try_into().unwrap()) as usize;
    let count = u16::from_le_bytes(native[56..58].try_into().unwrap()) as usize;
    let load = (0..count)
        .map(|i| phoff + i * 56)
        .find(|&p| native[p..p + 4] == 1u32.to_le_bytes())
        .unwrap();
    let mut cases = Vec::new();
    for offset in [load + 8, load + 32, load + 40] {
        let mut broken = native.clone();
        broken[offset..offset + 8].copy_from_slice(&u64::MAX.to_le_bytes());
        cases.push(broken);
    }
    let mut bad_alignment = native.clone();
    bad_alignment[load + 48..load + 56].copy_from_slice(&3u64.to_le_bytes());
    cases.push(bad_alignment);
    let mut bad_entry = native.clone();
    bad_entry[24..32].copy_from_slice(&u64::MAX.to_le_bytes());
    cases.push(bad_entry);
    for executable in cases {
        assert!(matches!(
            Candidate::parse(&container(MANIFEST, &executable), 1, TARGET),
            Err(Error::InvalidExecutable)
        ));
    }
}

#[test]
fn accepts_exact_size_limits_but_rejects_one_more_and_invalid_contracts() {
    use rad_plugin_package::{MAX_EXECUTABLE_BYTES, MAX_MANIFEST_BYTES};
    let mut manifest = MANIFEST.to_vec();
    manifest.resize(MAX_MANIFEST_BYTES, b' ');
    let mut executable = std::fs::read(std::env::current_exe().unwrap()).unwrap();
    executable.resize(MAX_EXECUTABLE_BYTES, 0);
    let mut bytes = container(&manifest, &executable);
    let candidate = Candidate::parse(&bytes, 1, TARGET).unwrap();
    assert_eq!(candidate.manifest_bytes(), manifest);
    assert_eq!(candidate.executable(), executable);
    assert_eq!(candidate.manifest().id, "fixture.package");
    // Package total and declared lengths are independently bounded.
    bytes.push(0);
    assert!(matches!(
        Candidate::parse(&bytes, 1, TARGET),
        Err(Error::InvalidSize)
    ));
    bytes[12..20].copy_from_slice(&((MAX_EXECUTABLE_BYTES + 1) as u64).to_be_bytes());
    assert!(matches!(
        Candidate::parse(&bytes, 1, TARGET),
        Err(Error::InvalidSize)
    ));
    let native = &executable[..1024];
    for bad_manifest in [
        b"{".as_slice(),
        b"{}".as_slice(),
        br#"{"contract":2}"#.as_slice(),
    ] {
        assert!(matches!(
            Candidate::parse(&container(bad_manifest, native), 1, TARGET),
            Err(Error::Contract(_))
        ));
    }
    let other_target = String::from_utf8(MANIFEST.to_vec())
        .unwrap()
        .replace(TARGET, "aarch64-unknown-linux-gnu");
    assert!(matches!(
        Candidate::parse(
            &container(other_target.as_bytes(), &executable),
            1,
            "aarch64-unknown-linux-gnu"
        ),
        Err(Error::UnsupportedTarget)
    ));
    let traversal = String::from_utf8(MANIFEST.to_vec())
        .unwrap()
        .replace("fixture.package", "../escape");
    assert!(matches!(
        Candidate::parse(&container(traversal.as_bytes(), &executable), 1, TARGET),
        Err(Error::Contract(_))
    ));
}

#[test]
fn executable_entry_must_be_backed_by_file_bytes_not_zero_fill() {
    let mut executable = std::fs::read(std::env::current_exe().unwrap()).unwrap();
    let phoff = u64::from_le_bytes(executable[32..40].try_into().unwrap()) as usize;
    let count = u16::from_le_bytes(executable[56..58].try_into().unwrap()) as usize;
    let load = (0..count)
        .map(|i| phoff + i * 56)
        .find(|&p| executable[p..p + 4] == 1u32.to_le_bytes() && executable[p + 4] & 1 != 0)
        .unwrap();
    let address = u64::from_le_bytes(executable[load + 16..load + 24].try_into().unwrap());
    let file_size = u64::from_le_bytes(executable[load + 32..load + 40].try_into().unwrap());
    executable[load + 40..load + 48].copy_from_slice(&(file_size + 16).to_le_bytes());
    executable[24..32].copy_from_slice(&(address + file_size).to_le_bytes());
    assert!(matches!(
        Candidate::parse(&container(MANIFEST, &executable), 1, TARGET),
        Err(Error::InvalidExecutable)
    ));
}

#[test]
fn separate_parts_are_borrowed_and_encoder_matches_independent_wire() {
    let executable = std::fs::read(std::env::current_exe().unwrap()).unwrap();
    let candidate = Candidate::from_parts(MANIFEST, &executable, 1, TARGET).unwrap();
    assert_eq!(candidate.executable().as_ptr(), executable.as_ptr());
    assert_eq!(candidate.manifest_bytes().as_ptr(), MANIFEST.as_ptr());
    assert_eq!(
        rad_plugin_package::encode(&candidate),
        container(MANIFEST, &executable)
    );
    assert!(matches!(
        Candidate::from_parts(MANIFEST, b"not elf", 1, TARGET),
        Err(Error::InvalidExecutable)
    ));
}
