"""Contratos críticos del artefacto distribuible, con paquetes sintéticos."""
import hashlib
import io
import json
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


class ReleaseContract(unittest.TestCase):
    def package(self, directory, version=None, startdir=None):
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


if __name__ == "__main__":
    unittest.main()
