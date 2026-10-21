//! Session-only desktop companion. No GUI, GPU, runtime state or device access.
mod actions;
mod menu;
use actions::{Action, Actions};
use gio::glib::{self, variant::ToVariant};
const NAME: &str = "io.github.KroxiDev.RAD.Tray";
const WATCHER: &str = "org.kde.StatusNotifierWatcher";
type Pixmaps = Vec<(i32, i32, Vec<u8>)>;
fn pixmaps() -> Pixmaps {
    let image = image::load_from_memory_with_format(
        include_bytes!("../../../packaging/rad.png"),
        image::ImageFormat::Png,
    )
    .expect("packaged RAD PNG");
    [16, 22, 32, 48]
        .into_iter()
        .map(|size| {
            let rgba = image
                .resize_exact(size, size, image::imageops::FilterType::Lanczos3)
                .into_rgba8();
            // SNI uses network-order ARGB, not the RGBA byte order of a PNG decoder.
            let argb = rgba
                .pixels()
                .flat_map(|p| [p[3], p[0], p[1], p[2]])
                .collect();
            (size as i32, size as i32, argb)
        })
        .collect()
}
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let bus = gio::bus_get_sync(gio::BusType::Session, gio::Cancellable::NONE)?;
    let editor = std::env::current_exe()?
        .parent()
        .ok_or("no tray directory")?
        .join("rad-editor");
    let actions = Actions::new(bus.clone(), editor);
    let info = gio::DBusNodeInfo::for_xml(include_str!("item.xml"))?;
    let icons = pixmaps();
    let title = menu::catalog()["title"].clone();
    let item = bus
        .register_object("/StatusNotifierItem", &info.interfaces()[0])
        .property(move |_, _, _, _, name| match name {
            "Category" => "ApplicationStatus".to_variant(),
            "Id" => "RAD".to_variant(),
            "Title" => title.to_variant(),
            "Status" => "Active".to_variant(),
            "WindowId" => 0u32.to_variant(),
            "IconPixmap" => icons.to_variant(),
            "OverlayIconPixmap" | "AttentionIconPixmap" => Pixmaps::new().to_variant(),
            "ItemIsMenu" => false.to_variant(),
            "Menu" => glib::variant::ObjectPath::try_from(menu::PATH)
                .expect("menu object path")
                .to_variant(),
            "ToolTip" => ("", Pixmaps::new(), title.as_str(), "").to_variant(),
            _ => "".to_variant(),
        })
        .method_call({
            let actions = actions.clone();
            move |_, _, _, _, method, _, invocation| {
                if matches!(method, "Activate" | "SecondaryActivate") {
                    actions.request(Action::Open);
                }
                invocation.return_value(None);
            }
        })
        .build()?;
    let menu = menu::register(&bus, actions)?;
    let reply = bus.call_sync(
        Some("org.freedesktop.DBus"),
        "/org/freedesktop/DBus",
        "org.freedesktop.DBus",
        "RequestName",
        Some(&(NAME, 4u32).to_variant()),
        None,
        gio::DBusCallFlags::NONE,
        2000,
        gio::Cancellable::NONE,
    )?;
    if reply.get::<(u32,)>() != Some((1,)) {
        return Ok(());
    }
    let loop_ = glib::MainLoop::new(None, false);
    let quit = loop_.clone();
    bus.connect_closed(move |_, _, _| quit.quit());
    let watcher = gio::bus_watch_name_on_connection(
        &bus,
        WATCHER,
        gio::BusNameWatcherFlags::NONE,
        |connection, _, owner| {
            let service = connection.unique_name().expect("message-bus unique name");
            connection.call(
                Some(owner),
                "/StatusNotifierWatcher",
                WATCHER,
                "RegisterStatusNotifierItem",
                Some(&(service.as_str(),).to_variant()),
                None,
                gio::DBusCallFlags::NONE,
                2000,
                gio::Cancellable::NONE,
                |result| {
                    if let Err(error) = result {
                        eprintln!("RAD tray registration: {error}");
                    }
                },
            );
        },
        |_, _| {},
    );
    loop_.run();
    gio::bus_unwatch_name(watcher);
    bus.unregister_object(item)?;
    bus.unregister_object(menu)?;
    Ok(())
}
