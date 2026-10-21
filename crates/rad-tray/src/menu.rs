use crate::actions::{Action, Actions};
use gio::glib::{self, variant::ToVariant};
use std::collections::BTreeMap;
pub const PATH: &str = "/MenuBar";
type Properties = BTreeMap<String, glib::Variant>;
type Event = (i32, String, glib::Variant, u32);

pub fn catalog() -> BTreeMap<String, String> {
    let locale = ["LC_ALL", "LC_MESSAGES", "LANG"]
        .into_iter()
        .find_map(|key| std::env::var(key).ok().filter(|v| !v.is_empty()))
        .unwrap_or_default();
    let json = if locale.to_ascii_lowercase().starts_with("es") {
        include_str!("locales/es.json")
    } else {
        include_str!("locales/en.json")
    };
    serde_json::from_str(json).expect("tray catalog")
}

struct Menu {
    open: String,
    quit: String,
}
impl Menu {
    fn new() -> Self {
        let labels = catalog();
        Self {
            open: labels["open"].clone(),
            quit: labels["quit"].clone(),
        }
    }
    fn properties(&self, id: i32, names: &[String]) -> Properties {
        let mut properties = BTreeMap::new();
        if id == 0 {
            properties.insert("children-display".into(), "submenu".to_variant());
        }
        if let Some(label) = match id {
            1 => Some(&self.open),
            2 => Some(&self.quit),
            _ => None,
        } {
            properties.insert("label".into(), label.to_variant());
            properties.insert("enabled".into(), true.to_variant());
            properties.insert("visible".into(), true.to_variant());
        }
        properties.retain(|key, _| names.is_empty() || names.contains(key));
        properties
    }
    fn layout(&self, id: i32, depth: i32, names: &[String]) -> glib::Variant {
        let children = if id == 0 && depth != 0 {
            vec![self.layout(1, 0, names), self.layout(2, 0, names)]
        } else {
            Vec::new()
        };
        (id, self.properties(id, names), children).to_variant()
    }
}
fn event(actions: &Actions, id: i32, name: &str) {
    if name != "clicked" {
        return;
    }
    match id {
        1 => actions.request(Action::Open),
        2 => actions.request(Action::Stop),
        _ => {}
    }
}
pub fn register(
    bus: &gio::DBusConnection,
    actions: Actions,
) -> Result<gio::RegistrationId, glib::Error> {
    let menu = Menu::new();
    let info = gio::DBusNodeInfo::for_xml(include_str!("menu.xml"))?;
    bus.register_object(PATH, &info.interfaces()[0])
        .property(|_, _, _, _, name| match name {
            "Version" => 4u32.to_variant(),
            "TextDirection" => "ltr".to_variant(),
            "Status" => "normal".to_variant(),
            _ => Vec::<String>::new().to_variant(),
        })
        .method_call(move |_, _, _, _, method, params, invocation| {
            let result = match method {
                "GetLayout" => {
                    let (id, depth, names) = params
                        .get::<(i32, i32, Vec<String>)>()
                        .expect("typed layout");
                    if !(0..=2).contains(&id) {
                        invocation.return_dbus_error(
                            "com.canonical.dbusmenu.Error.InvalidMenuItem",
                            "Unknown menu item",
                        );
                        return;
                    }
                    glib::Variant::tuple_from_iter([
                        1u32.to_variant(),
                        menu.layout(id, depth, &names),
                    ])
                }
                "GetGroupProperties" => {
                    let (mut ids, names) = params
                        .get::<(Vec<i32>, Vec<String>)>()
                        .expect("typed group");
                    if ids.is_empty() {
                        ids = vec![0, 1, 2];
                    }
                    (ids.into_iter()
                        .filter(|id| (0..=2).contains(id))
                        .map(|id| (id, menu.properties(id, &names)))
                        .collect::<Vec<_>>(),)
                        .to_variant()
                }
                "GetProperty" => {
                    let (id, name) = params.get::<(i32, String)>().expect("typed property");
                    let Some(value) = menu.properties(id, &[]).remove(&name) else {
                        invocation.return_dbus_error(
                            "com.canonical.dbusmenu.Error.InvalidProperty",
                            "Unknown menu property",
                        );
                        return;
                    };
                    (value,).to_variant()
                }
                "Event" => {
                    let (id, name, _, _) = params.get::<Event>().expect("typed event");
                    event(&actions, id, &name);
                    ().to_variant()
                }
                "EventGroup" => {
                    let (events,) = params.get::<(Vec<Event>,)>().expect("typed events");
                    let mut errors = Vec::new();
                    for (id, name, _, _) in events {
                        if (0..=2).contains(&id) {
                            event(&actions, id, &name);
                        } else {
                            errors.push(id);
                        }
                    }
                    (errors,).to_variant()
                }
                "AboutToShow" => (false,).to_variant(),
                "AboutToShowGroup" => {
                    let (ids,) = params.get::<(Vec<i32>,)>().expect("typed show group");
                    (
                        Vec::<i32>::new(),
                        ids.into_iter()
                            .filter(|id| !(0..=2).contains(id))
                            .collect::<Vec<_>>(),
                    )
                        .to_variant()
                }
                _ => {
                    invocation.return_dbus_error(
                        "org.freedesktop.DBus.Error.UnknownMethod",
                        "Unknown method",
                    );
                    return;
                }
            };
            invocation.return_value(Some(&result));
        })
        .build()
}
