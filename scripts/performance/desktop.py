#!/usr/bin/env python3
"""Explicit real desktop/XL measurement; default performs no inspection or launch."""
import argparse
from contextlib import contextmanager
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import shlex
import signal
import shutil
import stat
import subprocess
import threading
import time

REPO = Path(__file__).resolve().parents[2]


def load(name, filename):
    spec = importlib.util.spec_from_file_location(name, REPO/'scripts/performance'/filename)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


COMMON = load('desktop_common', 'officials.py')
PHYSICAL = load('desktop_physical', 'physical.py')
SAMPLER = COMMON.SAMPLER


class MeasurementCancelled(RuntimeError):
    pass


_spawn_depth = 0
_pending_signal = None


def raise_cancelled(signum):
    for sig in (signal.SIGTERM,signal.SIGINT):
        signal.signal(sig,signal.SIG_IGN)
    raise MeasurementCancelled('measurement cancelled by signal '+str(signum))


def install_shutdown_signals():
    def cancel(signum, _frame):
        global _pending_signal
        if _spawn_depth:
            # Do not change signal disposition before Popen execs its child.
            _pending_signal = signum
        else:
            raise_cancelled(signum)
    for sig in (signal.SIGTERM,signal.SIGINT):
        signal.signal(sig,cancel)


@contextmanager
def registered_spawn():
    global _spawn_depth, _pending_signal
    _spawn_depth += 1
    try:
        yield
    finally:
        _spawn_depth -= 1
        if not _spawn_depth and _pending_signal is not None:
            signum, _pending_signal = _pending_signal, None
            raise_cancelled(signum)


@contextmanager
def protected_cleanup():
    # Finish reap before delivering a FIRST cancellation; do not silently lose it.
    # Following an already-delivered cancellation the handler stays SIG_IGN.
    with registered_spawn():
        yield


def validate_output(output):
    out = output.resolve()
    if (out.parent != REPO/'target/performance' or out.exists()
            or output.absolute() != out or len(str(out/'r0/runtime/rad/editor.sock').encode()) >= 108):
        raise ValueError('use a new short direct child of target/performance without symlinks')
    return out


def focus_matches(value, pid):
    return type(value.get('pid')) is int and value['pid'] == pid and value.get('mapped') is True


def guard_owners(device, allowed):
    if PHYSICAL.locate_xl() != device:
        raise RuntimeError('XL topology changed; no recovery experiment authorized')
    seen = set().union(*(PHYSICAL.holders(device[key]) for key in ('hidraw','usb')))
    if seen-allowed:
        raise RuntimeError('unexpected XL owner; stop only the owned runtime without logo writes')
    return len(seen)


def desktop_environment(root, wayland, bus):
    env = {'PATH':str(root/'bin')+':/usr/bin:/bin','LANG':'C.UTF-8',
           'DBUS_SESSION_BUS_ADDRESS':bus,'WAYLAND_DISPLAY':str(wayland),
           'XDG_SESSION_TYPE':'wayland','WINIT_UNIX_BACKEND':'wayland'}
    for key, name in (('HOME','home'),('XDG_RUNTIME_DIR','runtime'),('XDG_DATA_HOME','data'),
                      ('XDG_CONFIG_HOME','config'),('XDG_CACHE_HOME','cache'),
                      ('XDG_STATE_HOME','state'),('XDG_DATA_DIRS','system'),('XDG_CONFIG_DIRS','system')):
        path = root/name
        path.mkdir(mode=0o700,exist_ok=True)
        if path.is_symlink() or not path.is_dir() or path.stat().st_mode & 0o077:
            raise ValueError('desktop fixture paths must remain private directories')
        env[key] = str(path)
    return env


def write_systemctl_shim(root):
    (root/'bin').mkdir(mode=0o700,exist_ok=True)
    shim = root/'bin/systemctl'
    shim.write_text('''#!/bin/sh
printf '%s\\n' "$*" >> LOG
case "$*" in
  '--user daemon-reload'|'--user start rad-runtime.service') exit 0 ;;
  '--user show rad-runtime.service -p ActiveState -p Result -p ExecMainStatus')
    printf 'ActiveState=active\\nResult=success\\nExecMainStatus=0\\n'; exit 0 ;;
  *) printf 'DENIED\\n' >> LOG; exit 97 ;;
esac
'''.replace('LOG',shlex.quote(str(root/'systemctl.log'))))
    shim.chmod(0o700)
    return shim


class Peer(COMMON.Bridge):
    def __init__(self, world):
        self.world = world
        self.child = world.spawn([str(world.binaries/'conformance_client'),
                                  str(world.root/'runtime'),'editor'],'client',drain=False)


class DesktopWorld(COMMON.World):
    """Reuse audited owned-process lifecycle; never call official backend setup."""
    def spawn(self, *args, **kwargs):
        # A requested cancellation is delivered only after Popen, roots and the
        # PID/starttime/SID identity are registered, even before role assignment.
        with registered_spawn():
            return super().spawn(*args,**kwargs)

    def __init__(self, root, binaries, wayland, log, raw, device):
        self.root, self.binaries, self.log, self.raw = root, binaries, log, raw
        self.roots, self.owned = [], {}
        self.runtime = self.runtime_pid = self.editor = self.sink = None
        self.backends, self.rows, self.events, self.unobserved = [], [], [], set()
        self.epoch = time.monotonic()
        self.device, self.guard_errors, self.guards = device, [], []
        self.guard_stop, self.guard_thread = threading.Event(), None
        root.mkdir(mode=0o700)
        self.env = desktop_environment(root,wayland,'unix:path='+str(root/'bus'))
        self.hypr_env = {key:os.environ[key] for key in ('XDG_RUNTIME_DIR','HYPRLAND_INSTANCE_SIGNATURE')}
        self.hypr_env.update(PATH='/usr/bin:/bin',LANG='C.UTF-8')
        write_systemctl_shim(root)
        (root/'data/rad').mkdir(mode=0o700)
        self.machine(False)
        conf = root/'bus.conf'
        conf.write_text(f'''<busconfig><type>session</type><listen>unix:path={root}/bus</listen>
<auth>EXTERNAL</auth><policy context="default"><allow send_destination="*"/>
<allow receive_sender="*"/><allow own="*"/></policy></busconfig>''')
        bus = self.spawn(['/usr/bin/dbus-daemon',f'--config-file={conf}','--nofork','--print-address'],
                         'private_bus')
        address = self.line(bus)
        if not address.startswith('unix:path='+str(root/'bus')+','):
            raise ValueError('bus is not the newly created private bus')
        self.env['DBUS_SESSION_BUS_ADDRESS'] = address
        self.backends.append((bus,'private_bus'))

    def machine(self, enabled):
        path = self.root/'data/rad/machine.json'
        path.write_text(json.dumps(dict(version=1,use_stream_deck=enabled,start_with_session=False)))
        path.chmod(0o600)

    def check(self):
        if self.guard_errors:
            raise RuntimeError(self.guard_errors[0])
        if self.runtime is not None and self.runtime.poll() is not None:
            raise RuntimeError('owned runtime ended unexpectedly')
        shimlog = self.root/'systemctl.log'
        if shimlog.exists() and 'DENIED' in shimlog.read_text():
            raise RuntimeError('editor requested an out-of-scope service operation')

    def guard_loop(self):
        while not self.guard_stop.is_set():
            try:
                count = guard_owners(self.device,{self.runtime_pid})
                self.guards.append(dict(elapsed_s=time.monotonic()-self.epoch,holders=count,unexpected=0))
            except Exception as error:
                self.guard_errors.append(str(error))
                # Kill the only own writer immediately. No graceful/logo writes on contention.
                self.signal(self.runtime_pid,signal.SIGKILL)
                return
            self.guard_stop.wait(1)

    def start_product(self, physical):
        self.machine(physical)
        if physical:
            # Repeat immediately before opening, with no own editor/runtime alive yet.
            pre = PHYSICAL.preflight()
            if pre['device'] != self.device:
                raise RuntimeError('XL changed between preflights')
        spawned_wall_ns = time.time_ns()
        self.runtime = self.spawn([str(self.binaries/'rad-runtime')],'runtime')
        self.runtime_pid = self.runtime.pid
        if physical:
            self.guard_stop.clear()
            self.guard_thread = threading.Thread(target=self.guard_loop,daemon=True)
            self.guard_thread.start()
        def started():
            self.check()
            socket = self.root/'runtime/rad/editor.sock'
            return socket.exists() and socket.stat().st_ctime_ns >= spawned_wall_ns
        COMMON.until(started,bool)
        with Peer(self) as peer:
            COMMON.until(lambda:peer.call('status'),
                         lambda state:state['deck_state'] == ('connected' if physical else 'disabled'))
        self.events.append(dict(event='runtime_started',physical=physical,pid=self.runtime_pid))

    def stop_product(self):
        self.check()
        super().stop_runtime()
        self.guard_stop.set()
        if self.guard_thread is not None:
            self.guard_thread.join(timeout=5)
            if self.guard_thread.is_alive():
                raise RuntimeError('scoped guard did not finish')
            self.guard_thread = None
        if self.guard_errors:
            raise RuntimeError(self.guard_errors[0])
        if self.device is not None:
            self.guards.append(dict(event='postflight',holders=guard_owners(self.device,set()),unexpected=0))

    def seed(self):
        with Peer(self) as peer:
            peer.call('set_settings',changes=dict(brightness_percent=25,sleep_ms='0',press_threshold_ms='0'))
            root = peer.call('status')['active_folder_id']
            peer.call('create_folder',parent_id=root,position=0,name='ENTRAR')
            enter = peer.call('get',id=root)['fields']['slots'][0]['id']
            self.execute(peer,enter)
            child = peer.call('status')['active_folder_id']
            back = peer.call('get',id=child)['fields']['slots'][0]['id']
            for folder in (root,child):
                for position in range(1,32):
                    peer.call('create_button',folder_id=folder,position=position,name=f'Tecla {position}')
            self.execute(peer,back)
        self.seeded = dict(root=root,child=child,enter=enter,back=back)
        self.events.append(dict(event='seeded',folders=2,visible_slots=32,brightness_percent=25,
                                preparation_outside_measured_runtime=True))

    def execute(self, peer, button):
        self.check()
        before = time.monotonic_ns()
        receipt = peer.call('run',id=button)
        deadline = time.monotonic()+3
        while True:
            result = peer.call('result',execution_id=receipt['execution_id'])
            if result['state'] == 'succeeded':
                return time.monotonic_ns()-before
            if result['state'] not in ('running','waiting') or time.monotonic()>deadline:
                raise RuntimeError('navigation did not complete successfully')

    def hypr(self, *arguments):
        result = subprocess.run(['/usr/bin/hyprctl',*arguments],env=self.hypr_env,
                                capture_output=True,text=True,timeout=3,check=True)
        return result.stdout

    def owned_window(self, child):
        if child.poll() is not None:
            raise RuntimeError('owned window process ended')
        start, session = self.owned[child.pid]
        if not SAMPLER.same_owned_process(child.pid,start,session):
            raise RuntimeError('owned window identity changed')
        # Never persist the complete desktop list or unrelated titles/clients.
        values = json.loads(self.hypr('-j','clients'))
        matches = [value for value in values if value.get('pid') == child.pid and value.get('mapped')]
        if len(matches) != 1:
            return None
        address = matches[0]['address']
        if not address.startswith('0x') or not all(c in '0123456789abcdefABCDEF' for c in address[2:]):
            raise ValueError('invalid owned window address')
        return address

    def focus(self, child):
        address = COMMON.until(lambda:self.owned_window(child),bool)
        self.hypr('dispatch','hl.dsp.focus({ window = "address:'+address+'" })')
        def focused():
            value = json.loads(self.hypr('-j','activewindow'))
            return focus_matches(value,child.pid) and value.get('address') == address
        COMMON.until(focused,bool,timeout=3)
        return address

    def assert_focus(self, child, address):
        value = json.loads(self.hypr('-j','activewindow'))
        if not focus_matches(value,child.pid) or value.get('address') != address:
            raise RuntimeError('focus changed; invalidate phase rather than taking it back from user')

    def start_editor(self):
        self.editor = self.spawn([str(self.binaries/'rad-editor')],'editor')
        self.editor_address = self.focus(self.editor)
        self.events.append(dict(event='editor_started',pid=self.editor.pid,real_window=True,
                                service_backend='private_fail_closed_shim'))
        time.sleep(1)
        self.check()

    def close_editor(self):
        if self.editor is None:
            return
        child = self.editor
        self.remember_descendants(child.pid)
        result = subprocess.run(['/usr/bin/gdbus','call','--address',self.env['DBUS_SESSION_BUS_ADDRESS'],
            '--dest','io.github.KroxiDev.RAD','--object-path','/io/github/KroxiDev/RAD',
            '--method','org.gtk.Actions.Activate','quit','[]','{}'],
            env=self.env,capture_output=True,text=True,timeout=4)
        if result.returncode != 0 or child.wait(timeout=8) != 0:
            raise RuntimeError('private editor did not close cleanly through its own action')
        child.reader.join(timeout=1)
        child.stdin.close()
        child.stdout.close()
        self.editor = None
        self.events.append(dict(event='editor_closed',clean_shutdown=True))

    def capture_desktop(self, phase, focus_observation):
        self.check()
        values = []
        roots = [(self.runtime,'runtime'),(self.editor,'editor'),(self.sink,'focus_sink'),*self.backends]
        for child, role in roots:
            if child is None:
                continue
            if child.poll() is not None:
                raise RuntimeError('owned '+role+' ended during window')
            for pid in SAMPLER.descendants(child.pid):
                try:
                    value = SAMPLER.read_process(pid,child.pid)
                    previous = self.owned.get(pid)
                    identity = (value['start_ticks'],child.pid)
                    if previous is not None and previous != identity:
                        raise RuntimeError('process identity changed while sampling')
                    self.owned[pid] = identity
                    value['role'] = role if pid == child.pid else role+'_child'
                    values.append(value)
                except FileNotFoundError:
                    continue
        row = dict(elapsed_s=time.monotonic()-self.epoch,phase=phase,processes=values,
                   focus=focus_observation)
        self.rows.append(row)
        self.raw.write(json.dumps(row)+'\n')

    def measure(self, phase, seconds, focused=None, navigate=False):
        self.events.append(dict(event='phase_begin',phase=phase))
        print('PHASE '+phase,flush=True)
        if focused is not None:
            address = self.focus(focused)
        before, tick = time.monotonic(), 0
        with Peer(self) as peer:
            while True:
                focus_observation = None
                if focused is not None:
                    self.assert_focus(focused,address)
                    focus_observation = dict(own_pid=focused.pid,verified=True,
                        role='editor' if focused is self.editor else 'focus_sink',
                        observed_elapsed_s=time.monotonic()-self.epoch)
                if navigate:
                    target = self.seeded['enter'] if tick%2 == 0 else self.seeded['back']
                    ns = self.execute(peer,target)
                    expected = self.seeded['child'] if tick%2 == 0 else self.seeded['root']
                    if peer.call('status')['active_folder_id'] != expected:
                        raise RuntimeError('navigation published the wrong folder')
                    self.events.append(dict(event='sample',operation='ipc_navigation_observed',
                                            phase=phase,ns=ns,iteration=tick))
                self.capture_desktop(phase,focus_observation)
                if time.monotonic()-before >= seconds:
                    break
                tick += 1
                time.sleep(max(0,min(before+tick*.25,before+seconds)-time.monotonic()))
        self.raw.flush()
        self.events.append(dict(event='phase_end',phase=phase))

    def close(self):
        # Normal completion has already used stop_product(). Exceptional teardown must
        # not write a logo after a contender/observation error or leave a restart path.
        for child, role in self.roots:
            if role == 'runtime' and child.poll() is None:
                self.remember_descendants(child.pid)
                self.signal(child.pid,signal.SIGKILL)
                self.events.append(dict(event='emergency_writer_stopped',logo_restoration_verified=False))
        self.guard_stop.set()
        if self.guard_thread is not None:
            self.guard_thread.join(timeout=5)
        super().close()


def run_case(out, binaries, wayland, physical, repeat, seconds):
    root = out/f'r{repeat}'
    with (out/f'r{repeat}.stderr.log').open('w') as log, \
         (out/f'r{repeat}.samples.jsonl').open('w') as raw:
        world = DesktopWorld.__new__(DesktopWorld)
        try:
            world.__init__(root,binaries,wayland,log,raw,physical)
            world.start_product(False)
            world.seed()
            world.stop_product()
            world.start_product(physical is not None)
            time.sleep(2)
            world.measure('runtime_only',seconds)
            world.start_editor()
            world.measure('editor_focused',seconds,world.editor)
            world.sink = world.spawn(['/usr/bin/foot','--hold','--app-id=rad-measurement-focus',
                '--title=RAD measurement focus','--','/usr/bin/true'],'focus_sink')
            # A terminal normally gives its child a new SID. Use an already-finished
            # command, not a live foreign-session child that cannot be safely owned.
            COMMON.until(lambda:SAMPLER.descendants(world.sink.pid),
                         lambda pids:pids == [world.sink.pid],timeout=3)
            world.measure('editor_unfocused',seconds,world.sink)
            world.measure('editor_navigation',seconds,world.editor,navigate=True)
            world.close_editor()
            world.measure('editor_closed',seconds,world.sink)
            with Peer(world) as peer:
                log_entries = peer.call('runtime_log')['entries']
            codes = [entry['code'] for entry in log_entries]
            expected = {'started','deck_connected'}
            if any(code not in expected for code in codes):
                raise RuntimeError('unexpected runtime event during controlled case: '+repr(codes))
            world.events.append(dict(event='runtime_log_codes',codes=codes))
            world.stop_product()
            world.close()
            world.events.append(dict(event='complete',physical=physical is not None,clean_shutdown=True,
                                    editor_action_closed=True,all_owned_reaped=True))
            summaries = []
            for phase in ('runtime_only','editor_focused','editor_unfocused','editor_navigation','editor_closed'):
                for group, roles in (('product_tree',{'runtime','runtime_child','editor','editor_child'}),
                                     ('runtime_tree',{'runtime','runtime_child'}),('editor',{'editor','editor_child'})):
                    if group == 'editor' and phase in ('runtime_only','editor_closed'):
                        continue
                    selected = [dict(row,processes=[p for p in row['processes'] if p['role'] in roles])
                                for row in world.rows]
                    summary = SAMPLER.summarize_phase(selected,phase)
                    summaries.append(dict(summary,repetition=repeat,group=group))
            return summaries,world.events
        finally:
            if hasattr(world,'roots'):
                with protected_cleanup():
                    world.close()
                    (out/f'r{repeat}.events.jsonl').write_text(''.join(json.dumps(e)+'\n' for e in world.events))
                    (out/f'r{repeat}.guards.json').write_text(json.dumps(world.guards,indent=2)+'\n')


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('output', type=Path)
    parser.add_argument('--run-desktop', action='store_true')
    parser.add_argument('--physical', action='store_true')
    parser.add_argument('--seconds',type=int,default=8)
    parser.add_argument('--repetitions',type=int,default=3)
    args = parser.parse_args(argv)
    if args.physical and not args.run_desktop:
        parser.error('--physical also requires explicit --run-desktop')
    if not args.run_desktop:
        print('DRY: no desktop, service or device inspection/launch')
        return
    if not 5 <= args.seconds <= 15 or not 1 <= args.repetitions <= 3:
        parser.error('seconds 5..15; repetitions 1..3')
    install_shutdown_signals()
    out = validate_output(args.output)
    for name in ('hyprctl','foot','dbus-daemon','gdbus'):
        if shutil.which(name) is None:
            raise RuntimeError('required desktop tool unavailable; do not install automatically: '+name)
    wayland = Path(os.environ['WAYLAND_DISPLAY'])
    if not wayland.is_absolute():
        wayland = Path(os.environ['XDG_RUNTIME_DIR'])/wayland
    if not stat.S_ISSOCK(wayland.stat().st_mode):
        raise ValueError('explicit current Wayland transport is not a socket')
    if not os.environ.get('HYPRLAND_INSTANCE_SIGNATURE'):
        raise ValueError('cannot verify desktop focus without current compositor transport')
    pre = PHYSICAL.preflight() if args.physical else None
    out.mkdir(mode=0o700)
    binaries = out/'elf'
    binaries.mkdir(mode=0o700)
    files = {name:REPO/'target/release'/name for name in
             ('rad-runtime','rad-editor','rad-launcher-helper','rad-tray')}
    files['conformance_client'] = REPO/'target/release/examples/conformance_client'
    for name, source in files.items():
        shutil.copy2(source,binaries/name)
    sources = ('scripts/performance/desktop.py','scripts/performance/profile.py',
               'scripts/performance/physical.py','scripts/performance/officials.py')
    digest = lambda path:hashlib.sha256(path.read_bytes()).hexdigest()
    metadata = dict(base_commit=subprocess.check_output(['git','rev-parse','HEAD'],cwd=REPO,text=True).strip(),
        physical=args.physical,preflight=pre,repetitions=args.repetitions,seconds=args.seconds,
        source_sha256={p:digest(REPO/p) for p in sources},
        binary_sha256={name:digest(binaries/name) for name in files},
        limits=['real Wayland eframe/Glow window; service backend is private shim',
                'own processes only; compositor/GPU telemetry NOT measured',
                'IPC/navigation completion is NOT physical contact/USB/pixel latency',
                'no personal sensors/services/plugins; CPU ticks/context switches not exact wakeups',
                'focus sink, private bus, typed IPC and guard overhead excluded from product_tree',
                'sampled peaks/non-atomic reads; current desktop workload not controlled'])
    meta = out/'metadata.json'
    meta.write_text(json.dumps(metadata,indent=2)+'\n')
    try:
        resources,events = [],[]
        for repeat in range(args.repetitions):
            rows,values = run_case(out,binaries,wayland,pre['device'] if pre else None,repeat,args.seconds)
            resources.extend(rows)
            events.extend(dict(event,repetition=repeat) for event in values)
        if any(digest(REPO/p)!=sha for p,sha in metadata['source_sha256'].items()):
            raise RuntimeError('measured controller source changed during run')
        if any(digest(binaries/name)!=sha for name,sha in metadata['binary_sha256'].items()):
            raise RuntimeError('retained measured ELF changed during run')
        (out/'resources.json').write_text(json.dumps(resources,indent=2)+'\n')
        timings = SAMPLER.summarize_timings(events,('phase','repetition','operation'))
        (out/'timings.json').write_text(json.dumps(timings,indent=2)+'\n')
        metadata.update(complete=True,all_owned_reaped=True,source_fingerprints_match=True)
        if pre:
            metadata['post_test_holder_count'] = guard_owners(pre['device'],set())
        print('COMPLETE '+str(out.relative_to(REPO)),flush=True)
    except BaseException as error:
        metadata.update(complete=False,error_type=type(error).__name__)
        raise
    finally:
        with protected_cleanup():
            meta.write_text(json.dumps(metadata,indent=2)+'\n')


if __name__ == '__main__':
    main()
