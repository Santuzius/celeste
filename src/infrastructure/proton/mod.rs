//! Native ProtonDrive client — talks directly to Proton's API via
//! `celeste-go` (the combined Go archive), no rclone in the
//! path. Implements [`crate::domain::ports::BackendClient`] so the
//! sync engine and auth flow can consume it via the same trait that
//! `LibrcloneClient` satisfies.

pub mod client;
