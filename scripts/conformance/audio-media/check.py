#!/usr/bin/env python3
"""Private conformance instrument, not product runtime. Invoked by the Rust test."""
import json
import os
from pathlib import Path
import queue
import signal
import struct
import subprocess
import sys
import threading
import time
import zlib


def equal(label, actual, expected):
    if actual != expected:
        raise AssertionError(f'{label}: expected={expected!r}, actual={actual!r}')
    print(f'PASS {label}: {actual!r}', flush=True)


# Reuse the typed Rust client bridge, not a second runtime wire implementation.
sys.dont_write_bytecode = True
import importlib.util
_peer_spec = importlib.util.spec_from_file_location("typed_peer", Path(__file__).resolve().parents[1] / 'obs/check.py')
_peer_module = importlib.util.module_from_spec(_peer_spec)
_peer_spec.loader.exec_module(_peer_module)
Peer = _peer_module.Peer

class World:
    def __init__(self, root, repo):
        self.root, self.repo = root, repo
        self.children = []
        self.peers = []
        for name in ['runtime', 'home', 'config', 'cache', 'system']:
            (root / name).mkdir(mode=0o700)
        self.env = dict(PATH='/usr/bin:/bin', HOME=str(root / 'home'),
                        XDG_RUNTIME_DIR=str(root / 'runtime'), XDG_DATA_HOME=str(root / 'data'),
                        XDG_CONFIG_HOME=str(root / 'config'), XDG_CACHE_HOME=str(root / 'cache'),
                        XDG_DATA_DIRS=str(root / 'system'), XDG_CONFIG_DIRS=str(root / 'system'),
                        PULSE_SERVER='unix:' + str(root / 'no-audio-server'),
                        PULSE_CLIENTCONFIG=str(root / 'pulse.conf'))
        (root / 'pulse.conf').write_text('autospawn = no\n')
        config = root / 'bus.conf'
        config.write_text(f'''<busconfig><type>session</type><listen>unix:path={root}/bus</listen>
<auth>EXTERNAL</auth><policy context="default"><allow send_destination="*"/>
<allow receive_sender="*"/><allow own="*"/></policy></busconfig>''')
        self.bus = self.spawn(['/usr/bin/dbus-daemon', f'--config-file={config}', '--nofork', '--print-address'])
        try:
            self.env['DBUS_SESSION_BUS_ADDRESS'] = self.line(self.bus)
            assert self.env['DBUS_SESSION_BUS_ADDRESS'].startswith(f'unix:path={root}/bus,')
        except BaseException:
            self.close()
            raise

    def spawn(self, command, extra=None):
        child = subprocess.Popen(command, env=self.env | (extra or {}), stdin=subprocess.PIPE,
                                 stdout=subprocess.PIPE, stderr=sys.stderr, text=True, bufsize=1,
                                 start_new_session=True)
        child.lines = queue.Queue()
        child.events = []
        def drain():
            for line in child.stdout:
                child.events.append(line.strip())
                child.lines.put(line.strip())
        threading.Thread(target=drain, daemon=True).start()
        self.children.append(child)
        return child

    @staticmethod
    def line(child):
        return child.lines.get(timeout=10)

    def player(self, suffix):
        child = self.spawn([str(self.repo / 'target/plugins/release/examples/fake_player'),
                            'org.mpris.MediaPlayer2.' + suffix], {'FAKE_PLAYER_PRIVATE_BUS': '1'})
        equal('fake private player ready', self.line(child), 'ready')
        return child

    def command(self, child, text):
        token = str(time.monotonic_ns())
        child.stdin.write(text + '\nping ' + token + '\n')
        child.stdin.flush()
        while self.line(child) != 'pong ' + token:
            pass

    @staticmethod
    def calls(child):
        return [s for s in child.events if s.startswith('call ')]

    def start(self, expected_host='absent'):
        # The host and pactl deliberately discard PULSE_*; they are NOT our isolation.
        # Only the private fixture tree is writable. No host home/run/Pulse config or network.
        isolation = ['/usr/bin/bwrap', '--die-with-parent', '--unshare-net',
                     '--ro-bind', '/usr', '/usr', '--symlink', 'usr/bin', '/bin',
                     '--symlink', 'usr/lib', '/lib', '--symlink', 'usr/lib', '/lib64',
                     '--dir', '/run', '--dir', '/home', '--dir', '/etc', '--dir', '/etc/pulse',
                     '--proc', '/proc', '--dev', '/dev', '--tmpfs', '/tmp',
                     '--ro-bind', str(self.repo), str(self.repo),
                     '--bind', str(self.root), str(self.root), '--chdir', str(self.root), '--']
        pidfile = self.root / 'runtime.pid'
        pidfile.unlink(missing_ok=True)
        self.runtime = self.spawn(isolation + ['/bin/sh', '-c', 'echo $$ > "$1"; exec "$2"',
                                              'private-runtime', str(pidfile),
                                              str(self.repo / 'target/release/rad-runtime')])
        end = time.monotonic() + 10
        while not (self.root / 'runtime/rad/editor.sock').exists():
            assert self.runtime.poll() is None, 'runtime died'
            assert time.monotonic() < end
            time.sleep(.02)
        self.runtime_pidfd = os.pidfd_open(int(pidfile.read_text().strip()))
        self.editor = Peer(self.root)
        self.peers.append(self.editor)
        self.until(lambda: self.editor.call('plugin_status'),
                   lambda v: v['state'] == 'idle' and len(v['items']) == 2 and
                   all(i['host'] == expected_host for i in v['items']))
        equal('runtime without hardware', self.editor.call('status')['deck_state'], 'disabled')

    @staticmethod
    def until(read, predicate):
        end = time.monotonic() + 12
        while True:
            value = read()
            if predicate(value):
                return value
            assert time.monotonic() < end, value
            time.sleep(.025)

    def cli(self, *args, code=0):
        result = subprocess.run([str(self.repo / 'target/release/rad-cli'), *args, '--json'],
                                env=self.env, capture_output=True, text=True, timeout=15)
        equal('CLI exit ' + ' '.join(args), result.returncode, code)
        value = json.loads(result.stdout)
        print('CLI', json.dumps(value, sort_keys=True), flush=True)
        return value

    def stop_runtime(self):
        for peer in self.peers:
            peer.close()
        self.peers.clear()
        signal.pidfd_send_signal(self.runtime_pidfd, signal.SIGTERM)
        equal('orderly runtime shutdown', self.runtime.wait(timeout=12), 0)
        os.close(self.runtime_pidfd)
        self.runtime_pidfd = None

    def close(self):
        if getattr(self, 'runtime_pidfd', None) is not None:
            os.close(self.runtime_pidfd)
            self.runtime_pidfd = None
        for peer in self.peers:
            peer.close()
        for child in reversed(self.children):
            if child.poll() is None:
                child.terminate()
                try:
                    child.wait(timeout=12)
                except subprocess.TimeoutExpired:
                    os.killpg(child.pid, signal.SIGKILL)
                    child.wait(timeout=5)


def parameters(**values):
    return dict(schema_version=1, values=values)


def scenario(w):
    first = w.player('brave.instance10')
    second = w.player('brave.instance9')
    w.command(first, 'identity First Brave')
    w.command(second, 'identity Second Brave')
    w.command(first, 'status Playing')
    w.start()
    e = w.editor
    folder = e.call('status')['active_folder_id']
    for position, name in enumerate(['Media', 'Audio', 'Healthy']):
        e.call('create_button', folder_id=folder, position=position, name=name)
    e.call('add_action', button_id='b1', type='org.rad.media:control',
           parameters=parameters(operation='play', player='brave'))
    e.call('add_action', button_id='b2', type='org.rad.audio:output',
           parameters=parameters(operation='mute', device='default'))
    e.call('add_action', button_id='b3', type='wait', parameters=dict(milliseconds=0))
    e.call('edit', changes=[dict(kind='set', id='b1', field='visual', value=dict(
        type='org.rad.media#now_playing', parameters=parameters(player='brave')))])
    w.until(lambda: e.call('plugin_status'),
            lambda v: all(i['host'] == 'ready' for i in v['items']))
    v = w.until(lambda: e.call('plugin_visual', id='b1'), lambda v: v['availability'] == 'current')
    equal('visual selected first instance', v['text'], 'Playing')
    options = w.cli('options', 'a1', 'player')['data']
    equal('options first app and closed preset', options['items'], [
        dict(label='First Brave', value='brave'), dict(label='Strawberry (not running)', value='strawberry')])
    equal('editor/CLI same options', e.call('options', id='a1', field='player', cursor=None), options)
    w.cli('schema', 'org.rad.media:control')
    # Negative control: merely querying options/schema must not execute a method.
    equal('queries have no media effects', w.calls(first) + w.calls(second), [])
    denied = w.cli('run', 'a1', '--wait', code=3)
    equal('default execute permission denied', denied['error']['code'], 'permission_denied')
    automation = Peer(w.root, 'automation')
    w.peers.append(automation)
    for op, args in [('plugin_admin', dict(operation='uninstall', plugin_id='org.rad.media', source=None, trusted=True)),
                     ('set_permissions', dict(target='global', value=dict(own=dict(consult=True, modify=True, execute=True))))]:
        equal('automation rejects ' + op, automation.request(op, **args)['error']['code'], 'permission_denied')
    e.call('set_permissions', target='global', value=dict(own=dict(consult=True, modify=True, execute=True)))
    result = w.cli('run', 'a1', '--wait')
    equal('media execution successful', result['data']['state'], 'succeeded')
    w.command(first, 'identity First Brave')  # barrier for observations
    equal('AC30 first matching instance receives method', w.calls(first), ['call Play'])
    equal('AC30 other instance untouched', w.calls(second), [])
    w.command(first, 'can play false')
    result = w.cli('run', 'a1', '--wait', code=6)
    equal('capability failure reaches CLI', result['data']['steps'][0]['code'], 'action_failed')
    equal('unsupported method not dispatched', w.calls(first), ['call Play'])
    w.cli('set', 'a1', 'parameters', json.dumps(parameters(operation='play', player='absent')))
    result = w.cli('run', 'a1', '--wait', code=6)
    equal('absent player reaches CLI', result['data']['steps'][0]['code'], 'service_unavailable')
    lost = w.cli('options', 'a1', 'player')['data']
    equal('absent selected value retained', (lost['saved'], lost['saved_listed']), ('absent', False))
    equal('AC29 no fallback to another player', w.calls(first) + w.calls(second), ['call Play'])
    equal('audio without server reaches CLI', w.cli('run', 'a2', '--wait', code=6)['data']['steps'][0]['code'], 'service_unavailable')
    audio = w.cli('options', 'a2', 'device')['data']
    equal('audio source unavailable not empty available', (audio['availability'], audio['items']), ('unavailable', []))
    e.call('edit', changes=[dict(kind='set', id='b3', field='visual', value=dict(
        type='org.rad.media#now_playing', parameters=parameters(player='absent')))])
    w.until(lambda: e.call('plugin_visual', id='b3'), lambda v: v['availability'] == 'unavailable')
    equal('visual failure independent from healthy action', w.cli('run', 'a3', '--wait')['data']['state'], 'succeeded')
    # Real PNG -> official media plugin JPEG -> RAD normalized image over IPC.
    def chunk(kind, payload):
        return struct.pack('>I', len(payload)) + kind + payload + struct.pack('>I', zlib.crc32(kind + payload))
    art = w.root / 'cover.png'
    art.write_bytes(b'\x89PNG\r\n\x1a\n' + chunk(b'IHDR', struct.pack('>IIBBBBB', 1, 1, 8, 2, 0, 0, 0)) +
                    chunk(b'IDAT', zlib.compress(b'\x00\xff\x00\x00')) + chunk(b'IEND', b''))
    w.command(first, 'art ' + art.as_uri())
    image = w.until(lambda: e.call('plugin_visual', id='b1'), lambda v: v['availability'] == 'current' and v['image'])
    normalized = e.call('plugin_visual_read', id='b1', revision=image['revision'], offset='0')
    equal('cover normalized to PNG by RAD', normalized['hex'][:16], '89504e470d0a1a0a')
    w.command(first, 'art')
    w.command(first, 'status Paused')
    paused = w.until(lambda: e.call('plugin_visual', id='b1'), lambda v: v['text'] == 'Paused' and v['availability'] == 'current')
    equal('external status change reflected', paused['text'], 'Paused')
    first.stdin.close()
    first.wait(timeout=5)
    second.stdin.close()
    second.wait(timeout=5)
    unavailable = w.until(lambda: e.call('plugin_visual', id='b1'), lambda v: v['availability'] == 'unavailable')
    equal('absent selected visual visibly unavailable', unavailable['availability'], 'unavailable')
    replacement = w.player('brave.instance10')
    w.command(replacement, 'status Playing')
    w.until(lambda: e.call('plugin_visual', id='b1'), lambda v: v['availability'] == 'current' and v['text'] == 'Playing')
    equal('reappearance does not replay methods', w.calls(replacement), [])
    before = e.call('plugin_configuration', id='a1', cursor=None)
    w.stop_runtime()
    w.start(expected_host='ready')
    after = w.editor.call('plugin_configuration', id='a1', cursor=None)
    equal('private persistence preserves plugin configuration', after, before)
    equal('CLI persisted absent selected player', w.cli('options', 'a1', 'player')['data']['saved'], 'absent')
    equal('restart never replays media action', w.calls(first) + w.calls(second), ['call Play'])
    equal('restart never replays into replacement', w.calls(replacement), [])
    w.stop_runtime()


if __name__ == '__main__':
    root, repo = map(lambda s: Path(s).resolve(), sys.argv[1:])
    assert root.name.startswith('rad-d7-') and root.parent == Path('/tmp')
    world = World(root, repo)
    try:
        scenario(world)
        print('PASS official plugin conformance', flush=True)
    finally:
        world.close()
