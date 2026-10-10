# EffectCraft on Android

A native Android app: the same engine and egui interface as the desktop build, compiled to a
`cdylib` and hosted by a `GameActivity`. There is no web view; eframe draws with wgpu (Vulkan,
OpenGL ES fallback) into the activity's surface.

> **Status: written, not yet run on a device.** The gesture recognizer is unit-tested (host
> `rustc`, 12 tests). Everything else was written against the exact pinned crate sources
> (winit 0.30.13, android-activity 0.6.1, egui 0.36.2) and parse-checked, but has **not been
> compiled for an Android target or run on hardware**. Treat the first build as a bring-up: see
> [Bring-up checklist](#bring-up-checklist).

## Layout

| Path | What |
|---|---|
| `apps/effectcraft-android/` | The Rust app (`android_main`, file/audio/haptic hooks, JNI bridge). Empty on non-Android targets, so desktop CI is unaffected. |
| `crates/ui-egui/src/touch/` | Touch layer: `gesture.rs` (pure recognizer) and `mod.rs` (egui adapter, on-screen modifier bar, touch-sized metrics). Shared code; inert unless a touch arrives or the target is Android. |
| `android/` | Gradle project: `MainActivity` (GameActivity), Storage Access Framework pickers, export mirroring, manifest. |
| `android/patches/winit-0.30.13-android-input.patch` | Makes winit's Android backend deliver mouse, hardware-keyboard modifiers and soft-keyboard text. |
| `crates/text/src/fonts.rs` | Adds `/system/fonts` to the system font search. |

## Build

```sh
rustup target add aarch64-linux-android          # and x86_64-linux-android for an emulator
cargo install cargo-ndk
export ANDROID_NDK_HOME=/path/to/ndk             # r26 or newer
android/scripts/build.sh                         # release, arm64-v8a  → android/app/build/outputs/apk/
android/scripts/build.sh --debug --abi x86_64    # emulator
```

Needs Rust 1.95+, a JDK 17 and Gradle (or run `gradle wrapper` once in `android/`). The script
downloads winit 0.30.13, applies the patch into `android/vendor/` (git-ignored) and builds with
`--config patch.crates-io.winit.path=…`, so **desktop builds keep using the stock winit**.

## Input: touch, mouse and keyboard

Touch is translated, never special-cased in panels. A real mouse and keyboard keep working.

| Finger | Becomes | Why |
|---|---|---|
| Tap | Left click | |
| Press, move >10 pt | Left press + drag (from where the finger landed) | Timeline, viewer, keyframes, docks |
| **Press and hold ~0.45 s** | **Right click** + haptic tick | Every context menu |
| Two fingers moving | Scroll / pan | Viewer, timeline, panels |
| Pinch | Zoom | egui's own multi-touch (`zoom_delta`), passed through untouched |
| Quick two-finger tap | Undo (`edit.undo`) | |
| Quick three-finger tap | Redo (`edit.redo`) | |

The left press is deferred until the finger lifts or moves, which is what lets a hold become a
right click without a stray left click first. Hover is never emitted for fingers.

**On-screen bar** (bottom centre; collapsible): `Shift`, `Ctrl`, `Alt`, `Space` plus Undo, Redo,
Del, Esc, Enter and the soft keyboard. Tap a modifier to arm it for the *next* tap, double-tap to
lock (🔒). `Space` toggles the hand tool until tapped again. This covers the After Effects
idioms that need a key held while pointing: Alt-click stopwatch, Shift-constrain, Ctrl-click,
Space-pan.

The bar can be hidden and shown again from **Help ▸ On-Screen Touch Bar** (not remembered between
launches). **Help ▸ Show Debug Log...** opens a copyable dialog with the app's own log, which also
appears by itself after a crash.

**Mouse** (USB, Bluetooth, DeX): the winit patch maps hover, left/right/middle/back/forward
buttons and the wheel. **Hardware keyboard**: modifier state is taken from Android's meta state,
so Ctrl/Shift/Alt shortcuts work. **Soft keyboard**: GameActivity text events are diffed into
Backspace and IME-commit events.

A desktop touchscreen also gets the touch layer (switched on by its first touch).

## Files

Android gives apps `content://` URIs; the engine reads and writes paths. So:

* **Import / Open**: the system picker runs, each document is copied into the app's folder and
  its path is handed to the engine (pickers are asynchronous: they return at once and the result
  is applied on a later frame, as in the web build).
* **Save / Render**: the engine writes to `…/Android/data/ai.storyteller.effectcraft/files/exports/`
  and `Exports.kt` mirrors the file to the document you chose. Renders are copied once the file
  stops changing (~2 s); projects are copied again after every save while the app runs.
* **Open with**: `.ecproj` files from other apps open directly.

## Known limitations

* Large footage is **copied** on import (no streaming from the URI). Plenty of free space needed.
* "Stopped changing" is a heuristic for finished renders (see `Exports.kt`).
* Pickers whose result the UI consumes synchronously (Settings ▸ paths, shortcut-preset
  import/export, Open Folder) are not wired: they behave as on the web build.
* Dense desktop panels (timeline keyframes, graph-editor handles, small icons) are only enlarged
  by global metrics in `touch::apply_style`; expect per-panel tuning once seen on a device.
* The wgpu device is whatever the phone's driver offers. Heavy 3D, particle and simulation
  effects will be much slower than on a desktop GPU, and CPU fallbacks slower still.
* Roto Brush / face-tracking model downloads, the control channel and MCP bridge are desktop
  features and are not started on Android.
* No Play Store packaging, signing config or CI job yet.

## Bring-up checklist

Things most likely to need a fix on first run, in the order to try them:

1. `cargo ndk … build -p effectcraft-android` compiles (any Android-only dependency feature
   issues surface here: `wgpu`, `cpal`, `rayon`, `boa_engine`, `wasmi`).
2. The app starts and draws a first frame (`adb logcat`, and *Help ▸ Enable Logging* writes
   `<files>/config/Logs/EffectCraft Log.txt`; `adb shell run-as ai.storyteller.effectcraft`).
3. Long press opens context menus (check the press isn't also a click).
4. Two-finger pan and pinch in the Composition and Timeline panels.
5. Mouse: hover, right click, wheel; keyboard: Ctrl+Z, Ctrl+S, Space.
6. Soft keyboard text entry in a layer-rename field (GameActivity IME; Gboard composition is the
   most likely thing to misbehave).
7. Import a video, render, and confirm the file lands where the picker said.
8. Rotate the device / background and resume (surface recreation).
