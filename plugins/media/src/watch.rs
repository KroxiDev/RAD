use crate::{Timing, bus::Bus, cover::Converter, monitor::Monitor, visual::Reader, wake::Wake};
use rad_plugin_sdk::{MAX_SUBSCRIPTIONS, Subscription, SubscriptionId, VisualState, is_label};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex, MutexGuard, PoisonError, Weak},
    thread,
};

const MAX_READERS: usize = MAX_SUBSCRIPTIONS;

fn locked<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

#[derive(Default)]
struct Buttons {
    subscriptions: Vec<Subscription>,
    shown: Option<VisualState>,
    interval: Arc<()>,
}

struct Group {
    buttons: Arc<Mutex<Buttons>>,
    wake: Arc<Wake>,
    served: bool,
}

impl Group {
    fn new() -> Self {
        Self {
            buttons: Arc::default(),
            wake: Arc::new(Wake::new()),
            served: false,
        }
    }

    fn has_buttons(&self) -> bool {
        !locked(&self.buttons).subscriptions.is_empty()
    }
}

struct Job {
    app: String,
    buttons: Arc<Mutex<Buttons>>,
    wake: Arc<Wake>,
    reader: Reader,
}

impl Job {
    fn of(app: &str, group: &Group, converter: Arc<Converter>) -> Self {
        group.wake.now();
        Self {
            app: app.to_owned(),
            buttons: Arc::clone(&group.buttons),
            wake: Arc::clone(&group.wake),
            reader: Reader::new(app.to_owned(), converter),
        }
    }
}

fn publish(buttons: &Mutex<Buttons>, interval: &Arc<()>, state: VisualState) {
    let mut buttons = locked(buttons);
    if !Arc::ptr_eq(&buttons.interval, interval) {
        return;
    }
    {
        for subscription in &buttons.subscriptions {
            subscription.update(state.clone());
        }
        buttons.shown = Some(state);
    }
}

fn serve(watch: &Weak<Watch>, bus: &Bus, timing: &Timing, mut job: Job) {
    loop {
        while job.wake.wait(
            timing.settle,
            timing.max_settle,
            timing.recheck,
            timing.linger,
        ) {
            let (shown, interval) = {
                let buttons = locked(&job.buttons);
                (buttons.shown.clone(), Arc::clone(&buttons.interval))
            };
            let state = job.reader.read(bus, timing, shown.as_ref());
            publish(&job.buttons, &interval, state);
        }
        match watch.upgrade().and_then(|watch| watch.next_job(job)) {
            Some(next) => job = next,
            None => return,
        }
    }
}

struct State {
    groups: HashMap<String, Group>,
    monitor: Option<Monitor>,
    readers: usize,
    converter: Option<Arc<Converter>>,
}

impl State {
    fn take_waiting(&mut self) -> Option<Job> {
        let (app, group) = self
            .groups
            .iter_mut()
            .find(|(_, group)| !group.served && group.has_buttons())?;
        group.served = true;
        Some(Job::of(
            app,
            group,
            Arc::clone(
                self.converter
                    .get_or_insert_with(|| Arc::new(Converter::default())),
            ),
        ))
    }

    fn stop_if_idle(&mut self) {
        if self.groups.is_empty() {
            self.converter = None;
        }
        if self.groups.is_empty()
            && let Some(monitor) = &self.monitor
        {
            monitor.stop();
        }
    }

    fn give_up(&mut self, app: &str) {
        if let Some(group) = self.groups.remove(app) {
            for subscription in &locked(&group.buttons).subscriptions {
                subscription.update(VisualState::Unavailable);
            }
        }
    }
}

pub(crate) struct Watch {
    bus: Arc<Bus>,
    timing: Timing,
    state: Mutex<State>,
}

impl Watch {
    #[cfg(feature = "fixture-tests")]
    pub(crate) fn with_fixture_converter(
        bus: Arc<Bus>,
        timing: Timing,
        converter: Arc<Converter>,
    ) -> Arc<Self> {
        let watch = Self::new(bus, timing);
        locked(&watch.state).converter = Some(converter);
        watch
    }
    pub(crate) fn new(bus: Arc<Bus>, timing: Timing) -> Arc<Self> {
        Arc::new(Self {
            bus,
            timing,
            state: Mutex::new(State {
                groups: HashMap::new(),
                monitor: None,
                readers: 0,
                converter: None,
            }),
        })
    }

    fn changed(&self) {
        for group in locked(&self.state).groups.values() {
            group.wake.changed();
        }
    }

    fn retire_monitor(&self) -> bool {
        let mut state = locked(&self.state);
        let ends = state.groups.is_empty();
        if ends {
            state.monitor = None;
        }
        ends
    }

    fn start_monitor(self: &Arc<Self>) -> Option<Monitor> {
        let (changing, retiring) = (Arc::downgrade(self), Arc::downgrade(self));
        let changed = Arc::new(move || {
            if let Some(watch) = Weak::upgrade(&changing) {
                watch.changed();
            }
        });
        let retire = Arc::new(move || Weak::upgrade(&retiring).is_none_or(|w| w.retire_monitor()));
        Monitor::start(Arc::clone(&self.bus), self.timing.clone(), changed, retire)
    }

    fn next_job(&self, job: Job) -> Option<Job> {
        let mut state = locked(&self.state);
        let own = state
            .groups
            .get(&job.app)
            .filter(|group| Arc::ptr_eq(&group.wake, &job.wake));
        if own.is_some_and(Group::has_buttons) {
            job.wake.resume();
            return Some(job);
        }
        if own.is_some() {
            state.groups.remove(&job.app);
        }
        let next = state.take_waiting();
        if next.is_none() {
            state.readers -= 1;
            state.stop_if_idle();
        }
        next
    }

    fn spawn_reader(self: &Arc<Self>, job: Job) -> bool {
        let (watch, bus, timing) = (
            Arc::downgrade(self),
            Arc::clone(&self.bus),
            self.timing.clone(),
        );
        thread::Builder::new()
            .name("media-reader".into())
            .spawn(move || serve(&watch, &bus, &timing, job))
            .is_ok()
    }

    fn start_readers(self: &Arc<Self>, state: &mut State) {
        while state.readers < MAX_READERS
            && let Some(job) = state.take_waiting()
        {
            let app = job.app.clone();
            if self.spawn_reader(job) {
                state.readers += 1;
            } else {
                state.give_up(&app);
            }
        }
    }

    pub(crate) fn add(self: &Arc<Self>, subscription: Subscription) {
        let app = match subscription.parameters().text("player") {
            Some(app) if subscription.source() == "now_playing" && is_label(app) => app.to_owned(),
            _ => return subscription.update(VisualState::Unavailable),
        };
        let mut state = locked(&self.state);
        if state.monitor.is_none() {
            state.monitor = self.start_monitor();
        }
        if state.monitor.is_none() {
            return subscription.update(VisualState::Unavailable);
        }
        let group = state.groups.entry(app).or_insert_with(Group::new);
        group.wake.resume();
        {
            let mut buttons = locked(&group.buttons);
            if buttons.subscriptions.is_empty() {
                buttons.shown = None;
                buttons.interval = Arc::new(());
                group.wake.now();
            }
            group.wake.now();
            buttons.subscriptions.push(subscription);
        }
        self.start_readers(&mut state);
    }

    pub(crate) fn remove(&self, id: SubscriptionId) {
        let mut state = locked(&self.state);
        state.groups.retain(|_, group| {
            let mut buttons = locked(&group.buttons);
            buttons.subscriptions.retain(|s| s.id() != id);
            if !buttons.subscriptions.is_empty() {
                return true;
            }
            group.wake.stop();
            group.served
        });
        state.stop_if_idle();
    }
}
