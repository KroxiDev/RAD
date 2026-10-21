//! One editor per session bus. Registration precedes window creation, so even
//! simultaneous launches become activations of the same owner, not extra windows.
use gio::{glib, prelude::*};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
    mpsc,
};

pub struct Instance {
    context: glib::MainContext,
    loop_: glib::MainLoop,
    thread: Option<std::thread::JoinHandle<()>>,
    alive: Arc<AtomicBool>,
}
impl Instance {
    /// `None` means the existing editor was activated and this process must exit.
    /// The callback may arrive before a window exists; its owner must retain that intent.
    pub fn claim(
        app_id: &str,
        activate: impl Fn() + Send + 'static,
    ) -> Result<Option<Self>, String> {
        let context = glib::MainContext::new();
        let loop_ = glib::MainLoop::new(Some(&context), false);
        let alive = Arc::new(AtomicBool::new(true));
        let c = context.clone();
        let l = loop_.clone();
        let a = alive.clone();
        let app_id = app_id.to_owned();
        let (tx, ready) = mpsc::sync_channel(1);
        let thread = std::thread::Builder::new()
            .name("rad-editor-activation".into())
            .spawn(move || {
                let result = c.with_thread_default(|| {
                    let app =
                        gio::Application::new(Some(&app_id), gio::ApplicationFlags::FLAGS_NONE);
                    app.connect_activate(move |_| activate());
                    if let Err(error) = app.register(gio::Cancellable::NONE) {
                        let _ = tx.send(Err(error.to_string()));
                        return;
                    }
                    let Some(bus) = app.dbus_connection() else {
                        let _ = tx.send(Err(
                            "no session D-Bus connection for editor activation".into()
                        ));
                        return;
                    };
                    if app.is_remote() {
                        app.activate();
                        let result = bus
                            .flush_sync(gio::Cancellable::NONE)
                            .map(|_| false)
                            .map_err(|e| e.to_string());
                        let _ = tx.send(result);
                        return;
                    }
                    let quit = l.clone();
                    bus.connect_closed(move |_, _, _| quit.quit());
                    let _ = tx.send(Ok(true));
                    l.run();
                });
                if let Err(error) = result {
                    eprintln!("editor activation context: {error}");
                }
                a.store(false, Ordering::Release);
            })
            .map_err(|e| e.to_string())?;
        match ready.recv().map_err(|e| e.to_string())? {
            Ok(true) => Ok(Some(Self {
                context,
                loop_,
                thread: Some(thread),
                alive,
            })),
            result => {
                let _ = thread.join();
                result.map(|_| None)
            }
        }
    }
    pub fn is_alive(&self) -> bool {
        self.alive.load(Ordering::Acquire)
    }
}
impl Drop for Instance {
    fn drop(&mut self) {
        let loop_ = self.loop_.clone();
        self.context.invoke(move || loop_.quit());
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

#[cfg(feature = "gui")]
#[derive(Default)]
pub struct WindowActivation(std::sync::Mutex<WindowState>);
#[cfg(feature = "gui")]
#[derive(Default)]
struct WindowState {
    context: Option<eframe::egui::Context>,
    pending: bool,
}
#[cfg(feature = "gui")]
impl WindowActivation {
    pub fn activate(&self) {
        let mut state = self.0.lock().expect("window activation mutex");
        if let Some(context) = &state.context {
            Self::focus(context);
        } else {
            state.pending = true;
        }
    }
    pub fn bind(&self, context: &eframe::egui::Context) {
        let mut state = self.0.lock().expect("window activation mutex");
        state.context = Some(context.clone());
        if std::mem::take(&mut state.pending) {
            Self::focus(context);
        }
    }
    fn focus(context: &eframe::egui::Context) {
        use eframe::egui::ViewportCommand;
        context.send_viewport_cmd(ViewportCommand::Visible(true));
        context.send_viewport_cmd(ViewportCommand::Minimized(false));
        context.send_viewport_cmd(ViewportCommand::Focus);
        // On Wayland winit::focus_window is a no-op. Its attention request uses
        // xdg_activation_v1.activate; the compositor retains final focus policy.
        context.send_viewport_cmd(ViewportCommand::RequestUserAttention(
            eframe::egui::UserAttentionType::Informational,
        ));
        context.request_repaint();
    }
}

#[cfg(all(test, feature = "gui"))]
mod tests {
    use super::WindowActivation;
    use eframe::egui::{Context, ViewportCommand, ViewportId};
    #[test]
    fn activation_during_startup_is_delivered_when_the_window_is_ready() {
        let activation = WindowActivation::default();
        activation.activate();
        let context = Context::default();
        let output = context.run_ui(Default::default(), |ui| activation.bind(ui.ctx()));
        let commands = &output.viewport_output[&ViewportId::ROOT].commands;
        assert!(commands.iter().any(|c| matches!(c, ViewportCommand::Focus)));
        assert!(
            commands
                .iter()
                .any(|c| matches!(c, ViewportCommand::Minimized(false)))
        );
        assert!(
            commands
                .iter()
                .any(|c| matches!(c, ViewportCommand::RequestUserAttention(_)))
        );
    }
}
