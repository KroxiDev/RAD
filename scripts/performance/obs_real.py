#!/usr/bin/env python3
"""Opt-in OBS production-plugin measurements behind a read-only local relay."""
import argparse
import importlib.util
import json
import os
from pathlib import Path
import queue
import shutil
import signal
import statistics
import subprocess
import time

REPO = Path(__file__).resolve().parents[2]
SPEC = importlib.util.spec_from_file_location('obs_services', Path(__file__).with_name('services_real.py'))
SERVICES = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(SERVICES)
COMMON, SAMPLER, DESKTOP = SERVICES.COMMON, SERVICES.SAMPLER, SERVICES.DESKTOP
PHASES = SERVICES.PHASES
BINARIES = {name:REPO/'target/release'/name for name in ('rad-runtime', 'rad-launcher-helper')}
BINARIES.update({name:REPO/'target/release/examples'/name for name in
                 ('conformance_client', 'official_profile_seed')})
BINARIES.update({
    'rad-plugin-obs':REPO/'target/plugins/release/rad-plugin-obs',
    'obs-readonly-relay':REPO/'target/plugins/release/examples/obs-readonly-relay',
    'obs-conformance-server':REPO/'target/plugins/release/examples/obs-conformance-server',
    'bwrap':Path('/usr/bin/bwrap'), 'zenity-audit':Path('/usr/bin/zenity'),
})
GATE_FIELDS = {'complete', 'synthetic_only', 'three_reads_permitted', 'writes_denied_post_auth',
               'wrong_auth_before_upstream', 'zero_upstream_effects', 'upstream_outputs_only',
               'events_filtered_and_stripped', 'private_response_fields_stripped',
               'version_is_upstream_not_synthetic', 'snapshot_exact',
               'finish_closes_owned_fds', 'eof_closes_owned_fds'}


class ObsPeer(COMMON.Bridge):
    def __init__(self, world):
        self.world = world
        self.child = world.spawn([str(world.elf/'conformance_client'),
                                  str(world.root/'runtime'), 'editor'], 'client', drain=False)

    def call(self, operation, **args):
        if operation in ('status', 'machine_settings', 'plugin_status'):
            valid = not args
        elif operation == 'plugin_visual':
            valid = set(args) == {'id'} and button_id(args['id'])
        elif operation == 'create_button':
            position = args.get('position')
            valid = (set(args) == {'folder_id', 'position', 'name'} and
                     type(position) is int and 0 <= position < 32 and
                     args['name'] == f'Private {position}' and isinstance(args['folder_id'], str))
        elif operation == 'plugin_settings':
            valid = args == {'plugin_id':'org.rad.obs'}
        elif operation == 'set_plugin_settings':
            valid = (set(args) == {'plugin_id', 'parameters'} and args['plugin_id'] == 'org.rad.obs'
                     and private_settings(args['parameters'], self.world.relay.port))
        elif operation == 'credential_replace':
            valid = (set(args) == {'plugin_id', 'slot', 'secret'} and args['plugin_id'] == 'org.rad.obs'
                     and args['slot'] == 'password' and isinstance(args['secret'], str)
                     and args['secret'] == self.world.relay.credential)
        elif operation == 'edit':
            changes = args.get('changes')
            valid = (set(args) == {'changes'} and isinstance(changes, list) and
                     1 <= len(changes) <= 32 and all(private_visual_edit(c) for c in changes) and
                     len({c['id'] for c in changes}) == len(changes))
        else:
            valid = False
        if not valid:
            raise ValueError('non-observer private OBS operation refused')
        try:
            return super().call(operation, **args)
        except Exception:
            # Never surface untrusted IPC details, parameters or synthetic credentials.
            raise RuntimeError('private OBS client request failed') from None


def button_id(value):
    return isinstance(value, str) and value in {f'b{i}' for i in range(1, 33)}


def private_settings(parameters, port):
    return (type(port) is int and 1 <= port <= 65535 and port != 4455 and
            isinstance(parameters, dict) and type(parameters.get('schema_version')) is int and
            parameters == {'schema_version':1, 'values':{'port':port}} and
            type(parameters['values']['port']) is int)


def private_visual_edit(change):
    if (not isinstance(change, dict) or set(change) != {'kind', 'id', 'field', 'value'} or
            change['kind'] != 'set' or change['field'] != 'visual' or not button_id(change['id'])):
        return False
    value = change['value']
    if not isinstance(value, dict) or set(value) != {'type', 'parameters'}:
        return False
    if value['type'] == 'manual':
        return value['parameters'] is None
    return (value['type'] in ('org.rad.obs#record', 'org.rad.obs#stream') and
            isinstance(value['parameters'], dict) and type(value['parameters'].get('schema_version')) is int and
            value['parameters'] == {'schema_version':1, 'values':{}})


def provenance():
    sources = ['scripts/performance/'+name+'.py' for name in (
        'obs_real', 'test_obs_real', 'services_real', 'metrics_real', 'desktop',
        'physical', 'officials', 'profile')]
    sources += ['crates/rad-runtime/examples/official_profile_seed.rs',
                'plugins/obs/examples/readonly-relay.rs', 'plugins/obs/manifest.json',
                'plugins/obs/Cargo.toml', 'plugins/Cargo.lock', 'Cargo.lock']
    sources += [str(p.relative_to(REPO)) for p in sorted((REPO/'plugins/obs/src').glob('*.rs'))]
    return dict(source_sha256={p:SERVICES.file_digest(REPO/p) for p in sources},
                executable_sha256={n:SERVICES.file_digest(p) for n,p in BINARIES.items()})


def visual_observation(source, value):
    availability = value.get('availability')
    if source not in ('record', 'stream') or availability not in ('current', 'stale', 'unavailable'):
        raise ValueError('unexpected OBS visual kind or availability')
    generic = value.get('text') in (('Recording', 'Paused', 'Stopped') if source == 'record'
                                  else ('Streaming', 'Stopped'))
    image = bool(value.get('image', False))
    return dict(availability=availability, generic_state=generic, has_image=image,
                valid_current=availability == 'current' and generic and not image and not value.get('fault', False))


def verify_settings(value, port):
    try:
        if (value.get('plugin_id') != 'org.rad.obs' or value.get('encoding') != 'hex' or
                not isinstance(value.get('hex'), str) or len(value['hex']) > 1024):
            raise ValueError
        parameters = json.loads(bytes.fromhex(value['hex']))
        if not private_settings(parameters, port):
            raise ValueError
    except (ValueError, KeyError, TypeError, UnicodeError):
        raise ValueError('private OBS settings do not select the read-only guard') from None


def retain(out, metadata):
    elf = out/'elf'
    elf.mkdir(mode=0o700)
    for name, path in BINARIES.items():
        shutil.copy2(path, elf/name)
        if SERVICES.file_digest(elf/name) != metadata['executable_sha256'][name]:
            raise ValueError('executable changed while retaining preexec input')
    for name, sha in metadata['source_sha256'].items():
        path = out/'source-copies'/name
        path.parent.mkdir(mode=0o700, parents=True, exist_ok=True)
        shutil.copy2(REPO/name, path)
        if SERVICES.file_digest(path) != sha:
            raise ValueError('source changed while retaining preexec input')
    return elf


class RelayOwner(COMMON.World):
    """Owns only the relay and optional private oracle, never personal OBS."""
    def __init__(self, root, real):
        self.root, self.log = root, subprocess.DEVNULL
        self.roots, self.owned, self.events, self.unobserved = [], {}, [], set()
        self.runtime = self.runtime_pid = None
        root.mkdir(mode=0o700)
        self.env = SERVICES.METRICS.private_environment(root)
        if real:
            for key in ('DISPLAY', 'WAYLAND_DISPLAY', 'XDG_RUNTIME_DIR', 'DBUS_SESSION_BUS_ADDRESS'):
                if key in os.environ:
                    self.env[key] = os.environ[key]

    def spawn(self, *args, **kwargs):
        with DESKTOP.registered_spawn():
            return super().spawn(*args, **kwargs)


class RelayProcess:
    def __init__(self, out, elf, hashes, real):
        self.owner = RelayOwner(out/'relay', real)
        self.credential = None
        self.child, self.final = None, None
        self.hashes = hashes
        try:
            command = [str(elf/'obs-readonly-relay'), '--real', str(self.owner.root), str(os.getpid())]
            if not real:
                oracle = self.owner.spawn([str(elf/'obs-conformance-server')], 'synthetic_oracle')
                reply = json.loads(COMMON.World.line(oracle))
                port = reply.get('port')
                if type(port) is not int or not 1 <= port <= 65535 or port == 4455:
                    raise ValueError('synthetic oracle did not select an ephemeral endpoint')
                command = [str(elf/'obs-readonly-relay'), '--synthetic', str(self.owner.root),
                           str(port), str(os.getpid())]
            self.child = self.owner.spawn(command, 'readonly_relay')
            value = self.reply('ready', 145)
            if (set(value) != {'event', 'port', 'credential'} or type(value['port']) is not int or
                    not 1 <= value['port'] <= 65535 or value['port'] == 4455 or
                    not isinstance(value['credential'], str) or len(value['credential']) != 64 or
                    any(c not in '0123456789abcdef' for c in value['credential'])):
                raise ValueError('relay did not provide a bounded private session')
            self.port, self.credential = value['port'], value['credential']
        except BaseException:
            with DESKTOP.protected_cleanup():
                self.owner.close()
            raise

    def reply(self, event, timeout=8):
        deadline = time.monotonic()+timeout
        while time.monotonic() < deadline:
            try:
                line = self.child.lines.get(timeout=.1)
            except queue.Empty:
                if self.child.poll() is not None:
                    raise RuntimeError('owned relay ended before response') from None
                continue
            try:
                value = json.loads(line)
            except ValueError:
                raise ValueError('invalid relay control response') from None
            if not isinstance(value, dict) or value.get('event') != event:
                raise ValueError('unexpected relay control response')
            return value
        raise TimeoutError('owned relay control deadline')

    def snapshot(self):
        if self.child.poll() is not None:
            raise RuntimeError('owned relay ended during measurement')
        self.child.stdin.write('snapshot\n')
        self.child.stdin.flush()
        return validate_snapshot(self.reply('snapshot'))

    def resource(self, elf):
        if self.child.poll() is not None:
            raise RuntimeError('owned relay ended during resource capture')
        path = Path('/proc')/str(self.child.pid)/'stat'
        p = SAMPLER.parse_stat(path.read_text())
        p['pid'] = self.child.pid
        if (self.owner.owned.get(p['pid']) != (p['start_ticks'], self.child.pid) or
                p['session'] != self.child.pid or p['state'] == 'Z' or
                set(SAMPLER.descendants(self.child.pid)) != {self.child.pid}):
            raise ValueError('relay identity or descendants changed')
        after = SAMPLER.parse_stat(path.read_text())
        if after['start_ticks'] != p['start_ticks'] or after['session'] != p['session']:
            raise ValueError('relay PID changed while sampling')
        # PR_SET_DUMPABLE=0 protects the password. No ptrace bypass: smaps, fd
        # and live /proc/exe are unavailable; never substitute zero measurements.
        p.update(role='readonly_relay', measurement='stat_only_nondumpable')
        return p

    def close(self):
        try:
            if self.child is None:
                return
            if self.child.poll() is None:
                self.child.stdin.write('finish\n')
                self.child.stdin.flush()
                value = self.reply('finished')
                if (value.get('active_connections') != 0 or value.get('owned_fds_closed') is not True or
                        value.get('ui_child_reaped') is not True):
                    raise ValueError('relay did not confirm closure of owned resources')
                self.final = dict(event='finished', active_connections=0,
                                  owned_fds_closed=True, ui_child_reaped=True)
                if self.child.wait(timeout=5) != 0:
                    raise ValueError('relay did not exit cleanly')
            elif self.final is None:
                raise ValueError('relay ended without complete resource acknowledgement')
        finally:
            self.credential = None
            with DESKTOP.protected_cleanup():
                self.owner.close()


def validate_snapshot(value):
    keys = {'event', 'reads', 'denied_frames', 'upstream_connections_opened', 'authenticated_upstreams',
            'active_connections', 'front_auth_failures', 'events_forwarded', 'events_dropped', 'upstream_version'}
    if (not isinstance(value, dict) or set(value) != keys or value['event'] != 'snapshot' or
            not isinstance(value['reads'], dict) or
            set(value['reads']) != {'GetVersion', 'GetRecordStatus', 'GetStreamStatus'} or
            any(type(n) is not int or n < 0 for k,n in value.items() if k not in ('event', 'reads', 'upstream_version')) or
            any(type(n) is not int or n < 0 for n in value['reads'].values()) or
            not public_version(value['upstream_version']) or
            value['active_connections'] > 1 or value['denied_frames'] != 0 or value['front_auth_failures'] != 0):
        raise ValueError('relay did not confirm bounded read-only service activity')
    return value


def public_version(value):
    if value is None:
        return True
    if not isinstance(value, dict) or set(value) != {'obsVersion', 'obsWebSocketVersion'}:
        return False
    allowed = 'abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789._-'
    return all(v is None or (isinstance(v, str) and 1 <= len(v) <= 64 and all(c in allowed for c in v))
               for v in value.values())


def validate_window(before, after, visuals):
    if before['active_connections'] != int(visuals > 0) or after['active_connections'] != int(visuals > 0):
        raise ValueError('OBS connection lifecycle does not match demand at both window edges')
    for field in ('upstream_connections_opened', 'authenticated_upstreams'):
        if before[field] != after[field]:
            raise ValueError('OBS reconnected inside a fixed observation window')
    required = ('GetRecordStatus', 'GetStreamStatus') if visuals == 32 else ('GetRecordStatus',) if visuals else ()
    if any(after['reads'][name] <= before['reads'][name] for name in required):
        raise ValueError('OBS reads made no observed progress during the window')
    if not visuals and after['reads'] != before['reads']:
        raise ValueError('OBS reads continued without plugin demand')


class ObsWorld(SERVICES.ServicesWorld):
    def __init__(self, root, elf, hashes, relay):
        self.root, self.name, self.elf, self.hashes, self.relay = root, 'obs', elf, hashes, relay
        self.roots, self.owned, self.backends = [], {}, []
        self.runtime = self.runtime_pid = None
        self.rows, self.events, self.unobserved, self.observations = [], [], set(), []
        self.log, self.roles, self.bus_proxy = subprocess.DEVNULL, {}, None
        self.epoch, self.visual_count, self.configured = time.monotonic(), 0, False
        root.mkdir(mode=0o700)
        with DESKTOP.registered_spawn():
            result = subprocess.run([str(elf/'official_profile_seed'), str(root), 'obs', '--real-obs'],
                env={'PATH':'/usr/bin:/bin'}, stdin=subprocess.DEVNULL,
                stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, text=True, timeout=30, check=True)
        event = json.loads(result.stdout)
        if (event.get('executable') != 'target/plugins/release/rad-plugin-obs' or
                event.get('real_obs') is not True or event.get('plugin') != 'obs'):
            raise ValueError('fixed production OBS seed required')
        self.events.append(dict(event='seeded', production_obs=True, separate_process=True))
        inventory = root/'data/rad/plugins'
        self.events.append(dict(event='inventory_before_execution', sha256={
            str(p.relative_to(inventory)):SERVICES.file_digest(p) for p in sorted(inventory.rglob('*'))
            if p.is_file()}))
        self.env = SERVICES.METRICS.private_environment(root)
        machine = root/'data/rad/machine.json'
        machine.write_text(json.dumps(dict(version=1, use_stream_deck=False, start_with_session=False)))
        machine.chmod(0o600)

    def start_runtime(self):
        # Only filesystem/PID isolation: trusted fixed production plugin is pinned
        # to the checked guard port. This is not a network sandbox for hostile code.
        command = [v for v in SERVICES.namespace_command(self.root, self.elf) if v != '--unshare-net']
        started = time.time_ns()
        self.runtime = self.spawn(command, 'runtime_namespace')
        def ready():
            if self.runtime.poll() is not None:
                raise RuntimeError('private OBS runtime ended before readiness')
            p = self.root/'runtime/rad/editor.sock'
            return p.exists() and p.stat().st_ctime_ns >= started
        COMMON.until(ready, bool)
        self.runtime_pid = SERVICES.select_runtime(self.namespace_processes(False), self.runtime.pid)
        with ObsPeer(self) as peer:
            self.wait_host(peer, 'absent')
            self.check_disabled(peer)
            if self.configured:
                verify_settings(peer.call('plugin_settings', plugin_id='org.rad.obs'), self.relay.port)
        self.events.append(dict(event='runtime_started', pid=self.runtime_pid, host_sid=self.runtime.pid))

    def configure_buttons(self):
        with ObsPeer(self) as peer:
            folder = peer.call('status')['active_folder_id']
            for i in range(32):
                peer.call('create_button', folder_id=folder, position=i, name=f'Private {i}')
            peer.call('set_plugin_settings', plugin_id='org.rad.obs',
                      parameters=dict(schema_version=1, values=dict(port=self.relay.port)))
            verify_settings(peer.call('plugin_settings', plugin_id='org.rad.obs'), self.relay.port)
            peer.call('credential_replace', plugin_id='org.rad.obs', slot='password', secret=self.relay.credential)
            self.wait_host(peer, 'absent')
        self.configured = True

    def set_visuals(self, count):
        if count not in (0, 1, 32) or not self.configured:
            raise ValueError('only configured bounded OBS visual phases are allowed')
        def visual(i):
            return (dict(type='manual', parameters=None) if i >= count else
                    dict(type='org.rad.obs#'+('record', 'stream')[i%2],
                         parameters=dict(schema_version=1, values={})))
        with ObsPeer(self) as peer:
            verify_settings(peer.call('plugin_settings', plugin_id='org.rad.obs'), self.relay.port)
            peer.call('edit', changes=[dict(kind='set', id=f'b{i+1}', field='visual', value=visual(i))
                                       for i in range(32)])
            self.wait_host(peer, 'ready' if count else 'absent')
            self.check_disabled(peer)
        self.visual_count = count
        self.events.append(dict(event='transition', visuals=count))
        time.sleep(2)

    def capture(self, phase):
        row = super().capture(phase)
        row['relay_resource'] = self.relay.resource(self.elf)
        return row

    def observe_visuals(self, peer, phase, all_visuals=False):
        self.check_disabled(peer)
        verify_settings(peer.call('plugin_settings', plugin_id='org.rad.obs'), self.relay.port)
        count = self.visual_count if all_visuals else min(self.visual_count, 2)
        sources = [visual_observation(('record', 'stream')[i%2], peer.call('plugin_visual', id=f'b{i+1}'))
                   for i in range(count)]
        row = dict(phase=phase, elapsed_s=time.monotonic()-self.epoch, visuals=self.visual_count, sources=sources)
        self.observations.append(row)
        return row

    def window(self, phase, seconds, interval, raw):
        snapshot = self.relay.snapshot()
        if (self.visual_count == 0 and snapshot['active_connections'] != 0) or (
                self.visual_count > 0 and snapshot['active_connections'] != 1):
            raise ValueError('relay connection lifecycle disagrees with plugin demand')
        self.events.append(dict(event='phase_begin', phase=phase, visuals=self.visual_count, relay=snapshot))
        epoch, tick, next_visual = time.monotonic(), 0, 0.
        with ObsPeer(self) as peer:
            initial = self.observe_visuals(peer, phase, all_visuals=True)
            if any(not v['valid_current'] for v in initial['sources']):
                raise ValueError('real OBS visual availability is not verified')
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
        after = self.relay.snapshot()
        validate_window(snapshot, after, self.visual_count)
        self.events.append(dict(event='phase_end', phase=phase, relay=after))


def run_case(out, elf, hashes, relay, repetition, seconds, interval):
    world = ObsWorld.__new__(ObsWorld)
    primary = None
    with (out/f'obs{repetition}.samples.jsonl').open('w') as raw:
        try:
            world.__init__(out/f'obs{repetition}', elf, hashes, relay)
            world.start_runtime()
            world.events.append(dict(event='setup_step', step='configure_buttons'))
            world.configure_buttons()
            world.events.append(dict(event='setup_step', step='stop_preparation_runtime'))
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
                raise ValueError('unobserved descendants prevent complete evidence')
            summary = SERVICES.availability_summary(world.observations)
            if summary['status'] != 'observed_current':
                raise ValueError('OBS visual observations did not remain current')
            world.events.append(dict(event='complete', clean_shutdown=True, all_owned_reaped=True))
            return summarize(world, repetition), dict(repetition=repetition, **summary)
        except BaseException as error:
            primary = error
            if hasattr(world, 'events'):
                world.events.append(dict(event='case_error', error_type=type(error).__name__,
                                         error_errno=error_errno(error),
                                         error_frames=error_frames(error)))
            raise
        finally:
            if hasattr(world, 'roots'):
                with DESKTOP.protected_cleanup():
                    try:
                        world.close()
                    except BaseException as error:
                        world.events = [e for e in world.events if e.get('event') != 'complete']
                        world.events.append(dict(event='cleanup_failed', error_type=type(error).__name__,
                                                 error_errno=error_errno(error),
                                                 error_frames=error_frames(error)))
                        if primary is None:
                            raise
                    finally:
                        (out/f'obs{repetition}.events.jsonl').write_text(
                            ''.join(json.dumps(e)+'\n' for e in world.events))


def summarize(world, repetition):
    # Do not let the generic summarizer turn protected relay metrics into zeros.
    result = [r for r in SERVICES.summarize(world, repetition) if r['group'] != 'readonly_proxy']
    for phase in PHASES:
        rows = [r for r in world.rows if r['phase'] == phase]
        if len(rows) < 2 or rows[-1]['elapsed_s'] <= rows[0]['elapsed_s']:
            raise ValueError('insufficient relay observations')
        values = [r['relay_resource'] for r in rows]
        ids = {(v['pid'], v['start_ticks'], v['session']) for v in values}
        if len(ids) != 1 or any(b['ticks'] < a['ticks'] for a,b in zip(values, values[1:])):
            raise ValueError('relay identity or CPU counter changed within window')
        duration = rows[-1]['elapsed_s']-rows[0]['elapsed_s']
        result.append(dict(phase=phase, duration_s=duration, samples=len(rows), repetition=repetition,
            plugin='obs', group='readonly_relay', measurement='stat_only_nondumpable',
            cpu_percent_one_core=(values[-1]['ticks']-values[0]['ticks'])/SAMPLER.CLK_TCK/duration*100,
            rss_bytes_median=statistics.median(v['rss_bytes'] for v in values),
            rss_bytes_sampled_peak=max(v['rss_bytes'] for v in values),
            threads_median=statistics.median(v['threads'] for v in values),
            threads_sampled_peak=max(v['threads'] for v in values), processes_sampled_peak=1,
            stable_process_set=True, pss_bytes_median=None, pss_bytes_sampled_peak=None,
            private_bytes_median=None, private_bytes_sampled_peak=None,
            fds_median=None, fds_sampled_peak=None, observed_context_switches=None))
    return result


def validate_gate(out, elf):
    root = out/'validation'
    root.mkdir(mode=0o700)
    with DESKTOP.registered_spawn():
        result = subprocess.run([str(elf/'obs-readonly-relay'), '--validate', str(root)],
            env={'PATH':'/usr/bin:/bin', 'LANG':'C.UTF-8'}, stdin=subprocess.DEVNULL,
            stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, text=True, timeout=45, check=True)
    try:
        report = json.loads(result.stdout)
    except ValueError:
        raise ValueError('synthetic relay gate returned invalid report') from None
    validate_gate_report(report)
    (out/'filter-validation.json').write_text(json.dumps(report, indent=2)+'\n')
    return report


def validate_gate_report(value):
    if (not isinstance(value, dict) or set(value) != GATE_FIELDS | {'event'} or
            value['event'] != 'validation' or any(value[k] is not True for k in GATE_FIELDS)):
        raise ValueError('synthetic relay gate did not verify the full read-only boundary')
    return value


def execute(out, elf, metadata, args):
    validate_gate(out, elf)
    if args.validate_only:
        return dict(gate_only=True)
    relay = RelayProcess(out, elf, metadata['executable_sha256'], args.run_obs)
    resources, availability = [], []
    primary = cleanup = None
    try:
        for repetition in range(args.repetitions):
            rows, current = run_case(out, elf, metadata['executable_sha256'], relay,
                                     repetition, args.seconds, args.interval)
            resources.extend(rows)
            availability.append(current)
            (out/'resources.json').write_text(json.dumps(resources, indent=2)+'\n')
            (out/'availability.json').write_text(json.dumps(availability, indent=2)+'\n')
        snapshot = relay.snapshot()
        if snapshot['active_connections'] != 0:
            raise ValueError('read-only OBS connection survived last demand')
    except BaseException as error:
        primary = error
        metadata.update(observation_error_type=type(error).__name__,
                        observation_error_errno=error_errno(error),
                        observation_error_frames=error_frames(error))
    finally:
        try:
            with DESKTOP.protected_cleanup():
                relay.close()
        except BaseException as error:
            cleanup = error
            metadata.update(relay_cleanup_error_type=type(error).__name__,
                            relay_cleanup_error_errno=error_errno(error),
                            relay_cleanup_error_frames=error_frames(error))
        finally:
            metadata.update(relay_cleanup=relay.final, relay_reaped=relay.child.poll() is not None,
                            all_owned_reaped=primary is None and cleanup is None)
    if primary is not None:
        raise primary
    if cleanup is not None:
        raise cleanup
    return dict(relay=snapshot, relay_cleanup=relay.final, all_owned_reaped=True)


def error_frames(error):
    """Only code location: never exception text, source lines, chains or locals."""
    rows, tb = [], error.__traceback__
    while tb is not None and len(rows) < 64:
        path = Path(tb.tb_frame.f_code.co_filename)
        try:
            name = str(path.relative_to(REPO))
        except ValueError:
            name = path.name
        rows.append(dict(file=name, function=tb.tb_frame.f_code.co_name, line=tb.tb_lineno))
        tb = tb.tb_next
    return rows


def error_errno(error):
    value = getattr(error, 'errno', None)
    return value if type(value) is int else None


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('output', type=Path)
    mode = parser.add_mutually_exclusive_group()
    mode.add_argument('--run-obs', action='store_true')
    mode.add_argument('--validate-only', action='store_true')
    mode.add_argument('--synthetic-only', action='store_true', help='owned private oracle only; no GUI or real OBS')
    parser.add_argument('--port', type=int, choices=(4455,), default=4455)
    parser.add_argument('--seconds', type=int, default=5)
    parser.add_argument('--repetitions', type=int, default=3)
    parser.add_argument('--interval', type=float, default=.2)
    args = parser.parse_args(argv)
    if not args.run_obs and not args.validate_only and not args.synthetic_only:
        print('DRY: no OBS, credential, GUI, process or endpoint inspection/launch')
        return 0
    try:
        out = COMMON.validate_output(args.output, args.seconds, args.repetitions, args.interval)
    except (ValueError, OSError):
        parser.error('new bounded private output under target/performance required')
    out.mkdir(mode=0o700)
    metadata = dict(schema_version=1, complete=False, real_obs=args.run_obs,
        synthetic_only=args.synthetic_only, validation_only=args.validate_only,
        server_port=4455 if args.run_obs else None, repetitions=args.repetitions,
        seconds=args.seconds, interval_s=args.interval, no_actions=True,
        limits=['trusted fixed production plugin through a checked local guard, not a hostile-code network sandbox',
                'only GetVersion/GetRecordStatus/GetStreamStatus; no writes, batches, options or actions',
                'actual password stays in relay memory, never product/controller/files/argv/environment',
                'product authenticates to ephemeral relay token; upstream auth belongs to relay, not direct product-to-OBS',
                'upstream Outputs subscription and filtered events/status fields, not original unfiltered traffic',
                'relay and namespace supervisor costs separate; personal OBS process resources not measured',
                'non-dumpable relay: only stat RSS/CPU/threads observed; PSS/private/FD/context unavailable, never zero',
                'relay live /proc/exe protected; retained preexec hash and PID/start/SID bind trusted executable, not ptrace attestation',
                'one visual is recording;32 bindings alternate recording/streaming;32 checked initially then2 per second',
                'sampled availability/resources are not continuous freshness or absolute peaks',
                'context switches are not exact wakeups; no resource budget adopted or changed'])
    previous = {s:signal.getsignal(s) for s in (signal.SIGINT, signal.SIGTERM)}
    DESKTOP.install_shutdown_signals()
    try:
        metadata.update(provenance(), base_commit=subprocess.check_output(
            ['git', 'rev-parse', 'HEAD'], cwd=REPO, text=True).strip(), kernel=os.uname().release,
            architecture=os.uname().machine)
        elf = retain(out, metadata)
        write_metadata(out, metadata)  # Before gate, prompt or personal endpoint.
        metadata['stage'] = 'synthetic_gate_then_observation'
        metadata.update(execute(out, elf, metadata, args))
        if provenance() != {k:metadata[k] for k in ('source_sha256', 'executable_sha256')}:
            raise ValueError('sources or executable aliases changed during observation')
        if any(SERVICES.file_digest(elf/n) != h for n,h in metadata['executable_sha256'].items()):
            raise ValueError('retained executable changed during observation')
        metadata.update(complete=True, stage='complete', fingerprints_match=True)
        print('COMPLETE '+str(out.relative_to(REPO)), flush=True)
        return 0
    except BaseException as error:
        metadata.update(complete=False, error_type=type(error).__name__, error_errno=error_errno(error),
                        error_frames=error_frames(error))
        print('FAILED: '+type(error).__name__+'; no raw OBS or credential diagnostics retained', flush=True)
        return 1
    finally:
        with DESKTOP.protected_cleanup():
            write_metadata(out, metadata)
        for s, handler in previous.items():
            signal.signal(s, handler)


def write_metadata(out, metadata):
    pending = out/'metadata.pending'
    with pending.open('w') as handle:
        json.dump(metadata, handle, indent=2)
        handle.write('\n')
        handle.flush()
        os.fsync(handle.fileno())
    pending.replace(out/'metadata.json')


if __name__ == '__main__':
    raise SystemExit(main())
