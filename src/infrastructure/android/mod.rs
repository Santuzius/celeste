//! What Celeste needs from Android's Java side: the Keystore for credentials, the folder picker, notifications and the sync service that keeps the process alive. The Java half is android/app/src/main/java/io/github/santuzius/celeste/.
//!
//! Calls go through the Application's class loader (from `ndk-context`), so they also work while no activity exists, e.g. after the user swiped Celeste away and the service keeps syncing.

pub mod folders;
pub mod secrets;

use std::path::Path;

use jni::{
    objects::{JClass, JObject, JValue},
    JNIEnv, JavaVM,
};

/// The Java class with Celeste's static helpers.
const BRIDGE: &str = "io.github.santuzius.celeste.Bridge";

/// Points the XDG and temp directories Celeste, Go and rclone use at the app's private storage; Android sets none of them. Call first in `android_main`, before any other thread starts.
pub fn set_dirs(files: &Path) {
    let run = files.join("run");
    let cache = files.join("cache");
    for dir in [&run, &cache] {
        let _ = std::fs::create_dir_all(dir);
    }
    // SAFETY: called on the only thread that exists yet.
    unsafe {
        std::env::set_var("HOME", files);
        std::env::set_var("XDG_DATA_HOME", files);
        std::env::set_var("XDG_CONFIG_HOME", files);
        std::env::set_var("XDG_CACHE_HOME", &cache);
        std::env::set_var("XDG_RUNTIME_DIR", &run);
        // Go's os.TempDir falls back to /data/local/tmp, which apps cannot write.
        std::env::set_var("TMPDIR", &cache);
    }
}

/// Runs `f` with a JNI environment and the Application context, attaching the current thread to the Java VM first. Logs Java exceptions instead of leaving them pending.
pub fn with_context<T>(f: impl FnOnce(&mut JNIEnv, &JObject) -> jni::errors::Result<T>) -> Option<T> {
    let context = ndk_context::android_context();
    // SAFETY: android-activity initialises ndk-context with the process's VM and a global reference to the Application, both valid for the life of the process.
    let vm = unsafe { JavaVM::from_raw(context.vm().cast()) }.ok()?;
    let application = unsafe { JObject::from_raw(context.context().cast()) };
    let mut env = vm.attach_current_thread_permanently().ok()?;

    match f(&mut env, &application) {
        Ok(value) => Some(value),
        Err(error) => {
            if env.exception_check().unwrap_or(false) {
                let _ = env.exception_describe();
                let _ = env.exception_clear();
            }
            log::error!("Java call failed: {error}");
            None
        }
    }
}

/// Loads [`BRIDGE`] through the app's class loader; `FindClass` on a native thread only sees system classes.
fn bridge<'local>(env: &mut JNIEnv<'local>, context: &JObject) -> jni::errors::Result<JClass<'local>> {
    let loader = env.call_method(context, "getClassLoader", "()Ljava/lang/ClassLoader;", &[])?.l()?;
    let name = env.new_string(BRIDGE)?;
    let class = env.call_method(&loader, "loadClass", "(Ljava/lang/String;)Ljava/lang/Class;", &[JValue::Object(&name)])?.l()?;
    Ok(JClass::from(class))
}

/// Shows a notification in Celeste's "Problems" channel, e.g. when an account needs signing in again.
pub fn notify(title: &str, text: &str) {
    with_context(|env, context| {
        let class = bridge(env, context)?;
        let title = env.new_string(title)?;
        let text = env.new_string(text)?;
        env.call_static_method(class, "notify", "(Landroid/content/Context;Ljava/lang/String;Ljava/lang/String;)V", &[JValue::Object(context), JValue::Object(&title), JValue::Object(&text)])?;
        Ok(())
    });
}

/// Starts the foreground service that keeps the process, and with it syncing, alive while Celeste is not on screen, or updates its notification text.
pub fn show_sync_status(text: &str) {
    with_context(|env, context| {
        let class = bridge(env, context)?;
        let text = env.new_string(text)?;
        env.call_static_method(class, "showSyncStatus", "(Landroid/content/Context;Ljava/lang/String;)V", &[JValue::Object(context), JValue::Object(&text)])?;
        Ok(())
    });
}
