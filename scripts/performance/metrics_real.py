#!/usr/bin/env python3
"""Opt-in production procfs/NVML measurements; no GUI, deck or plugin actions.

Without --run-real-metrics this command does not inspect or launch anything.
Only an authorized operator may opt in; GPU index 0 is read by the product reader.
"""
import argparse
import hashlib
import importlib.util
import json
from pathlib import Path
import re
import shutil
import signal
import subprocess
import time

REPO = Path(__file__).resolve().parents[2]
SPEC = importlib.util.spec_from_file_location('metrics_real_desktop_lifecycle', Path(__file__).with_name('desktop.py'))
DESKTOP = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(DESKTOP)
COMMON, SAMPLER = DESKTOP.COMMON, DESKTOP.SAMPLER
SOURCES = ('cpu', 'ram', 'gpu', 'vram')
PHASES = ('installed', 'one', 'many', 'released', 'fresh')
TARGET = SAMPLER.METRICS_RSS_TARGET_BYTES
BINARIES = {
    'runtime': 'target/release/rad-runtime',
    'launcher_helper': 'target/release/rad-launcher-helper',
    'client': 'target/release/examples/conformance_client',
    'seed': 'target/release/examples/official_profile_seed',
    'metrics_plugin': 'target/plugins/release/rad-plugin-metrics',
}


def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def private_environment(root):
    env = dict(PATH='/usr/bin:/bin', LANG='C.UTF-8')
    for key, directory in (
            ('HOME', 'home'), ('XDG_RUNTIME_DIR', 'runtime'), ('XDG_DATA_HOME', 'data'),
            ('XDG_CONFIG_HOME', 'config'), ('XDG_CACHE_HOME', 'cache'), ('XDG_STATE_HOME', 'state'),
            ('XDG_DATA_DIRS', 'system'), ('XDG_CONFIG_DIRS', 'system')):
        (root/directory).mkdir(mode=0o700, exist_ok=True)
        env[key] = str(root/directory)
    env['DBUS_SESSION_BUS_ADDRESS'] = 'unix:path='+str(root/'no-personal-bus')
    return env


def seed_command(root):
    return [str(REPO/BINARIES['seed']), str(root), 'metrics', '--real-metrics']


def visual_observation(source, value):
    """Retain only fixed numeric metric formats, never arbitrary plugin strings/IDs."""
    text = value.get('text')
    valid_text = False
    if isinstance(text, str):
        if source in ('cpu', 'gpu'):
            match = re.fullmatch(source.upper()+r' ([0-9]{1,3})%', text)
            valid_text = bool(match and int(match[1]) <= 100)
        elif source == 'ram':
            match = re.fullmatch(r'RAM ([0-9]{1,11}\.[0-9]) GiB used ([0-9]{1,11}\.[0-9]) GiB avail', text)
            valid_text = bool(match and float(match[1])+float(match[2]) > 0)
        elif source == 'vram':
            match = re.fullmatch(r'VRAM ([0-9]{1,11}\.[0-9])/([0-9]{1,11}\.[0-9]) GiB', text)
            valid_text = bool(match and 0 <= float(match[1]) <= float(match[2]) and float(match[2]) > 0)
    state = value.get('availability')
    if state not in ('current', 'stale', 'unavailable'):
        raise ValueError('unexpected visual availability')
    return dict(source=source, availability=state, text=text if valid_text else None,
                valid_current=state == 'current' and valid_text and not value.get('fault', False))


def attribute_processes(values, executables, active):
    """Attribute by verified ELF roots and parent closure, not comm-name guesses."""
    roots = {p['pid'] for p in values if executables[p['pid']] == 'metrics_plugin'}
    if len(roots) != int(active):
        raise ValueError('unexpected production metrics process count for demand')
    plugin_tree = set(roots)
    while True:
        more = {p['pid'] for p in values if p['ppid'] in plugin_tree}
        if more <= plugin_tree:
            break
        plugin_tree.update(more)
    result = []
    for process in values:
        role = executables[process['pid']]
        if process['pid'] in plugin_tree:
            role = 'metrics_plugin' if process['pid'] in roots else 'metrics_descendant'
        elif role not in ('runtime', 'launcher_helper'):
            raise ValueError('unattributed process outside the observed metrics tree')
        result.append(dict(process, role=role))
    return result


class MetricsWorld(COMMON.World):
    """Owned lifecycle only; never initialize historical fake or personal backends."""
    def spawn(self, *args, **kwargs):
        with DESKTOP.registered_spawn():
            return super().spawn(*args, **kwargs)

    def __init__(self, root, log, hashes):
        self.root, self.name, self.log = root, 'metrics', log
        self.roots, self.owned, self.backends = [], {}, []
        self.runtime = self.runtime_pid = None
        self.rows, self.events, self.unobserved = [], [], set()
        self.epoch, self.visual_count = time.monotonic(), 0
        self.executable_roles, self.hashes = {}, hashes
        self.observations = []
        root.mkdir(mode=0o700)
        # Subprocess.run kills/reaps on timeout; defer cancellation until it returns.
        with DESKTOP.registered_spawn():
            result = subprocess.run(seed_command(root), env={'PATH':'/usr/bin:/bin'},
                stdin=subprocess.DEVNULL, stdout=subprocess.PIPE, stderr=log,
                text=True, timeout=30, check=True)
        event = json.loads(result.stdout)
        if (event.get('executable') != BINARIES['metrics_plugin'] or
                event.get('real_metrics') is not True or event.get('plugin') != 'metrics'):
            raise ValueError('seed did not select the fixed production metrics executable')
        self.events.append(event)
        machine = root/'data/rad/machine.json'
        machine.write_text(json.dumps(dict(version=1, use_stream_deck=False, start_with_session=False)))
        machine.chmod(0o600)
        self.env = private_environment(root)

    def set_visuals(self, count):
        if count not in (0, 1, 32):
            raise ValueError('only bounded 0/1/32 visual phases are allowed')
        epoch = time.monotonic_ns()
        with COMMON.Bridge(self) as peer:
            peer.call('edit', changes=[dict(kind='set', id=f'b{i+1}', field='visual',
                value=COMMON.visual_for('metrics', i, i < count)) for i in range(32)])
            self.wait_host(peer, 'ready' if count else 'absent')
            if peer.call('status')['deck_state'] != 'disabled':
                raise ValueError('hardware became enabled during metrics measurement')
        self.visual_count = count
        self.events.append(dict(event='transition', visuals=count, ns=time.monotonic_ns()-epoch,
            includes='typed client connect, edit and host readiness; not metric readiness'))
        # CPU requires two samples; missing GPU is legitimate evidence, not a timeout.
        time.sleep(2)

    def executable_role(self, process):
        identity = (process['pid'], process['start_ticks'])
        if identity not in self.executable_roles:
            exe = Path('/proc')/str(process['pid'])/'exe'
            path = exe.resolve(strict=True)
            if process['pid'] == self.runtime_pid:
                role = 'runtime'
            elif path == (REPO/BINARIES['launcher_helper']).resolve():
                role = 'launcher_helper'
            elif path.is_relative_to(self.root/'data/rad/plugins'):
                role = 'metrics_plugin'
            else:
                role = 'other'
            if role != 'other' and digest(exe) != self.hashes[role]:
                raise ValueError('owned executable differs from the frozen measurement ELF')
            if not SAMPLER.same_owned_process(process['pid'], process['start_ticks'], self.runtime.pid):
                raise ValueError('owned executable identity changed during attribution')
            self.executable_roles[identity] = role
        return self.executable_roles[identity]

    def capture(self, phase):
        if self.runtime is None or self.runtime.poll() is not None:
            raise RuntimeError('private runtime ended during metrics resource window')
        values, roles = [], {}
        for pid in SAMPLER.descendants(self.runtime.pid):
            try:
                process = SAMPLER.read_process(pid, self.runtime.pid)
                identity = (process['start_ticks'], self.runtime.pid)
                if pid in self.owned and self.owned[pid] != identity:
                    raise ValueError('owned PID identity changed during measurement')
                self.owned[pid] = identity
                roles[pid] = self.executable_role(process)
                values.append(process)
            except FileNotFoundError:
                continue
        # Incomplete/session-escaped trees must not produce a passing budget.
        values = attribute_processes(values, roles, active=self.visual_count > 0)
        if sum(p['role'] == 'runtime' for p in values) != 1:
            raise ValueError('runtime missing from its own process tree')
        row = dict(elapsed_s=time.monotonic()-self.epoch, phase=phase, processes=values)
        self.rows.append(row)
        return row

    def observe_visuals(self, peer, phase, all_visuals=False):
        if peer.call('status')['deck_state'] != 'disabled':
            raise ValueError('refusing hardware-enabled metrics measurement')
        machine = peer.call('machine_settings')
        if machine['use_stream_deck'] or machine['start_with_session']:
            raise ValueError('private machine settings changed during metrics measurement')
        count = self.visual_count if all_visuals else min(self.visual_count, 4)
        sources = [visual_observation(SOURCES[i % 4], peer.call('plugin_visual', id=f'b{i+1}'))
                   for i in range(count)]
        observation = dict(phase=phase, elapsed_s=time.monotonic()-self.epoch,
                           visuals=self.visual_count, sources=sources)
        self.observations.append(observation)
        return observation

    def window(self, phase, seconds, interval, raw):
        self.events.append(dict(event='phase_begin', phase=phase, visuals=self.visual_count))
        epoch, tick, next_visual = time.monotonic(), 0, 0.
        with COMMON.Bridge(self) as peer:
            # Record all configured visual bindings once; subsequent samples use
            # one representative per shared source to avoid a polling-heavy load.
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


def summarize(world, repetition):
    result = []
    for phase in PHASES:
        for group, roles in (
                ('metrics_plugin_tree', {'metrics_plugin', 'metrics_descendant'}),
                ('metrics_plugin_only', {'metrics_plugin'}),
                ('runtime_tree', {'runtime', 'launcher_helper', 'metrics_plugin', 'metrics_descendant'})):
            selected = [dict(row, processes=[p for p in row['processes'] if p['role'] in roles])
                        for row in world.rows]
            result.append(dict(SAMPLER.summarize_phase(selected, phase),
                               repetition=repetition, group=group))
    return result


def run_case(out, repetition, seconds, interval, hashes):
    world = MetricsWorld.__new__(MetricsWorld)
    with (out/f'm{repetition}.stderr.log').open('w') as log, \
         (out/f'm{repetition}.samples.jsonl').open('w') as raw:
        try:
            world.__init__(out/f'm{repetition}', log, hashes)
            world.start_runtime()
            world.configure_buttons()
            world.stop_runtime()  # Setup/editing history is outside the measured PID.
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
                raise ValueError('unobserved descendants prevent complete metrics evidence')
            world.events.append(dict(event='complete', clean_shutdown=True, all_owned_reaped=True))
            observations = [dict(o, repetition=repetition) for o in world.observations
                            if len(o['sources']) <= 4]
            return summarize(world, repetition), observations
        finally:
            if hasattr(world, 'roots'):
                with DESKTOP.protected_cleanup():
                    try:
                        world.close()
                    finally:
                        (out/f'm{repetition}.events.jsonl').write_text(
                            ''.join(json.dumps(event)+'\n' for event in world.events))


def validate_output(output, seconds, repetitions, interval):
    return COMMON.validate_output(output, seconds, repetitions, interval)


def provenance():
    sources = ['scripts/performance/metrics_real.py', 'scripts/performance/desktop.py',
               'scripts/performance/physical.py', 'scripts/performance/officials.py',
               'scripts/performance/profile.py', 'crates/rad-runtime/examples/official_profile_seed.rs',
               'plugins/metrics/manifest.json']
    sources += [str(p.relative_to(REPO)) for p in sorted((REPO/'plugins/metrics/src').glob('*.rs'))]
    return dict(source_sha256={p:digest(REPO/p) for p in sources},
                executable_sha256={name:digest(REPO/path) for name, path in BINARIES.items()})


def retain_executables(out, hashes):
    retained = out/'elf'
    retained.mkdir(mode=0o700)
    for name, source in BINARIES.items():
        shutil.copy2(REPO/source, retained/name)
        if digest(retained/name) != hashes[name]:
            raise ValueError('ELF changed during retention before measurement')
    return {name:str((retained/name).relative_to(out)) for name in BINARIES}


def evaluate_target(summaries, observations, repetitions):
    active = [s for s in summaries if s['group'] == 'metrics_plugin_tree'
              and s['phase'] in ('one', 'many')]
    gaps, gpu_windows = [], []
    if not 1 <= repetitions <= 3:
        gaps.append('no valid bounded repetition count')
    for repeat in range(repetitions):
        selected = [s for s in active if s['repetition'] == repeat]
        if (sorted(s['phase'] for s in selected) != ['many', 'one'] or
                any(not s['stable_process_set'] or s['processes_sampled_peak'] < 1 for s in selected)):
            gaps.append(f'repetition {repeat}: incomplete or unstable active plugin tree')
        readings = [o for o in observations if o['repetition'] == repeat and o['phase'] == 'many']
        valid = (len(readings) >= 3 and
                 readings[-1]['elapsed_s']-readings[0]['elapsed_s'] > 1.5 and
                 all(len(o['sources']) == 4 and {v['source'] for v in o['sources']} == set(SOURCES)
                     and all(v['availability'] == 'current' and v['valid_current'] for v in o['sources'])
                     for o in readings))
        gpu_windows.append(dict(repetition=repeat, samples=len(readings), all_sources_current_valid=valid))
        if not valid:
            gaps.append(f'repetition {repeat}: CPU/RAM/GPU/VRAM freshness or availability not verified')
    within = all(s['rss_bytes_sampled_peak'] <= TARGET for s in active) if not gaps else None
    return dict(kind='observed_production_gpu_scenario', status='not_verified' if gaps else
                'within_observed_target' if within else 'exceeds_observed_target',
                target_bytes=TARGET, metric='plugin_tree_rss_bytes_sampled_peak',
                observed_gpu_scenario_target_met=within, product_target_certified=False,
                is_absolute_peak=False, universal_certification=False, evidence_gaps=gaps,
                gpu_windows=gpu_windows,
                measured_peaks=[dict(repetition=s['repetition'], phase=s['phase'],
                    rss_bytes=s['rss_bytes_sampled_peak'], pss_bytes=s['pss_bytes_sampled_peak']) for s in active],
                scope='this run only; production plugin and observed descendants, GPU index 0; not all GPUs/drivers',
                limits=['sampled peaks are not absolute maxima', 'availability is sampled, not continuous',
                        'RSS tree sums can double-count shared pages; PSS is reported alongside'])


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('output', type=Path)
    parser.add_argument('--run-real-metrics', action='store_true',
                        help='explicit authorization to use production procfs/NVML readers')
    parser.add_argument('--seconds', type=int, default=5)
    parser.add_argument('--repetitions', type=int, default=3)
    parser.add_argument('--interval', type=float, default=.2)
    args = parser.parse_args(argv)
    if not args.run_real_metrics:
        print('DRY: no procfs/NVML, service, device or application inspection/launch')
        return
    try:
        out = validate_output(args.output, args.seconds, args.repetitions, args.interval)
    except ValueError as error:
        parser.error(str(error))
    metadata = provenance()
    metadata.update(schema_version=1, seconds=args.seconds, repetitions=args.repetitions,
                    interval_s=args.interval, complete=False, hardware_enabled=False,
                    readers='production /proc/stat, /proc/meminfo and NVML GPU utilization/VRAM at index 0',
                    limits=['no GUI, Stream Deck, plugin actions or personal services',
                            'no GPU identity, hostname, temperature, power or other sensor probes',
                            'controller/typed IPC clients excluded from runtime/plugin resource trees',
                            'resource samples and API reads are not atomic',
                            'CPU ticks are one-core percent; zero may be below clock-tick resolution',
                            'context switches are NOT exact wakeups',
                            'fresh is a new PID after withdrawal, not a leak proof',
                            'NVML result applies only to index 0 in this run; no enumeration or universality'])
    out.mkdir(mode=0o700)
    metadata['retained_executables'] = retain_executables(out, metadata['executable_sha256'])
    resources, observations = [], []
    target = dict(status='not_verified', observed_gpu_scenario_target_met=None,
                  product_target_certified=False, evidence_gaps=['measurement incomplete'])
    previous_handlers = {sig:signal.getsignal(sig) for sig in (signal.SIGTERM, signal.SIGINT)}
    DESKTOP.install_shutdown_signals()
    try:
        (out/'metadata.json').write_text(json.dumps(metadata, indent=2)+'\n')
        (out/'metrics-target.json').write_text(json.dumps(target, indent=2)+'\n')
        for repeat in range(args.repetitions):
            print(f'Measuring production metrics repetition {repeat}', flush=True)
            summary, readings = run_case(out, repeat, args.seconds, args.interval,
                                        metadata['executable_sha256'])
            resources.extend(summary)
            observations.extend(readings)
            (out/'resources.json').write_text(json.dumps(resources, indent=2)+'\n')
            (out/'availability.json').write_text(json.dumps(observations, indent=2)+'\n')
        if provenance() != {key:metadata[key] for key in ('source_sha256', 'executable_sha256')}:
            raise RuntimeError('measured sources or ELF changed during the run')
        target = evaluate_target(resources, observations, args.repetitions)
        metadata.update(complete=True, all_owned_reaped=True, source_fingerprints_match=True)
        print('COMPLETE '+str(out.relative_to(REPO)), flush=True)
    except BaseException as error:
        target = dict(status='not_verified', observed_gpu_scenario_target_met=None,
                      product_target_certified=False, evidence_gaps=['measurement incomplete'])
        metadata['complete'] = False
        metadata['error_type'] = type(error).__name__
        raise
    finally:
        with DESKTOP.protected_cleanup():
            (out/'metadata.json').write_text(json.dumps(metadata, indent=2)+'\n')
            (out/'metrics-target.json').write_text(json.dumps(target, indent=2)+'\n')
        for sig, handler in previous_handlers.items():
            signal.signal(sig, handler)


if __name__ == '__main__':
    main()
