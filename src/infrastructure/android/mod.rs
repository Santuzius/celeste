//! What Celeste needs from Android's Java side: the Keystore for credentials, the folder picker, notifications and the sync service that keeps the process alive. The Java half is android/app/src/main/java/io/github/santuzius/celeste/.
//!
//! `CelesteApplication` hands over the Java VM and the Application when the process starts, before any activity or service, so calls also work while no activity exists, e.g. when the sync service started the engine after a reboot. They go through the Application's class loader.

pub mod folders;
pub mod secrets;

use std::{
    sync::{Mutex, OnceLock},
    time::Duration,
};

use jni::{
    objects::{GlobalRef, JClass, JObject, JValue},
    sys::{jboolean, JNI_TRUE},
    JNIEnv, JavaVM,
};

use crate::{
    engine::Cadence,
    services::power::{Conditions, PowerSettings},
};

/// The Java class with Celeste's static helpers.
const BRIDGE: &str = "io.github.santuzius.celeste.Bridge";

/// The process's Java VM and its Application, from [`CelesteApplication.nativeInit`](Java_io_github_santuzius_celeste_CelesteApplication_nativeInit).
static CONTEXT: OnceLock<(JavaVM, GlobalRef)> = OnceLock::new();

/// Called by `CelesteApplication.onCreate`, first thing in every process, after it has pointed the XDG and temp directories at the app's private storage (from Java, so the Go runtime sees them too).
#[unsafe(no_mangle)]
extern "system" fn Java_io_github_santuzius_celeste_CelesteApplication_nativeInit(env: JNIEnv, _: JClass, application: JObject) {
    // sqlx logs every query and wgpu its whole adapter at info level.
    let filter = android_logger::FilterBuilder::new().parse("info,sqlx=warn,wgpu_core=error,wgpu_hal=error,iced_wgpu=warn,iced_winit=warn").build();
    android_logger::init_once(android_logger::Config::default().with_max_level(log::LevelFilter::Info).with_tag("celeste").with_filter(filter));
    std::panic::set_hook(Box::new(|info| log::error!("{info}")));
    match (env.get_java_vm(), env.new_global_ref(application)) {
        (Ok(vm), Ok(application)) => {
            let _ = CONTEXT.set((vm, application));
        }
        _ => log::error!("no Java VM or Application"),
    }
}

/// Called by `SyncService.onCreate`: starts the sync engine without a GUI, unless it already runs.
#[unsafe(no_mangle)]
extern "system" fn Java_io_github_santuzius_celeste_SyncService_nativeStartEngine(_: JNIEnv, _: JClass) {
    // Setting up opens the database and resumes sessions over the network; keep that off the service's main thread.
    std::thread::spawn(crate::start_engine);
}

/// The screen, the charger and the battery saver, as `SyncService` last reported them.
static CONDITIONS: Mutex<Conditions> = Mutex::new(Conditions { screen_off: false, charging: false, battery_saver: false });

/// Called by `SyncService` when the screen goes off or on, the charger is plugged in or out, or Android's battery saver toggles: the power mode decides how often to sync (see [`crate::services::power`]).
#[unsafe(no_mangle)]
extern "system" fn Java_io_github_santuzius_celeste_SyncService_nativeConditions(_: JNIEnv, _: JClass, screen_off: jboolean, charging: jboolean, battery_saver: jboolean) {
    if let Ok(mut conditions) = CONDITIONS.lock() {
        *conditions = Conditions { screen_off: screen_off == JNI_TRUE, charging: charging == JNI_TRUE, battery_saver: battery_saver == JNI_TRUE };
    }
    apply_power_state();
}

/// Hands the cadence for the saved power mode and the current conditions to the engine, if it runs yet; it starts asynchronously and calls this once it does, and Preferences after a change.
pub fn apply_power_state() {
    let conditions = CONDITIONS.lock().map(|c| *c).unwrap_or_default();
    let cadence = PowerSettings::load(&crate::util::get_data_dir()).mode.cadence(conditions);
    // Google Drive's and Dropbox's change logs in the same rhythm; hourly while held.
    celeste_go::set_change_poll_interval(match cadence {
        Cadence::Full => None,
        Cadence::AtMost(floor) => Some(floor),
        Cadence::Held => Some(Duration::from_secs(60 * 60)),
    });
    if let Some(engine) = crate::engine::get() {
        engine.send(crate::engine::Command::Cadence(cadence));
    }
}

/// Whether Android leaves Celeste out of battery optimization, so it may keep running in the background.
pub fn ignores_battery_optimizations() -> bool {
    with_context(|env, context| {
        let class = bridge(env, context)?;
        env.call_static_method(class, "ignoresBatteryOptimizations", "(Landroid/content/Context;)Z", &[JValue::Object(context)])?.z()
    })
    .unwrap_or(true)
}

/// Opens Android's question whether Celeste may always run in the background, or the battery optimization list where that is missing.
pub fn ask_to_ignore_battery_optimizations() {
    with_context(|env, context| {
        let class = bridge(env, context)?;
        env.call_static_method(class, "askToIgnoreBatteryOptimizations", "(Landroid/content/Context;)V", &[JValue::Object(context)])?;
        Ok(())
    });
}

/// Starts or stops the sync service after "Run in background" was switched.
pub fn run_in_background(on: bool) {
    with_context(|env, context| {
        let class = bridge(env, context)?;
        env.call_static_method(class, "runInBackground", "(Landroid/content/Context;Z)V", &[JValue::Object(context), JValue::Bool(on.into())])?;
        Ok(())
    });
}

/// Shows the folder at `path` in the default file manager.
pub fn open_folder(path: &str) {
    with_context(|env, context| {
        let class = bridge(env, context)?;
        let path = env.new_string(path)?;
        env.call_static_method(class, "openFolder", "(Landroid/content/Context;Ljava/lang/String;)V", &[JValue::Object(context), JValue::Object(&path)])?;
        Ok(())
    });
}

/// Runs `f` with a JNI environment and the Application context, attaching the current thread to the Java VM first. Logs Java exceptions instead of leaving them pending.
pub fn with_context<T>(f: impl FnOnce(&mut JNIEnv, &JObject) -> jni::errors::Result<T>) -> Option<T> {
    let (vm, application) = CONTEXT.get()?;
    let mut env = vm.attach_current_thread_permanently().ok()?;

    match f(&mut env, application.as_obj()) {
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

/// Holds (`true`) or releases a partial wakelock, which keeps the CPU running with the screen off.
pub fn keep_awake(awake: bool) {
    with_context(|env, context| {
        let class = bridge(env, context)?;
        env.call_static_method(class, "keepAwake", "(Landroid/content/Context;Z)V", &[JValue::Object(context), JValue::Bool(awake.into())])?;
        Ok(())
    });
}

/// Opens `url` in the default browser.
pub fn open_url(url: &str) {
    with_context(|env, context| {
        let class = bridge(env, context)?;
        let url = env.new_string(url)?;
        env.call_static_method(class, "openUrl", "(Landroid/content/Context;Ljava/lang/String;)V", &[JValue::Object(context), JValue::Object(&url)])?;
        Ok(())
    });
}

/// Starts the foreground service that keeps the process, and with it syncing, alive while Celeste is not on screen, or updates its notification text.
/// `since_millis` (Unix time, 0 for none) is shown as the age of the status, e.g. of the last sync.
pub fn show_sync_status(text: &str, since_millis: i64) {
    with_context(|env, context| {
        let class = bridge(env, context)?;
        let text = env.new_string(text)?;
        env.call_static_method(class, "showSyncStatus", "(Landroid/content/Context;Ljava/lang/String;J)V", &[JValue::Object(context), JValue::Object(&text), JValue::Long(since_millis)])?;
        Ok(())
    });
}
