//! Per-domain handler modules. Each defines `pub(super)` methods on
//! [`super::CelesteApp`] that the root `update()` dispatch in
//! [`super`] (`app/mod.rs`) calls into.

pub(super) mod conflicts;
pub(super) mod remotes;
pub(super) mod sync;
pub(super) mod sync_dir;
pub(super) mod tray;
