#!/usr/bin/env python3
"""Critical runtime/IPC contract; only the installed deterministic metrics fixture."""
import importlib.util
import json
from pathlib import Path
import subprocess
import sys
import time

ROOT, REPO = (Path(value).resolve() for value in sys.argv[1:3])
PHASE = sys.argv[3]
assert ROOT.parent == REPO / "target" and ROOT.name.startswith("f5-")
spec = importlib.util.spec_from_file_location("private_ipc", REPO / "scripts/conformance/obs/check.py")
ipc = importlib.util.module_from_spec(spec)
spec.loader.exec_module(ipc)
Peer, equal, until = ipc.Peer, ipc.equal, ipc.until
SOURCES = ["cpu", "ram", "gpu", "vram"]
TEXTS = ["CPU 25%", "RAM 8.0 GiB used 8.0 GiB avail", "GPU 73%", "VRAM 2.0/8.0 GiB"]


class Runtime:
    def __init__(self):
        self.child = None
        self.peer = None
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
        if self.peer is not None:
            self.peer.close()
            self.peer = None
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
            self.child = None
        self.log.close()


def fault_scenario(e):
    mode = ROOT / "runtime/f5-reader-mode"
    def all_visuals(availability):
        return all(e.call("plugin_visual", id=f"b{i+1}")["availability"] == availability for i in range(32))
    until(lambda: all_visuals("current"), bool)
    mode.write_text("missing")
    until(lambda: all_visuals("unavailable"), bool, timeout=5)
    equal("reader failure leaves host ready", e.call("plugin_status")["items"][0]["host"], "ready")
    mode.write_text("healthy")
    until(lambda: all_visuals("current"), bool, timeout=5)
    e.call("add_action", button_id="b1", type="wait", parameters=dict(milliseconds=0))
    e.call("set_permissions", target="global", value=dict(own=dict(consult=True,modify=True,execute=True)))
    mode.write_text("block")
    until(lambda: all_visuals("stale"), bool, timeout=5)
    count = int((ROOT / "runtime/f5-reader-count").read_text())
    for _ in range(6):
        equal("blocked reader leaves host ready", e.call("plugin_status")["items"][0]["host"], "ready")
        equal("blocked reader leaves runtime status responsive", e.call("status")["deck_state"], "disabled")
        admitted = e.call("run", id="b1")
        result = until(lambda: e.call("result", execution_id=admitted["execution_id"]),
                       lambda value: value["state"] != "running")
        equal("healthy core action with stale visual", result["state"], "succeeded")
        time.sleep(.5)
    equal("blocked reader has no replacement/retry reads",
          int((ROOT / "runtime/f5-reader-count").read_text()), count)
    equal("all32 visuals still stale", all_visuals("stale"), True)
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
    runtime = Runtime()
    try:
        scenario(runtime.peer)
    finally:
        runtime.close()
