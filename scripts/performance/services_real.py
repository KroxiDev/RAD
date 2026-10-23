#!/usr/bin/env python3
"""Explicit read-only production audio/MPRIS measurement; STOP before real OBS.

Default is inert. ROOT alone may use --run-services after safety review.
Personal endpoints stay in memory; only a fixed public app stem can temporarily
exist in the private RAD Store. Do not publish that profile; retire it to trash.
"""
import argparse
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import shlex
import shutil
import signal
import stat
import subprocess
import time

REPO = Path(__file__).resolve().parents[2]
SPEC = importlib.util.spec_from_file_location('services_owned_metrics_lifecycle', Path(__file__).with_name('metrics_real.py'))
METRICS = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(METRICS)
COMMON, SAMPLER, DESKTOP = METRICS.COMMON, METRICS.SAMPLER, METRICS.DESKTOP
PUBLIC_APPS = ('brave', 'chromium', 'firefox', 'spotify', 'vlc', 'mpv')
READ_QUERIES = (('-f', 'json', 'list', 'sinks'), ('-f', 'json', 'list', 'sources'),
                ('-f', 'json', 'info'), ('subscribe',))
PHASES = ('installed', 'one', 'many', 'released', 'fresh')
BINARY_SOURCES = {name:REPO/'target/release'/name for name in ('rad-runtime', 'rad-launcher-helper')}
BINARY_SOURCES.update({name:REPO/'target/release/examples'/name for name in
                       ('conformance_client', 'official_profile_seed')})
BINARY_SOURCES.update({f'rad-plugin-{name}':REPO/f'target/plugins/release/rad-plugin-{name}'
                       for name in ('audio', 'media')})
BINARY_SOURCES.update({name:Path('/usr/bin')/name for name in
                       ('bwrap', 'xdg-dbus-proxy', 'dbus-daemon')})
BINARY_SOURCES.update({'pactl-real':Path('/usr/bin/pactl'), 'sh':Path('/bin/sh'),
                       'fake_player':REPO/'target/plugins/release/examples/fake_player',
                       'readonly-proxy-probe':REPO/'target/plugins/release/examples/readonly-proxy-probe'})
PROXY_REPORT_FIELDS = {'complete', 'persistent_connection', 'known_getall_allowed', 'unique_getall_allowed',
                       'known_play_denied', 'known_set_denied', 'unique_play_denied', 'unique_set_denied',
                       'helper_no_auto_start_flag'}


def allowed_pactl_args(args):
    return tuple(args) in READ_QUERIES


def player_identity(app):
    if app not in PUBLIC_APPS:
        raise ValueError('only a fixed public generic app stem is accepted')
    return app


def validate_endpoint(path):
    if not path.is_absolute():
        raise ValueError('explicit absolute Unix socket required')
    found = path.lstat()
    if not stat.S_ISSOCK(found.st_mode) or found.st_uid != os.geteuid():
        raise ValueError('endpoint must be an owned Unix socket, not a file/symlink/fallback')
    return path


def proxy_command(program, address, socket, app):
    name = 'org.mpris.MediaPlayer2.'+player_identity(app)+'.*'
    return [str(program), address, str(socket), '--filter', '--see='+name,
            '--call='+name+'=org.freedesktop.DBus.Properties.GetAll@/org/mpris/MediaPlayer2',
            '--broadcast='+name+'=org.freedesktop.DBus.Properties.PropertiesChanged@/org/mpris/MediaPlayer2']


def validate_proxy_report(value):
    if not isinstance(value, dict) or set(value) != PROXY_REPORT_FIELDS or any(v is not True for v in value.values()):
        raise ValueError('persistent known/unique owner read-only gate was not fully verified')
    return value


def visual_observation(plugin, value):
    availability = value.get('availability')
    if availability not in ('current', 'stale', 'unavailable'):
        raise ValueError('unexpected service visual availability')
    generic = value.get('text') in (('On', 'Muted') if plugin == 'audio' else ('Playing', 'Paused', 'Stopped'))
    image = bool(value.get('image', False))
    return dict(availability=availability, generic_state=generic, has_image=image,
                valid_current=availability == 'current' and generic and not image and not value.get('fault', False))


def namespace_command(root, elf, pulse=None, bus=None, command=None):
    libraries = [part for path in ('/lib', '/lib64') if Path(path).exists()
                 for part in ('--ro-bind', path, path)]
    result = [str(elf/'bwrap'), '--die-with-parent', '--unshare-net', '--unshare-pid',
              '--ro-bind', '/usr', '/usr', '--symlink', 'usr/bin', '/bin', *libraries,
              '--dir', '/home', '--dir', '/run', '--dir', '/etc', '--dir', '/etc/pulse',
              '--proc', '/proc', '--dev', '/dev', '--tmpfs', '/tmp',
              '--ro-bind', str(elf), str(elf), '--bind', str(root), str(root)]
    if pulse is not None:
        result += ['--ro-bind', str(pulse), str(root/'endpoint/pulse'),
                   '--ro-bind', str(root/'guard-pactl'), '/usr/bin/pactl']
    if bus is not None:
        result += ['--ro-bind', str(bus), str(root/'endpoint/bus')]
    return result+['--chdir', str(root), '--', *(command or [str(elf/'rad-runtime')])]


def write_pactl_guard(root, elf):
    config, cookie = root/'pulse-client.conf', root/'synthetic-cookie'
    config.write_text('autospawn=no\ndaemon-binary=/usr/bin/false\n'
                      'auto-connect-localhost=no\nauto-connect-display=no\n'
                      'cookie-file='+str(cookie)+'\n')
    cookie.write_bytes(bytes(256))  # Public deterministic dummy; never read a personal authentication cookie.
    cookie.chmod(0o600)
    branches = []
    for args in READ_QUERIES:
        condition = ' && '.join(f'[ "${i}" = {shlex.quote(value)} ]' for i, value in enumerate(args, 1))
        branches.append(f'if [ "$#" -eq {len(args)} ] && {condition}; then allowed=1; fi')
    env = {'HOME':root/'home', 'XDG_CONFIG_HOME':root/'config',
           'XDG_RUNTIME_DIR':root/'runtime', 'PULSE_CLIENTCONFIG':config}
    exports = '\n'.join('export '+key+'='+shlex.quote(str(value)) for key, value in env.items())
    guard = root/'guard-pactl'
    guard.write_text('#!/bin/sh\nallowed=0\n'+'\n'.join(branches)+'\n'
        '[ "$allowed" -eq 1 ] || exit 97\n'+exports+'\n'
        'exec '+shlex.quote(str(elf/'pactl-real'))+' --server '+
        shlex.quote('unix:'+str(root/'endpoint/pulse'))+' --client-name RAD-read-only-measurement "$@"\n')
    guard.chmod(0o700)


def select_runtime(processes, session):
    if any(p['session'] != session for p in processes):
        raise ValueError('runtime discovery encountered an escaped session')
    selected = [p['pid'] for p in processes if p['executable'] == 'runtime']
    if len(selected) != 1:
        raise ValueError('one hash-verified host runtime PID is required')
    return selected[0]


def availability_summary(observations):
    phases = {}
    for phase in ('one', 'many'):
        rows = [row for row in observations if row['phase'] == phase]
        values = [v for row in rows for v in row['sources']]
        phases[phase] = dict(samples=len(rows), observations=len(values),
            current=sum(v['availability'] == 'current' for v in values),
            stale=sum(v['availability'] == 'stale' for v in values),
            unavailable=sum(v['availability'] == 'unavailable' for v in values),
            all_sampled_current_valid=bool(values) and all(v['valid_current'] for v in values))
    return dict(status='observed_current' if all(p['all_sampled_current_valid'] for p in phases.values())
                else 'not_verified', phases=phases, continuous_freshness_certified=False)


class ReadPeer(COMMON.Bridge):
    """Only visual configuration in a new private document; never execution APIs."""
    def __init__(self, world):
        self.world = world
        self.child = world.spawn([str(world.elf/'conformance_client'), str(world.root/'runtime'), 'editor'],
                                 'client', drain=False)

    def call(self, operation, **args):
        if operation not in ('status', 'machine_settings', 'plugin_status', 'create_button', 'edit', 'plugin_visual'):
            raise ValueError('non-read-only service operation refused')
        if operation == 'edit':
            if not 1 <= len(args.get('changes', [])) <= 32:
                raise ValueError('bounded visual changes required')
            for change in args['changes']:
                if change.get('kind') != 'set' or change.get('field') != 'visual':
                    raise ValueError('only private visual binding edits are allowed')
                value = change.get('value', {})
                if value.get('type') not in ('manual', 'org.rad.audio#output',
                        'org.rad.audio#microphone', 'org.rad.media#now_playing'):
                    raise ValueError('only fixed service visuals are allowed')
                parameters = value.get('parameters')
                if value['type'] != 'manual':
                    expected = 'player' if value['type'].startswith('org.rad.media#') else 'device'
                    if (not isinstance(parameters, dict) or parameters.get('schema_version') != 1 or
                            set(parameters.get('values', {})) != {expected}):
                        raise ValueError('invalid private visual parameters')
                    selected = parameters['values'][expected]
                    if (expected == 'device' and selected != '@default') or (
                            expected == 'player' and selected not in PUBLIC_APPS):
                        raise ValueError('personal visual selector refused')
        return super().call(operation, **args)


def file_digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


class ServicesWorld(COMMON.World):
    def spawn(self, *args, **kwargs):
        with DESKTOP.registered_spawn():
            return super().spawn(*args, **kwargs)

    def __init__(self, root, elf, hashes, name, pulse=None, bus=None, app=None):
        self.root, self.name, self.elf, self.hashes = root, name, elf, hashes
        self.roots, self.owned, self.backends = [], {}, []
        self.runtime = self.runtime_pid = None
        self.rows, self.events, self.unobserved, self.observations = [], [], set(), []
        self.log = subprocess.DEVNULL  # Never retain raw service/runtime stderr.
        self.epoch, self.visual_count = time.monotonic(), 0
        self.roles, self.pulse, self.app = {}, pulse, app
        root.mkdir(mode=0o700)
        with DESKTOP.registered_spawn():
            seeded = subprocess.run([str(elf/'official_profile_seed'), str(root), name],
                env={'PATH':'/usr/bin:/bin'}, stdin=subprocess.DEVNULL,
                stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, text=True, timeout=30, check=True)
        event = json.loads(seeded.stdout)
        if event.get('plugin') != name or event.get('executable') != f'target/plugins/release/rad-plugin-{name}':
            raise ValueError('fixed production plugin seed required')
        self.events.append(dict(event='seeded', plugin=name, separate_process=True,
                                executable_bytes=event['executable_bytes'], package_bytes=event['package_bytes']))
        inventory = root/'data/rad/plugins'
        self.events.append(dict(event='inventory_before_execution', sha256={
            str(path.relative_to(inventory)):file_digest(path) for path in sorted(inventory.rglob('*'))
            if path.is_file()}))
        self.env = METRICS.private_environment(root)
        machine = root/'data/rad/machine.json'
        machine.write_text(json.dumps(dict(version=1, use_stream_deck=False, start_with_session=False)))
        machine.chmod(0o600)
        (root/'endpoint').mkdir(mode=0o700)
        self.bus_proxy = None
        if name == 'audio':
            (root/'endpoint/pulse').touch(mode=0o600)
            write_pactl_guard(root, elf)
        elif name == 'media':
            player_identity(app)
            self.bus_proxy = self.start_proxy('unix:path='+str(bus), root/'proxy.sock', app)
            (root/'endpoint/bus').touch(mode=0o600)
            self.env['DBUS_SESSION_BUS_ADDRESS'] = 'unix:path='+str(root/'endpoint/bus')
        else:
            raise ValueError('only production audio/media services are permitted, never OBS')

    def start_proxy(self, address, socket, app):
        proxy = self.spawn(proxy_command(self.elf/'xdg-dbus-proxy', address, socket, app), 'readonly_proxy')
        def ready():
            if proxy.poll() is not None:
                raise RuntimeError('read-only proxy exited before readiness')
            return socket.exists() and stat.S_ISSOCK(socket.lstat().st_mode)
        COMMON.until(ready, bool)
        return proxy

    def executable_role(self, process):
        key = (process['pid'], process['start_ticks'])
        if key not in self.roles:
            exe = Path('/proc')/str(process['pid'])/'exe'
            path = exe.resolve(strict=True)
            by_path = {'rad-runtime':'runtime', 'rad-launcher-helper':'launcher_helper',
                       'bwrap':'sandbox_supervisor', 'pactl-real':'audio_pactl', 'sh':'read_guard'}
            role, binary = None, None
            for name, candidate in by_path.items():
                if path == self.elf/name:
                    role, binary = candidate, name
            # The guard's absolute /bin/sh interpreter is the same frozen shell ELF.
            if path == Path('/bin/sh').resolve():
                role, binary = 'read_guard', 'sh'
            if path.is_relative_to(self.root/'data/rad/plugins'):
                role, binary = 'service_plugin', 'rad-plugin-'+self.name
            if role is None or file_digest(exe) != self.hashes[binary]:
                raise ValueError('unrecognized or changed executable in the owned service namespace')
            if not SAMPLER.same_owned_process(process['pid'], process['start_ticks'], self.runtime.pid):
                raise ValueError('namespace process identity changed during attribution')
            self.roles[key] = role
        return self.roles[key]

    def namespace_processes(self, resources):
        values = []
        for pid in SAMPLER.descendants(self.runtime.pid):
            try:
                process = SAMPLER.read_process(pid, self.runtime.pid) if resources else dict(
                    SAMPLER.parse_stat((Path('/proc')/str(pid)/'stat').read_text()), pid=pid)
                if process['session'] != self.runtime.pid:
                    raise ValueError('namespace process escaped the owned host SID')
                identity = (process['start_ticks'], self.runtime.pid)
                if pid in self.owned and self.owned[pid] != identity:
                    raise ValueError('owned host PID identity was reused')
                self.owned[pid] = identity
                process['executable'] = self.executable_role(process)
                values.append(process)
            except FileNotFoundError:
                continue
        return values

    def start_runtime(self):
        bus = self.root/'proxy.sock' if self.name == 'media' else None
        started_ns = time.time_ns()
        self.runtime = self.spawn(namespace_command(self.root, self.elf, self.pulse, bus), 'runtime_namespace')
        def started():
            if self.runtime.poll() is not None:
                raise RuntimeError('private service runtime exited before readiness')
            socket = self.root/'runtime/rad/editor.sock'
            return socket.exists() and socket.stat().st_ctime_ns >= started_ns
        COMMON.until(started, bool)
        self.runtime_pid = select_runtime(self.namespace_processes(False), self.runtime.pid)
        with ReadPeer(self) as peer:
            self.wait_host(peer, 'absent')
            self.check_disabled(peer)
        self.events.append(dict(event='runtime_started', pid=self.runtime_pid,
                                namespace_pid_not_used=True, host_sid=self.runtime.pid))

    @staticmethod
    def check_disabled(peer):
        if peer.call('status')['deck_state'] != 'disabled':
            raise ValueError('hardware-enabled state refused')
        machine = peer.call('machine_settings')
        if machine['use_stream_deck'] or machine['start_with_session']:
            raise ValueError('private machine configuration must remain disabled')

    def configure_buttons(self):
        with ReadPeer(self) as peer:
            folder = peer.call('status')['active_folder_id']
            for position in range(32):
                peer.call('create_button', folder_id=folder, position=position, name=f'Private {position}')
            self.wait_host(peer, 'absent')

    def set_visuals(self, count):
        if count not in (0, 1, 32):
            raise ValueError('only bounded 0/1/32 visual phases are allowed')
        def visual(index):
            if index >= count:
                return dict(type='manual', parameters=None)
            if self.name == 'audio':
                return COMMON.visual_for('audio', index)
            return dict(type='org.rad.media#now_playing',
                        parameters=dict(schema_version=1, values=dict(player=self.app)))
        with ReadPeer(self) as peer:
            peer.call('edit', changes=[dict(kind='set', id=f'b{i+1}', field='visual', value=visual(i))
                                       for i in range(32)])
            self.wait_host(peer, 'ready' if count else 'absent')
            self.check_disabled(peer)
        self.visual_count = count
        self.events.append(dict(event='transition', visuals=count))
        time.sleep(2)  # A missing real endpoint/player remains unavailable, not a fake timeout PASS.

    def capture(self, phase):
        if self.runtime is None or self.runtime.poll() is not None:
            raise RuntimeError('service runtime ended during resource window')
        values = self.namespace_processes(True)
        roots = {p['pid'] for p in values if p['executable'] == 'service_plugin'}
        if len(roots) != int(self.visual_count > 0):
            raise ValueError('unexpected demanded production plugin process count')
        tree = set(roots)
        while True:
            more = {p['pid'] for p in values if p['ppid'] in tree}
            if more <= tree:
                break
            tree.update(more)
        for process in values:
            role = process.pop('executable')
            if process['pid'] in tree:
                role = 'service_plugin' if process['pid'] in roots else 'plugin_child'
            elif role not in ('runtime', 'launcher_helper', 'sandbox_supervisor'):
                raise ValueError('unattributed child outside the production plugin tree')
            process['role'] = role
        if sum(p['role'] == 'runtime' for p in values) != 1:
            raise ValueError('runtime host identity missing from the sampled namespace')
        if self.bus_proxy is not None:
            proxy = self.bus_proxy
            if proxy.poll() is not None:
                raise ValueError('owned read-only proxy ended during service measurement')
            process = SAMPLER.read_process(proxy.pid, proxy.pid)
            if self.owned.get(proxy.pid) != (process['start_ticks'], proxy.pid):
                raise ValueError('owned proxy identity changed')
            process['role'] = 'readonly_proxy'
            values.append(process)
        row = dict(elapsed_s=time.monotonic()-self.epoch, phase=phase, processes=values)
        self.rows.append(row)
        return row

    def observe_visuals(self, peer, phase, all_visuals=False):
        self.check_disabled(peer)
        count = self.visual_count if all_visuals else min(self.visual_count, 2 if self.name == 'audio' else 1)
        values = [visual_observation(self.name, peer.call('plugin_visual', id=f'b{i+1}')) for i in range(count)]
        row = dict(phase=phase, elapsed_s=time.monotonic()-self.epoch, visuals=self.visual_count, sources=values)
        self.observations.append(row)
        return row

    def window(self, phase, seconds, interval, raw):
        self.events.append(dict(event='phase_begin', phase=phase, visuals=self.visual_count))
        epoch, tick, next_visual = time.monotonic(), 0, 0.
        with ReadPeer(self) as peer:
            initial = self.observe_visuals(peer, phase, all_visuals=True)
            self.events.append(dict(event='initial_visual_availability', **initial))
            while True:
                row = self.capture(phase)
                now = time.monotonic()
                if now >= next_visual:
                    row['availability'] = self.observe_visuals(peer, phase)
                    next_visual = now+1
                raw.write(json.dumps(row)+'\n')
                if now-epoch >= seconds:
                    break
                tick += 1
                time.sleep(max(0, min(epoch+tick*interval, epoch+seconds)-time.monotonic()))
        raw.flush()
        self.events.append(dict(event='phase_end', phase=phase))


def bounded_probe(world, command):
    """Owned synthetic checks only: no real endpoint is ever passed here."""
    child = world.spawn([str(world.elf/'sh'), '-c', 'exec "$@" 2>&1',
                         'private-service-filter-check', *map(str, command)], 'synthetic_probe', drain=False)
    try:
        output, _ = child.communicate(timeout=8)
    except subprocess.TimeoutExpired:
        world.signal(child.pid, signal.SIGKILL)
        child.communicate(timeout=3)
        raise RuntimeError('synthetic service guard deadline exceeded') from None
    return child.returncode, output


def validate_filters(out, elf):
    """ROOT runs this owned fixture gate BEFORE any real endpoint is connected."""
    world = ServicesWorld.__new__(ServicesWorld)
    world.root, world.elf, world.log = out/'validation', elf, subprocess.DEVNULL
    world.roots, world.owned, world.events, world.unobserved = [], {}, [], set()
    world.root.mkdir(mode=0o700)
    world.env = METRICS.private_environment(world.root)
    world.env['RAD_READONLY_PROXY_PRIVATE_GATE'] = '1'
    result = dict(complete=False, synthetic_only=True)
    try:
        config = world.root/'bus.conf'
        config.write_text(f'''<busconfig><type>session</type><listen>unix:path={world.root}/bus</listen>
<auth>EXTERNAL</auth><policy context="default"><allow send_destination="*"/>
<allow receive_sender="*"/><allow own="*"/></policy></busconfig>''')
        bus = world.spawn([str(elf/'dbus-daemon'), f'--config-file={config}', '--nofork', '--print-address'], 'fixture_bus')
        address = world.line(bus)
        if not address.startswith('unix:path='+str(world.root/'bus')+','):
            raise ValueError('fixture did not create its own private bus')
        world.env['DBUS_SESSION_BUS_ADDRESS'] = address
        player = world.spawn([str(elf/'fake_player'), 'org.mpris.MediaPlayer2.brave.instance1'],
                              'fixture_player', {'FAKE_PLAYER_PRIVATE_BUS':'1'})
        if world.line(player) != 'ready':
            raise ValueError('owned fixture player failed readiness')
        proxy_socket = world.root/'filtered.sock'
        world.start_proxy(address, proxy_socket, 'brave')
        code, output = bounded_probe(world, [elf/'readonly-proxy-probe', world.root])
        if code != 0:
            raise ValueError('persistent read-only proxy probe failed')
        result['persistent_proxy_gate'] = validate_proxy_report(json.loads(output))
        fake_elf = world.root/'fake-elf'
        fake_elf.mkdir(mode=0o700)
        fake = fake_elf/'pactl-real'
        fake.write_text('#!/bin/sh\n[ "$1" = --server ] && [ "$3" = --client-name ] || exit 96\n'
                        '[ -n "$PULSE_CLIENTCONFIG" ] && [ -n "$HOME" ] || exit 96\n'
                        'printf "guard-accepted\\n"\n')
        fake.chmod(0o700)
        write_pactl_guard(world.root, fake_elf)
        for query in READ_QUERIES:
            code, output = bounded_probe(world, [world.root/'guard-pactl', *query])
            if code != 0 or output.strip() != 'guard-accepted':
                raise ValueError('read-only pactl guard refused a known query')
        for query in (('set-sink-mute', '@DEFAULT_SINK@', 'toggle'), ('play-sample', 'x'),
                      ('--server=other', 'subscribe'), ('exit',), ('-f', 'json', 'info', 'extra')):
            code, output = bounded_probe(world, [world.root/'guard-pactl', *query])
            if code != 97 or output:
                raise ValueError('pactl guard did not fail closed before its executable')
        result['pactl_allowlist_validated'] = True
        sentinel = out/'synthetic-private-sentinel'
        sentinel.write_text('synthetic privacy sentinel; never personal data')
        code = ('import json,os,pathlib; '
                f'print(json.dumps(dict(hidden_file=not pathlib.Path({str(sentinel)!r}).exists(),'
                f'hidden_host_proc=not pathlib.Path("/proc/{os.getpid()}/environ").exists(),'
                f'private_pid=os.getpid()!={os.getpid()})))')
        status, output = bounded_probe(world, namespace_command(world.root, elf,
            command=['/usr/bin/python3', '-I', '-S', '-B', '-c', code]))
        isolation = json.loads(output) if status == 0 else {}
        if not isolation or not all(isolation.values()):
            raise ValueError('private namespace did not hide host files/processes')
        result['namespace_isolation_validated'] = True
        with DESKTOP.protected_cleanup():
            world.close()
        if world.unobserved:
            raise ValueError('fixture gate had unobserved descendants')
        result['complete'] = True
        return result
    finally:
        with DESKTOP.protected_cleanup():
            try:
                world.close()
            except BaseException:
                result['complete'] = False
                raise
            finally:
                (out/'filter-validation.json').write_text(json.dumps(result, indent=2)+'\n')


def summarize(world, repetition):
    result = []
    groups = (('service_plugin_tree', {'service_plugin', 'plugin_child'}),
              ('service_plugin_only', {'service_plugin'}),
              ('product_tree', {'runtime', 'launcher_helper', 'service_plugin', 'plugin_child'}),
              ('sandbox_supervisor', {'sandbox_supervisor'}),
              ('readonly_proxy', {'readonly_proxy'}))
    for phase in PHASES:
        for group, roles in groups:
            selected = [dict(row, processes=[p for p in row['processes'] if p['role'] in roles])
                        for row in world.rows]
            result.append(dict(SAMPLER.summarize_phase(selected, phase), repetition=repetition,
                               plugin=world.name, group=group))
    return result


def run_case(out, elf, hashes, name, repetition, seconds, interval, pulse=None, bus=None, app=None):
    world = ServicesWorld.__new__(ServicesWorld)
    stem = f'{name}{repetition}'
    with (out/(stem+'.samples.jsonl')).open('w') as raw:
        try:
            world.__init__(out/stem, elf, hashes, name, pulse, bus, app)
            world.start_runtime()
            world.configure_buttons()
            world.stop_runtime()
            world.start_runtime()
            time.sleep(1)
            world.window('installed', seconds, interval, raw)
            world.set_visuals(1)
            world.window('one', seconds, interval, raw)
            world.set_visuals(32)
            world.window('many', seconds, interval, raw)
            world.set_visuals(0)
            world.window('released', seconds, interval, raw)
            world.stop_runtime()
            world.start_runtime()
            time.sleep(1)
            world.window('fresh', seconds, interval, raw)
            world.stop_runtime()
            with DESKTOP.protected_cleanup():
                world.close()
            if world.unobserved:
                raise ValueError('unobserved descendants prevent complete service evidence')
            world.events.append(dict(event='complete', clean_shutdown=True, all_owned_reaped=True))
            return summarize(world, repetition), dict(plugin=name, repetition=repetition,
                                                      **availability_summary(world.observations))
        finally:
            if hasattr(world, 'roots'):
                with DESKTOP.protected_cleanup():
                    try:
                        world.close()
                    except BaseException:
                        world.events = [event for event in world.events if event.get('event') != 'complete']
                        world.events.append(dict(event='cleanup_failed'))
                        raise
                    finally:
                        (out/(stem+'.events.jsonl')).write_text(
                            ''.join(json.dumps(event)+'\n' for event in world.events))


def provenance():
    sources = ['scripts/performance/services_real.py', 'scripts/performance/test_services_real.py',
               'scripts/performance/metrics_real.py',
               'scripts/performance/desktop.py', 'scripts/performance/physical.py',
               'scripts/performance/officials.py', 'scripts/performance/profile.py',
               'crates/rad-runtime/examples/official_profile_seed.rs',
               'plugins/media/examples/readonly-proxy-probe.rs']
    for name in ('audio', 'media'):
        sources.append(f'plugins/{name}/manifest.json')
        sources.extend(str(p.relative_to(REPO)) for p in sorted((REPO/f'plugins/{name}/src').glob('*.rs')))
    return dict(source_sha256={path:file_digest(REPO/path) for path in sources},
                executable_sha256={name:file_digest(path) for name, path in BINARY_SOURCES.items()})


def retain_provenance(out, metadata):
    elf = out/'elf'
    elf.mkdir(mode=0o700)
    for name, source in BINARY_SOURCES.items():
        shutil.copy2(source, elf/name)
        if file_digest(elf/name) != metadata['executable_sha256'][name]:
            raise ValueError('executable changed during retention')
    sources = out/'source-copies'
    for relative, sha in metadata['source_sha256'].items():
        destination = sources/relative
        destination.parent.mkdir(mode=0o700, parents=True, exist_ok=True)
        shutil.copy2(REPO/relative, destination)
        if file_digest(destination) != sha:
            raise ValueError('source changed during retention')
    return elf


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('output', type=Path)
    mode = parser.add_mutually_exclusive_group()
    mode.add_argument('--run-services', action='store_true')
    mode.add_argument('--validate-only', action='store_true', help='ROOT: only owned fake-service/filter checks')
    parser.add_argument('--plugins', nargs='+', choices=('audio', 'media'), default=['audio', 'media'])
    parser.add_argument('--pulse-socket', type=Path)
    parser.add_argument('--bus-socket', type=Path)
    parser.add_argument('--player-app', help='fixed public generic app stem only; never a title or instance ID')
    parser.add_argument('--seconds', type=int, default=5)
    parser.add_argument('--repetitions', type=int, default=3)
    parser.add_argument('--interval', type=float, default=.2)
    args = parser.parse_args(argv)
    if not args.run_services and not args.validate_only:
        print('DRY: no service/endpoint, application, device or sensor inspection/launch')
        return 0
    try:
        out = COMMON.validate_output(args.output, args.seconds, args.repetitions, args.interval)
        if args.player_app is not None:
            player_identity(args.player_app)
        if args.run_services:
            if 'audio' in args.plugins:
                if args.pulse_socket is None:
                    raise ValueError('audio requires an explicit Pulse Unix socket')
                validate_endpoint(args.pulse_socket)
            if 'media' in args.plugins and args.player_app is not None:
                if args.bus_socket is None:
                    raise ValueError('media requires an explicit session-bus Unix socket')
                validate_endpoint(args.bus_socket)
    except (ValueError, OSError):
        parser.error('new bounded output and explicit owned Unix endpoints/public generic selector required')
    metadata = provenance()
    metadata.update(schema_version=1, complete=False, services_read_only=True, real_obs=False,
        repetitions=args.repetitions, seconds=args.seconds, interval_s=args.interval,
        media_selector_supplied=args.player_app is not None,
        limits=['ROOT only; no plugin execution actions, playback, volume, mute or default-device changes',
                'audio uses real pactl through an exact read-query allowlist and explicit Unix server',
                'private Pulse client config disables autospawn and uses a deterministic dummy cookie only',
                'auth requiring a personal cookie remains unavailable; no personal cookie/config is read',
                'media sees a method-filtered proxy only; one persistent connection must allow GetAll and deny Play/Set for known and unique owners',
                'no-autoactivation is supplied by product Bus NO_AUTO_START, not inferred from proxy policy',
                'namespace hides personal home/tmp/proc, so personal cover-art files are not measured',
                'fixed public app stem may temporarily exist only in private RAD Store; do not publish profile; retire to trash',
                'no player/device/title/artURL or raw service stdout/stderr is retained in results',
                'proxy and namespace supervisor resources are separate from the production plugin tree',
                'resource samples/API reads are not atomic; peaks are sampled, not absolute',
                'short-lived pactl CPU/memory can escape samples; zero CPU may be below tick resolution',
                'context switches are not exact wakeups; this harness does not change any metrics budget'])
    out.mkdir(mode=0o700)
    elf = retain_provenance(out, metadata)
    previous = {sig:signal.getsignal(sig) for sig in (signal.SIGTERM, signal.SIGINT)}
    DESKTOP.install_shutdown_signals()
    resources, availability = [], []
    try:
        (out/'metadata.json').write_text(json.dumps(metadata, indent=2)+'\n')
        metadata['stage'] = 'synthetic_filter_gate'
        validate_filters(out, elf)
        if args.run_services:
            for repeat in range(args.repetitions):
                for name in dict.fromkeys(args.plugins):
                    if name == 'media' and args.player_app is None:
                        availability.append(dict(plugin=name, repetition=repeat, status='not_verified',
                                                 reason='no generic player selector supplied'))
                        continue
                    metadata['stage'] = name
                    rows, current = run_case(out, elf, metadata['executable_sha256'], name, repeat,
                        args.seconds, args.interval, args.pulse_socket if name == 'audio' else None,
                        args.bus_socket if name == 'media' else None, args.player_app if name == 'media' else None)
                    resources.extend(rows)
                    availability.append(current)
                    (out/'resources.json').write_text(json.dumps(resources, indent=2)+'\n')
                    (out/'availability.json').write_text(json.dumps(availability, indent=2)+'\n')
        if provenance() != {key:metadata[key] for key in ('source_sha256', 'executable_sha256')}:
            raise ValueError('measured sources or executable aliases changed during the run')
        if any(file_digest(elf/name) != sha for name, sha in metadata['executable_sha256'].items()):
            raise ValueError('retained executable changed during the run')
        metadata.update(complete=True, all_owned_reaped=True, fingerprints_match=True, stage='complete')
        (out/'availability.json').write_text(json.dumps(availability, indent=2)+'\n')
        print('COMPLETE '+str(out.relative_to(REPO)), flush=True)
        return 0
    except BaseException as error:
        metadata.update(complete=False, error_type=type(error).__name__)
        print('FAILED: '+type(error).__name__+'; no raw service diagnostics retained', flush=True)
        return 1
    finally:
        with DESKTOP.protected_cleanup():
            (out/'metadata.json').write_text(json.dumps(metadata, indent=2)+'\n')
        for sig, handler in previous.items():
            signal.signal(sig, handler)


if __name__ == '__main__':
    raise SystemExit(main())
