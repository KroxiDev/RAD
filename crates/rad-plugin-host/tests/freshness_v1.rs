use rad_plugin_contract::*;
use rad_plugin_host::{Host, State};
use std::{
    process::Command,
    time::{Duration, Instant},
};
fn parameters() -> Parameters {
    Parameters {
        schema_version: 1,
        values: Default::default(),
    }
}
fn pump_until(host: &mut Host, now: u64, condition: impl Fn(&Host) -> bool) {
    let end = Instant::now() + Duration::from_secs(3);
    while !condition(host) {
        host.pump(now).unwrap();
        assert!(Instant::now() < end);
        std::thread::sleep(Duration::from_millis(1));
    }
}
#[test]
fn plugin_declared_validity_expires_and_content_free_observation_renews() {
    let manifest:Manifest=serde_json::from_value(serde_json::json!({"contract":1,"capabilities":["visual_freshness"],"id":"org.example.fresh","version":{"major":1,"minor":0,"patch":0},"host_min":1,"host_max":1,"targets":["x86_64-unknown-linux-gnu"],"actions":[{"id":"renew","label":"Renew","schema":{"version":1,"fields":[]},"credential_slots":[]}],"visuals":[{"id":"state","label":"State","schema":{"version":1,"fields":[]},"credential_slots":[]}]})).unwrap();
    let mut child = Command::new("/usr/bin/python3");
    child.args(["-c",r#"
import sys,json,struct
r=sys.stdin.buffer; w=sys.stdout.buffer
sub=None
def emit(c,m):
 b=json.dumps(dict(protocol=1,correlation=c,message=m)).encode();w.write(struct.pack('>I',len(b))+b);w.flush()
while True:
 h=r.read(4)
 if not h:break
 e=json.loads(r.read(struct.unpack('>I',h)[0]));m=e['message'];c=e['correlation']
 if m['kind']=='hello':emit(c,dict(kind='ready',capabilities=m['capabilities'],plugin_id=m['plugin_id'],plugin_version=m['plugin_version']))
 elif m['kind']=='subscribe':
  sub=c;emit(c,dict(kind='visual',valid_for_ms=1000,state=dict(availability='current',content=dict(text='paused',png_hex=None,color=None))))
 elif m['kind']=='invoke':
  emit(sub,dict(kind='visual_renew',valid_for_ms=1000));emit(c,dict(kind='result',outcome=dict(status='success')))
"#]);
    let mut host = Host::default();
    let generation = host.activate(child, manifest, 0).unwrap();
    pump_until(&mut host, 0, |h| {
        h.state("org.example.fresh") == Some(State::Ready)
    });
    let sub = host
        .subscribe("org.example.fresh", "state", parameters(), 60000)
        .unwrap();
    pump_until(&mut host, 0, |h| {
        matches!(
            h.visual("org.example.fresh", sub),
            Some(VisualState::Current { .. })
        )
    });
    host.pump(1000).unwrap();
    assert!(matches!(
        host.visual("org.example.fresh", sub),
        Some(VisualState::Stale { .. })
    ));
    host.invoke("org.example.fresh", generation, "renew", parameters(), 6000)
        .unwrap();
    pump_until(&mut host, 1000, |h| {
        matches!(
            h.visual("org.example.fresh", sub),
            Some(VisualState::Current { .. })
        )
    });
    host.pump(2000).unwrap();
    assert!(matches!(
        host.visual("org.example.fresh", sub),
        Some(VisualState::Stale { .. })
    ));
    host.shutdown();
    pump_until(&mut host, 2000, |h| {
        h.state("org.example.fresh") == Some(State::Reaped)
    });
}

#[test]
fn renewal_without_a_first_visual_is_a_protocol_violation() {
    let manifest:Manifest=serde_json::from_value(serde_json::json!({"contract":1,"capabilities":["visual_freshness"],"id":"org.example.fresh","version":{"major":1,"minor":0,"patch":0},"host_min":1,"host_max":1,"targets":[rad_plugin_host::TARGET],"actions":[],"visuals":[{"id":"state","label":"State","schema":{"version":1,"fields":[]},"credential_slots":[]}]})).unwrap();
    let mut command = Command::new("/usr/bin/python3");
    command.args(["-c",r#"
import sys,json,struct
r=sys.stdin.buffer;w=sys.stdout.buffer
while True:
 h=r.read(4)
 if not h:break
 e=json.loads(r.read(struct.unpack('>I',h)[0]));m=e['message']
 if m['kind']=='hello':m=dict(kind='ready',capabilities=m['capabilities'],plugin_id=m['plugin_id'],plugin_version=m['plugin_version'])
 elif m['kind']=='subscribe':m=dict(kind='visual_renew',valid_for_ms=1000)
 else:continue
 b=json.dumps(dict(protocol=1,correlation=e['correlation'],message=m)).encode();w.write(struct.pack('>I',len(b))+b);w.flush()
"#]);
    let mut host = Host::default();
    host.activate(command, manifest, 0).unwrap();
    pump_until(&mut host, 0, |h| {
        h.state("org.example.fresh") == Some(State::Ready)
    });
    host.subscribe("org.example.fresh", "state", parameters(), 60000)
        .unwrap();
    let end = Instant::now() + Duration::from_millis(300);
    while Instant::now() < end {
        host.pump(0).unwrap();
        std::thread::sleep(Duration::from_millis(1));
    }
    assert_ne!(host.state("org.example.fresh"), Some(State::Ready));
    assert_eq!(
        host.last_diagnostic("org.example.fresh"),
        Some(rad_plugin_host::Diagnostic::Protocol)
    );
}
