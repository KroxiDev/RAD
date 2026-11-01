"""Contratos críticos del artefacto distribuible, con paquetes sintéticos."""
import hashlib
import io
import json
import os
from pathlib import Path
import subprocess
import shutil
import tarfile
import tempfile
import unittest
import importlib.util

ROOT = Path(__file__).resolve().parents[2]
SOURCE = json.loads((ROOT / "packaging/release.json").read_text())
FILENAME = f"{SOURCE['package']}-{SOURCE['version']}-{SOURCE['pkgrel']}-x86_64.pkg.tar.zst"

SPEC = importlib.util.spec_from_file_location("rad_release", ROOT / "packaging/release.py")
TOOL = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(TOOL)


def synthetic_package(directory, version=None, startdir=None):
    path = Path(directory) / FILENAME
    info = (f"pkgname = rust-action-deck\npkgver = {version or SOURCE['version'] + '-' + str(SOURCE['pkgrel'])}\narch = x86_64\n").encode()
    with tarfile.open(path, "w") as archive:
        member = tarfile.TarInfo(".PKGINFO")
        member.size = len(info)
        archive.addfile(member, io.BytesIO(info))
        if startdir is not None:
            buildinfo = f"format = 2\nstartdir = {startdir}\nbuilddir = /tmp/rad-package-fixture\n".encode()
            entry = tarfile.TarInfo(".BUILDINFO")
            entry.size = len(buildinfo)
            archive.addfile(entry, io.BytesIO(buildinfo))
    return path


class ReleaseContract(unittest.TestCase):
    def package(self, directory, version=None, startdir=None):
        return synthetic_package(directory, version, startdir)

    def run_metadata(self, path):
        return subprocess.run([
            "python3", str(ROOT / "packaging/release.py"), "metadata",
            "--package", str(path), "--commit", "1" * 40,
        ], text=True, capture_output=True)

    def test_metadata_binds_exact_package_bytes_and_arch_revision(self):
        with tempfile.TemporaryDirectory() as directory:
            package = self.package(directory)
            result = self.run_metadata(package)
            self.assertEqual(result.returncode, 0, result.stderr)
            metadata = json.loads(result.stdout)
            self.assertEqual(metadata, {
                **SOURCE, "file": FILENAME, "size": package.stat().st_size,
                "sha256": hashlib.sha256(package.read_bytes()).hexdigest(),
                "commit": "1" * 40,
            })

    def test_rejects_package_with_another_version_despite_expected_filename(self):
        with tempfile.TemporaryDirectory() as directory:
            package = self.package(directory, "9.9.9-7")
            result = self.run_metadata(package)
            self.assertNotEqual(result.returncode, 0)
            self.assertEqual(result.stdout, "")

    def test_stage_contains_only_production_siblings_and_stable_desktop_units(self):
        with tempfile.TemporaryDirectory() as directory:
            base = Path(directory)
            binaries = base / "bin"
            binaries.mkdir()
            names = {"rad-editor", "rad-runtime", "rad-launcher-helper", "rad-tray", "rad-cli",
                     "rad-update-coordinator", "rad-update-installer"}
            for name in names | {"plugin-integration-fixture", "rad-launcher-fixture"}:
                (binaries / name).write_bytes(Path("/usr/bin/true").read_bytes())
            stage = base / "stage"
            result = subprocess.run([
                "python3", str(ROOT / "packaging/release.py"), "stage",
                "--bin-dir", str(binaries), "--output", str(stage),
            ], capture_output=True, text=True)
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertEqual({p.name for p in (stage / "usr/lib/rad").iterdir()}, names)
            entry = (stage / "usr/share/applications/io.github.KroxiDev.RAD.desktop").read_text()
            self.assertIn("Exec=/usr/lib/rad/rad-editor\n", entry)
            unit = (stage / "usr/lib/systemd/user/rad-runtime.service").read_text()
            self.assertIn("ExecStart=/usr/lib/rad/rad-runtime\n", unit)
            self.assertNotIn("target/release", unit + entry)
            self.assertTrue((stage / "usr/bin/rad-editor").is_symlink())
            self.assertTrue((stage / "usr/share/licenses/rust-action-deck/LICENSE").is_file())
            self.assertEqual(
                (stage / "usr/share/licenses/rust-action-deck/third-party/phosphor/LICENSE").read_bytes(),
                (ROOT / "crates/rad-editor/assets/phosphor/LICENSE").read_bytes())
            self.assertFalse(any(p.name in {".env", "store.db", "editor.json", "examples"}
                                 for p in stage.rglob("*")))

    def test_production_build_refuses_a_dirty_tree_before_invoking_cargo(self):
        with tempfile.TemporaryDirectory() as directory:
            repo = Path(directory)
            (repo / "packaging").mkdir()
            for name in ("release.py", "release.json"):
                shutil.copyfile(ROOT / "packaging" / name, repo / "packaging" / name)
            subprocess.run(["git", "init", "--quiet", str(repo)], check=True)
            result = subprocess.run([
                "python3", str(repo / "packaging/release.py"), "build", "--plan",
            ], text=True, capture_output=True)
            self.assertNotEqual(result.returncode, 0)
            self.assertIn("dirty", result.stderr.lower())
            self.assertNotIn("cargo build", result.stdout)

    def test_embedded_fonts_preserve_their_individual_notices(self):
        with tempfile.TemporaryDirectory() as directory:
            cache = Path(directory) / "fonts-crate"
            (cache / "fonts").mkdir(parents=True)
            (cache / "LICENSE-MIT").write_text("crate license")
            (cache / "fonts/UFL.txt").write_text("Ubuntu font license")
            (cache / "fonts/Hack-Regular.txt").write_text("Hack font notice")
            package = {"name": "epaint_default_fonts", "version": "0.36.2", "source": "registry",
                       "manifest_path": str(cache / "Cargo.toml"), "license_file": None}
            output = Path(directory) / "stage"
            TOOL.collect_licenses([package], output)
            notices = output / "usr/share/licenses/rust-action-deck/third-party/epaint_default_fonts-0.36.2"
            self.assertEqual((notices / "fonts/UFL.txt").read_text(), "Ubuntu font license")
            self.assertEqual((notices / "fonts/Hack-Regular.txt").read_text(), "Hack font notice")

    def test_missing_cache_notice_uses_only_exact_revision_pinned_upstream_text(self):
        with tempfile.TemporaryDirectory() as directory:
            cache = Path(directory) / "accesskit"
            cache.mkdir()
            (cache / ".cargo_vcs_info.json").write_text(json.dumps({"git": {
                "sha1": "a55d3e1a18bb9ef0e4bccc9083fb13c3e0ad8969"}}))
            package = {"name": "accesskit", "version": "0.24.1", "source": "registry",
                       "manifest_path": str(cache / "Cargo.toml"), "license_file": None}
            output = Path(directory) / "stage"
            TOOL.collect_licenses([package], output)
            notices = output / "usr/share/licenses/rust-action-deck/third-party/accesskit-0.24.1"
            self.assertEqual((notices / "LICENSE-MIT").read_bytes(),
                             (ROOT / "packaging/notices/accesskit/LICENSE-MIT").read_bytes())
            self.assertTrue((notices / "AUTHORS").is_file())
            (cache / ".cargo_vcs_info.json").write_text(json.dumps({"git": {"sha1": "0" * 40}}))
            with self.assertRaisesRegex(ValueError, "revision"):
                TOOL.collect_licenses([package], Path(directory) / "other-stage")

    def test_package_environment_cannot_restore_personal_paths_or_signing(self):
        hostile = {"PKGEXT": ".pkg.tar.review", "PACKAGER": "private synthetic name",
                   "BUILDDIR": "/synthetic/wrong", "GPGKEY": "synthetic-unused-key",
                   "CARCH": "aarch64", "PATH": "/usr/bin"}
        fixed = {"PKGEXT": ".pkg.tar.zst", "PACKAGER": "RAD release build",
                 "BUILDDIR": "/synthetic/build"}
        environment = TOOL.makepkg_environment(hostile, fixed)
        self.assertEqual(environment["PKGEXT"], ".pkg.tar.zst")
        self.assertEqual(environment["PACKAGER"], "RAD release build")
        self.assertEqual(environment["BUILDDIR"], "/synthetic/build")
        self.assertEqual(environment["CARCH"], "x86_64")
        self.assertNotIn("GPGKEY", environment)
        self.assertEqual(environment["PATH"], "/usr/bin")

    def test_metadata_rejects_personal_build_directories(self):
        with tempfile.TemporaryDirectory() as directory:
            package = self.package(directory, startdir="/home/synthetic/checkout")
            result = self.run_metadata(package)
            self.assertNotEqual(result.returncode, 0)
            self.assertIn("neutral", result.stderr.lower())
            self.assertEqual(result.stdout, "")


TAG = f"v{SOURCE['version']}-{SOURCE['pkgrel']}"
# Records every call and answers like gh: no releases unless told otherwise.
FAKE_GH = """#!/usr/bin/env python3
import json, os, sys
args = sys.argv[1:]
with open(os.environ["FAKE_GH_LOG"], "a") as log:
    log.write(json.dumps(args) + "\\n")
if args[:2] == ["release", "list"]:
    print(os.environ.get("FAKE_GH_RELEASES", "[]"))
elif args[:2] == ["release", "create"]:
    print("https://github.com/KroxiDev/RAD/releases/tag/" + args[2])
else:
    sys.exit(2)
"""


class PublishContract(unittest.TestCase):
    """A release is created only from a verified production build of the current main."""

    def setUp(self):
        self.base = Path(tempfile.mkdtemp())
        self.addCleanup(shutil.rmtree, self.base)
        bindir = self.base / "bin"
        bindir.mkdir()
        (bindir / "gh").write_text(FAKE_GH)
        (bindir / "gh").chmod(0o755)
        self.log = self.base / "gh.log"
        self.env = {**os.environ, "PATH": f"{bindir}:{os.environ['PATH']}",
                    "FAKE_GH_LOG": str(self.log), "GIT_CONFIG_GLOBAL": "/dev/null",
                    "GIT_CONFIG_NOSYSTEM": "1", "GIT_AUTHOR_NAME": "Synthetic",
                    "GIT_AUTHOR_EMAIL": "synthetic@example.invalid",
                    "GIT_COMMITTER_NAME": "Synthetic",
                    "GIT_COMMITTER_EMAIL": "synthetic@example.invalid"}
        self.origin = self.base / "origin.git"
        self.repo = self.base / "repo"
        self.git("init", "--quiet", "--bare", "--initial-branch=main", str(self.origin), cwd=self.base)
        self.git("init", "--quiet", "--initial-branch=main", str(self.repo), cwd=self.base)
        (self.repo / "packaging").mkdir()
        for name in ("release.py", "release.json"):
            shutil.copyfile(ROOT / "packaging" / name, self.repo / "packaging" / name)
        self.git("add", "packaging")
        self.git("commit", "--quiet", "-m", "source")
        self.git("remote", "add", "origin", str(self.origin))
        self.git("push", "--quiet", "origin", "main")
        self.commit = self.git("rev-parse", "HEAD")
        self.dist = self.base / "dist"
        self.dist.mkdir()
        self.package = synthetic_package(self.dist)
        recorded = subprocess.run([
            "python3", str(self.repo / "packaging/release.py"), "metadata",
            "--package", str(self.package), "--commit", self.commit,
        ], text=True, capture_output=True, check=True).stdout
        (self.dist / "rad-update.json").write_text(recorded)
        digest = json.loads(recorded)["sha256"]
        (self.dist / "SHA256SUMS").write_text(f"{digest}  {FILENAME}\n")

    def git(self, *args, cwd=None):
        return subprocess.run(["git", *args], cwd=cwd or self.repo, env=self.env, check=True,
                              capture_output=True, text=True).stdout.strip()

    def publish(self, *extra, releases="[]"):
        return subprocess.run([
            "python3", str(self.repo / "packaging/release.py"), "publish", "--dist", str(self.dist), *extra,
        ], cwd=self.repo, env={**self.env, "FAKE_GH_RELEASES": releases}, text=True, capture_output=True)

    def created(self):
        calls = [json.loads(line) for line in self.log.read_text().splitlines()] if self.log.exists() else []
        return [call for call in calls if call[:2] == ["release", "create"]]

    def test_creates_a_draft_titled_by_version_with_empty_notes_and_the_three_assets(self):
        result = self.publish()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(self.created(), [[
            "release", "create", TAG, "--repo", "KroxiDev/RAD", "--target", self.commit,
            "--title", f"RAD {SOURCE['version']}-{SOURCE['pkgrel']}", "--notes", "", "--draft",
            str(self.package), str(self.dist / "rad-update.json"), str(self.dist / "SHA256SUMS"),
        ]])

    def test_publish_flag_makes_it_visible_to_update_rad_at_once(self):
        result = self.publish("--publish")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(len(self.created()), 1)
        self.assertNotIn("--draft", self.created()[0])

    def test_refuses_a_build_that_is_no_longer_the_current_main(self):
        (self.repo / "later.txt").write_text("merged later\n")
        self.git("add", "later.txt")
        self.git("commit", "--quiet", "-m", "later")
        self.git("push", "--quiet", "origin", "main")
        result = self.publish()
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("main", result.stderr)
        self.assertEqual(self.created(), [])

    def test_refuses_a_version_that_already_has_a_tag_or_a_draft(self):
        self.git("tag", TAG)
        self.git("push", "--quiet", "origin", TAG)
        tagged = self.publish()
        self.assertNotEqual(tagged.returncode, 0)
        self.assertIn("release.json", tagged.stderr)
        self.git("push", "--quiet", "--delete", "origin", TAG)
        drafted = self.publish(releases=json.dumps([{"tagName": TAG}]))
        self.assertNotEqual(drafted.returncode, 0)
        self.assertIn(TAG, drafted.stderr)
        self.assertEqual(self.created(), [])

    def test_refuses_development_or_altered_artifacts_before_contacting_github(self):
        (self.dist / "SHA256SUMS").write_text(f"{'0' * 64}  {FILENAME}\n")
        altered = self.publish()
        self.assertNotEqual(altered.returncode, 0)
        (self.dist / "SHA256SUMS").unlink()
        (self.dist / "DEVELOPMENT-NOT-FOR-RELEASE").write_text("local only\n")
        development = self.publish()
        self.assertNotEqual(development.returncode, 0)
        self.assertIn("development", development.stderr.lower())
        self.assertFalse(self.log.exists(), "GitHub must not be contacted for an invalid artifact")


if __name__ == "__main__":
    unittest.main()
