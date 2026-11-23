use crate::{Timing, bus::Bus, names::MPRIS_PREFIX, wake::Wake};
use gio::{DBusConnection, DBusSignalFlags, SignalSubscription};
use glib::{MainContext, prelude::*};
use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::Instant,
};

pub(crate) type Changed = Arc<dyn Fn() + Send + Sync + 'static>;

pub(crate) type Retire = Arc<dyn Fn() -> bool + Send + Sync + 'static>;

pub(crate) struct Monitor {
    control: Arc<Control>,
}

struct Control {
    wake: Wake,
    context: MainContext,
    ended: AtomicBool,
}

impl Control {
    fn must_end(&self, retire: &Retire) -> bool {
        if self.ended.load(Ordering::Acquire) {
            return true;
        }
        if !self.wake.is_stopped() {
            return false;
        }
        if retire() {
            self.ended.store(true, Ordering::Release);
            return true;
        }
        self.wake.resume();
        false
    }
}

impl Monitor {
    pub(crate) fn start(
        bus: Arc<Bus>,
        timing: Timing,
        changed: Changed,
        retire: Retire,
    ) -> Option<Self> {
        let control = Arc::new(Control {
            wake: Wake::new(),
            context: MainContext::new(),
            ended: AtomicBool::new(false),
        });
        let thread_control = Arc::clone(&control);
        thread::Builder::new()
            .name("media-monitor".into())
            .spawn(move || {
                let _ = thread_control.context.with_thread_default(|| {
                    serve(&bus, &timing, &thread_control, &changed, &retire);
                });
            })
            .ok()?;
        Some(Self { control })
    }

    pub(crate) fn stop(&self) {
        self.control.wake.stop();
        self.control.context.wakeup();
    }
}

fn serve(bus: &Bus, timing: &Timing, control: &Control, changed: &Changed, retire: &Retire) {
    while !control.must_end(retire) {
        let deadline = Instant::now() + timing.refresh_deadline;
        if let Ok(connection) = bus.connect(&control.context, deadline) {
            listen(&connection, control, changed, retire);
        }
        if control.wake.is_stopped() {
            continue;
        }
        changed();
        control.wake.sleep(timing.retry_wait);
    }
}

fn listen(connection: &DBusConnection, control: &Control, changed: &Changed, retire: &Retire) {
    let closed = Arc::new(AtomicBool::new(false));
    let on_close = {
        let (closed, context) = (Arc::clone(&closed), control.context.clone());
        connection.connect_closed(move |_, _, _| {
            closed.store(true, Ordering::Release);
            context.wakeup();
        })
    };
    let _signals = subscribe(connection, changed);
    changed();
    while !control.must_end(retire) && !closed.load(Ordering::Acquire) && !connection.is_closed() {
        control.context.iteration(true);
    }
    connection.disconnect(on_close);
}

fn subscribe(connection: &DBusConnection, changed: &Changed) -> [SignalSubscription; 2] {
    let on_owner = {
        let changed = Arc::clone(changed);
        move |signal: gio::DBusSignalRef| {
            let mpris = signal
                .parameters
                .get::<(String, String, String)>()
                .is_some_and(|(name, _, _)| name.starts_with(MPRIS_PREFIX));
            if mpris {
                changed();
            }
        }
    };
    let on_properties = {
        let changed = Arc::clone(changed);
        move |_: gio::DBusSignalRef| changed()
    };
    [
        connection.subscribe_to_signal(
            Some("org.freedesktop.DBus"),
            Some("org.freedesktop.DBus"),
            Some("NameOwnerChanged"),
            Some("/org/freedesktop/DBus"),
            None,
            DBusSignalFlags::NONE,
            on_owner,
        ),
        connection.subscribe_to_signal(
            None,
            Some("org.freedesktop.DBus.Properties"),
            Some("PropertiesChanged"),
            Some("/org/mpris/MediaPlayer2"),
            Some("org.mpris.MediaPlayer2.Player"),
            DBusSignalFlags::NONE,
            on_properties,
        ),
    ]
}
