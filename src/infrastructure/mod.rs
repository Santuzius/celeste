#[cfg(target_os = "android")]
pub mod android;
pub mod client_router;
pub mod persistence;
#[cfg_attr(target_os = "android", path = "android/portal.rs")]
pub mod portal;
pub mod proton;
pub mod rclone;
pub mod single_instance;
pub mod stderr_capture;
pub mod translators;
pub mod tray;
