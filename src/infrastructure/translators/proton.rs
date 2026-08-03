//! Translator for the native ProtonDrive backend (go-proton-api via FFI).
//!
//! Proton returns structured error codes in the form `Code=NNNN`. The
//! canonical codes we handle:
//!   - `Code=401`   — auth / session expired
//!   - `Code=10013` — invalid refresh token (server force-revoked the
//!                    session; only the user re-entering credentials can
//!                    recover it)
//!   - `Code=2500`  — already exists (mkdir idempotence, also "already exists"
//!                    substring from the Go bridge)
//!   - `Code=429`   — rate limited (Proton reports this directly, unlike
//!                    rclone which hides 429s in retry loops)

use crate::domain::backend_events::{BackendEvent, EventTranslator, Operation};

pub struct ProtonTranslator;

impl EventTranslator for ProtonTranslator {
    fn classify(&self, op: Operation, msg: &str) -> BackendEvent {
        let lower = msg.to_ascii_lowercase();

        if msg.contains("Code=401")
            || msg.contains("Code=10013")
            || lower.contains("unauthenticated")
            || lower.contains("unauthorized")
            || lower.contains("invalid access token")
            || lower.contains("invalid refresh token")
            || lower.contains("session expired")
            // go-proton-api wraps a server-driven session revocation as
            // "failed to refresh auth, de-auth: …". The "de-auth" marker
            // is unique to that path; matching it covers future Code=
            // values the API may add for the same condition.
            || lower.contains("de-auth")
        {
            return BackendEvent::AuthExpired;
        }

        if msg.contains("Code=429") || lower.contains("too many requests") {
            return BackendEvent::RateLimited;
        }

        // Folder-already-exists from the Go bridge (create_folder returns
        // Code=2500 for duplicate names on Proton's API).
        if op == Operation::Mkdir
            && (msg.contains("Code=2500") || lower.contains("already exists"))
        {
            return BackendEvent::AlreadyExists;
        }

        if lower.contains("not found") || msg.contains("Code=2501") {
            return BackendEvent::NotFound;
        }

        if lower.contains("connection reset")
            || lower.contains("broken pipe")
            || lower.contains("eof")
            || lower.contains("timeout")
            // Connectivity failures reaching Proton at all. go-proton-api
            // wraps these as `NetError` ("received no response from API",
            // "network error while communicating with API"); the Go
            // dialer supplies the rest. Without these, a laptop that
            // suspended or booted before DNS came up reports an
            // unclassified error instead of a retryable one.
            || lower.contains("no such host")
            || lower.contains("dial tcp")
            || lower.contains("network is unreachable")
            || lower.contains("no route to host")
            || lower.contains("connection refused")
            || lower.contains("received no response from api")
            || lower.contains("network error while communicating with api")
        {
            return BackendEvent::TransientNetwork;
        }

        BackendEvent::Other(msg.to_owned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The exact string go-proton-api produces when Celeste starts
    /// before DNS is up (autostart, or resume-from-suspend). Resuming a
    /// session needs an HTTPS round-trip, so this used to reach the
    /// startup resume path and get mistaken for a dead session —
    /// costing the user a full 2FA login for a perfectly valid one.
    const DNS_FAILURE: &str = "received no response from API: Get \
        \"https://mail.proton.me/api/core/v4/users\": dial tcp: \
        lookup mail.proton.me: no such host";

    #[test]
    fn connectivity_failures_are_transient_not_auth() {
        for msg in [
            DNS_FAILURE,
            "network error while communicating with API: connection refused",
            "dial tcp 1.2.3.4:443: connect: network is unreachable",
            "dial tcp 1.2.3.4:443: connect: no route to host",
        ] {
            assert_eq!(
                ProtonTranslator.classify(Operation::List, msg),
                BackendEvent::TransientNetwork,
                "should be retryable: {msg}",
            );
            assert!(
                !ProtonTranslator.is_auth_failure(msg),
                "must never demand reauth: {msg}",
            );
        }
    }

    #[test]
    fn revoked_sessions_still_route_to_reauth() {
        for msg in [
            "failed to refresh auth, de-auth: Code=10013",
            "Code=401 unauthorized",
            "invalid refresh token",
        ] {
            assert_eq!(
                ProtonTranslator.classify(Operation::Auth, msg),
                BackendEvent::AuthExpired,
                "should demand reauth: {msg}",
            );
            assert!(ProtonTranslator.is_auth_failure(msg));
        }
    }

    #[test]
    fn keyring_faults_are_not_auth_failures() {
        // Secret-Service transport hiccups surface through the same
        // resume path; they're recoverable, not a dead session.
        let msg = "keyring read failed: Platform secure storage failure: \
                   Crypto error: Unpad Error";
        assert!(!ProtonTranslator.is_auth_failure(msg));
    }
}
