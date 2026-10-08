//! Framework-agnostic application services.
//!
//! Services depend on port traits from [`crate::domain::ports`] — never on
//! infrastructure types directly, which is what makes them unit-testable
//! against in-memory fakes.

pub mod appearance;
pub mod auth;
pub mod autostart;
pub mod editor_temp;
pub mod leftovers;
pub mod remote_lifecycle;
pub mod secrets;
pub mod sync;
