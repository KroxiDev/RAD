#!/usr/bin/env python3
"""Run the SAME public checker with real manifests and explicitly private fixtures."""
import json
import os
from pathlib import Path
import queue
import signal
import subprocess
import sys
import tempfile
import threading

REPO = Path(__file__).resolve().parents[3]
BIN = REPO / 'target/plugins/release/examples'

def params(**values):
    return dict(schema_version=1, values=values)

def target(name, kind='scene', item=None, source=None):
    return json.dumps(dict(v=1, collection='C', kind=kind, name=name, item=item, source=source), separators=(',', ':'))

class Services:
    def __init__(self, root):
        self.children = []
        self.env = dict(PATH='/usr/bin:/bin', HOME=str(root), XDG_RUNTIME_DIR=str(root),
                        DBUS_SESSION_BUS_ADDRESS='unix:path='+str(root/'no-personal-bus'))
    def spawn(self, args, extra=None):
        child = subprocess.Popen(args, env=self.env | (extra or {}), stdin=subprocess.PIPE,
                                 stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, text=True,
                                 start_new_session=True, bufsize=1)
        self.children.append(child)
        child.lines = queue.Queue()
        def drain():
            for line in child.stdout:
                child.lines.put(line.strip())
        threading.Thread(target=drain, daemon=True).start()
        return child
    def close(self):
        for child in reversed(self.children):
            child.stdin.close()
            try:
                child.wait(timeout=3)
            except subprocess.TimeoutExpired:
                os.killpg(child.pid, signal.SIGTERM)
                try:
                    child.wait(timeout=3)
                except subprocess.TimeoutExpired:
                    os.killpg(child.pid, signal.SIGKILL)
                    child.wait(timeout=3)

def run(name, out):
    with tempfile.TemporaryDirectory(prefix='rad-check-official-') as scratch:
        root = Path(scratch)
        services = Services(root)
        try:
            values, visual_values = {}, {}
            args = []
            settings = None
            if name == 'audio':
                (root/'state.json').write_text(json.dumps(dict(
                    sinks=[dict(name='sink', description='Fixture output', mute=False)],
                    sources=[dict(name='mic', description='Fixture microphone', mute=False)],
                    default_sink='sink', default_source='mic')))
                executable = BIN/'plugin_with_fake'
                args = [str(BIN/'fake_pactl'), str(root)]
                values = {k: dict(operation='mute', device='@default') for k in ['output','microphone']}
                visual_values = {k: dict(device='@default') for k in values}
            elif name == 'media':
                config = root/'bus.conf'
                config.write_text(f'''<busconfig><type>session</type><listen>unix:path={root}/bus</listen>
<auth>EXTERNAL</auth><policy context="default"><allow send_destination="*"/>
<allow receive_sender="*"/><allow own="*"/></policy></busconfig>''')
                bus = services.spawn(['/usr/bin/dbus-daemon', f'--config-file={config}', '--nofork', '--print-address'])
                address = bus.lines.get(timeout=10)
                assert address.startswith(f'unix:path={root}/bus,')
                services.env['DBUS_SESSION_BUS_ADDRESS'] = address
                player = services.spawn([str(BIN/'fake_player'), 'org.mpris.MediaPlayer2.brave.instance10'], {'FAKE_PLAYER_PRIVATE_BUS':'1'})
                assert player.lines.get(timeout=10) == 'ready'
                executable = BIN/'media-fixture'
                args = [address]
                values = dict(control=dict(operation='play', player='brave'))
                visual_values = dict(now_playing=dict(player='brave'))
            elif name == 'obs':
                server = services.spawn([str(BIN/'obs-conformance-server'), '--checker-canary'])
                port = json.loads(server.lines.get(timeout=10))['port']
                assert port not in (0,4455)
                executable = BIN/'obs-fixture'
                args = [str(port)]
                settings = params(port=port)
                # Manifest order: the recording starts before its pause toggles.
                values = dict(scene=dict(scene=target('Live')),
                    filter=dict(source=target('Camera','input'), filter='Blur', operation='on'),
                    item=dict(scene=target('Root'), item=target('G','group',8,'Camera'), operation='show'),
                    record=dict(operation='start'), pause=dict(operation='toggle'), stream=dict(operation='start'),
                    mute=dict(input=target('Camera','input'), operation='toggle'),
                    virtualcam=dict(operation='toggle'), replay=dict(operation='start'))
                visual_values = {k:{f:v for f,v in values[k].items() if f != 'operation'} for k in values}
            elif name == 'metrics':
                executable = BIN/'metrics-fixture'
                visual_values = {k:{} for k in ['cpu','ram','gpu','vram']}
            else:
                raise AssertionError(name)
            manifest_path = REPO/f'plugins/{name}/manifest.json'
            manifest = json.loads(manifest_path.read_text())
            scenario = dict(version=1, trusted_native_code=True, timeout_ms=10000, freshness_ms=26000,
                            args=args, actions=[], options=[], visuals=[])
            if settings is not None:
                scenario['settings'] = settings
            # Complete manifest-driven coverage with explicit fixture-only values; never guess an effect.
            for action in manifest['actions']:
                for gesture in action['gestures']:
                    scenario['actions'].append(dict(id=action['id'], gesture=gesture, parameters=params(**values[action['id']])))
            for visual in manifest['visuals']:
                scenario['visuals'].append(dict(id=visual['id'], parameters=params(**visual_values[visual['id']])))
            for scope, definitions, contexts in [('action',manifest['actions'],values),('visual',manifest['visuals'],visual_values)]:
                for definition in definitions:
                    for field in definition['schema']['fields']:
                        if field['kind']['type'] == 'dynamic_choice':
                            scenario['options'].append(dict(scope=scope,definition=definition['id'],field=field['id'],context=params(**{key:contexts[definition['id']][key] for key in field['kind']['depends_on']})))
            # No effectful action replay: options are read-only, metrics uses synthetic visual subscriptions.
            scenario['pressure'] = dict(kind='options' if scenario['options'] else 'visual', index=0, safe_to_repeat=True)
            package, vector = out/f'{name}.radp', out/f'{name}-scenario.json'
            subprocess.run([str(REPO/'target/release/examples/conformance_client'), 'pack', str(manifest_path), str(executable), str(package)], check=True, timeout=15)
            vector.write_text(json.dumps(scenario, indent=2)+'\n')
            with (out/f'{name}-checker.json').open('w') as report, (out/f'{name}-checker.stderr').open('w') as error:
                result = subprocess.run([str(REPO/'target/release/rad-plugin-check'), str(package), str(vector)],
                    env=services.env, stdout=report, stderr=error, timeout=190)
            report = json.loads((out/f'{name}-checker.json').read_text())
            assert result.returncode == 0 and report['version'] == 1 and report['status'] == 'PASS', report
            print('PASS generic checker', name, flush=True)
        finally:
            services.close()

if __name__ == '__main__':
    out = Path(sys.argv[1]).resolve()
    assert out.is_dir()
    for name in sys.argv[2:]:
        run(name, out)
