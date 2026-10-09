//! The folder picker: `Bridge.pickFolder` opens Android's document picker on the activity and answers through `nativeFolderPicked` with a file path, or `null` when cancelled or when the choice has no path (e.g. a cloud provider's folder). Without "All files access" it opens that setting instead and answers `null`.

use std::sync::{mpsc, Mutex};
use std::time::Duration;

use jni::{
    objects::{JClass, JString},
    JNIEnv,
};

use super::{bridge, with_context};

/// Where the pending pick's answer goes.
static ANSWER: Mutex<Option<mpsc::Sender<Option<String>>>> = Mutex::new(None);

/// Blocks until the user picked a folder or cancelled.
pub fn pick() -> Option<String> {
    let (sender, receiver) = mpsc::channel();
    *ANSWER.lock().ok()? = Some(sender);

    let started = with_context(|env, context| {
        let class = bridge(env, context)?;
        env.call_static_method(class, "pickFolder", "()Z", &[])?.z()
    });
    if started != Some(true) {
        return None;
    }

    receiver.recv_timeout(Duration::from_secs(30 * 60)).ok().flatten()
}

#[unsafe(no_mangle)]
extern "system" fn Java_io_github_santuzius_celeste_Bridge_nativeFolderPicked(mut env: JNIEnv, _: JClass, path: JString) {
    let path = if path.is_null() { None } else { env.get_string(&path).ok().map(String::from) };
    if let Some(sender) = ANSWER.lock().ok().and_then(|mut answer| answer.take()) {
        let _ = sender.send(path);
    }
}

