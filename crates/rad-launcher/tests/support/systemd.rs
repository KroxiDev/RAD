//! Fake `org.freedesktop.systemd1` for tests: records each `StartTransientUnit`.
use gio::prelude::*;
const XML: &str = r#"<node><interface name="org.freedesktop.systemd1.Manager">
<method name="StartTransientUnit">
<arg type="s" direction="in"/><arg type="s" direction="in"/>
<arg type="a(sv)" direction="in"/><arg type="a(sa(sv))" direction="in"/>
<arg type="o" direction="out"/></method></interface></node>"#;
type Call = (
    String,
    String,
    Vec<(String, glib::Variant)>,
    Vec<(String, Vec<(String, glib::Variant)>)>,
);
fn main() {
    let log = std::path::PathBuf::from(std::env::var_os("RAD_FAKE_SYSTEMD_LOG").expect("log"));
    let info = gio::DBusNodeInfo::for_xml(XML).unwrap();
    let interface = info
        .lookup_interface("org.freedesktop.systemd1.Manager")
        .unwrap();
    let main_loop = glib::MainLoop::new(None, false);
    let quit = main_loop.clone();
    let _owner = gio::bus_own_name(
        gio::BusType::Session,
        "org.freedesktop.systemd1",
        gio::BusNameOwnerFlags::NONE,
        move |connection, _| {
            let log = log.clone();
            connection
                .register_object("/org/freedesktop/systemd1", &interface)
                .method_call(move |_, _, _, _, method, params, invocation| {
                    assert_eq!(method, "StartTransientUnit");
                    let (name, mode, props, aux) = params.get::<Call>().unwrap();
                    let pids = props
                        .iter()
                        .find(|(k, _)| k == "PIDs")
                        .and_then(|(_, v)| v.get::<Vec<u32>>())
                        .unwrap_or_default();
                    let line =
                        serde_json::json!({"name":name,"mode":mode,"pids":pids,"aux":aux.len()});
                    let tmp = log.with_extension("tmp");
                    std::fs::write(&tmp, format!("{line}\n")).unwrap();
                    std::fs::rename(&tmp, &log).unwrap();
                    let job =
                        glib::variant::ObjectPath::try_from("/org/freedesktop/systemd1/job/1")
                            .unwrap();
                    invocation.return_value(Some(&(job,).to_variant()));
                })
                .build()
                .unwrap();
        },
        |_, _| {},
        move |_, _| quit.quit(),
    );
    main_loop.run();
}
