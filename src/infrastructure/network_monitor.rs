//! Whether the computer has a network, from the XDG desktop portal's network monitor (`org.freedesktop.portal.NetworkMonitor`). It asks NetworkManager where there is one and otherwise looks for a default route, so it works whatever manages the network. Without a portal Syncative counts as online, and failed connections show as Offline anyway.

use std::time::Duration;

use dbus::{blocking::Connection, channel::MatchingReceiver, message::MatchRule, Message};

const DESTINATION: &str = "org.freedesktop.portal.Desktop";
const PATH: &str = "/org/freedesktop/portal/desktop";
const INTERFACE: &str = "org.freedesktop.portal.NetworkMonitor";

/// Ask once, so the first pass and the resuming of sessions already know, then follow the portal's changes on a thread of its own.
pub fn start() {
    let Ok(conn) = Connection::new_session() else { return };
    match available(&conn) {
        Ok(online) => crate::services::network::set_online(online),
        Err(err) => {
            eprintln!("syncative: no network monitor in the desktop portal ({err}); offline is noticed from failed connections only.");
            return;
        }
    }
    let spawned = std::thread::Builder::new().name("syncative-network".to_owned()).spawn(move || {
        let rule = MatchRule::new_signal(INTERFACE, "changed");
        let receiving = conn.add_match_no_cb(&rule.match_str()).map(|()| {
            conn.start_receive(
                rule,
                Box::new(|_: Message, conn: &Connection| {
                    if let Ok(online) = available(conn) {
                        crate::services::network::set_online(online);
                    }
                    true
                }),
            )
        });
        if let Err(err) = receiving {
            eprintln!("syncative: can't follow the network monitor ({err}).");
            return;
        }
        while conn.process(Duration::from_secs(60)).is_ok() {}
    });
    if let Err(err) = spawned {
        eprintln!("syncative: can't start the network monitor ({err}).");
    }
}

/// Whether there is a network; GLib's `network-available`, the portal's answer since its first version.
fn available(conn: &Connection) -> Result<bool, dbus::Error> {
    let proxy = conn.with_proxy(DESTINATION, PATH, Duration::from_secs(2));
    let (available,): (bool,) = proxy.method_call(INTERFACE, "GetAvailable", ())?;
    Ok(available)
}
