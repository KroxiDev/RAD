#!/usr/bin/env python3
"""Construcción y metadata local de RAD. No instala ni publica."""
import argparse
import hashlib
import json
from pathlib import Path
import re
import sys
import subprocess
import shutil
import struct
import os
import platform
import tempfile
import fcntl
import shlex

ROOT = Path(__file__).resolve().parents[1]


def release_source():
    source = json.loads((ROOT / "packaging/release.json").read_text())
    if (source.get("schema") != 1 or source.get("product") != "RAD"
            or source.get("package") != "rust-action-deck"
            or source.get("architecture") != "x86_64"
            or not re.fullmatch(r"(?:0|[1-9][0-9]*)\.(?:0|[1-9][0-9]*)\.(?:0|[1-9][0-9]*)", source.get("version", ""))
            or type(source.get("pkgrel")) is not int or source["pkgrel"] < 1
            or source.get("base") != "omarchy-4.0.6"):
        raise ValueError("invalid release source")
    return source


def filename(source):
    return f"{source['package']}-{source['version']}-{source['pkgrel']}-{source['architecture']}.pkg.tar.zst"


def metadata(package, commit):
    source = release_source()
    if not re.fullmatch(r"[a-f0-9]{40}", commit):
        raise ValueError("commit must be a full lowercase SHA-1 revision")
    if package.name != filename(source) or package.is_symlink() or not package.is_file():
        raise ValueError("package path does not name the expected regular package")
    size = package.stat().st_size
    if not 0 < size <= 536870912:
        raise ValueError("package size is outside the update contract")
    info = subprocess.run(["/usr/bin/bsdtar", "-xOf", str(package), ".PKGINFO"],
                          check=True, capture_output=True, timeout=15).stdout
    if len(info) > 65536:
        raise ValueError("package identity exceeds limit")
    fields = {}
    for line in info.decode("utf-8").splitlines():
        if " = " in line:
            key, value = line.split(" = ", 1)
            fields.setdefault(key, []).append(value)
    expected = {"pkgname": source["package"],
                "pkgver": f"{source['version']}-{source['pkgrel']}",
                "arch": source["architecture"]}
    if any(fields.get(key) != [value] for key, value in expected.items()):
        raise ValueError("package identity does not match the release source")
    buildinfo = subprocess.run(["/usr/bin/bsdtar", "-xOf", str(package), ".BUILDINFO"],
                               capture_output=True, timeout=15)
    if buildinfo.returncode == 0:
        if len(buildinfo.stdout) > 262144:
            raise ValueError("build information exceeds limit")
        for line in buildinfo.stdout.decode("utf-8").splitlines():
            if " = " not in line:
                continue
            key, value = line.split(" = ", 1)
            if key in ("builddir", "startdir") and not value.startswith(
                    ("/tmp/rad-package-", "/tmp/rad-recipe-", "/build/")):
                raise ValueError("package build directories must be neutral, without personal checkout paths")
    with package.open("rb") as stream:
        digest = hashlib.file_digest(stream, "sha256").hexdigest()
    return {**source, "file": package.name, "size": size, "sha256": digest, "commit": commit}



BINARIES = ("rad-editor", "rad-runtime", "rad-launcher-helper", "rad-tray", "rad-cli",
            "rad-update-coordinator", "rad-update-installer")


def stage(binary_dir, output):
    """Assemble an allowlisted layout, never copy a build directory recursively."""
    if output.exists():
        raise ValueError("staging destination must not exist")
    for name in BINARIES:
        binary = binary_dir / name
        if binary.is_symlink() or not binary.is_file():
            raise ValueError(f"missing regular production binary: {name}")
        with binary.open("rb") as stream:
            header = stream.read(20)
        if (len(header) != 20 or header[:6] != b"\x7fELF\x02\x01"
                or struct.unpack("<H", header[18:20])[0] != 62):
            raise ValueError(f"not an x86-64 ELF: {name}")
    lib = output / "usr/lib/rad"
    lib.mkdir(parents=True)
    for name in BINARIES:
        shutil.copyfile(binary_dir / name, lib / name)
        (lib / name).chmod(0o755)
    links = output / "usr/bin"
    links.mkdir(parents=True)
    for name in ("rad-editor", "rad-cli"):
        (links / name).symlink_to(f"../lib/rad/{name}")
    files = {
        "usr/share/applications/io.github.KroxiDev.RAD.desktop":
            (ROOT / "packaging/io.github.KroxiDev.RAD.desktop").read_text().replace(
                "Exec=rad-editor\n", "Exec=/usr/lib/rad/rad-editor\n").encode(),
        "usr/share/pixmaps/io.github.KroxiDev.RAD.png": (ROOT / "packaging/rad.png").read_bytes(),
        "usr/share/licenses/rust-action-deck/LICENSE": (ROOT / "LICENSE").read_bytes(),
        "usr/share/rad/release.json": (ROOT / "packaging/release.json").read_bytes(),
        "usr/share/polkit-1/actions/io.github.KroxiDev.RAD.update.policy":
            (ROOT / "packaging/io.github.KroxiDev.RAD.update.policy").read_bytes(),
    }
    for name in ("rad-runtime", "rad-tray"):
        template = (ROOT / f"packaging/{name}.service").read_text()
        template = template.replace(f"ExecStart=/usr/local/bin/{name}", f"ExecStart=/usr/lib/rad/{name}")
        files[f"usr/lib/systemd/user/{name}.service"] = template.encode()
    for relative, contents in files.items():
        destination = output / relative
        destination.parent.mkdir(parents=True, exist_ok=True)
        destination.write_bytes(contents)
        destination.chmod(0o644)



def run(*args, **kwargs):
    return subprocess.run(args, cwd=ROOT, check=True, **kwargs)


def git(*args):
    return run("git", *args, capture_output=True, text=True).stdout.strip()


def third_party_licenses(output, env):
    data = run("cargo", "metadata", "--offline", "--locked", "--format-version", "1",
               "--filter-platform", "x86_64-unknown-linux-gnu", env=env,
               capture_output=True, text=True).stdout
    collect_licenses(json.loads(data)["packages"], output)
    compiler = run("rustc", "--version", "--verbose", capture_output=True, text=True).stdout
    commit = re.search(r"^commit-hash: ([a-f0-9]{40})$", compiler, re.M)
    manifest = notice_manifest()
    if not commit or commit.group(1) != manifest["groups"]["rust-std"]["revision"]:
        raise ValueError("review pinned Rust standard-library notices for this compiler revision")
    copy_notice_group("rust-std", output / "usr/share/licenses/rust-action-deck/third-party/rust-std")



def notice_manifest():
    manifest = json.loads((ROOT / "packaging/notices/manifest.json").read_text())
    if manifest["schema"] != 1:
        raise ValueError("unsupported notice manifest")
    return manifest


def copy_notice_group(group, destination):
    provenance = notice_manifest()["groups"][group]
    destination.mkdir(parents=True, exist_ok=True)
    for name, digest in provenance["files"].items():
        source = ROOT / "packaging/notices" / group / name
        data = source.read_bytes()
        if source.is_symlink() or hashlib.sha256(data).hexdigest() != digest:
            raise ValueError(f"upstream notice integrity mismatch: {group}/{name}")
        (destination / name).write_bytes(data)
        (destination / name).chmod(0o644)
    (destination / "SOURCE.json").write_text(json.dumps(provenance, indent=2) + "\n")


def collect_licenses(packages, output):
    destination = output / "usr/share/licenses/rust-action-deck/third-party"
    for package in packages:
        if package["source"] is None:
            continue
        source = Path(package["manifest_path"]).parent
        label = f"{package['name']}-{package['version']}"
        wanted = []
        for path in source.rglob("*"):
            if not path.is_file() or path.is_symlink():
                continue
            name = path.name.lower()
            if any(word in name for word in ("license", "licence", "copying", "notice", "copyright", "ofl.txt", "ufl.txt")):
                wanted.append(path)
        if package["name"] == "epaint_default_fonts":
            if package["version"] != "0.36.2":
                raise ValueError("review embedded font notices for this epaint_default_fonts version")
            for relative in ("fonts/UFL.txt", "fonts/Hack-Regular.txt"):
                notice = source / relative
                if not notice.is_file() or notice.is_symlink():
                    raise ValueError(f"missing embedded font notice {relative}")
                if notice not in wanted:
                    wanted.append(notice)
        explicit = package.get("license_file")
        if explicit:
            explicit = source / explicit
            if explicit.is_file() and explicit not in wanted:
                wanted.append(explicit)
        if not wanted:
            manifest = notice_manifest()
            group = manifest["crates"].get(f"{package['name']}@{package['version']}")
            if not group:
                raise ValueError(f"missing license texts in cached dependency {label}; review before distribution")
            vcs = json.loads((source / ".cargo_vcs_info.json").read_text())
            if vcs.get("git", {}).get("sha1") != manifest["groups"][group]["revision"]:
                raise ValueError(f"upstream notice revision mismatch: {label}")
            copy_notice_group(group, destination / label)
            continue
        for path in wanted:
            relative = path.relative_to(source)
            target = destination / label / relative
            target.parent.mkdir(parents=True, exist_ok=True)
            shutil.copyfile(path, target)
            target.chmod(0o644)


def glibc_min(binary_dir):
    versions = []
    for name in BINARIES:
        output = run("readelf", "--version-info", str(binary_dir / name),
                     text=True, capture_output=True).stdout
        versions.extend(tuple(map(int, version.split(".")))
                        for version in re.findall(r"GLIBC_([0-9]+(?:\.[0-9]+)+)", output))
    if not versions:
        raise ValueError("no glibc ABI version found")
    return ".".join(map(str, max(versions)))


def makepkg_environment(base, overrides):
    environment = base.copy()
    for name in ("PKGDEST", "SRCDEST", "SRCPKGDEST", "LOGDEST", "BUILDDIR", "PKGEXT",
                 "SRCEXT", "GPGKEY", "GNUPGHOME", "GPG_TTY", "PACKAGER", "CARCH"):
        environment.pop(name, None)
    environment.update(overrides)
    environment["CARCH"] = "x86_64"
    return environment


def build(development=False, plan=False, reuse_build=False):
    source = release_source()
    dirty = bool(git("status", "--porcelain", "--untracked-files=normal"))
    if dirty and not development:
        raise ValueError("dirty source tree: production releases require a clean identified revision; use --development only for local tests")
    if reuse_build and not development:
        raise ValueError("--reuse-build is allowed only for non-publishable development packages")
    if os.geteuid() == 0:
        raise ValueError("build packages as a normal user, not root")
    if platform.machine() != "x86_64":
        raise ValueError("the initial target is x86_64")
    os_release = dict(line.split("=", 1) for line in Path("/etc/os-release").read_text().splitlines()
                      if "=" in line)
    if (os_release.get("ID", "").strip('"') != "omarchy"
            or os_release.get("VERSION_ID", "").strip('"') != "4.0.6"):
        raise ValueError("the identified build base must be Omarchy 4.0.6; this does not certify its rolling snapshot")
    commit = git("rev-parse", "HEAD")
    epoch = git("show", "-s", "--format=%ct", "HEAD")
    tag = f"v{source['version']}-{source['pkgrel']}"
    if plan:
        print(f"{tag}: commit={commit}, development={development}, dirty={dirty}")
        print("cargo build --offline --locked --release --workspace --bins --target x86_64-unknown-linux-gnu")
        print("assemble allowlisted layout, preserve dependency licenses, makepkg (no install, no publish)")
        return
    env = os.environ.copy()
    cargo_home = Path(env.get("CARGO_HOME", ROOT / "target/cargo-home")).absolute()
    target = Path(env.get("CARGO_TARGET_DIR", ROOT / "target")).absolute()
    env.update(CARGO_HOME=str(cargo_home), CARGO_TARGET_DIR=str(target),
               CARGO_BUILD_JOBS="2", CARGO_INCREMENTAL="0", SOURCE_DATE_EPOCH=epoch)
    env.pop("RUSTFLAGS", None)
    env.pop("CARGO_BUILD_TARGET", None)
    env["CARGO_ENCODED_RUSTFLAGS"] = "\x1f".join([
        f"--remap-path-prefix={ROOT}=/rad", f"--remap-path-prefix={cargo_home}=/cargo"])
    if not reuse_build:
        run("cargo", "build", "--offline", "--locked", "--release", "--workspace", "--bins",
            "--target", "x86_64-unknown-linux-gnu", env=env)
    destination = ROOT / "target/dist"
    destination.mkdir(parents=True, exist_ok=True)
    directory = Path(tempfile.mkdtemp(prefix=f"{tag}-{'development-' if development else ''}{commit[:12]}-", dir=destination))
    binary_dir = target / "release" if reuse_build else target / "x86_64-unknown-linux-gnu/release"
    tree = directory / "rad"
    stage(binary_dir, tree)
    third_party_licenses(tree, env)
    snapshot = run("/usr/bin/pacman", "-Q", "glibc", "gcc-libs", "glib2", "libxkbcommon",
                   "wayland", "libglvnd", "systemd", "polkit", "pacman", "curl",
                   capture_output=True, text=True).stdout
    info = (f"base={source['base']}\ncommit={commit}\ndirty={str(dirty).lower()}\n"
            f"development={str(development).lower()}\n" + snapshot)
    docs = tree / "usr/share/doc/rust-action-deck"
    docs.mkdir(parents=True)
    (docs / "build-environment.txt").write_text(info)
    for name in ("README.md", "README.es.md"):
        shutil.copyfile(ROOT / name, docs / name)
    bundle = directory / f"rad-{source['version']}-{source['pkgrel']}.tar.zst"
    run("tar", "--sort=name", f"--mtime=@{epoch}", "--owner=0", "--group=0", "--numeric-owner",
        "-I", "zstd -T1 -19", "-cf", str(bundle), "-C", str(directory), "rad", env=env)
    with bundle.open("rb") as stream:
        bundle_digest = hashlib.file_digest(stream, "sha256").hexdigest()
    recipe = (ROOT / "packaging/arch/PKGBUILD.in").read_text()
    values = {"PACKAGE": source["package"], "VERSION": source["version"], "PKGREL": str(source["pkgrel"]),
              "GLIBC_MIN": glibc_min(tree / "usr/lib/rad"), "BUNDLE_SHA256": bundle_digest}
    for key, value in values.items():
        recipe = recipe.replace(f"@{key}@", value)
    if re.search(r"@[A-Z_]+@", recipe):
        raise ValueError("unresolved PKGBUILD field")
    (directory / "PKGBUILD").write_text(recipe)
    shutil.copyfile(ROOT / "packaging/arch/rad.install", directory / "rad.install")
    make_env = env.copy()
    make_env["PKGDEST"] = str(directory)
    # A neutral staging path avoids embedding a personal source-directory path in BUILDINFO.
    # Cargo itself always builds on the repository disk, never in this temporary directory.
    with (tempfile.TemporaryDirectory(prefix="rad-package-") as package_build,
          tempfile.TemporaryDirectory(prefix="rad-recipe-") as recipe_directory):
        neutral = Path(recipe_directory)
        for path in (directory / "PKGBUILD", directory / "rad.install", bundle):
            shutil.copyfile(path, neutral / path.name)
        configuration = neutral / "makepkg.conf"
        overrides = {"PKGEXT": ".pkg.tar.zst", "PACKAGER": "RAD release build",
                     "BUILDDIR": package_build, "PKGDEST": str(directory),
                     "SRCDEST": str(directory / "sources"), "SRCPKGDEST": str(directory),
                     "LOGDEST": str(directory)}
        configuration.write_text("source /etc/makepkg.conf\n" + "".join(
            f"{key}={shlex.quote(value)}\n" for key, value in overrides.items()))
        subprocess.run(["/usr/bin/makepkg", "--config", str(configuration),
                        "--noconfirm", "--nosign", "--cleanbuild"], cwd=neutral,
                       env=makepkg_environment(make_env, overrides), check=True)
    package = directory / filename(source)
    # Identity is checked even for a deliberately non-publishable development build.
    document = metadata(package, commit)
    (directory / "build-environment.txt").write_text(info)
    if not development:
        if git("status", "--porcelain", "--untracked-files=normal") or git("rev-parse", "HEAD") != commit:
            raise ValueError("source changed during build; package is not a release artifact")
        (directory / "rad-update.json").write_text(json.dumps(document, indent=2) + "\n")
        (directory / "SHA256SUMS").write_text(f"{document['sha256']}  {package.name}\n")
    else:
        (directory / "DEVELOPMENT-NOT-FOR-RELEASE").write_text(
            "Local behavior check only. No update metadata is emitted. Do not upload as an official stable release.\n")
    print(directory)
    print("No installation or publication performed.")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest="command", required=True)
    meta = commands.add_parser("metadata", help="validate package and emit rad-update.json on stdout")
    meta.add_argument("--package", type=Path, required=True)
    meta.add_argument("--commit", required=True)
    layout = commands.add_parser("stage", help="assemble production layout; no build or installation")
    layout.add_argument("--bin-dir", type=Path, required=True)
    layout.add_argument("--output", type=Path, required=True)
    package_build = commands.add_parser("build", help="build an Arch binary package without installing or publishing")
    package_build.add_argument("--development", action="store_true", help="allow dirty source; emit no public update metadata")
    package_build.add_argument("--plan", action="store_true", help="check source/base and print intended build without executing it")
    package_build.add_argument("--reuse-build", action="store_true", help="development only: use existing binaries, not release evidence")
    args = parser.parse_args()
    if args.command == "metadata":
        print(json.dumps(metadata(args.package, args.commit), indent=2))
    elif args.command == "stage":
        stage(args.bin_dir, args.output)
    elif args.command == "build":
        (ROOT / "target").mkdir(exist_ok=True)
        with (ROOT / "target/rad-validation.lock").open("w") as lock:
            fcntl.flock(lock, fcntl.LOCK_EX)
            build(args.development, args.plan, args.reuse_build)


if __name__ == "__main__":
    try:
        main()
    except (ValueError, OSError, subprocess.SubprocessError) as error:
        print(f"RAD release: {error}", file=sys.stderr)
        sys.exit(1)
