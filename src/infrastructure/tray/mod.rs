//! StatusNotifier (KDE/freedesktop) tray adapter. Spawns a `ksni` service
//! inside an Iced subscription, translating menu clicks and left-clicks
//! into [`TraySignal`]s and accepting state snapshots back from the app
//! to drive the icon and tooltip.
//!
//! The app never talks to `ksni` directly — it batches
//! [`subscription`] into its own subscription list, hands the returned
//! `Sender<TrayUpdate>` to itself on receipt of [`TraySignal::Ready`],
//! and pushes [`TrayUpdate::Status`] whenever its aggregate sync state
//! changes plus [`TrayUpdate::Theme`] whenever the iced runtime
//! reports a fresh `system::theme()` (which iced derives from the
//! freedesktop colour-scheme portal via `mundy`).
//!
//! If the running desktop exposes no StatusNotifier host (plain GNOME
//! without an extension, a session missing a D-Bus broker, …) the
//! service fails to spawn; we log once and keep the subscription alive
//! so the app's `TrayReady`-gated push path is a no-op instead of a
//! back-pressure source.

#[cfg(not(target_os = "android"))]
mod icons;

use std::{
    collections::{HashMap, HashSet},
    time::{Duration, Instant},
};

#[cfg(not(target_os = "android"))]
use iced::stream;
use iced::{theme, Subscription};
#[cfg(not(target_os = "android"))]
use ksni::{
    menu::{StandardItem, TextDirection},
    Icon, MenuItem, ToolTip, TrayMethods,
};
use tokio::sync::mpsc;

use crate::domain::{
    remote::{Remote, RemoteId},
    run_state::{AppState, RunState},
};

#[cfg(not(target_os = "android"))]
use self::icons::IconSet;

/// One user-visible action surfaced from the tray. The app maps each
/// of these to a window-lifecycle command.
#[derive(Clone, Copy, Debug)]
pub enum TrayAction {
    /// Show the main window (left-click or "Open Celeste" menu entry).
    Open,
    /// Hide the main window; syncing continues in the background.
    Hide,
    /// Close the main window and exit the process.
    Quit,
}

/// Aggregate sync state the app wants the tray icon and tooltip to
/// reflect. Recomputed by the app after every state change and pushed
/// via the sender handed over in [`TraySignal::Ready`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TrayStatus {
    /// Initial state before remotes have been loaded.
    Loading,
    /// No remotes are configured.
    Disconnected,
    /// Every remote is disabled.
    Paused,
    /// These remotes need the user to reauthenticate before syncing
    /// can resume.
    AuthNeeded { remotes: Vec<String> },
    /// At least one remote is actively syncing.
    Syncing { count: usize },
    /// These remotes ended their last pass with errors / warnings or
    /// are backing off after provider rate-limiting.
    Warning { remotes: Vec<String> },
    /// All enabled remotes are idle and up to date. The age is rounded
    /// down to whole minutes so the tooltip (and the D-Bus traffic it
    /// causes) changes at most once a minute.
    Done { last_sync_ago: Option<Duration> },
}

/// What the app pushes back into the tray task: either a refreshed
/// aggregate sync status, or a colour-scheme change picked up from
/// the iced runtime. Two senders would have done the job too, but a
/// single typed channel keeps the handshake (one [`TraySignal::Ready`])
/// and the back-pressure semantics straightforward.
#[derive(Debug)]
pub enum TrayUpdate {
    Status(TrayStatus),
    Theme(theme::Mode),
}

/// Everything the tray subscription emits into the Iced runtime.
#[derive(Debug)]
pub enum TraySignal {
    /// Delivered once at startup. The attached sender is how the app
    /// pushes fresh [`TrayUpdate`]s back to the tray.
    Ready(mpsc::Sender<TrayUpdate>),
    /// A tray action the user triggered.
    Action(TrayAction),
}

/// Build the Iced subscription that owns the ksni service. Batch this
/// alongside the app's existing subscriptions.
#[cfg(not(target_os = "android"))]
pub fn subscription() -> Subscription<TraySignal> {
    Subscription::run(|| {
        stream::channel(32, async move |mut output| {
            use iced::futures::SinkExt;

            let (click_tx, mut click_rx) = mpsc::channel::<TrayAction>(32);
            let (update_tx, mut update_rx) = mpsc::channel::<TrayUpdate>(32);

            let tray = CelesteTray {
                status: TrayStatus::Loading,
                click_tx,
                icons: IconSet::load(),
                // Seed with `None` so the first push from the app
                // (which fires immediately on `TrayReady`) decides
                // the actual tone. If the runtime never reports a
                // theme, the `None` fallback in `ThemedIcon::pick`
                // takes over.
                theme: theme::Mode::None,
            };

            match tray.spawn().await {
                Ok(handle) => {
                    // Handshake: give the app the update sender so it
                    // can start pushing state and theme.
                    let _ = output.send(TraySignal::Ready(update_tx)).await;

                    loop {
                        tokio::select! {
                            Some(action) = click_rx.recv() => {
                                let _ = output.send(TraySignal::Action(action)).await;
                            }
                            Some(update) = update_rx.recv() => {
                                let _ = handle.update(|t: &mut CelesteTray| match update {
                                    TrayUpdate::Status(s) => t.status = s,
                                    TrayUpdate::Theme(m) => t.theme = m,
                                }).await;
                            }
                            else => break,
                        }
                    }
                }
                Err(err) => {
                    eprintln!(
                        "celeste: tray service failed to start ({err}); running without tray icon."
                    );
                    // Deliver Ready anyway so the app's try_send path
                    // stays wired to *something*; drain the receiver
                    // forever so the bounded channel can't backpressure.
                    let _ = output.send(TraySignal::Ready(update_tx)).await;
                    while update_rx.recv().await.is_some() {}
                }
            }

            std::future::pending::<()>().await;
        })
    })
}

/// The tray state held inside the ksni service task. Menu callbacks
/// receive `&mut Self`, so the click sender lives here. `icons` is
/// rasterised once at startup — clone-on-read keeps `ksni::Tray`
/// methods `&self`. `theme` is whatever the iced runtime last
/// reported via `system::theme_changes`; the app pushes the initial
/// value on the [`TraySignal::Ready`] handshake.
#[cfg(not(target_os = "android"))]
struct CelesteTray {
    status: TrayStatus,
    click_tx: mpsc::Sender<TrayAction>,
    icons: IconSet,
    theme: theme::Mode,
}

#[cfg(not(target_os = "android"))]
impl CelesteTray {
    /// Pick the icon that matches the current status. `Loading`
    /// reuses the syncing glyph (mid-transition feel); `Disconnected`
    /// reuses the paused glyph (no remote to talk to).
    fn pixmap_for_current_status(&self) -> Vec<Icon> {
        let icon = match &self.status {
            TrayStatus::Loading | TrayStatus::Syncing { .. } => &self.icons.syncing,
            TrayStatus::Disconnected | TrayStatus::Paused => &self.icons.paused,
            TrayStatus::AuthNeeded { .. } => &self.icons.auth_needed,
            TrayStatus::Warning { .. } => &self.icons.warning,
            TrayStatus::Done { .. } => &self.icons.synced,
        };
        icon.pick(self.theme)
    }
}

#[cfg(not(target_os = "android"))]
impl ksni::Tray for CelesteTray {
    fn id(&self) -> String {
        "com.hunterwittenborn.Celeste".to_owned()
    }

    fn title(&self) -> String {
        "Celeste".to_owned()
    }

    fn icon_name(&self) -> String {
        // Deliberately empty. KDE prefers icon_name when it's set and
        // then runs the hicolor symbolic icon through its recolour
        // pipeline, which flattens these Inkscape masked paths to a
        // solid black square. An empty name forces the host onto
        // `icon_pixmap`, which we render ourselves below.
        String::new()
    }

    fn icon_pixmap(&self) -> Vec<Icon> {
        self.pixmap_for_current_status()
    }

    fn tool_tip(&self) -> ToolTip {
        ToolTip {
            icon_name: String::new(),
            icon_pixmap: self.pixmap_for_current_status(),
            title: "Celeste".to_owned(),
            description: description_for(&self.status),
        }
    }

    fn menu(&self) -> Vec<MenuItem<Self>> {
        vec![
            MenuItem::Standard(StandardItem {
                label: description_for(&self.status),
                enabled: false,
                ..Default::default()
            }),
            MenuItem::Separator,
            MenuItem::Standard(StandardItem {
                label: "Open Celeste".to_owned(),
                activate: Box::new(|t: &mut Self| {
                    let _ = t.click_tx.try_send(TrayAction::Open);
                }),
                ..Default::default()
            }),
            MenuItem::Standard(StandardItem {
                label: "Hide window".to_owned(),
                activate: Box::new(|t: &mut Self| {
                    let _ = t.click_tx.try_send(TrayAction::Hide);
                }),
                ..Default::default()
            }),
            MenuItem::Separator,
            MenuItem::Standard(StandardItem {
                label: "Quit Celeste".to_owned(),
                activate: Box::new(|t: &mut Self| {
                    let _ = t.click_tx.try_send(TrayAction::Quit);
                }),
                ..Default::default()
            }),
        ]
    }

    fn activate(&mut self, _x: i32, _y: i32) {
        let _ = self.click_tx.try_send(TrayAction::Open);
    }

    fn text_direction(&self) -> TextDirection {
        TextDirection::LeftToRight
    }
}

/// Android has no tray; the sync service's notification takes its place.
#[cfg(target_os = "android")]
pub fn subscription() -> Subscription<TraySignal> {
    Subscription::none()
}

pub fn description_for(status: &TrayStatus) -> String {
    match status {
        TrayStatus::Loading => "Starting up…".to_owned(),
        TrayStatus::Disconnected => "No remotes configured".to_owned(),
        TrayStatus::Paused => "All remotes are disabled".to_owned(),
        TrayStatus::AuthNeeded { remotes } => {
            format!("Reauthentication required: {}", remotes.join(", "))
        }
        TrayStatus::Syncing { count } => {
            if *count == 1 {
                "Syncing 1 remote…".to_owned()
            } else {
                format!("Syncing {count} remotes…")
            }
        }
        TrayStatus::Warning { remotes } => format!("Sync problems: {}", remotes.join(", ")),
        TrayStatus::Done { last_sync_ago } => match last_sync_ago {
            Some(age) => format!("Up to date — last sync {}", format_ago(*age)),
            None => "Up to date".to_owned(),
        },
    }
}

fn format_ago(age: Duration) -> String {
    let secs = age.as_secs();
    if secs < 60 {
        "just now".to_owned()
    } else if secs < 3_600 {
        format!("{}m ago", secs / 60)
    } else {
        format!("{}h ago", secs / 3_600)
    }
}

/// Roll the per-remote run-state machine plus the app's "what's running
/// right now" inputs into a single [`TrayStatus`]. Owned by the tray
/// module so the mapping rule lives next to the icon set it drives.
///
/// Priority: reauth (blocks syncing) > problems (sticky until a clean
/// pass, so they don't flicker with every scheduled pass) > syncing >
/// all paused > up to date.
pub fn compute_status(
    state: &AppState,
    remotes: &[Remote],
    syncing: &HashSet<RemoteId>,
    last_sync_at: &HashMap<RemoteId, Instant>,
) -> TrayStatus {
    if remotes.is_empty() {
        return TrayStatus::Disconnected;
    }
    let names_where = |pred: &dyn Fn(&Remote) -> bool| -> Vec<String> {
        remotes.iter().filter(|r| pred(r)).map(|r| r.name.clone()).collect()
    };
    let auth = names_where(&|r| state.needs_reauth(r.id));
    if !auth.is_empty() {
        return TrayStatus::AuthNeeded { remotes: auth };
    }
    let problems = names_where(&|r| {
        r.policy.enabled
            && (state.in_backoff(r.id)
                || state.roll_up(r.id).is_some_and(|s| matches!(s, RunState::Warning | RunState::Error)))
    });
    if !problems.is_empty() {
        return TrayStatus::Warning { remotes: problems };
    }
    if !syncing.is_empty() {
        return TrayStatus::Syncing { count: syncing.len() };
    }
    if remotes.iter().all(|r| !r.policy.enabled) {
        return TrayStatus::Paused;
    }
    let now = Instant::now();
    let last_sync_ago = last_sync_at
        .values()
        .map(|t| now.duration_since(*t))
        .min()
        .map(|d| Duration::from_secs(d.as_secs() / 60 * 60));
    TrayStatus::Done { last_sync_ago }
}
