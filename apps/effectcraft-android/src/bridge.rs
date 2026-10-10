//! JNI bridge to the Kotlin half (`android/app/src/main/kotlin/…`).
//!
//! Rust → Kotlin: methods on `MainActivity` (`pickFiles`, `requestExport`, `haptic`).
//! Kotlin → Rust: [`Java_ai_storyteller_effectcraft_NativeBridge_onPicked`], which queues the
//! picked paths and wakes the UI.

use std::sync::Mutex;
use std::sync::OnceLock;

use android_activity::AndroidApp;
use jni::objects::{JClass, JObject, JObjectArray, JString, JValue};
use jni::sys::jint;
use jni::{JNIEnv, JavaVM};

/// Why a picker was opened (the Kotlin side echoes it back as an int).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Import = 0,
    OpenProject = 1,
}

static CTX: OnceLock<egui::Context> = OnceLock::new();
static INBOX: Mutex<Vec<Vec<String>>> = Mutex::new(Vec::new());

pub fn set_context(ctx: egui::Context) {
    let _ = CTX.set(ctx);
}

/// Picked files since the last call, one entry per picker result.
pub fn take_picked() -> Vec<Vec<String>> {
    std::mem::take(&mut *INBOX.lock().unwrap_or_else(|e| e.into_inner()))
}

/// Run `f` on a thread attached to the JVM with the activity as `this`.
fn with_activity<R>(app: &AndroidApp, f: impl FnOnce(&mut JNIEnv, &JObject) -> jni::errors::Result<R>) -> Option<R> {
    // SAFETY: android-activity guarantees both pointers stay valid for the life of the app.
    let vm = unsafe { JavaVM::from_raw(app.vm_as_ptr().cast()) }.ok()?;
    let activity = unsafe { JObject::from_raw(app.activity_as_ptr().cast()) };
    let mut env = vm.attach_current_thread().ok()?;
    match f(&mut env, &activity) {
        Ok(v) => Some(v),
        Err(e) => {
            log::warn!("JNI call failed: {e}");
            // A pending Java exception would abort the next call.
            let _ = env.exception_clear();
            None
        }
    }
}

/// Open the system file picker (Storage Access Framework). The result arrives later through
/// `onPicked`, as paths in the app's folder: Kotlin copies each picked document there.
pub fn pick(app: &AndroidApp, kind: Kind, multiple: bool) {
    with_activity(app, |env, activity| env.call_method(activity, "pickFiles", "(IZ)V", &[JValue::Int(kind as jint), JValue::Bool(multiple as u8)]).map(|_| ()));
}

/// Ask for a destination file and return the app-folder path to write to now. Kotlin copies
/// the finished file to the destination: once it has stopped growing, or (`keep_syncing`, for
/// projects) again after every save while the app runs.
pub fn stage_export(app: &AndroidApp, display_name: &str, keep_syncing: bool) -> Option<String> {
    let dir = app.external_data_path().or_else(|| app.internal_data_path())?.join("exports");
    std::fs::create_dir_all(&dir).ok()?;
    let staged = dir.join(display_name);
    let staged_str = staged.to_string_lossy().to_string();
    with_activity(app, |env, activity| {
        let path: JString = env.new_string(&staged_str)?;
        let name: JString = env.new_string(display_name)?;
        env.call_method(activity, "requestExport", "(Ljava/lang/String;Ljava/lang/String;Z)V", &[
            JValue::Object(&path),
            JValue::Object(&name),
            JValue::Bool(keep_syncing as u8),
        ])
        .map(|_| ())
    });
    Some(staged_str)
}

/// Help ▸ Show Debug Log: the Kotlin side opens a dialog with this app's log (copy / share).
pub fn show_log(app: &AndroidApp) {
    with_activity(app, |env, activity| env.call_method(activity, "showLog", "()V", &[]).map(|_| ()));
}

/// A short vibration (the long press that opens a context menu).
pub fn haptic(app: &AndroidApp) {
    with_activity(app, |env, activity| env.call_method(activity, "haptic", "()V", &[]).map(|_| ()));
}

/// `NativeBridge.onPicked(kind: Int, paths: Array<String>)`, called on the Java UI thread.
#[unsafe(no_mangle)]
pub extern "system" fn Java_ai_storyteller_effectcraft_NativeBridge_onPicked(mut env: JNIEnv, _class: JClass, _kind: jint, paths: JObjectArray) {
    let n = env.get_array_length(&paths).unwrap_or(0);
    let mut out = Vec::new();
    for i in 0..n {
        let Ok(element) = env.get_object_array_element(&paths, i) else { continue };
        let element = JString::from(element);
        if let Ok(s) = env.get_string(&element) {
            out.push(String::from(s));
        }
    }
    if out.is_empty() {
        return; // the picker was cancelled
    }
    INBOX.lock().unwrap_or_else(|e| e.into_inner()).push(out);
    if let Some(ctx) = CTX.get() {
        ctx.request_repaint();
    }
}
