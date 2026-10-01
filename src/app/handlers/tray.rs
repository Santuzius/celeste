//! Tray-related message handlers: lifecycle handshake, theme
//! propagation, and click actions.

use std::sync::atomic::Ordering;

use iced::{theme as iced_theme, window, Task};
use tokio::sync::mpsc;

use crate::infrastructure::tray::{TrayAction, TrayUpdate};

use super::super::{main_window_settings, CelesteApp, Message};

impl CelesteApp {
    /// Handle [`Message::TrayReady`] — store the sender, push the
    /// cached colour-scheme so the rasteriser flips tone immediately,
    /// and push an initial status snapshot so the icon reflects
    /// reality without waiting for the next state change.
    pub(in crate::app) fn handle_tray_ready(
        &mut self,
        tx: mpsc::Sender<TrayUpdate>,
    ) -> Task<Message> {
        self.tray_tx = Some(tx);
        self.last_tray_status = None;
        self.push_tray_theme();
        self.push_tray_status();
        Task::none()
    }

    /// Handle [`Message::SystemThemeChanged`] — cache the new value
    /// and forward it to the tray so the next paint uses the matching
    /// glyph tone.
    ///
    /// Iced's runtime races two paths: `iced::system::theme()`
    /// (one-shot `GetTheme`) returns whatever value mundy seeded into
    /// the cache during its 200 ms `once_blocking` startup query, while
    /// the `theme_changes()` subscription streams the freedesktop
    /// portal's broadcasts. On Wayland sessions where the portal is a
    /// hair slow, those two arrive in either order, and a stale
    /// `Mode::None` from the one-shot can land *after* a real `Light`
    /// or `Dark` from the subscription. Treat `None` as "no fresh
    /// information" so the most recent concrete value sticks.
    pub(in crate::app) fn handle_system_theme_changed(
        &mut self,
        mode: iced_theme::Mode,
    ) -> Task<Message> {
        if mode == self.system_theme {
            return Task::none();
        }
        if mode == iced_theme::Mode::None {
            return Task::none();
        }
        self.system_theme = mode;
        self.push_tray_theme();
        Task::none()
    }

    /// Handle [`Message::TrayClick`] — translate the user-action into
    /// a window-lifecycle change. Open allocates a fresh surface when
    /// none is live (the daemon starts windowless and stays that way
    /// after Hide); Hide destroys the surface entirely so the taskbar
    /// entry disappears, matching Signal / Telegram-style hide-to-tray.
    pub(in crate::app) fn handle_tray_click(&mut self, action: TrayAction) -> Task<Message> {
        match action {
            TrayAction::Open => {
                if let Some(id) = self.window_id {
                    // Window already alive — un-minimise and pull it
                    // forward. `set_mode(Windowed)` is the safety net
                    // for X11 which honours `set_visible`; on Wayland
                    // `minimize(false)` is the actual restore primitive.
                    return Task::batch([
                        window::set_mode(id, window::Mode::Windowed),
                        window::minimize(id, false),
                        window::gain_focus(id),
                    ]);
                }
                // No live window — open one. Defer the focus call until
                // the window has finished opening; some compositors
                // ignore activation requests targeting a not-yet-mapped
                // surface.
                let (id, opened) = window::open(main_window_settings());
                self.window_id = Some(id);
                opened.then(|id| window::gain_focus(id))
            }
            TrayAction::Hide => {
                // Actually destroy the window — `set_visible(false)` is
                // a no-op on Wayland once mapped, so the only way to
                // keep the taskbar entry from sticking around is to
                // close the surface. The daemon stays running with no
                // windows; tray Open spawns a fresh one.
                //
                // Known caveat: iced 0.14's `Clipboard` (see
                // `iced_winit/src/clipboard.rs`) latches onto the first
                // window's `Arc<Window>` and never reconnects, so
                // repeated open/close cycles can — on some Wayland
                // compositors — leak a stale `wl_data_offer` and
                // wedge the event loop ("not a valid new object id …
                // message data_offer"). Not yet reproducible; revisit
                // if it resurfaces, otherwise leave for an iced fix.
                if let Some(id) = self.window_id.take() {
                    return window::close(id);
                }
                Task::none()
            }
            TrayAction::Quit => self.handle_quit(),
        }
    }

    /// Handle [`Message::WindowClosed`] — drop the cached id when a
    /// window is destroyed so the next "Open Celeste" allocates a
    /// fresh one. Fires for both our own `window::close` from `Hide`
    /// and external destroys (X button, Alt-F4, compositor-driven).
    pub(in crate::app) fn handle_window_closed(&mut self, id: window::Id) -> Task<Message> {
        if self.window_id == Some(id) {
            self.window_id = None;
        }
        Task::none()
    }

    /// Hard-exit the process. We can't wait for in-flight FFI calls
    /// (librclone's RPC surface has no cancel handle, so a mid-listing
    /// Google Drive pass would block for minutes); set every cancel
    /// flag as a courtesy for any non-FFI work, then bail. The OS will
    /// reap the threads and the GUI window when the process exits.
    pub(in crate::app) fn handle_quit(&mut self) -> Task<Message> {
        for flag in self.cancel_flags.values() {
            flag.store(true, Ordering::Release);
        }
        std::process::exit(0);
    }
}
