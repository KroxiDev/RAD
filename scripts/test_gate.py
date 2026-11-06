"""Contrato del gate con compilador y procesos sintéticos, sin compilar RAD."""
import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import time
import unittest


REPO = Path(__file__).resolve().parents[1]


class GateTests(unittest.TestCase):
    def setUp(self):
        self.scratch = tempfile.TemporaryDirectory(prefix="rad-gate-test-")
        self.addCleanup(self.scratch.cleanup)
        self.root = Path(self.scratch.name)
        shutil.copytree(REPO / "scripts", self.root / "scripts")
        (self.root / ".gitignore").write_text("target/\ntrace.jsonl\nmarkers/\n")
        (self.root / "tracked").write_text("original\n")
        self.bin = self.root / "bin"
        self.bin.mkdir()
        self.write_executable(self.bin / "cargo", '''#!/usr/bin/env python3
import json, os, pathlib, sys
root = pathlib.Path(os.environ['TEST_ROOT'])
args = sys.argv[1:]
with (root/'trace.jsonl').open('a') as f: f.write(json.dumps({'cargo':args})+'\\n')
if args[0] == 'build' and os.environ.get('TEST_FAIL') == 'build': sys.exit(21)
if '--no-run' in args:
    plugin = '--manifest-path' in args and args[args.index('--manifest-path')+1] == 'plugins/Cargo.toml'
    names = ['plugins'] if plugin else ['root','official_audio_media','official_obs','official_metrics']
    for name in names:
        print(json.dumps({'reason':'compiler-artifact','executable':str(root/'bin'/name),
            'manifest_path':str(root/'package/Cargo.toml'),'profile':{'test':True},
            'target':{'name':name,'kind':['test']}}))
    print(json.dumps({'reason':'build-finished','success':True}))
''')
        # Estos comandos son validadores sin efectos en el checkout sintético.
        self.write_executable(self.bin / "rustfmt", "#!/bin/sh\nexit 0\n")
        for name in ("root", "plugins", "official_audio_media", "official_obs", "official_metrics"):
            self.write_executable(self.bin / name, '''#!/usr/bin/env python3
import json, os, pathlib, sys
root = pathlib.Path(os.environ['TEST_ROOT'])
name = pathlib.Path(sys.argv[0]).name
with (root/'trace.jsonl').open('a') as f:
    f.write(json.dumps({'test':name,'args':sys.argv[1:],'cwd':os.getcwd()})+'\\n')
if name == os.environ.get('TEST_FAIL'): sys.exit(17)
if name == 'root' and os.environ.get('TEST_MUTATE'): (root/'tracked').write_text('changed\\n')
''')
        (self.root / "package").mkdir()
        (self.root / "scripts/conformance/audio-media/checker.py").write_text('''import json, os, pathlib, subprocess, sys, time
root = pathlib.Path(os.environ['TEST_ROOT'])
markers = root/'markers'
markers.mkdir(exist_ok=True)
for name in sys.argv[2:]:
    with (root/'trace.jsonl').open('a') as f:
        f.write(json.dumps({'checker':name,'runtime':os.environ['XDG_RUNTIME_DIR']})+'\\n')
    (markers/name).touch()
    if os.environ.get('TEST_WAIT'):
        child = subprocess.Popen([sys.executable, '-c', 'import time; time.sleep(30)'], start_new_session=True)
        (root/(name+'.pid')).write_text(str(child.pid))
        child.wait()
    end = time.monotonic()+2
    while len(list(markers.iterdir())) < 4:
        if time.monotonic() > end: raise SystemExit('los cuatro checkers deben solaparse')
        time.sleep(.005)
    if name == os.environ.get('TEST_FAIL'): raise SystemExit(19)
''')
        (self.root / "packaging/tests").mkdir(parents=True)
        trivial_test = "import unittest\nclass Check(unittest.TestCase):\n    def test_fixture(self): pass\n"
        (self.root / "packaging/tests/test_fixture.py").write_text(trivial_test)
        # El gate prueba su orquestación real; el checkout hijo no se prueba a sí mismo.
        (self.root / "scripts/test_gate.py").write_text(trivial_test)
        self.env = os.environ | {
            "TEST_ROOT": str(self.root),
            "PATH": str(self.bin) + ":" + os.environ["PATH"],
            "PYTHONDONTWRITEBYTECODE": "1",
        }
        for args in (["init", "-q"], ["add", "."], ["-c", "user.name=Fixture", "-c", "user.email=fixture@example.test", "commit", "-qm", "fixture"]):
            subprocess.run(["git", *args], cwd=self.root, env=self.env, check=True, capture_output=True)

    @staticmethod
    def write_executable(path, content):
        path.write_text(content)
        path.chmod(0o755)

    def run_gate(self, **extra):
        return subprocess.run(["bash", "scripts/gate.sh"], cwd=self.root,
                              env=self.env | extra, capture_output=True, text=True, timeout=20)

    def test_parallel_checks_reuse_builds_and_preserve_test_execution(self):
        result = self.run_gate()
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        trace = [json.loads(line) for line in (self.root / "trace.jsonl").read_text().splitlines()]
        checks = [entry for entry in trace if "checker" in entry]
        self.assertEqual({entry["checker"] for entry in checks}, {"audio", "media", "obs", "metrics"})
        self.assertEqual(len({entry["runtime"] for entry in checks}), 4)
        first_check = next(i for i, entry in enumerate(trace) if "checker" in entry)
        self.assertFalse(any("cargo" in entry for entry in trace[first_check:]), "no compilar durante ejecución")
        tests = [entry for entry in trace if "test" in entry]
        for name in ("root", "plugins"):
            self.assertEqual(sum(entry["test"] == name for entry in tests), 1)
        for name in ("official_audio_media", "official_obs", "official_metrics"):
            self.assertEqual(sum(entry["test"] == name and "--ignored" in entry["args"] for entry in tests), 1)
        self.assertTrue(all(entry["cwd"] == str(self.root / "package") for entry in tests))
        self.assertIn("GATE OK", result.stdout)
        state = next((self.root / "target/gate").glob("*/estado.txt")).read_text()
        self.assertIn("segundos ", state)

    def test_failed_build_does_not_run_stale_artifacts(self):
        result = self.run_gate(TEST_FAIL="build")
        self.assertNotEqual(result.returncode, 0)
        trace = [json.loads(line) for line in (self.root / "trace.jsonl").read_text().splitlines()]
        self.assertFalse(any("test" in entry or "checker" in entry for entry in trace))
        self.assertIn("GATE FALLÓ", result.stdout)

    def test_failed_parallel_checker_rejects_gate(self):
        result = self.run_gate(TEST_FAIL="obs")
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("GATE FALLÓ", result.stdout)
        summary = next((self.root / "target/gate").glob("*/resumen.tsv")).read_text()
        self.assertIn("checker-obs\t19\t", summary)

    def test_failed_native_suite_rejects_gate(self):
        result = self.run_gate(TEST_FAIL="root")
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("GATE FALLÓ", result.stdout)

    def test_unexecutable_artifact_rejects_gate(self):
        (self.bin / "root").chmod(0o644)
        result = self.run_gate()
        self.assertNotEqual(result.returncode, 0, result.stdout)
        summary = next((self.root / "target/gate").glob("*/resumen.tsv")).read_text()
        self.assertIn("test-raiz\t1\t", summary)

    def test_changed_tree_rejects_gate_without_touching_user_index(self):
        index = (self.root / ".git/index").read_bytes()
        result = self.run_gate(TEST_MUTATE="1")
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("El árbol cambió", result.stdout)
        self.assertEqual((self.root / ".git/index").read_bytes(), index)

    def test_interruption_stops_children_even_in_other_process_groups(self):
        process = subprocess.Popen(["bash", "scripts/gate.sh"], cwd=self.root,
                                   env=self.env | {"TEST_WAIT": "1"}, stdout=subprocess.PIPE,
                                   stderr=subprocess.STDOUT, text=True)
        try:
            end = time.monotonic() + 10
            while len(list(self.root.glob("*.pid"))) < 4:
                self.assertLess(time.monotonic(), end, "checkers did not start")
                time.sleep(.01)
            process.terminate()
            output = process.communicate(timeout=5)[0]
            self.assertNotEqual(process.returncode, 0, output)
            for file in self.root.glob("*.pid"):
                pid = int(file.read_text())
                stat = Path(f"/proc/{pid}/stat")
                self.assertTrue(not stat.exists() or stat.read_text().split()[2] == "Z", f"live fixture {pid}")
        finally:
            if process.poll() is None:
                process.kill()
                process.wait()


if __name__ == "__main__":
    unittest.main()
