#!/usr/bin/env python3
"""Opt-in XL measurement. Default is a dry run with NO device inspection or I/O."""
import argparse
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import shutil
import stat
import subprocess
import time

REPO=Path(__file__).resolve().parents[2]
_spec=importlib.util.spec_from_file_location('rad_observer',REPO/'scripts/performance/profile.py')
observer=importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(observer)


def holders(node):
    # Paths are discovered absolute /dev nodes (or owned test fixtures), never options.
    # This installed fuser treats a standalone '--' as ending its file list.
    result=subprocess.run(['fuser',str(node)],capture_output=True,text=True,timeout=3)
    words=result.stdout.split()
    if result.returncode not in (0,1) or any(not w.isdecimal() for w in words):
        raise RuntimeError('cannot verify scoped device holders')
    warning=result.stderr.replace(str(node)+':','').strip()
    if warning:
        raise RuntimeError('device holder check incomplete: '+warning)
    if (result.returncode==0)!=bool(words):
        raise RuntimeError('inconsistent holder result')
    return {int(word) for word in words}


def locate_xl():
    matches=[]
    for entry in Path('/sys/class/hidraw').iterdir():
        real=entry.resolve()
        for usb in real.parents:
            if (usb/'idVendor').is_file() and (usb/'idProduct').is_file():
                if ((usb/'idVendor').read_text().strip(),(usb/'idProduct').read_text().strip())==('0fd9','006c'):
                    interfaces=[p for p in real.parents if (p/'bInterfaceNumber').is_file()]
                    interface=interfaces[0] if interfaces else None
                    if interface is None or (interface/'driver').resolve().name!='usbhid':
                        raise RuntimeError('XL interface is not held by expected usbhid driver')
                    serial=(usb/'serial').read_bytes().strip()
                    if not 1<=len(serial)<=32 or not all(33<=b<=126 for b in serial):
                        raise RuntimeError('XL identity is not valid for the production adapter')
                    node=Path('/dev')/entry.name
                    if not stat.S_ISCHR(node.stat().st_mode) or not os.access(node,os.R_OK|os.W_OK):
                        raise RuntimeError('XL hidraw is not an accessible read/write character device')
                    bus=int((usb/'busnum').read_text()); address=int((usb/'devnum').read_text())
                    matches.append(dict(hidraw=str(node),usb=f'/dev/bus/usb/{bus:03}/{address:03}',driver='usbhid'))
                break
    if len(matches)!=1:
        raise RuntimeError('expected exactly one eligible XL; refusing to select another device')
    return matches[0]


def preflight():
    if shutil.which('fuser') is None or shutil.which('systemctl') is None:
        raise RuntimeError('required scoped verification tool missing; do not install automatically')
    device=locate_xl()
    services={}
    for unit in ('rad-runtime.service','rad-tray.service'):
        result=subprocess.run(['systemctl','--user','is-active',unit],capture_output=True,text=True,timeout=3)
        state=result.stdout.strip()
        if state not in ('inactive','failed','unknown'):
            raise RuntimeError('personal '+unit+' is not confirmed inactive; do not stop it implicitly')
        services[unit]=state
    processes=[]
    for path in Path('/proc').iterdir():
        if not path.name.isdecimal():continue
        try:name=(path/'comm').read_text().strip()
        except (FileNotFoundError,PermissionError,ProcessLookupError):continue
        if name.startswith(('rad-runtime','rad-editor','rad-tray')) or name.lower().startswith('streamcontrol'):
            processes.append(dict(pid=int(path.name),name=name))
    if processes:
        raise RuntimeError('relevant personal application still running; do not stop it implicitly')
    for key in ('hidraw','usb'):
        if holders(device[key]):
            raise RuntimeError('another process has the XL open; do not claim it')
    return dict(device=device,services=services,relevant_processes_observed=0,
                scoped_holders_observed=0,serial_published=False)


def completion_status(event,physical):
    expected='physical' if physical else 'dry_run'
    if event.get('mode')!=expected or event.get('clean_shutdown') is not True:
        raise ValueError('completion mode or orderly shutdown not verified')
    for key in ('human_complete','logo_restored'):
        if type(event.get(key)) is not bool:
            raise ValueError('missing boolean completion evidence: '+key)
    counts=event.get('human_counts')
    if not isinstance(counts,dict):raise ValueError('missing human counters')
    for key in ('navigation','sleep','wake'):
        if type(counts.get(key)) is not int or counts[key]<0:
            raise ValueError('invalid human counter: '+key)
    if type(counts.get('all_released')) is not bool:
        raise ValueError('missing final released-key evidence')
    enough=(counts['navigation']>=12 and counts['sleep']>=1 and counts['wake']>=1
            and counts['all_released'])
    if event['human_complete'] and not enough:
        raise ValueError('human completion contradicts observed counters')
    reasons=[]
    if not event['human_complete']:reasons.append('human_sequence_incomplete')
    if physical and not event['logo_restored']:reasons.append('device_logo_not_confirmed')
    return dict(complete=not reasons,partial_reasons=reasons)


def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('output',type=Path)
    parser.add_argument('--physical',action='store_true',help='explicit hardware I/O, only after user authorization')
    args=parser.parse_args()
    output=args.output.resolve()
    if output.parent!=REPO/'target/performance' or output.exists():
        parser.error('use a new direct child of target/performance')
    pre=preflight() if args.physical else dict(mode='dry_no_device_inspection')
    output.mkdir(mode=0o700)
    binary=REPO/'target/release/examples/physical_profile'
    helper=REPO/'target/release/rad-launcher-helper'
    sources=('scripts/performance/physical.py','scripts/performance/profile.py',
             'crates/rad-runtime/examples/physical_profile.rs')
    metadata=dict(physical=args.physical,preflight=pre,
        base_commit=subprocess.check_output(['git','rev-parse','HEAD'],cwd=REPO,text=True).strip(),
        source_sha256={p:hashlib.sha256((REPO/p).read_bytes()).hexdigest() for p in sources},
        binary_sha256=hashlib.sha256(binary.read_bytes()).hexdigest(),
        helper_sha256=hashlib.sha256(helper.read_bytes()).hexdigest(),
        limits=['report dequeue to Desired submission, NOT finger/HID-read/USB-ack/pixel latency',
                'connected fixture includes blocking wake relay and client thread',
                'external scoped fuser watcher every 1s adds observer cost',
                'normal shutdown restores device logo, NOT previous framebuffer or brightness',
                'contender/disconnect abort kills only our process without logo restoration'])
    shutil.copy2(binary,output/'measured-executable')
    shutil.copy2(helper,output/'measured-launcher-helper')
    if hashlib.sha256((output/'measured-executable').read_bytes()).hexdigest()!=metadata['binary_sha256']:
        raise ValueError('measured executable copy does not match preflight hash')
    if hashlib.sha256((output/'measured-launcher-helper').read_bytes()).hexdigest()!=metadata['helper_sha256']:
        raise ValueError('measured helper copy does not match preflight hash')
    meta_file=output/'metadata.json'
    meta_file.write_text(json.dumps(metadata,indent=2)+'\n')
    root_identity=None
    last_watch=0
    guards=[]

    def abort_owned(reason):
        if root_identity is not None:
            pid,start=root_identity
            observer.terminate_owned(pid,start,pid)
        metadata['aborted']=reason
        metadata['logo_restored_on_abort']=False
        meta_file.write_text(json.dumps(metadata,indent=2)+'\n')
        raise RuntimeError(reason)

    def on_sample(row):
        nonlocal root_identity,last_watch
        roots=[p for p in row['processes'] if p['role']=='runtime_fixture']
        if roots:
            root=roots[0]; root_identity=(root['pid'],root['start_ticks'])
        if not args.physical or root_identity is None or time.monotonic()-last_watch<1:
            return
        last_watch=time.monotonic()
        try:
            device=locate_xl()
            if device!=pre['device']:
                abort_owned('XL topology changed during physical test')
            seen=set()
            for key in ('hidraw','usb'):seen.update(holders(device[key]))
            unexpected=seen-{root_identity[0]}
            guards.append(dict(elapsed_s=row['elapsed_s'],holders=len(seen),unexpected=len(unexpected)))
            if unexpected:abort_owned('another owner appeared during XL test; own process stopped immediately')
        except Exception as error:
            if metadata.get('aborted'):raise
            abort_owned('scoped exclusivity could not be verified: '+str(error))

    def on_event(event):
        kind=event.get('event')
        if kind=='phase_begin':print('PHASE '+event['phase'],flush=True)
        if kind=='ready_human':
            if args.physical:
                (output/'ready_human').write_text('ready\n')
                print('READY_HUMAN: test profile is ready for physical presses',flush=True)
            else:
                print('DRY_SIMULATED_HUMAN_PHASE: no physical input requested',flush=True)
        if kind=='complete':print('FIXTURE_COMPLETE '+json.dumps(event),flush=True)
        if args.physical and kind in ('deck_disconnected','unexpected_disconnect'):
            abort_owned('device disconnected unexpectedly; no driver recovery experiment authorized')

    try:
        command=[str(binary),str(output/'runtime-profile')]
        command+=['--physical','--exclusive-authorized'] if args.physical else ['--dry-run']
        rows,events=observer.observe(command,output/'observer',.2,660,
            event_callback=on_event,sample_callback=on_sample)
        completed=[e for e in events if e.get('event')=='complete']
        if len(completed)!=1:raise ValueError('missing single explicit completion')
        metadata['completion']=completed[0]
        metadata['run_status']=completion_status(completed[0],args.physical)
        summaries=[]
        for phase in ('idle','auto','human'):
            selected=[r for r in rows if r['phase']==phase]
            if len(selected)>=2:summaries.append(observer.summarize_phase(rows,phase))
        (output/'resources.json').write_text(json.dumps(summaries,indent=2)+'\n')
        (output/'timings.json').write_text(json.dumps(observer.summarize_timings(
            [e for e in events if 'operation' in e and 'ns' in e],('operation',)),indent=2)+'\n')
        (output/'guards.json').write_text(json.dumps(guards,indent=2)+'\n')
        for path,digest in metadata['source_sha256'].items():
            if hashlib.sha256((REPO/path).read_bytes()).hexdigest()!=digest:
                raise ValueError('source changed during physical test')
        metadata['source_fingerprints_match']=True
        if (hashlib.sha256(binary.read_bytes()).hexdigest()!=metadata['binary_sha256']
                or hashlib.sha256(helper.read_bytes()).hexdigest()!=metadata['helper_sha256']):
            raise ValueError('measured executable or helper changed during the test')
        if args.physical:
            post={key:len(holders(pre['device'][key])) for key in ('hidraw','usb')}
            metadata['post_test_holder_counts']=post
            metadata['post_test_device_released']=not any(post.values())
            if not metadata['post_test_device_released']:
                metadata['run_status']['complete']=False
                metadata['run_status']['partial_reasons'].append('device_not_confirmed_free_after_test')
    except BaseException:
        metadata['run_failed']=True
        raise
    finally:
        meta_file.write_text(json.dumps(metadata,indent=2)+'\n')
    if not metadata['run_status']['complete']:
        print('PARTIAL '+json.dumps(metadata['run_status']),flush=True)
        raise SystemExit(2)
    print(output.relative_to(REPO),flush=True)


if __name__=='__main__':main()
