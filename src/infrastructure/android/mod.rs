//! What Celeste needs from Android's Java side: the Keystore for credentials, the folder picker, notifications and the sync service that keeps the process alive. The Java half is android/app/src/main/java/io/github/santuzius/celeste/.
//!
//! `CelesteApplication` hands over the Java VM and the Application when the process starts, before any activity or service, so calls also work while no activity exists, e.g. when the sync service started the engine after a reboot. They go through the Application's class loader.

pub mod folders;
pub mod secrets;

use std::sync::OnceLock;

use jni::{
    objects::{GlobalRef, JClass, JObject, JValue},
    JNIEnv, JavaVM,
};

/// The Java class with Celeste's static helpers.
const BRIDGE: &str = "io.github.santuzius.celeste.Bridge";

/// The process's Java VM and its Application, from [`CelesteApplication.nativeInit`](Java_io_github_santuzius_celeste_CelesteApplication_nativeInit).
static CONTEXT: OnceLock<(JavaVM, GlobalRef)> = OnceLock::new();

/// Called by `CelesteApplication.onCreate`, first thing in every process, after it has pointed the XDG and temp directories at the app's private storage (from Java, so the Go runtime sees them too).
#[unsafe(no_mangle)]
extern "system" fn Java_io_github_santuzius_celeste_CelesteApplication_nativeInit(env: JNIEnv, _: JClass, application: JObject) {
    // sqlx logs every query and wgpu its whole adapter at info level.
    let filter = android_logger::FilterBuilder::new().parse("info,sqlx=warn,wgpu_core=warn,wgpu_hal=error,iced_wgpu=warn").build();
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
pub fn show_sync_status(text: &str) {
    with_context(|env, context| {
        let class = bridge(env, context)?;
        let text = env.new_string(text)?;
        env.call_static_method(class, "showSyncStatus", "(Landroid/content/Context;Ljava/lang/String;)V", &[JValue::Object(context), JValue::Object(&text)])?;
        Ok(())
    });
}
