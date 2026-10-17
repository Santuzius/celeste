//! Whether the device has a network at all, as the platform reports it: Android's ConnectivityManager (`Bridge.watchNetwork`), on the desktop the XDG desktop portal's network monitor. Without a network the engine schedules nothing and folders show Offline; without a report it counts as online, and failed connections show as Offline anyway.

use std::sync::atomic::{AtomicBool, Ordering};

static ONLINE: AtomicBool = AtomicBool::new(true);

/// As far as the platform said so far.
pub fn online() -> bool {
    ONLINE.load(Ordering::Acquire)
}

/// Record what the platform reports, and tell the engine, if it runs yet, about a change. The engine reads [`online`] when it starts.
pub fn set_online(online: bool) {
    if ONLINE.swap(online, Ordering::AcqRel) == online {
        return;
    }
    log_change(online);
    if let Some(engine) = crate::engine::get() {
        engine.send(crate::engine::Command::Online(online));
    }
}

#[cfg(target_os = "android")]
fn log_change(online: bool) {
    log::info!("{}", if online { "online" } else { "offline" });
}

#[cfg(not(target_os = "android"))]
fn log_change(online: bool) {
    eprintln!("syncative: {}", if online { "online" } else { "offline" });
}
