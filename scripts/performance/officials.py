#!/usr/bin/env python3
"""Official implementations, private fake backends, fresh runtime and lifecycle windows.

No personal services, hardware, GUI or external network. Audio fails closed inside
bwrap with fake_pactl mounted over the only selected production pactl path.
"""
import argparse
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import queue
import select
import signal
import subprocess
import threading
import time

REPO = Path(__file__).resolve().parents[2]
PLUGIN_BIN = REPO/'target/plugins/release'
EXAMPLES = PLUGIN_BIN/'examples'
NAMES = ('audio', 'media', 'obs', 'metrics')
SPEC = importlib.util.spec_from_file_location('private_profile_sampler', REPO/'scripts/performance/profile.py')
SAMPLER = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(SAMPLER)


def visual_for(name, index, enabled=True):
    if not enabled:
        return dict(type='manual', parameters=None)
    if name == 'audio':
        definition, values = ('output', 'microphone')[index % 2], dict(device='@default')
    elif name == 'media':
        definition, values = 'now_playing', dict(player='brave')
    elif name == 'obs':
        definition = 'scene'
        values = dict(scene=json.dumps(dict(v=1, collection='C', kind='scene', name='Live',
                                           item=None, source=None), separators=(',', ':')))
    elif name == 'metrics':
        definition, values = ('cpu', 'ram', 'gpu', 'vram')[index % 4], {}
    else:
        raise ValueError('unknown official fixture')
    return dict(type=f'org.rad.{name}#{definition}', parameters=dict(schema_version=1, values=values))


def validate_output(output, seconds, repetitions, interval):
    out = output.resolve()
    if (out.parent != REPO/'target/performance' or out.exists() or
            not 5 <= seconds <= 15 or not 1 <= repetitions <= 3 or not .1 <= interval <= 1):
        raise ValueError('new direct child of target/performance; seconds5..15, repetitions1..3, interval0.1..1')
    if len(str(out/'metrics2/runtime/rad/editor.sock').encode()) >= 108:
        raise ValueError('output path too long for private Unix sockets')
    return out


def until(read, accept, timeout=15):
    deadline = time.monotonic()+timeout
    while True:
        value = read()
        if accept(value):
            return value
        if time.monotonic() >= deadline:
            raise TimeoutError(f'private fixture state deadline: {value}')
        time.sleep(.025)


class Bridge:
    """Canonical typed Rust IPC client; controller is outside the measured tree."""
    def __init__(self, world):
        self.world = world
        self.child = world.spawn([str(REPO/'target/release/examples/conformance_client'),
                                  str(world.root/'runtime'), 'editor'], 'client', drain=False)

    def call(self, operation, **args):
        self.child.stdin.write(json.dumps([operation, args])+'\n')
        self.child.stdin.flush()
        if not select.select([self.child.stdout], [], [], 12)[0]:
            raise TimeoutError('typed private IPC client deadline')
        line = self.child.stdout.readline()
        if not line:
            raise RuntimeError('typed private IPC client ended')
        response = json.loads(line)
        if response.get('error'):
            raise RuntimeError(f'private IPC {operation}: {response["error"]}')
        if response.get('cli_version') != 1:
            raise ValueError('unexpected typed client envelope')
        return response['data']

    def __enter__(self):
        return self

    def __exit__(self, *_):
        self.child.stdin.close()
        try:
            code = self.child.wait(timeout=5)
        except subprocess.TimeoutExpired:
            self.world.signal(self.child.pid, signal.SIGKILL)
            self.child.wait(timeout=3)
            raise
        finally:
            self.child.stdout.close()
        if code != 0:
            raise RuntimeError(f'typed client failed: {code}')


class World:
    def __init__(self, root, name, log):
        self.root, self.name, self.log = root, name, log
        self.roots, self.owned = [], {}
        self.runtime = None
        self.runtime_pid = None
        self.backends = []
        self.rows, self.events, self.unobserved = [], [], set()
        self.epoch = time.monotonic()
        root.mkdir(mode=0o700)
        # Seeding runs elsewhere: its executable/package buffers cannot inflate this PID.
        result = subprocess.run([str(REPO/'target/release/examples/official_profile_seed'), str(root), name],
                                env={'PATH':'/usr/bin:/bin'}, stdin=subprocess.DEVNULL,
                                stdout=subprocess.PIPE, stderr=log, text=True, timeout=30, check=True)
        self.events.append(json.loads(result.stdout))
        machine = root/'data/rad/machine.json'
        machine.write_text(json.dumps(dict(version=1,use_stream_deck=False,start_with_session=False)))
        machine.chmod(0o600)  # Explicit before product startup, not merely a default or post-start assertion.
        self.env = dict(PATH='/usr/bin:/bin', LANG='C.UTF-8')
        for key, directory in (
                ('HOME','home'), ('XDG_RUNTIME_DIR','runtime'), ('XDG_DATA_HOME','data'),
                ('XDG_CONFIG_HOME','config'), ('XDG_CACHE_HOME','cache'), ('XDG_STATE_HOME','state'),
                ('XDG_DATA_DIRS','system'), ('XDG_CONFIG_DIRS','system')):
            (root/directory).mkdir(mode=0o700, exist_ok=True)
            self.env[key] = str(root/directory)
        self.env['DBUS_SESSION_BUS_ADDRESS'] = 'unix:path='+str(root/'no-personal-bus')
        self.prepare_backend()

    def spawn(self, command, role, extra=None, drain=True):
        child = subprocess.Popen(command, cwd=REPO, env=self.env | (extra or {}),
                                 stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=self.log,
                                 text=True, bufsize=1, start_new_session=True)
        self.roots.append((child, role))
        self.remember(child.pid, child.pid)
        if drain:
            child.lines = queue.Queue(maxsize=256)
            def consume():
                for line in child.stdout:
                    try:
                        child.lines.put_nowait(line.strip())
                    except queue.Full:
                        pass  # Only bounded readiness/control responses are needed.
            child.reader = threading.Thread(target=consume, daemon=True)
            child.reader.start()
        return child

    @staticmethod
    def line(child):
        return child.lines.get(timeout=10)

    def remember(self, pid, session):
        try:
            stat = SAMPLER.parse_stat((Path('/proc')/str(pid)/'stat').read_text())
            if stat['session'] != session:
                raise ValueError('new child escaped its private session')
            self.owned[pid] = (stat['start_ticks'], session)
            return stat
        except FileNotFoundError:
            return None

    def note_unobserved(self, pid, session):
        if (pid,session) not in self.unobserved:
            self.unobserved.add((pid,session))
            self.events.append(dict(event='unobserved_descendant',pid=pid,
                reason='session differs; guard retained',expected_session=session))

    def remember_descendants(self, session):
        for pid in SAMPLER.descendants(session):
            try:
                self.remember(pid,session)
            except ValueError:
                # This identity must neither be sampled nor signalled. Its
                # discovery must not prevent stopping/reaping the known roots.
                self.note_unobserved(pid,session)

    def signal(self, pid, sig):
        if pid not in self.owned:
            raise ValueError('refusing to signal an unobserved identity')
        start, session = self.owned[pid]
        try:
            fd = os.pidfd_open(pid)
        except ProcessLookupError:
            return
        try:
            if SAMPLER.same_owned_process(pid, start, session):
                signal.pidfd_send_signal(fd, sig)
        except ProcessLookupError:
            pass
        finally:
            os.close(fd)

    def prepare_backend(self):
        if self.name == 'audio':
            (self.root/'runtime/state.json').write_text(json.dumps(dict(
                sinks=[dict(name='sink',description='Private output',mute=False)],
                sources=[dict(name='mic',description='Private microphone',mute=False)],
                default_sink='sink', default_source='mic')))
        elif self.name == 'media':
            conf = self.root/'bus.conf'
            conf.write_text(f'''<busconfig><type>session</type><listen>unix:path={self.root}/bus</listen>
<auth>EXTERNAL</auth><policy context="default"><allow send_destination="*"/>
<allow receive_sender="*"/><allow own="*"/></policy></busconfig>''')
            bus = self.spawn(['/usr/bin/dbus-daemon', f'--config-file={conf}', '--nofork', '--print-address'], 'private_bus')
            address = self.line(bus)
            if not address.startswith(f'unix:path={self.root}/bus,'):
                raise ValueError('bus is not the newly created private bus')
            self.env['DBUS_SESSION_BUS_ADDRESS'] = address
            self.backends.append((bus, 'private_bus'))
            player = self.spawn([str(EXAMPLES/'fake_player'), 'org.mpris.MediaPlayer2.brave.instance10'],
                                'fake_player', {'FAKE_PLAYER_PRIVATE_BUS':'1'})
            if self.line(player) != 'ready':
                raise ValueError('private fake player not ready')
            player.stdin.write('status Playing\nping configured\n')
            player.stdin.flush()
            until(lambda:self.line(player), lambda value:value == 'pong configured')
            self.backends.append((player, 'fake_player'))
        elif self.name == 'obs':
            server = self.spawn([str(EXAMPLES/'obs-conformance-server')], 'obs_backend')
            port = json.loads(self.line(server))['port']
            if not isinstance(port, int) or not 1 <= port <= 65535 or port == 4455:
                raise ValueError('OBS backend did not choose a safe ephemeral port')
            portfile = self.root/'runtime/rad-obs-fixture.port'
            portfile.write_text(str(port))
            portfile.chmod(0o600)
            self.backends.append((server, 'obs_backend'))

    def start_runtime(self):
        pidfile = self.root/'runtime.pid'
        if pidfile.exists():
            # Only a file this World wrote in its new private root is retired.
            previous = self.root/f'runtime-{len(self.events)}.pid'
            pidfile.rename(previous)
        if self.name == 'audio':
            # The first pactl search path is a readonly fake; network and personal
            # home/run/config are absent. Environment alone is NOT the isolation.
            libraries = [part for path in ('/lib','/lib64') if os.path.exists(path)
                         for part in ('--ro-bind',path,path)]
            command = ['/usr/bin/bwrap','--die-with-parent','--unshare-net',
                       '--ro-bind','/usr','/usr','--symlink','usr/bin','/bin', *libraries,
                       '--dir','/run','--dir','/home','--dir','/etc','--dir','/etc/pulse',
                       '--proc','/proc','--dev','/dev','--tmpfs','/tmp',
                       '--ro-bind',str(REPO),str(REPO),'--bind',str(self.root),str(self.root),
                       '--ro-bind',str(EXAMPLES/'fake_pactl'),'/usr/bin/pactl',
                       '--chdir',str(self.root),'--','/bin/sh','-c',
                       'umask 077; cmp -s "$1" /usr/bin/pactl || exit 98; echo $$ > "$2"; exec "$3"',
                       'private-audio-runtime',str(EXAMPLES/'fake_pactl'),str(pidfile),
                       str(REPO/'target/release/rad-runtime')]
        else:
            command = ['/bin/sh','-c','umask 077; echo $$ > "$1"; exec "$2"',
                       'private-runtime',str(pidfile),str(REPO/'target/release/rad-runtime')]
        self.runtime = self.spawn(command, 'runtime_root')
        def started():
            if self.runtime.poll() is not None:
                raise RuntimeError(f'private runtime exited early: {self.runtime.returncode}')
            return pidfile.exists() and (self.root/'runtime/rad/editor.sock').exists()
        until(started, bool)
        self.runtime_pid = int(pidfile.read_text().strip())
        if self.remember(self.runtime_pid, self.runtime.pid) is None:
            raise RuntimeError('private runtime PID disappeared')
        with Bridge(self) as peer:
            self.wait_host(peer, 'absent')
            if peer.call('status')['deck_state'] != 'disabled':
                raise ValueError('refusing to profile hardware-enabled state')
        self.events.append(dict(event='runtime_started',pid=self.runtime_pid, session=self.runtime.pid))

    def wait_host(self, peer, state):
        def read():
            value = peer.call('plugin_status')
            if value['state'] in ('failed','uncertain'):
                raise RuntimeError(f'private plugin administration failed: {value}')
            return value
        return until(read, lambda value:value['state'] == 'idle' and len(value['items']) == 1
                     and value['items'][0]['host'] == state)

    def configure_buttons(self):
        with Bridge(self) as peer:
            folder = peer.call('status')['active_folder_id']
            for position in range(32):
                peer.call('create_button', folder_id=folder, position=position, name=f'Private {position}')
            if self.name == 'obs':
                # This is a fixed oracle credential, never a personal secret.
                peer.call('credential_replace',plugin_id='org.rad.obs',slot='password',secret='correct')
            self.wait_host(peer, 'absent')

    def set_visuals(self, count):
        epoch = time.monotonic_ns()
        with Bridge(self) as peer:
            peer.call('edit', changes=[dict(kind='set',id=f'b{i+1}',field='visual',
                value=visual_for(self.name,i,i<count)) for i in range(32)])
            self.wait_host(peer, 'ready' if count else 'absent')
            for i in range(count):
                until(lambda:peer.call('plugin_visual',id=f'b{i+1}'),
                      lambda value:value['availability'] == 'current')
        self.events.append(dict(event='transition',visuals=count,ns=time.monotonic_ns()-epoch,
                                includes='typed client connect, edit, host and visual readiness observations'))
        time.sleep(1)

    def capture(self, phase):
        values = []
        roots = [(self.runtime, 'runtime_tree')] + [(child, role) for child, role in self.backends]
        for child, role in roots:
            if child is None or child.poll() is not None:
                raise RuntimeError(f'owned {role} ended during resource window')
            for pid in SAMPLER.descendants(child.pid):
                try:
                    value = SAMPLER.read_process(pid, child.pid)
                    old = self.owned.get(pid)
                    if old is not None and old != (value['start_ticks'], child.pid):
                        raise ValueError('owned PID identity changed during lifecycle')
                    self.owned[pid] = (value['start_ticks'], child.pid)
                    if role == 'runtime_tree':
                        comm = (Path('/proc')/str(pid)/'comm').read_text().strip()
                        value['role'] = ('runtime' if pid == self.runtime_pid else
                            'sandbox_supervisor' if pid == self.runtime.pid else
                            'launcher_helper' if comm.startswith('rad-launcher') else
                            'audio_fake_pactl' if self.name == 'audio' and comm == 'pactl' else
                            'official_plugin')
                    else:
                        value['role'] = role
                    values.append(value)
                except FileNotFoundError:
                    continue
                except ProcessLookupError:
                    try:
                        stat = SAMPLER.parse_stat((Path('/proc')/str(pid)/'stat').read_text())
                    except FileNotFoundError:
                        continue
                    if stat['session'] != child.pid:
                        self.note_unobserved(pid,child.pid)
        row = dict(elapsed_s=time.monotonic()-self.epoch,phase=phase,processes=values)
        self.rows.append(row)
        return row

    def window(self, phase, seconds, interval, raw):
        self.events.append(dict(event='phase_begin',phase=phase))
        epoch, tick = time.monotonic(), 0
        while True:
            row = self.capture(phase)
            raw.write(json.dumps(row)+'\n')
            if time.monotonic()-epoch >= seconds:
                break
            tick += 1
            time.sleep(max(0, min(epoch+tick*interval, epoch+seconds)-time.monotonic()))
        raw.flush()
        self.events.append(dict(event='phase_end',phase=phase))

    def stop_runtime(self):
        if self.runtime is None:
            return
        runtime = self.runtime
        self.remember_descendants(runtime.pid)
        self.signal(self.runtime_pid, signal.SIGTERM)
        code = runtime.wait(timeout=12)
        runtime.reader.join(timeout=1)
        runtime.stdin.close()
        runtime.stdout.close()
        session = runtime.pid
        self.runtime, self.runtime_pid = None, None
        if code != 0:
            raise RuntimeError(f'private runtime did not shut down cleanly: {code}')
        deadline = time.monotonic()+3
        while any(SAMPLER.same_owned_process(p,s,ss) for p,(s,ss) in self.owned.items()
                  if ss == session) and time.monotonic()<deadline:
            time.sleep(.05)
        if any(SAMPLER.same_owned_process(p,s,ss) for p,(s,ss) in self.owned.items() if ss == session):
            raise RuntimeError('runtime tree left a live owned process after shutdown')
        self.events.append(dict(event='runtime_stopped',clean_shutdown=True))

    def close(self):
        # Cleanup also covers exceptions during preparation or typed IPC startup.
        for child, _ in self.roots:
            if child.poll() is None:
                self.remember_descendants(child.pid)
        for child, _ in reversed(self.roots):
            if child.stdin and not child.stdin.closed:
                child.stdin.close()
            if child.poll() is None:
                self.signal(child.pid, signal.SIGTERM)
                try:
                    child.wait(timeout=3)
                except subprocess.TimeoutExpired:
                    self.signal(child.pid, signal.SIGKILL)
                    child.wait(timeout=3)
            if child.stdout and not child.stdout.closed:
                if hasattr(child,'reader'):
                    child.reader.join(timeout=1)
                child.stdout.close()
        deadline = time.monotonic()+3
        while any(SAMPLER.same_owned_process(p,s,ss) for p,(s,ss) in self.owned.items()) and time.monotonic()<deadline:
            time.sleep(.05)
        survivors = [p for p,(s,ss) in self.owned.items() if SAMPLER.same_owned_process(p,s,ss)]
        for pid in survivors:
            self.signal(pid, signal.SIGKILL)
        if survivors:
            raise RuntimeError(f'owned children leaked, terminated by pidfd: {survivors}')


def summarize(world, name, repetition):
    summaries = []
    for phase in ('installed','one','many','released','fresh'):
        rows = [row for row in world.rows if row['phase']==phase]
        groups = {'all_observed':None,
                  'product_tree':{'runtime','launcher_helper','official_plugin'}}
        for role in {p['role'] for row in rows for p in row['processes']}:
            groups[role] = {role}
        for group, roles in groups.items():
            selected = [dict(row, processes=[p for p in row['processes']
                        if roles is None or p['role'] in roles]) for row in rows]
            value = SAMPLER.summarize_phase(selected,phase)
            value.update(plugin=name,repetition=repetition,group=group)
            summaries.append(value)
    return summaries


def run_case(out, name, repetition, seconds, interval):
    root = out/f'{name}{repetition}'
    with (out/f'{name}{repetition}.stderr.log').open('w') as log, \
         (out/f'{name}{repetition}.samples.jsonl').open('w') as raw:
        world = None
        try:
            # __new__ lets finally clean children even if backend preparation raises.
            world = World.__new__(World)
            world.__init__(root,name,log)
            world.start_runtime()
            world.configure_buttons()
            world.stop_runtime()  # Configuration editing history is outside every measured runtime.
            world.start_runtime()
            time.sleep(1)
            world.window('installed',seconds,interval,raw)
            world.set_visuals(1)
            world.window('one',seconds,interval,raw)
            world.set_visuals(32)
            world.window('many',seconds,interval,raw)
            world.set_visuals(0)
            world.window('released',seconds,interval,raw)
            world.stop_runtime()
            world.start_runtime()
            time.sleep(1)
            world.window('fresh',seconds,interval,raw)
            world.stop_runtime()
            summary = summarize(world,name,repetition)
            world.events.append(dict(event='complete',clean_shutdown=True))
            (out/f'{name}{repetition}.events.jsonl').write_text(
                ''.join(json.dumps(event)+'\n' for event in world.events))
            return summary
        finally:
            if world is not None and hasattr(world,'roots'):
                world.close()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('output',type=Path)
    parser.add_argument('--seconds',type=int,default=5)
    parser.add_argument('--repetitions',type=int,default=3)
    parser.add_argument('--interval',type=float,default=.2)
    parser.add_argument('--plugins',nargs='+',choices=NAMES,default=list(NAMES))
    args = parser.parse_args()
    try:
        out = validate_output(args.output,args.seconds,args.repetitions,args.interval)
    except ValueError as error:
        parser.error(str(error))
    out.mkdir(mode=0o700)
    sources = ('scripts/performance/officials.py','scripts/performance/officials.sh',
               'scripts/performance/profile.py','crates/rad-runtime/examples/official_profile_seed.rs')
    binaries = ('target/release/rad-runtime','target/release/rad-launcher-helper',
                'target/release/examples/conformance_client','target/release/examples/official_profile_seed',
                'target/plugins/release/rad-plugin-audio','target/plugins/release/rad-plugin-media',
                'target/plugins/release/examples/obs-fixture','target/plugins/release/examples/metrics-fixture',
                'target/plugins/release/examples/fake_pactl','target/plugins/release/examples/fake_player',
                'target/plugins/release/examples/obs-conformance-server')
    metadata = dict(schema_version=1,seconds=args.seconds,repetitions=args.repetitions,
        interval_s=args.interval,plugins=args.plugins,
        source_sha256={p:hashlib.sha256((REPO/p).read_bytes()).hexdigest() for p in sources},
        executable_sha256={p:hashlib.sha256((REPO/p).read_bytes()).hexdigest() for p in binaries},
        attribution=['seeding and button setup run before a fresh measured runtime',
                     'installed/one/many/released share a PID and inventory loading history',
                     'visual edits/undo history are included in runtime lifecycle deltas',
                     'fresh is a new PID after release; it diagnoses retained lifecycle history, not a proof of leak',
                     'backend/sandbox/controller costs are separate from product_tree',
                     'audio uses production binary with fake_pactl readonly mount and unshared network',
                     'media uses production binary with private D-Bus and fake player, no cover image',
                     'OBS uses same implementation with explicit ephemeral test endpoint',
                     'metrics uses same implementation and SDK with deterministic reader, never proc/NVML',
                     'sampled peaks are not true maxima; context switches are not exact wakeups'])
    (out/'metadata.json').write_text(json.dumps(metadata,indent=2)+'\n')
    summaries = []
    for repetition in range(args.repetitions):
        for name in args.plugins:
            print(f'Measuring official {name} repetition {repetition}',flush=True)
            summaries.extend(run_case(out,name,repetition,args.seconds,args.interval))
            (out/'resources.json').write_text(json.dumps(summaries,indent=2)+'\n')
    metrics = [s for s in summaries if s['plugin']=='metrics' and s['phase'] in ('one','many')
               and s['group']=='official_plugin']
    evaluation = dict(kind='synthetic_budget_observation',target_bytes=SAMPLER.METRICS_RSS_TARGET_BYTES,
                      scope='metrics fixture process only; no real NVML/proc reader',
                      metric='rss_bytes_sampled_peak',observations=len(metrics),
                      all_observed_peaks_within_target=all(s['rss_bytes_sampled_peak']<=SAMPLER.METRICS_RSS_TARGET_BYTES for s in metrics)
                          if metrics else None,
                      measured_peaks_bytes=[s['rss_bytes_sampled_peak'] for s in metrics],
                      certifies_hardware=False,product_target_certified=False)
    (out/'metrics-target.json').write_text(json.dumps(evaluation,indent=2)+'\n')
    print(out.relative_to(REPO),flush=True)


if __name__ == '__main__':
    main()
