//! Saving and opening a single file through Android's document dialogs: `Bridge.saveDocument` (`ACTION_CREATE_DOCUMENT`) writes the given bytes where the user chose, `Bridge.openDocument` (`ACTION_OPEN_DOCUMENT`) reads the chosen file. Both answer through `nativeDocumentDone`.

use std::sync::{mpsc, Mutex};
use std::time::Duration;

use jni::{
    objects::{JByteArray, JClass, JString, JValue},
    JNIEnv,
};

use super::{bridge, with_context};

/// The dialog's answer: `Ok(None)` when cancelled, the file's bytes after opening (empty after saving), or what went wrong.
type Answer = Result<Option<Vec<u8>>, String>;

/// Where the pending dialog's answer goes.
static ANSWER: Mutex<Option<mpsc::Sender<Answer>>> = Mutex::new(None);

/// Blocks until `contents` is saved under a name the user chose (suggesting `name`); `Ok(false)` when cancelled.
pub fn save(name: &str, contents: &[u8]) -> Result<bool, String> {
    let answer = ask(|env, class| {
        let name = env.new_string(name)?;
        let contents = env.byte_array_from_slice(contents)?;
        env.call_static_method(class, "saveDocument", "(Ljava/lang/String;[B)Z", &[JValue::Object(&name), JValue::Object(&contents)])?.z()
    })?;
    Ok(answer.is_some())
}

/// Blocks until the user chose a file, and returns its contents; `Ok(None)` when cancelled.
pub fn open() -> Answer {
    ask(|env, class| env.call_static_method(class, "openDocument", "()Z", &[])?.z())
}

/// Shows a dialog through `start` (which reports whether an activity was there to show it) and waits for its answer.
fn ask(start: impl FnOnce(&mut JNIEnv, JClass) -> jni::errors::Result<bool>) -> Answer {
    let (sender, receiver) = mpsc::channel();
    *ANSWER.lock().map_err(|e| e.to_string())? = Some(sender);
    let started = with_context(|env, context| {
        let class = bridge(env, context)?;
        start(env, class)
    });
    if started != Some(true) {
        return Err("Open Celeste's window to choose a file.".to_owned());
    }
    receiver.recv_timeout(Duration::from_secs(30 * 60)).unwrap_or(Ok(None))
}

/// `error` non-null: what went wrong. Otherwise `data` null: cancelled; else the file's bytes (empty after saving).
#[unsafe(no_mangle)]
extern "system" fn Java_io_github_santuzius_celeste_Bridge_nativeDocumentDone(mut env: JNIEnv, _: JClass, data: JByteArray, error: JString) {
    let answer = if !error.is_null() {
        Err(env.get_string(&error).map(String::from).unwrap_or_else(|_| "unknown error".to_owned()))
    } else if data.is_null() {
        Ok(None)
    } else {
        env.convert_byte_array(&data).map(Some).map_err(|e| e.to_string())
    };
    if let Some(sender) = ANSWER.lock().ok().and_then(|mut slot| slot.take()) {
        let _ = sender.send(answer);
    }
}
