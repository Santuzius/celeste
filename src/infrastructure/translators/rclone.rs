//! Translator for the rclone backend (librclone RPC surface).
//!
//! Classifies error strings produced by rclone's JSON error envelope and
//! the OAuth / WebDAV auth layers. Rate-limit markers previously held on
//! `ProviderKind::rate_limit_markers` are also centralised here.

use crate::domain::backend_events::{BackendEvent, EventTranslator, Operation};

/// Marker sets for rate-limit detection via the stderr tap. Each inner
/// slice is a set of substrings that must ALL be present in a single log
/// line for it to count as a rate-limit hit.
pub const RATE_LIMIT_MARKERS: &[&[&str]] = &[
    &["rateLimitExceeded"],
    &["userRateLimitExceeded"],
    &["Quota exceeded"],
];

pub struct RcloneTranslator;

impl EventTranslator for RcloneTranslator {
    fn classify(&self, op: Operation, msg: &str) -> BackendEvent {
        let lower = msg.to_ascii_lowercase();

        // Auth failures — HTTP 401 phrasings from rclone's OAuth and
        // WebDAV layers. Not exhaustive; false negatives degrade gracefully
        // (user sees a per-file warning instead of the reauth banner).
        if lower.contains("invalid access token")
            || lower.contains("code=401")
            || lower.contains("status=401")
            || lower.contains(" 401 ")
            // Google API wording, e.g. "googleapi: Error 401: Request had
            // invalid authentication credentials … Reason: authError".
            || lower.contains("error 401")
            || lower.contains("reason: autherror")
            || lower.contains("invalid authentication credentials")
            || lower.contains("unauthenticated")
            || lower.contains("unauthorized")
            // OAuth refresh token revoked or expired (e.g. Google
            // Drive app in "testing" mode drops tokens after 7 days).
            || lower.contains("invalid_grant")
        {
            return BackendEvent::AuthExpired;
        }

        // Rate limits — rclone surfaces these as JSON errors wrapping the
        // upstream API response. The stderr markers above catch the retry
        // warnings; this catches the final error if rclone gives up.
        if lower.contains("ratelimitexceeded")
            || lower.contains("quota exceeded")
            || lower.contains("too many requests")
            || lower.contains("429")
        {
            return BackendEvent::RateLimited;
        }

        if op == Operation::Mkdir
            && (msg.contains("already exists") || msg.contains("AlreadyExist"))
        {
            return BackendEvent::AlreadyExists;
        }

        if lower.contains("not found") || lower.contains("no such") {
            return BackendEvent::NotFound;
        }

        if lower.contains("connection reset")
            || lower.contains("broken pipe")
            || lower.contains("eof")
            || lower.contains("timeout")
        {
            return BackendEvent::TransientNetwork;
        }

        BackendEvent::Other(msg.to_owned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn revoked_oauth_token_routes_to_reauth() {
        let msg = "couldn't fetch token: invalid_grant: maybe token expired? - try refreshing with \"rclone config reconnect gdrive:\"";
        assert!(RcloneTranslator.is_auth_failure(msg));
    }

    #[test]
    fn revoked_google_access_routes_to_reauth() {
        let msg = "error in ListJSON: couldn't list directory: googleapi: Error 401: Request had invalid authentication credentials. Expected OAuth 2 access token, login cookie or other valid authentication credential. See https://developers.google.com/identity/sign-in/web/devconsole-project.\nMore details:\nReason: authError, Message: Invalid Credentials";
        assert!(RcloneTranslator.is_auth_failure(msg));
    }

    #[test]
    fn plain_network_errors_are_not_auth_failures() {
        assert!(!RcloneTranslator.is_auth_failure("dial tcp: lookup www.googleapis.com: no such host"));
    }
}
