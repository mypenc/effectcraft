//! EffectCraft for Android.
//!
//! The same engine and egui interface as the desktop app, compiled as a `cdylib` that a
//! GameActivity (see `android/`) loads. There is no web view: eframe draws with wgpu (Vulkan)
//! straight into the activity's surface.
//!
//! What differs from the desktop app (`apps/effectcraft`):
//!
//! * **Files.** `rfd` has no Android backend, and Android hands apps `content://` URIs rather
//!   than paths. Kotlin's Storage Access Framework pickers copy what the user picks into the
//!   app's folder and call back with real paths ([`bridge`]); saves and renders are written to
//!   an app folder and mirrored to the file the user chose.
//! * **Input.** Fingers become mouse events in `effectcraft_ui_egui::touch` (long press =
//!   right click…); a mouse and keyboard reach egui through the patched winit
//!   (`android/patches/`).
//! * **Window.** One full-screen window: no title, icon, min size, agent control channel,
//!   AppImage glue or native menu bar.
//!
//! Everything here is `cfg(target_os = "android")`; on other targets the crate is empty.
#![cfg(target_os = "android")]

mod bridge;
// The desktop app's cpal output: cpal has an AAudio backend on Android.
#[path = "../../effectcraft/src/audio_out.rs"]
mod audio_out;

use std::sync::Arc;

use android_activity::AndroidApp;
use android_activity::input::Axis;
use effectcraft_ui_egui::EffectcraftApp;
use serde_json::json;

/// Entry point: android-activity calls this on its own thread, not the Java UI thread.
#[unsafe(no_mangle)]
fn android_main(app: AndroidApp) {
    // Make startup logs visible in `adb logcat` (stderr goes to the tag RustStdoutStderr); the
    // logger only prints to stderr when RUST_LOG is set. An explicit RUST_LOG wins.
    if std::env::var_os("RUST_LOG").is_none() {
        // SAFETY: first statement of `android_main`, before this app spawns any thread of its own.
        unsafe {
            std::env::set_var("RUST_LOG", "warn,effectcraft=debug,eframe=info,egui_wgpu=info,wgpu_hal=info,wgpu_core=warn,winit=info,android_activity=info");
        }
    }
    effectcraft_engine::logging::install();
    effectcraft_engine::logging::install_panic_hook();
    prepare_environment(&app);
    // Mouse wheel axes are off by default in GameActivity.
    app.enable_motion_axis(Axis::Hscroll);
    app.enable_motion_axis(Axis::Vscroll);
    effectcraft_engine::text::fonts::scan_system_in_background();

    let options = eframe::NativeOptions { android_app: Some(app.clone()), wgpu_options: wgpu_options(), ..Default::default() };
    let host = app.clone();
    let result = eframe::run_native(
        "EffectCraft",
        options,
        Box::new(move |cc| {
            match &cc.wgpu_render_state {
                Some(rs) => log::info!("graphics: {:?}", rs.adapter.get_info()),
                None => log::info!("graphics: no wgpu device"),
            }
            bridge::set_context(cc.egui_ctx.clone());
            let gpu_failures = effectcraft_ui_egui::gpu_failure::GpuFailureBridge::new(&cc.egui_ctx);
            if let Some(rs) = &cc.wgpu_render_state {
                let errors = gpu_failures.clone();
                rs.device.on_uncaptured_error(Arc::new(move |error| {
                    let message = format!("uncaptured GPU error: {error}");
                    if errors.report(&message, false) {
                        log::error!("{message}");
                    }
                }));
                let lost = gpu_failures.clone();
                rs.device.set_device_lost_callback(move |reason, message| {
                    lost.report(&format!("GPU device lost ({reason:?}): {message}"), true);
                });
            }

            let mut session = effectcraft_host::session();
            session.check_footage_on_open = true;
            session.autosave.background = true;
            if let Some(dir) = effectcraft_host::config_dir() {
                session.config = Some(Arc::new(effectcraft_engine::config::DirConfig::new(dir)));
            }
            session.load_settings();
            let recovery = session.begin_recovery();
            let show_home = session.prefs.startup.show_home_on_launch;
            let mut app = EffectcraftApp::new(session);
            app.set_gpu_failure_bridge(gpu_failures);
            app.ui.start_screen = show_home;
            if let Some(r) = recovery {
                app.offer_recovery(r);
            }
            install_hooks(&mut app, &host);
            Ok(Box::new(AndroidHost { app, android: host }))
        }),
    );
    if let Err(e) = result {
        log::error!("EffectCraft could not start: {e}");
    }
}

/// `$HOME`, the temp folder and the settings folder must point inside the app's own storage;
/// the desktop defaults (`~/.config`, `/data/local/tmp`) don't exist or aren't writable.
fn prepare_environment(app: &AndroidApp) {
    let Some(data) = app.internal_data_path() else { return };
    let cache = data.join("tmp");
    let _ = std::fs::create_dir_all(&cache);
    // SAFETY: first thing `android_main` does, before this process spawns any thread of ours.
    unsafe {
        std::env::set_var("HOME", &data);
        std::env::set_var("TMPDIR", &cache);
        std::env::set_var("EFFECTCRAFT_CONFIG_DIR", data.join("config"));
    }
}

/// Like the desktop window: ask the adapter for its own texture limit (up to 16384 px) so a
/// modest phone GPU still opens the app, and let the compositor fall back to the CPU for what
/// it can't run. Vulkan first, OpenGL ES as the fallback.
fn wgpu_options() -> eframe::WgpuConfiguration {
    use eframe::egui_wgpu::WgpuSetup;
    let mut config = eframe::WgpuConfiguration::default();
    if let WgpuSetup::CreateNew(setup) = &mut config.wgpu_setup {
        setup.instance_descriptor.backends = eframe::wgpu::Backends::VULKAN | eframe::wgpu::Backends::GL;
        setup.device_descriptor = Arc::new(|adapter| {
            let limits = adapter.limits();
            eframe::wgpu::DeviceDescriptor {
                label: Some("EffectCraft device"),
                required_limits: eframe::wgpu::Limits { max_texture_dimension_2d: limits.max_texture_dimension_2d.min(16384), ..limits },
                ..Default::default()
            }
        });
    }
    config
}

/// Android's file access is asynchronous (an activity result), so the pickers return at once
/// and the result is applied on a later frame in [`AndroidHost::logic`]: the same shape as the
/// web build's pickers.
fn install_hooks(app: &mut EffectcraftApp, host: &AndroidApp) {
    let h = host.clone();
    app.hooks.pick_files = Some(Box::new(move |_exts: &[&str]| {
        bridge::pick(&h, bridge::Kind::Import, true);
        Vec::new()
    }));
    let h = host.clone();
    app.hooks.pick_open_project = Some(Box::new(move || {
        bridge::pick(&h, bridge::Kind::OpenProject, false);
        None
    }));
    // Projects: stage in the app's folder, mirror to the chosen file on every save.
    let h = host.clone();
    app.hooks.pick_save = Some(Box::new(move |name: &str| bridge::stage_export(&h, if name.is_empty() { "Untitled Project.ecproj" } else { name }, true)));
    // Renders and other exports: mirrored once the file stops growing.
    let h = host.clone();
    app.hooks.pick_save_file = Some(Box::new(move |name: &str, _ext: &str| {
        let file = std::path::Path::new(name).file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        bridge::stage_export(&h, if file.is_empty() { "Export" } else { &file }, false)
    }));
    app.hooks.audio_device = Some(Box::new(audio_out::open));
    app.hooks.audio_devices = Some(Box::new(audio_out::devices));
    // Help ▸ Show Debug Log... opens the Kotlin log dialog; other app actions aren't handled here.
    let h = host.clone();
    app.hooks.app_action = Some(Box::new(move |id: &str| match id {
        "help.showLog" => {
            bridge::show_log(&h);
            true
        }
        _ => false,
    }));
    let h = host.clone();
    app.hooks.haptic = Some(Box::new(move || bridge::haptic(&h)));
}

/// Open a project or import media from picked paths (the web build's `open_or_import`).
fn open_or_import(app: &mut EffectcraftApp, paths: Vec<String>) {
    let (projects, media): (Vec<String>, Vec<String>) = paths.into_iter().partition(|p| p.to_ascii_lowercase().ends_with(".ecproj"));
    if let Some(p) = projects.last()
        && let Err(e) = app.session.execute("file.open", json!({"path": p}))
    {
        app.ui.status = e.to_string();
    }
    if !media.is_empty() {
        match app.session.execute("file.import", json!({"paths": media})) {
            Ok(r) => {
                if let Some(errs) = r["errors"].as_array().filter(|e| !e.is_empty()) {
                    app.ui.status = errs.iter().filter_map(|e| e.as_str()).collect::<Vec<_>>().join("; ");
                }
            }
            Err(e) => app.ui.status = e.to_string(),
        }
    }
}

struct AndroidHost {
    app: EffectcraftApp,
    android: AndroidApp,
}

impl eframe::App for AndroidHost {
    fn raw_input_hook(&mut self, ctx: &egui::Context, raw_input: &mut egui::RawInput) {
        self.app.raw_input_hook(ctx, raw_input);
    }

    fn logic(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
        for picked in bridge::take_picked() {
            open_or_import(&mut self.app, picked);
        }
        self.app.logic(ctx, frame);
    }

    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        self.app.ui(ui, frame);
        // The on-screen bar's keyboard button. (Focusing a text field raises it by itself:
        // egui-winit calls `set_ime_allowed`, which winit forwards to `show_soft_input`.)
        if std::mem::take(&mut self.app.touch.keyboard_request) {
            self.android.show_soft_input(false);
        }
    }

    fn on_exit(&mut self) {
        self.app.on_exit();
    }
}
