use gio::{
    Cancellable, DBusCallFlags, DBusConnection, DBusConnectionFlags, DBusError, DBusMessage,
    DBusMessageFlags, DBusSendMessageFlags, IOErrorEnum, prelude::CancellableExt,
};
use glib::{ControlFlow, MainContext, Priority, Variant, VariantTy, prelude::ToVariant};
use rad_plugin_sdk::FailureCode;
use std::{
    cell::RefCell,
    rc::Rc,
    sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError, Weak},
    time::{Duration, Instant},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Failure {
    Unavailable,
    TimedOut,
    Failed,
    Incompatible,
}

impl Failure {
    pub(crate) fn code(self) -> FailureCode {
        match self {
            Self::Unavailable => FailureCode::Unavailable,
            Self::TimedOut => FailureCode::TimedOut,
            Self::Failed => FailureCode::Failed,
            Self::Incompatible => FailureCode::Incompatible,
        }
    }

    fn of(error: &glib::Error) -> Self {
        if error.matches(IOErrorEnum::TimedOut)
            || error.matches(DBusError::TimedOut)
            || error.matches(DBusError::Timeout)
        {
            Self::TimedOut
        } else if error.matches(IOErrorEnum::Closed)
            || error.matches(DBusError::Disconnected)
            || error.matches(DBusError::ServiceUnknown)
            || error.matches(DBusError::NameHasNoOwner)
            || error.matches(DBusError::NoReply)
        {
            Self::Unavailable
        } else {
            Self::Failed
        }
    }
}

pub(crate) struct Method<'a> {
    pub(crate) destination: &'a str,
    pub(crate) path: &'a str,
    pub(crate) interface: &'a str,
    pub(crate) name: &'a str,
}

#[derive(Default)]
struct Link {
    open: Weak<Connection>,
    connecting: bool,
}

pub(crate) struct Connection(DBusConnection);

impl std::ops::Deref for Connection {
    type Target = DBusConnection;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl Drop for Connection {
    fn drop(&mut self) {
        let _ = self.0.close_sync(None::<&Cancellable>);
    }
}

pub(crate) struct Bus {
    address: Option<String>,
    link: Mutex<Link>,
    attempted: Condvar,
}

struct Attempt<'a>(&'a Bus);

impl Drop for Attempt<'_> {
    fn drop(&mut self) {
        self.0.locked().connecting = false;
        self.0.attempted.notify_all();
    }
}

impl Bus {
    pub(crate) fn new(address: Option<String>) -> Self {
        Self {
            address,
            link: Mutex::new(Link::default()),
            attempted: Condvar::new(),
        }
    }

    fn locked(&self) -> MutexGuard<'_, Link> {
        self.link.lock().unwrap_or_else(PoisonError::into_inner)
    }

    pub(crate) fn connect(
        &self,
        context: &MainContext,
        deadline: Instant,
    ) -> Result<DBusConnection, Failure> {
        let address = self.address.as_deref().ok_or(Failure::Unavailable)?;
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return Err(Failure::TimedOut);
        }
        context
            .with_thread_default(|| connect_within(address, context, left))
            .map_err(|_| Failure::Failed)?
    }

    pub(crate) fn connection(&self, deadline: Instant) -> Result<Arc<Connection>, Failure> {
        let mut link = self.locked();
        loop {
            if let Some(open) = link.open.upgrade().filter(|c| !c.is_closed()) {
                return Ok(open);
            }
            if !link.connecting {
                break;
            }
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return Err(Failure::TimedOut);
            }
            link = self
                .attempted
                .wait_timeout(link, left)
                .unwrap_or_else(PoisonError::into_inner)
                .0;
        }
        link.connecting = true;
        drop(link);
        let _attempt = Attempt(self);
        let connection = Arc::new(Connection(self.connect(&MainContext::new(), deadline)?));
        self.locked().open = Arc::downgrade(&connection);
        Ok(connection)
    }

    pub(crate) fn call(
        &self,
        method: &Method,
        parameters: Option<&Variant>,
        reply_type: &str,
        deadline: Instant,
    ) -> Result<Variant, Failure> {
        self.connection(deadline)?
            .call(method, parameters, reply_type, deadline)
    }

    pub(crate) fn player_names(&self, deadline: Instant) -> Result<Vec<String>, Failure> {
        self.connection(deadline)?.player_names(deadline)
    }
}

impl Connection {
    fn call(
        &self,
        method: &Method,
        parameters: Option<&Variant>,
        reply_type: &str,
        deadline: Instant,
    ) -> Result<Variant, Failure> {
        self.call_sync(
            Some(method.destination),
            method.path,
            method.interface,
            method.name,
            parameters,
            Some(VariantTy::new(reply_type).map_err(|_| Failure::Failed)?),
            DBusCallFlags::NO_AUTO_START,
            timeout(deadline)?,
            None::<&Cancellable>,
        )
        .map_err(|error| Failure::of(&error))
    }

    pub(crate) fn call_with_sender(
        &self,
        method: &Method,
        parameters: &Variant,
        deadline: Instant,
    ) -> Result<(glib::GString, Variant), Failure> {
        let request = DBusMessage::new_method_call(
            Some(method.destination),
            method.path,
            Some(method.interface),
            method.name,
        );
        request.set_flags(DBusMessageFlags::NO_AUTO_START);
        request.set_body(parameters);
        let (reply, _) = self
            .send_message_with_reply_sync(
                &request,
                DBusSendMessageFlags::NONE,
                timeout(deadline)?,
                None::<&Cancellable>,
            )
            .map_err(|error| Failure::of(&error))?;
        if let Some(error) = reply.error_name() {
            return Err(Failure::of(&DBusError::new_for_dbus_error(&error, "")));
        }
        Ok((
            reply.sender().ok_or(Failure::Failed)?,
            reply.body().ok_or(Failure::Failed)?,
        ))
    }

    pub(crate) fn player_names(&self, deadline: Instant) -> Result<Vec<String>, Failure> {
        let reply = self.call(&daemon_method("ListNames"), None, "(as)", deadline)?;
        let (mut names,) = reply.get::<(Vec<String>,)>().ok_or(Failure::Failed)?;
        names.retain(|name| crate::names::app_identity(name).is_some());
        Ok(names)
    }

    pub(crate) fn name_owner(&self, name: &str, deadline: Instant) -> Result<String, Failure> {
        let reply = self.call(
            &daemon_method("GetNameOwner"),
            Some(&(name,).to_variant()),
            "(s)",
            deadline,
        )?;
        reply
            .get::<(String,)>()
            .map(|(owner,)| owner)
            .ok_or(Failure::Failed)
    }
}

fn daemon_method(name: &str) -> Method<'_> {
    Method {
        destination: "org.freedesktop.DBus",
        path: "/org/freedesktop/DBus",
        interface: "org.freedesktop.DBus",
        name,
    }
}

fn timeout(deadline: Instant) -> Result<i32, Failure> {
    let left = deadline.saturating_duration_since(Instant::now());
    if left.is_zero() {
        return Err(Failure::TimedOut);
    }
    Ok(i32::try_from(left.as_millis().max(1)).unwrap_or(i32::MAX))
}

fn connect_within(
    address: &str,
    context: &MainContext,
    limit: Duration,
) -> Result<DBusConnection, Failure> {
    let cancellable = Cancellable::new();
    let outcome = Rc::new(RefCell::new(None));
    let made = Rc::clone(&outcome);
    DBusConnection::for_address(
        address,
        DBusConnectionFlags::AUTHENTICATION_CLIENT | DBusConnectionFlags::MESSAGE_BUS_CONNECTION,
        None,
        Some(&cancellable),
        move |result| *made.borrow_mut() = Some(result),
    );
    let timer = glib::timeout_source_new(limit, Some("media-connect-limit"), Priority::DEFAULT, {
        let cancellable = cancellable.clone();
        move || {
            cancellable.cancel();
            ControlFlow::Break
        }
    });
    timer.attach(Some(context));
    let result = loop {
        if let Some(result) = outcome.borrow_mut().take() {
            break result;
        }
        context.iteration(true);
    };
    timer.destroy();
    result.map_err(|error| {
        if error.matches(IOErrorEnum::Cancelled) {
            Failure::TimedOut
        } else {
            Failure::Unavailable
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn of(domain_error: glib::Error) -> Failure {
        Failure::of(&domain_error)
    }

    #[test]
    fn errors_of_the_bus_become_the_failure_the_plugin_reports() {
        let io = |kind| of(glib::Error::new(kind, "test"));
        let dbus = |kind| of(glib::Error::new(kind, "test"));
        assert_eq!(io(IOErrorEnum::TimedOut), Failure::TimedOut);
        assert_eq!(dbus(DBusError::TimedOut), Failure::TimedOut);
        assert_eq!(dbus(DBusError::Timeout), Failure::TimedOut);
        assert_eq!(dbus(DBusError::ServiceUnknown), Failure::Unavailable);
        assert_eq!(dbus(DBusError::NameHasNoOwner), Failure::Unavailable);
        assert_eq!(dbus(DBusError::NoReply), Failure::Unavailable);
        assert_eq!(dbus(DBusError::Disconnected), Failure::Unavailable);
        assert_eq!(io(IOErrorEnum::Closed), Failure::Unavailable);
        assert_eq!(dbus(DBusError::UnknownMethod), Failure::Failed);
        assert_eq!(dbus(DBusError::AccessDenied), Failure::Failed);
        assert_eq!(io(IOErrorEnum::InvalidArgument), Failure::Failed);
    }
}
