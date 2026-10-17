//! Per-backend `EventTranslator` implementations.
//!
//! Each submodule classifies the raw error strings and log lines produced by
//! one backend into the unified `BackendEvent` taxonomy. Upper layers import
//! the concrete translator they need rather than using dynamic dispatch, since
//! the translator is known at compile time from the adapter type.

pub mod proton;
pub mod rclone;

/// Whether `msg` says the provider couldn't be reached at all: no network, no DNS, or a connection cut off on the way (e.g. a VPN without a network below it). Stricter than [`TransientNetwork`](crate::domain::backend_events::BackendEvent::TransientNetwork), which also takes a single timeout or EOF.
pub fn is_offline(msg: &str) -> bool {
    const MARKERS: &[&str] = &[
        "no such host",
        "server misbehaving",
        "temporary failure in name resolution",
        "dial tcp",
        "dial udp",
        "network is unreachable",
        "no route to host",
        "connection refused",
        "connection reset by peer",
        "i/o timeout",
        "tls handshake timeout",
        // go-proton-api's `NetError`.
        "received no response from api",
        "network error while communicating with api",
    ];
    let lower = msg.to_ascii_lowercase();
    MARKERS.iter().any(|m| lower.contains(m))
}

#[cfg(test)]
mod tests {
    use super::is_offline;

    #[test]
    fn recognises_unreachable_providers() {
        for msg in [
            r#"received no response from API: Get "https://mail.proton.me/api/drive/shares/x/folders/y/children?Page=0": dial tcp 185.70.42.37:443: connect: connection refused"#,
            "Proton Drive session for 'P' not resumed yet (received no response from API: Get \"https://mail.proton.me/api/core/v4/users\": read tcp 10.111.222.1:48968->185.70.42.37:443: read: connection reset by peer). Retrying automatically — no action needed.",
            r#"couldn't list directory: Get "https://www.googleapis.com/drive/v3/files?alt=json": dial tcp: lookup www.googleapis.com: no such host"#,
        ] {
            assert!(is_offline(msg), "{msg}");
        }
        for msg in ["object not found", "googleapi: Error 403: Rate Limit Exceeded", "unexpected EOF"] {
            assert!(!is_offline(msg), "{msg}");
        }
    }
}
