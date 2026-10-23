#!/usr/bin/env python3
"""Critical runtime/IPC contract; only the installed deterministic metrics fixture."""
import importlib.util
import json
import os
from pathlib import Path
import subprocess
import sys
import time

ROOT, REPO = (Path(value).resolve() for value in sys.argv[1:3])
PHASE = sys.argv[3]
assert ROOT.parent == REPO / "target" and ROOT.name.startswith("f5-")
PROFILE = PHASE == "fault" and os.environ.get("RAD_PROFILE_INTEGRATED_METRICS") == "1"
spec = importlib.util.spec_from_file_location("private_ipc", REPO / "scripts/conformance/obs/check.py")
ipc = importlib.util.module_from_spec(spec)
spec.loader.exec_module(ipc)
Peer, equal, until = ipc.Peer, ipc.equal, ipc.until
SOURCES = ["cpu", "ram", "gpu", "vram"]
TEXTS = ["CPU 25%", "RAM 8.0 GiB used 8.0 GiB avail", "GPU 73%", "VRAM 2.0/8.0 GiB"]


def profile(kind, **fields):
    if PROFILE:
        print(json.dumps(dict(suite="integrated_metrics_faults", kind=kind, **fields), sort_keys=True), flush=True)


def measured(operation, work, **fields):
    if not PROFILE:
        return work()
    started = time.monotonic_ns()
    value = work()
    elapsed = time.monotonic_ns() - started
    profile("sample", operation=operation, ns=elapsed, **fields)
    return value


class Runtime:
    def __init__(self):
        self.child = None
        self.peer = None
        self.cleanup = {}
        self.log = (ROOT / ("runtime-" + PHASE + ".log")).open("wb")
        env = {"PATH": "/usr/bin:/bin"}
        for key, directory in [
            ("HOME", "home"), ("XDG_RUNTIME_DIR", "runtime"), ("XDG_DATA_HOME", "data"),
            ("XDG_CONFIG_HOME", "config"), ("XDG_CACHE_HOME", "cache"),
            ("XDG_STATE_HOME", "state"), ("XDG_DATA_DIRS", "system"),
            ("XDG_CONFIG_DIRS", "system"),
        ]:
            (ROOT / directory).mkdir(mode=0o700, exist_ok=True)
            env[key] = str(ROOT / directory)
        env["DBUS_SESSION_BUS_ADDRESS"] = "unix:path=" + str(ROOT / "no-bus")
        if PHASE == "fault":
            (ROOT / "runtime/f5-reader-mode").write_text("healthy")
        try:
            self.child = subprocess.Popen([str(REPO / "target/release/rad-runtime")],
                                          env=env, stdin=subprocess.DEVNULL,
                                          stdout=subprocess.DEVNULL, stderr=self.log)
            deadline = time.monotonic() + 10
            while not (ROOT / "runtime/rad/editor.sock").exists():
                assert self.child.poll() is None, "runtime startup failed"
                assert time.monotonic() < deadline, "runtime startup timeout"
                time.sleep(.025)
            self.peer = Peer(ROOT)
            expected_host = "absent" if PHASE == "installed" else "ready"
            until(lambda: self.peer.call("plugin_status"),
                  lambda v: v["state"] == "idle" and len(v["items"]) == 1 and v["items"][0]["host"] == expected_host)
            equal("hardware disabled", self.peer.call("status")["deck_state"], "disabled")
        except BaseException:
            self.close(require_success=False)
            raise

    def close(self, require_success=True):
        started = time.monotonic_ns() if PROFILE else None
        peer_returncode = None
        if self.peer is not None:
            peer_child = self.peer.child
            self.peer.close()
            peer_returncode = peer_child.poll()
            self.peer = None
        runtime_returncode = None
        if self.child is not None:
            if self.child.poll() is None:
                self.child.terminate()
                try:
                    self.child.wait(timeout=8)
                except subprocess.TimeoutExpired:
                    self.child.kill()
                    self.child.wait(timeout=3)
                    raise
            if self.child.returncode != 0:
                self.log.flush()
                print("RUNTIME DIAGNOSTIC", (ROOT / ("runtime-" + PHASE + ".log")).read_bytes()[:4096].decode(errors="replace"), flush=True)
            if require_success:
                equal("owned runtime orderly exit", self.child.returncode, 0)
            runtime_returncode = self.child.returncode
            self.child = None
        self.log.close()
        if PROFILE:
            self.cleanup = dict(runtime_reaped=runtime_returncode is not None,
                                runtime_returncode=runtime_returncode,
                                typed_peer_reaped=peer_returncode is not None,
                                typed_peer_returncode=peer_returncode,
                                plugin_descendants_independently_checked=False,
                                cleanup_ns=time.monotonic_ns() - started)
            profile("cleanup", **self.cleanup)


def fault_scenario(e):
    mode = ROOT / "runtime/f5-reader-mode"
    def all_visuals(availability):
        return all(e.call("plugin_visual", id=f"b{i+1}")["availability"] == availability for i in range(32))
    measured("initial_all32_current_observed", lambda: until(lambda: all_visuals("current"), bool))
    def transition(value, availability):
        mode.write_text(value)
        until(lambda: all_visuals(availability), bool, timeout=5)
    measured("set_missing_to_all32_unavailable_observed", lambda: transition("missing", "unavailable"))
    equal("reader failure leaves host ready", e.call("plugin_status")["items"][0]["host"], "ready")
    measured("set_healthy_to_all32_current_observed", lambda: transition("healthy", "current"))
    e.call("add_action", button_id="b1", type="wait", parameters=dict(milliseconds=0))
    e.call("set_permissions", target="global", value=dict(own=dict(consult=True,modify=True,execute=True)))
    measured("set_block_to_all32_stale_observed", lambda: transition("block", "stale"))
    count = int((ROOT / "runtime/f5-reader-count").read_text())
    def healthy_core():
        admitted = e.call("run", id="b1")
        return until(lambda: e.call("result", execution_id=admitted["execution_id"]),
                     lambda value: value["state"] != "running")
    for iteration in range(6):
        host = measured("blocked_plugin_status", lambda: e.call("plugin_status"), iteration=iteration)
        equal("blocked reader leaves host ready", host["items"][0]["host"], "ready")
        status = measured("blocked_healthy_status", lambda: e.call("status"), iteration=iteration)
        equal("blocked reader leaves runtime status responsive", status["deck_state"], "disabled")
        result = measured("blocked_wait_zero_result_observed", healthy_core, iteration=iteration)
        equal("healthy core action with stale visual", result["state"], "succeeded")
        time.sleep(.5)
    count_after = int((ROOT / "runtime/f5-reader-count").read_text())
    equal("blocked reader has no replacement/retry reads", count_after, count)
    equal("all32 visuals still stale", all_visuals("stale"), True)
    profile("reader_retained", count_at_blocked=count, count_after_healthy_checks=count_after,
            no_new_reads=count == count_after, healthy_iterations=6, stale_visuals=32)
    print("PASS I3 healthy -> missing -> healthy -> blocked/stale; host/status/core action responsive, no reader retry", flush=True)


def scenario(e):
    if PHASE == "fault":
        return fault_scenario(e)
    if PHASE in {"uninstall", "restored"}:
        expected = json.loads((ROOT / "saved-configurations.json").read_text())
        for i in range(32):
            button = f"b{i + 1}"
            equal("configuration retained " + button,
                  e.call("plugin_configuration", id=button, cursor=None), expected[i])
            current = until(lambda: e.call("plugin_visual", id=button),
                            lambda v: v["availability"] == "current")
            equal("recovered visual " + button, current["text"], TEXTS[i % 4])
        if PHASE == "uninstall":
            e.call("plugin_admin", operation="uninstall", plugin_id="org.rad.metrics",
                   source=None, trusted=True)
            until(lambda: e.call("plugin_status"), lambda v: v["state"] == "idle" and not v["items"])
            for i in range(32):
                button = f"b{i + 1}"
                until(lambda: e.call("plugin_visual", id=button),
                      lambda v: v["availability"] == "unavailable")
                equal("placeholder retains opaque configuration " + button,
                      e.call("plugin_configuration", id=button, cursor=None), expected[i])
            print("PASS I2 IPC uninstall retains32 placeholders and original configuration", flush=True)
        else:
            print("PASS I2 offline compatible reinstall recovers32 original visual assignments", flush=True)
        return
    assert PHASE == "installed"
    folder = e.call("status")["active_folder_id"]
    for source in SOURCES:
        schema = e.call("schema", type="org.rad.metrics#" + source, cursor=None)
        equal("schema " + source, schema["items"], [])
        equal("schema version " + source, schema["schema_version"], 1)
    for i in range(32):
        button = f"b{i + 1}"
        e.call("create_button", folder_id=folder, position=i, name="Metric" + str(i))
        e.call("edit", changes=[dict(kind="set", id=button, field="visual",
            value=dict(type="org.rad.metrics#" + SOURCES[i % 4],
                       parameters=dict(schema_version=1, values={})))])
        current = until(lambda: e.call("plugin_visual", id=button),
                        lambda v: v["availability"] == "current")
        equal("deterministic visual " + button, current["text"], TEXTS[i % 4])
        equal("no fabricated action " + button,
              e.call("get_section", id=button, section="actions", cursor=None)["items"], [])
    response = e.request("add_action", button_id="b1", type="org.rad.metrics:cpu",
                         parameters=dict(schema_version=1, values={}))
    assert response.get("error"), "visual must not become an action"
    equal("rejected invented action preserves configuration",
          e.call("get_section", id="b1", section="actions", cursor=None)["items"], [])
    saved = [e.call("plugin_configuration", id=f"b{i + 1}", cursor=None) for i in range(32)]
    (ROOT / "saved-configurations.json").write_text(json.dumps(saved))
    print("PASS I1 four sources / 32 simultaneous visual assignments / no invented actions", flush=True)


if __name__ == "__main__":
    profile("metadata", scope="runtime_server_typed_client_metrics_fixture", clock="monotonic_ns",
            path=["typed conformance_client", "Unix socket", "runtime binary / Server",
                  "Host", "official MetricsPlugin + SDK", "single held synthetic reader"],
            cases=["healthy_to_missing", "missing_to_healthy", "reader_blocked_stale", "healthy_status_wait0_while_blocked"],
            limits=["no system sensors, GPU or real backend", "client-observed intervals include 32-visual scans, scheduling and 25ms observation polls",
                    "Wait0 is execution-result observation, not native external effect or hardware latency",
                    "cleanup proves owned runtime/client Popen wait only, not independent plugin-descendant enumeration",
                    "not downloader, host no-reader, converter or Store-write uncertainty integration"])
    runtime = Runtime()
    scenario_ok = False
    try:
        scenario(runtime.peer)
        scenario_ok = True
    finally:
        runtime.close()
        profile("complete", scenario_ok=scenario_ok, **runtime.cleanup)
