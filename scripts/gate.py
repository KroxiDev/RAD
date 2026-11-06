#!/usr/bin/env python3
"""Gate offline: prepara artefactos Cargo y ejecuta pruebas con estado privado."""
import argparse
from concurrent.futures import ThreadPoolExecutor
from datetime import datetime, timezone
import json
import os
from pathlib import Path
import shutil
import signal
import subprocess
import tempfile
import threading
import time

ROOT = Path(__file__).resolve().parents[1]
CARGO_FLAGS = ["--offline", "--locked", "--release", "--workspace", "--all-features"]
CONFORMANCE = {"audio-media": "official_audio_media", "obs": "official_obs", "metrics": "official_metrics"}


def stamp():
    return datetime.now(timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")


def tree():
    """Incluye archivos nuevos sin modificar el índice real del usuario."""
    index = subprocess.check_output(["git", "rev-parse", "--git-path", "index"], cwd=ROOT, text=True).strip()
    with tempfile.TemporaryDirectory(prefix="rad-gate-index-") as scratch:
        copy = Path(scratch) / "index"
        shutil.copyfile(ROOT / index, copy)
        env = os.environ | {"GIT_INDEX_FILE": str(copy)}
        subprocess.run(["git", "add", "-A"], cwd=ROOT, env=env, check=True)
        return subprocess.check_output(["git", "write-tree"], cwd=ROOT, env=env, text=True).strip()


def stop_process(process):
    """También alcanza fixtures que crean una sesión propia; solo descendientes nuestros."""
    pending, owned = [process.pid], []
    while pending:
        pid = pending.pop()
        try:
            fd = os.pidfd_open(pid)
        except ProcessLookupError:
            continue
        owned.append(fd)
        for path in Path(f"/proc/{pid}/task").glob("*/children"):
            try:
                pending.extend(map(int, path.read_text().split()))
            except FileNotFoundError:
                pass
    for sig in (signal.SIGTERM, signal.SIGKILL):
        for fd in reversed(owned):
            try:
                signal.pidfd_send_signal(fd, sig)
            except ProcessLookupError:
                pass
        if sig == signal.SIGTERM:
            time.sleep(.1)
    for fd in owned:
        os.close(fd)
    process.wait()


class Gate:
    def __init__(self, mode):
        self.mode = mode
        self.started = time.monotonic()
        self.run = ROOT / "target/gate" / (datetime.now(timezone.utc).strftime("%Y%m%dT%H%M%SZ") + f"-{os.getpid()}")
        self.run.mkdir(parents=True)
        # Los fixtures agregan nombres y sockets propios: reservar margen bajo 108 bytes.
        self.scratch = tempfile.TemporaryDirectory(prefix="rg", dir="/tmp")
        self.environment_count = 0
        self.results = []
        self.lock = threading.Lock()
        self.cancelled = threading.Event()
        self.initial_tree = tree()
        commit = subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=ROOT, text=True).strip()
        (self.run / "estado.txt").write_text(
            f"modo {mode}\ncommit {commit}\narbol {self.initial_tree}\njobs {os.environ['CARGO_BUILD_JOBS']}\ninicio {stamp()}\n")
        print(f"Gate {mode} en {self.run.relative_to(ROOT)} (jobs {os.environ['CARGO_BUILD_JOBS']})", flush=True)

    def environment(self, name, plugin=False):
        # Rutas cortas: los sockets Unix tienen un límite de longitud reducido.
        with self.lock:
            self.environment_count += 1
            base = Path(self.scratch.name) / f"{self.environment_count:02}"
        env = os.environ.copy()
        for variable, child in (("XDG_RUNTIME_DIR", "r"), ("XDG_STATE_HOME", "s"), ("TMPDIR", "t")):
            path = base / child
            path.mkdir(parents=True, mode=0o700, exist_ok=True)
            env[variable] = str(path)
        if name in ("test-raiz", "test-rad-update", "conformidad-audio-media", "conformidad-obs"):
            # El instalador y estos harness validan /tmp como padre; sus fixtures ya
            # reservan raíces privadas únicas. XDG sigue siendo propio de la etapa.
            env["TMPDIR"] = "/tmp"
        env["PYTHONDONTWRITEBYTECODE"] = "1"
        env.setdefault("RUST_TEST_THREADS", str(max(1, int(env["CARGO_BUILD_JOBS"]) // 2)))
        if plugin:
            env["CARGO_TARGET_DIR"] = str(ROOT / "target/plugins")
        return env

    def command(self, command, log, env, cwd=ROOT, output=None, timeout=None):
        if self.cancelled.is_set():
            return 130
        print("+ " + " ".join(map(str, command)), file=log, flush=True)
        process = subprocess.Popen(command, cwd=cwd, env=env, stdout=output or log,
                                   stderr=log, start_new_session=True)
        started = time.monotonic()
        while process.poll() is None:
            if self.cancelled.wait(.01):
                stop_process(process)
                return 130
            if timeout is not None and time.monotonic() - started >= timeout:
                print(f"Plazo agotado: {timeout} s", file=log)
                stop_process(process)
                return 124
        return process.returncode

    def step(self, name, command=None, *, plugin=False, artifact=None, tests=None, arguments=(), timeout=None):
        started = time.monotonic()
        print(f"== {name}", flush=True)
        code = 1
        with (self.run / f"{name}.log").open("w") as log:
            try:
                env = self.environment(name, plugin)
                if tests is not None:
                    code = 0
                    for test in tests:
                        code = self.command([test["executable"], *arguments], log, env,
                                            Path(test["manifest_path"]).parent, timeout=timeout)
                        if code:
                            break
                elif artifact:
                    with artifact.open("w") as output:
                        code = self.command(command, log, env, output=output)
                else:
                    code = self.command(command, log, env)
            except (OSError, ValueError) as error:
                code = 1
                print(str(error), file=log)
        elapsed = time.monotonic() - started
        with self.lock:
            self.results.append((name, code, elapsed))
            with (self.run / "resumen.tsv").open("a") as summary:
                summary.write(f"{name}\t{code}\t{elapsed:.3f}\n")
        if code:
            print(f"   falló ({code}): {self.run.relative_to(ROOT)}/{name}.log", flush=True)
        return code == 0

    def cargo(self, operation, plugin=False, *extra):
        manifest = ["--manifest-path", "plugins/Cargo.toml"] if plugin else []
        return ["cargo", operation, *manifest, *CARGO_FLAGS, *extra]

    def prepare(self):
        for plugin, name in ((False, "raiz"), (True, "plugins")):
            if not self.step(f"build-{name}", self.cargo("build", plugin, "--bins", "--examples"), plugin=plugin):
                return None
        if not self.step("build-metrics-fixture", ["cargo", "build", "--manifest-path", "scripts/conformance/metrics/fixture/Cargo.toml", "--offline", "--locked", "--release"], plugin=True):
            return None
        suites = {}
        for plugin, name in ((False, "raiz"), (True, "plugins")):
            path = self.run / f"tests-{name}.jsonl"
            if not self.step(f"build-tests-{name}", self.cargo("test", plugin, "--no-run", "--message-format=json"), plugin=plugin, artifact=path):
                return None
            artifacts = [json.loads(line) for line in path.read_text().splitlines()]
            suites[name] = [item for item in artifacts if item.get("reason") == "compiler-artifact"
                            and item.get("executable") and item["profile"]["test"]]
            if not suites[name] or not any(item.get("reason") == "build-finished" and item.get("success") for item in artifacts):
                raise ValueError(f"Cargo no produjo la suite completa de {name}")
        return suites

    def execute(self, suites, kinds=None):
        kinds = list(CONFORMANCE) if kinds is None else kinds
        tasks = []
        if self.mode == "completo":
            for name in ("raiz", "plugins"):
                tasks.append((f"test-{name}", dict(tests=suites[name])))
            tasks.append(("packaging", dict(command=["python3", "-B", "-m", "unittest", "discover", "-s", "packaging/tests", "-v"])))
        out = ROOT / "target/conformance" / self.run.name
        out.mkdir(parents=True)
        for kind in kinds:
            names = ("audio", "media") if kind == "audio-media" else (kind,)
            for name in names:
                destination = out / name
                destination.mkdir()
                tasks.append((f"checker-{name}", dict(command=["python3", "-I", "-S", "-B", "scripts/conformance/audio-media/checker.py", str(destination), name])))
            tests = [item for item in suites["raiz"] if item["target"]["name"] == CONFORMANCE[kind]]
            if len(tests) != 1:
                raise ValueError(f"Se esperaba un único artefacto para {CONFORMANCE[kind]}")
            tasks.append((f"conformidad-{kind}", dict(tests=tests, arguments=("--ignored", "--nocapture"),
                                                    timeout=120 if kind == "metrics" else None)))
        if self.mode == "conformance" and "obs" in kinds:
            oracle = [item for item in suites["plugins"] if item["target"]["name"] == "obs-conformance-server"]
            if len(oracle) != 1:
                raise ValueError("Falta el artefacto de pruebas de obs-conformance-server")
            tasks.append(("oracle-obs", dict(tests=oracle)))
        with ThreadPoolExecutor(max_workers=len(tasks)) as pool:
            futures = [pool.submit(self.step, name, **options) for name, options in tasks]
            return all([future.result() for future in futures])

    def finish(self):
        elapsed = time.monotonic() - self.started
        final_tree = tree()
        with (self.run / "estado.txt").open("a") as state:
            state.write(f"fin {stamp()}\nsegundos {elapsed:.3f}\n")
            if final_tree != self.initial_tree:
                state.write(f"arbol-final {final_tree}\n")
        valid = bool(self.results) and all(code == 0 for _, code, _ in self.results)
        if final_tree != self.initial_tree:
            print("El árbol cambió durante el gate: el resultado no certifica ningún estado.", flush=True)
            valid = False
        if self.cancelled.is_set():
            valid = False
        for name, code, seconds in self.results:
            print(f"{name:<28} {code:>3} {seconds:>8.3f}")
        print(f"Tiempo total: {elapsed:.3f} s", flush=True)
        print(f"GATE {'OK' if valid else 'FALLÓ'} ({self.mode})  árbol {self.initial_tree}  ({self.run.relative_to(ROOT)})", flush=True)
        self.scratch.cleanup()
        return 0 if valid else 1


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("mode", nargs="?", choices=("completo", "version", "conformance"), default="completo")
    parser.add_argument("kind", nargs="?", choices=tuple(CONFORMANCE))
    args = parser.parse_args()
    if (args.mode == "conformance") != bool(args.kind):
        parser.error("conformance requiere audio-media, obs o metrics")
    gate = Gate(args.mode)
    for sig in (signal.SIGINT, signal.SIGTERM):
        signal.signal(sig, lambda _sig, _frame: gate.cancelled.set())
    try:
        if args.mode == "version":
            gate.step("test-rad-update", ["cargo", "test", "--offline", "--locked", "--release", "-p", "rad-update", "--all-features"])
            gate.step("packaging", ["python3", "-B", "-m", "unittest", "discover", "-s", "packaging/tests", "-v"])
        else:
            ready = True
            if args.mode == "completo":
                ready = gate.step("test-gate", ["python3", "-B", "-m", "unittest", "discover", "-s", "scripts", "-p", "test_gate.py", "-v"])
                for plugin, name in ((False, "raiz"), (True, "plugins")):
                    manifest = ["--manifest-path", "plugins/Cargo.toml"] if plugin else []
                    ready = gate.step(f"fmt-{name}", ["cargo", "fmt", *manifest, "--all", "--", "--check"]) and ready
            suites = gate.prepare() if ready else None
            if suites is not None:
                # Cargo solo corre en preparación; doctests y Clippy conservan su cobertura.
                if args.mode == "completo":
                    for plugin, name in ((False, "raiz"), (True, "plugins")):
                        ready = gate.step(f"doc-tests-{name}", gate.cargo("test", plugin, "--doc"), plugin=plugin) and ready
                        ready = gate.step(f"clippy-{name}", gate.cargo("clippy", plugin, "--all-targets", "--", "-D", "warnings"), plugin=plugin) and ready
                if ready:
                    gate.execute(suites, [args.kind] if args.kind else None)
    except (OSError, ValueError, subprocess.SubprocessError) as error:
        print(f"Gate no verificado: {error}", flush=True)
        gate.results.append(("preparacion", 1, 0))
    return gate.finish()


if __name__ == "__main__":
    raise SystemExit(main())
