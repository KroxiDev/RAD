use rad_plugin_contract::*;
use rad_plugin_host::{Error, Host, InvokeContext, State};
use std::{
    process::Command,
    time::{Duration, Instant},
};
fn params() -> Parameters {
    Parameters {
        schema_version: 1,
        values: Default::default(),
    }
}
fn context() -> InvokeContext {
    InvokeContext {
        settings: None,
        control: Some(ControlEvent {
            control: Control {
                kind: "button".into(),
                index: 0,
            },
            gesture: ControlGesture::HoldStart,
        }),
    }
}
fn setup_with_start_reply(reply: bool) -> (Host, u64) {
    setup_custom(reply, false)
}
fn setup_custom(reply: bool, credentials: bool) -> (Host, u64) {
    let mut manifest: Manifest = serde_json::from_value(serde_json::json!({"contract":1,"capabilities":["controls"],"id":"org.example.hold","version":{"major":1,"minor":0,"patch":0},"host_min":1,"host_max":1,"targets":["x86_64-unknown-linux-gnu"],"actions":[{"id":"hold","label":"Hold","gestures":["press","hold"],"schema":{"version":1,"fields":[]},"credential_slots":[]}],"visuals":[]})).unwrap();
    if credentials {
        manifest
            .capabilities
            .extend(["credentials".into(), "settings".into()]);
        manifest.actions[0].credential_slots.push("secret".into());
        manifest.settings = Some(Schema {
            version: 1,
            fields: vec![Field {
                id: "flag".into(),
                label: "Flag".into(),
                required: true,
                kind: FieldKind::Boolean,
            }],
        });
    }
    let mut command = Command::new("/usr/bin/python3");
    command.args(["-I","-S","-c",r#"
import sys,json,struct
r=sys.stdin.buffer; w=sys.stdout.buffer
def emit(c,m):
 b=json.dumps(dict(protocol=1,correlation=c,message=m)).encode();w.write(struct.pack('>I',len(b))+b);w.flush()
while True:
 h=r.read(4)
 if not h:break
 e=json.loads(r.read(struct.unpack('>I',h)[0]));m=e['message'];c=e['correlation']
 if m['kind']=='hello':emit(c,dict(kind='ready',capabilities=m['capabilities'],plugin_id=m['plugin_id'],plugin_version=m['plugin_version']))
 elif m['kind']=='credentials':emit(c,dict(kind='credentials_applied',revision=m['revision']))
 elif m['kind']=='credentials_release':emit(c,dict(kind='credentials_released',revision=m['revision']))
 elif m.get('control',{}).get('gesture')=='hold_end' and sys.argv[2]=='credentials':
  good=m.get('settings',{}).get('values',{}).get('flag')==True and m.get('credential_revision')==1
  emit(c,dict(kind='result',outcome=dict(status='success') if good else dict(status='failure',code='failed')))
 elif m.get('control',{}).get('gesture')=='hold_end' or (sys.argv[1]=='reply' and m.get('control',{}).get('gesture')=='hold_start'):emit(c,dict(kind='result',outcome=dict(status='success')))
"#]);
    command
        .arg(if reply { "reply" } else { "retain" })
        .arg(if credentials { "credentials" } else { "plain" });
    let mut host = Host::default();
    let g = host.activate(command, manifest, 0).unwrap();
    let deadline = Instant::now() + Duration::from_secs(3);
    while host.state("org.example.hold") != Some(State::Ready) {
        host.pump(0).unwrap();
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(1));
    }
    (host, g)
}
#[test]
fn reserved_end_bypasses_full_ordinary_slots_and_completes() {
    let (mut host, g) = setup_with_start_reply(false);
    let (_, ticket) = host
        .invoke_hold_start(
            "org.example.hold",
            g,
            "hold",
            params(),
            (5000, None),
            context(),
        )
        .unwrap();
    for _ in 1..rad_plugin_host::MAX_PLUGIN_JOBS {
        host.invoke("org.example.hold", g, "hold", params(), 5000)
            .unwrap();
    }
    assert_eq!(
        host.invoke("org.example.hold", g, "hold", params(), 5000),
        Err(Error::Capacity)
    );
    let end = host
        .release_hold(ticket, 5000)
        .expect("End has prepaid capacity");
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        host.pump(0).unwrap();
        if let Some((c, result)) = host.take_completion() {
            assert_eq!(c, end);
            assert_eq!(result, Outcome::Success);
            break;
        }
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(1));
    }
}

fn completion(host: &mut Host, now: u64) -> (Correlation, Outcome) {
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        host.pump(now).unwrap();
        if let Some(value) = host.take_completion() {
            return value;
        }
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(1));
    }
}
#[test]
fn hold_credits_bound_start_admission_and_end_residuals() {
    let (mut host, g) = setup_with_start_reply(true);
    let mut tickets = Vec::new();
    for _ in 0..rad_plugin_host::MAX_PLUGIN_HOLDS {
        let (c, t) = host
            .invoke_hold_start(
                "org.example.hold",
                g,
                "hold",
                params(),
                (5000, None),
                context(),
            )
            .unwrap();
        tickets.push(t);
        assert_eq!(completion(&mut host, 0), (c, Outcome::Success));
    }
    assert!(matches!(
        host.invoke_hold_start(
            "org.example.hold",
            g,
            "hold",
            params(),
            (5000, None),
            context()
        ),
        Err(Error::Capacity)
    ));
    for ticket in tickets {
        host.release_hold(ticket, 5000).unwrap();
    }
    // Undrained End ACKs still occupy their reserved credits.
    assert!(matches!(
        host.invoke_hold_start(
            "org.example.hold",
            g,
            "hold",
            params(),
            (5000, None),
            context()
        ),
        Err(Error::Capacity)
    ));
    for _ in 0..rad_plugin_host::MAX_PLUGIN_HOLDS {
        assert_eq!(completion(&mut host, 0).1, Outcome::Success);
    }
    assert!(
        host.invoke_hold_start(
            "org.example.hold",
            g,
            "hold",
            params(),
            (5000, None),
            context()
        )
        .is_ok()
    );
}
#[test]
fn start_timeout_keeps_release_authority_and_foreign_host_cannot_use_it() {
    let (mut host, g) = setup_with_start_reply(false);
    let (c, t) = host
        .invoke_hold_start(
            "org.example.hold",
            g,
            "hold",
            params(),
            (1, None),
            context(),
        )
        .unwrap();
    assert_eq!(completion(&mut host, 1).0, c);
    let end = host.release_hold(t, 5000).unwrap();
    assert_eq!(completion(&mut host, 1), (end, Outcome::Success));
    let (_, foreign) = host
        .invoke_hold_start(
            "org.example.hold",
            g,
            "hold",
            params(),
            (5000, None),
            context(),
        )
        .unwrap();
    assert_eq!(
        Host::default().release_hold(foreign, 5000),
        Err(Error::Invalid)
    );
}
#[test]
fn ordinary_context_cannot_bypass_hold_reservations() {
    let (mut host, g) = setup_with_start_reply(false);
    assert_eq!(
        host.invoke_with_context(
            "org.example.hold",
            g,
            "hold",
            params(),
            (5000, None),
            context()
        ),
        Err(Error::Invalid)
    );
    let mut end = context();
    end.control.as_mut().unwrap().gesture = ControlGesture::HoldEnd;
    assert_eq!(
        host.invoke_with_context("org.example.hold", g, "hold", params(), (5000, None), end),
        Err(Error::Invalid)
    );
}

#[test]
fn release_reuses_snapshot_and_pins_credentials_after_start_completion() {
    let (mut host, g) = setup_custom(true, true);
    let id = "org.example.hold";
    host.install_credentials(
        id,
        g,
        1,
        CredentialValues::new(vec![CredentialValue {
            slot: "secret".into(),
            value: "synthetic-hold-secret".into(),
        }])
        .unwrap(),
    )
    .unwrap();
    let deadline = Instant::now() + Duration::from_secs(3);
    while host.credentials_state(id, g, 1) != Some(rad_plugin_host::CredentialState::Applied) {
        host.pump(0).unwrap();
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(1));
    }
    let mut ctx = context();
    ctx.settings = Some(Parameters {
        schema_version: 1,
        values: [("flag".into(), serde_json::json!(true))]
            .into_iter()
            .collect(),
    });
    let (start, ticket) = host
        .invoke_hold_start(id, g, "hold", params(), (5000, Some(1)), ctx)
        .unwrap();
    assert_eq!(completion(&mut host, 0), (start, Outcome::Success));
    assert_eq!(host.release_credentials(id, g, 1), Err(Error::Residual));
    let end = host.release_hold(ticket, 5000).unwrap();
    assert_eq!(host.release_credentials(id, g, 1), Err(Error::Residual));
    assert_eq!(completion(&mut host, 0), (end, Outcome::Success));
    assert!(host.release_credentials(id, g, 1).is_ok());
}
#[test]
fn a_lost_generation_never_replays_end_and_live_discard_is_refused() {
    let (mut host, g) = setup_with_start_reply(true);
    let (_, ticket) = host
        .invoke_hold_start(
            "org.example.hold",
            g,
            "hold",
            params(),
            (5000, None),
            context(),
        )
        .unwrap();
    assert_eq!(host.discard_hold(ticket), Err(Error::Residual));
    let (_, ticket) = host
        .invoke_hold_start(
            "org.example.hold",
            g,
            "hold",
            params(),
            (5000, None),
            context(),
        )
        .unwrap();
    host.shutdown();
    assert_eq!(host.release_hold(ticket, 5000), Err(Error::Unavailable));
}
