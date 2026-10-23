#!/usr/bin/env python3
"""Observe only newly spawned RAD fixtures. Context switches are NOT wakeups."""
import argparse
import hashlib
import json
import math
import os
from pathlib import Path
import platform
import select
import signal
import statistics
import subprocess
import time

PAGE_SIZE = os.sysconf('SC_PAGE_SIZE')
CLK_TCK = os.sysconf('SC_CLK_TCK')
REPO = Path(__file__).resolve().parents[2]


def parse_stat(text):
    fields = text.rsplit(')', 1)[1].split()
    return dict(state=fields[0], ppid=int(fields[1]), session=int(fields[3]),
                ticks=int(fields[11])+int(fields[12]),
                threads=int(fields[17]), start_ticks=int(fields[19]),
                rss_bytes=max(0, int(fields[21]))*PAGE_SIZE)


def read_process(pid, root_pid):
    root = Path('/proc') / str(pid)
    stat = parse_stat((root/'stat').read_text())
    if stat['session'] != root_pid:
        raise ProcessLookupError('not in the newly created fixture session')
    memory = {}
    for line in (root/'smaps_rollup').read_text().splitlines():
        key, _, value = line.partition(':')
        if key in {'Rss', 'Pss', 'Private_Clean', 'Private_Dirty', 'Private_Hugetlb'}:
            memory[key] = int(value.split()[0])*1024
    contexts = {}
    for task in (root/'task').iterdir():
        try:
            identity = parse_stat((task/'stat').read_text())['start_ticks']
            counts = dict(line.split(':', 1) for line in (task/'status').read_text().splitlines())
            contexts[f'{task.name}:{identity}'] = [int(counts[k]) for k in
                ('voluntary_ctxt_switches', 'nonvoluntary_ctxt_switches')]
        except FileNotFoundError:
            continue
    name = (root/'comm').read_text().strip()
    stat.update(pid=pid, role='runtime_fixture' if pid == root_pid else
                'launcher_helper' if name.startswith('rad-launcher') else 'synthetic_plugin',
                rss_bytes=memory['Rss'], pss_bytes=memory['Pss'],
                private_bytes=sum(memory.get(k, 0) for k in
                    ('Private_Clean', 'Private_Dirty', 'Private_Hugetlb')),
                contexts=contexts, fds=len(list((root/'fd').iterdir())))
    if parse_stat((root/'stat').read_text())['start_ticks'] != stat['start_ticks']:
        raise ValueError('PID reused while sampling owned process')
    return stat


def descendants(pid):
    """Traverse children of our child, including children launched by any thread."""
    found, pending = set(), [pid]
    while pending:
        current = pending.pop()
        if current in found:
            continue
        found.add(current)
        try:
            stat = parse_stat((Path('/proc')/str(current)/'stat').read_text())
            if stat['session'] != pid:
                continue
            tasks = list((Path('/proc')/str(current)/'task').iterdir())
        except FileNotFoundError:
            continue
        for task in tasks:
            try:
                pending.extend(map(int, (task/'children').read_text().split()))
            except FileNotFoundError:
                continue
    return sorted(found)


def percentile(values, p):
    ordered = sorted(values)
    return ordered[max(0, math.ceil(len(ordered)*p/100)-1)]


def summarize_timings(events, keys):
    groups = {}
    for event in events:
        if 'ns' in event and 'operation' in event:
            key = tuple(event[k] for k in keys)
            groups.setdefault(key, []).append(event['ns'])
    return [dict(zip(keys, key), n=len(values), p50_ns=percentile(values, 50),
                 p95_ns=percentile(values, 95), p99_ns=percentile(values, 99), max_ns=max(values))
            for key, values in sorted(groups.items())]


def summarize_phase(rows, phase):
    rows = [r for r in rows if r['phase'] == phase]
    if len(rows) < 2:
        raise ValueError(f'insufficient samples for {phase}')
    duration = rows[-1]['elapsed_s']-rows[0]['elapsed_s']
    if duration <= 0:
        raise ValueError('nonpositive sampling duration')
    identities, prior, ticks, switches = {}, {}, 0, 0
    memberships = []
    for row in rows:
        members = set()
        for process in row['processes']:
            pid, start = process['pid'], process['start_ticks']
            if pid in identities and identities[pid] != start:
                raise ValueError('PID reused within measurement phase')
            identities[pid] = start
            members.add((pid, start))
            if pid in prior:
                previous = prior[pid]
                ticks += max(0, process['ticks']-previous['ticks'])
                for tid, counts in process['contexts'].items():
                    if tid in previous['contexts']:
                        switches += sum(max(0, b-a) for a, b in zip(previous['contexts'][tid], counts))
            prior[pid] = process
        memberships.append(members)
    result = dict(phase=phase, duration_s=duration, samples=len(rows),
                  cpu_percent_one_core=ticks/CLK_TCK/duration*100,
                  observed_context_switches=switches,
                  observed_context_switches_per_s=switches/duration,
                  stable_process_set=all(m == memberships[0] for m in memberships))
    for field in ('rss_bytes', 'pss_bytes', 'private_bytes', 'threads', 'fds'):
        totals = [sum(p[field] for p in r['processes']) for r in rows]
        result[field+'_median'] = statistics.median(totals)
        result[field+'_sampled_peak'] = max(totals)
    result['processes_sampled_peak'] = max(len(r['processes']) for r in rows)
    return result


def clean_environment(directory):
    env = {'PATH':'/usr/bin:/bin', 'LANG':'C.UTF-8'}
    for key in ('HOME', 'XDG_RUNTIME_DIR', 'XDG_DATA_HOME', 'XDG_CONFIG_HOME',
                'XDG_CACHE_HOME', 'XDG_STATE_HOME', 'XDG_DATA_DIRS', 'XDG_CONFIG_DIRS'):
        path = directory/key.lower()
        path.mkdir(mode=0o700, parents=True)
        env[key] = str(path)
    env['DBUS_SESSION_BUS_ADDRESS'] = 'unix:path='+str(directory/'no-bus')
    return env


def same_owned_process(pid, start, session):
    try:
        stat = parse_stat((Path('/proc')/str(pid)/'stat').read_text())
        return stat['start_ticks'] == start and stat['session'] == session and stat['state'] != 'Z'
    except FileNotFoundError:
        return False


def terminate_owned(pid, start, session):
    # A pidfd keeps the signal tied to this process even if its PID is recycled
    # after the identity check. Never fall back to signalling a bare descendant PID.
    try:
        fd = os.pidfd_open(pid)
    except ProcessLookupError:
        return
    try:
        if same_owned_process(pid, start, session):
            signal.pidfd_send_signal(fd, signal.SIGKILL)
    except ProcessLookupError:
        pass
    finally:
        os.close(fd)


def observe(command, directory, interval, timeout, *, event_callback=None, sample_callback=None):
    """Capture JSONL and samples; clean only identities observed as descendants."""
    directory.mkdir(mode=0o700)
    env = clean_environment(directory/'env')
    rows, events, owned = [], [], {}
    phase, buffered = 'setup', b''
    epoch = time.monotonic()
    next_sample = epoch
    with (directory/'stderr.log').open('wb') as err, \
         (directory/'samples.jsonl').open('w') as raw, \
         (directory/'events.jsonl').open('w') as output:
        process = subprocess.Popen(command, cwd=REPO, env=env, stdin=subprocess.DEVNULL,
                                   stdout=subprocess.PIPE, stderr=err, start_new_session=True)
        try:
            eof = False
            while not eof:
                now = time.monotonic()
                if now-epoch > timeout:
                    raise TimeoutError(f'owned fixture timed out: {directory.name}')
                if now >= next_sample:
                    snapshot = []
                    for pid in descendants(process.pid):
                        try:
                            state = read_process(pid, process.pid)
                            owned[pid] = state['start_ticks']
                            snapshot.append(state)
                        except (FileNotFoundError, ProcessLookupError):
                            continue
                    if snapshot:
                        row = dict(elapsed_s=now-epoch, phase=phase, processes=snapshot)
                        rows.append(row)
                        raw.write(json.dumps(row)+'\n')
                        if sample_callback is not None:
                            sample_callback(row)
                    next_sample = time.monotonic()+interval
                readable, _, _ = select.select([process.stdout], [], [],
                    max(0, min(next_sample-time.monotonic(), .2)))
                if readable:
                    chunk = os.read(process.stdout.fileno(), 65536)
                    eof = not chunk
                    buffered += chunk
                    while b'\n' in buffered:
                        line, buffered = buffered.split(b'\n', 1)
                        event = json.loads(line)
                        events.append(event)
                        output.write(json.dumps(event)+'\n')
                        if event_callback is not None:
                            event_callback(event)
                        if event.get('event') == 'phase_begin':
                            phase = event['phase']
                            next_sample = time.monotonic()
                        elif event.get('event') == 'phase_end':
                            phase = 'between'
            if buffered:
                raise ValueError('incomplete fixture JSONL output')
            if process.wait(timeout=5) != 0:
                raise RuntimeError(f"fixture failed; see {directory/'stderr.log'}")
        finally:
            if process.poll() is None:
                for pid in descendants(process.pid):
                    try:
                        owned[pid] = parse_stat((Path('/proc')/str(pid)/'stat').read_text())['start_ticks']
                    except FileNotFoundError:
                        pass
                process.terminate()
                try:
                    process.wait(timeout=5)
                except subprocess.TimeoutExpired:
                    process.kill()
                    process.wait(timeout=5)
            process.stdout.close()
            deadline = time.monotonic()+3
            while any(same_owned_process(p, s, process.pid) for p, s in owned.items()) and time.monotonic() < deadline:
                time.sleep(.05)
            survivors = [p for p, s in owned.items() if same_owned_process(p, s, process.pid)]
            for pid in survivors:
                terminate_owned(pid, owned[pid], process.pid)
            if survivors:
                raise RuntimeError('fixture leaked owned processes; terminated: '+str(survivors))
    return rows, events


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('output', type=Path, help='new directory under target/performance')
    parser.add_argument('--seconds', type=int, default=15)
    parser.add_argument('--repetitions', type=int, default=3)
    parser.add_argument('--interval', type=float, default=.2)
    args = parser.parse_args()
    out = args.output.resolve()
    if (out.parent != REPO/'target/performance' or not 3 <= args.seconds <= 20 or
            not 1 <= args.repetitions <= 10 or not .05 <= args.interval <= 1):
        parser.error('use a new direct child of target/performance and bounded sampling parameters')
    out.mkdir(mode=0o700)
    metadata = dict(schema_version=1, kernel=platform.release(), architecture=platform.machine(),
        logical_cpus=os.cpu_count(), python=platform.python_version(), clock_ticks=CLK_TCK,
        page_size=PAGE_SIZE, sample_interval_s=args.interval, idle_seconds=args.seconds,
        repetitions=args.repetitions,
        git_commit=subprocess.check_output(['git','rev-parse','HEAD'], cwd=REPO, text=True).strip(),
        limits=['synthetic fixtures, no hardware or GUI',
                'runtime fixture includes client thread and instrumentation',
                'CPU granularity 1/CLK_TCK; zero means below resolution',
                'RSS sum double-counts shared pages; prefer PSS',
                'context switches across observed live threads, NOT exact wakeups',
                'sampling non-atomic; sampled peaks exclude shorter spikes'])
    metadata['source_sha256'] = {str(path): hashlib.sha256((REPO/path).read_bytes()).hexdigest()
        for path in map(Path, ('scripts/performance/profile.py', 'scripts/performance/run.sh',
            'crates/rad-runtime/examples/runtime_profile.rs',
            'crates/rad-runtime/examples/domain_profile.rs'))}
    (out/'metadata.json').write_text(json.dumps(metadata, indent=2)+'\n')
    summaries, timing_samples = [], []
    binary = REPO/'target/release/examples/runtime_profile'
    for repeat in range(args.repetitions):
        for count in (0, 1, 4):
            name = f'r{repeat}-{count}'
            print(f'Measuring {name}', flush=True)
            rows, events = observe([str(binary), str(out/(name+'w')), str(count), str(args.seconds)],
                                   out/name, args.interval, args.seconds+90)
            timing_samples.extend(dict(event, repetition=repeat, plugins=count)
                                  for event in events if 'ns' in event)
            for phase in ('idle', 'load'):
                summary = summarize_phase(rows, phase)
                summary.update(repetition=repeat, plugins=count)
                summary['roles'] = {}
                roles = {p['role'] for r in rows if r['phase'] == phase for p in r['processes']}
                for role in sorted(roles):
                    filtered = [dict(r, processes=[p for p in r['processes'] if p['role'] == role]) for r in rows]
                    summary['roles'][role] = summarize_phase(filtered, phase)
                summaries.append(summary)
            if not any(e.get('event') == 'complete' for e in events):
                raise ValueError('missing explicit fixture completion')
    (out/'resources.json').write_text(json.dumps(summaries, indent=2)+'\n')
    (out/'runtime-timings.json').write_text(json.dumps(summarize_timings(
        timing_samples, ('plugins', 'repetition', 'operation')), indent=2)+'\n')
    domain = out/'domain'
    domain.mkdir(mode=0o700)
    with (out/'domain.jsonl').open('w') as output, (out/'domain.stderr.log').open('w') as err:
        subprocess.run([str(REPO/'target/release/examples/domain_profile'), str(domain/'work')],
                       cwd=REPO, env=clean_environment(domain/'env'), stdout=output, stderr=err,
                       timeout=900, check=True)
    events = [json.loads(line) for line in (out/'domain.jsonl').read_text().splitlines()]
    (out/'domain-timings.json').write_text(json.dumps(summarize_timings(
        events, ('identity_count', 'scenario', 'operation')), indent=2)+'\n')
    print(out.relative_to(REPO), flush=True)


if __name__ == '__main__':
    main()
