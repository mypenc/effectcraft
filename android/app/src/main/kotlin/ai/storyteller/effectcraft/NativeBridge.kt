package ai.storyteller.effectcraft

/**
 * Kotlin → Rust. The symbol is `Java_ai_storyteller_effectcraft_NativeBridge_onPicked` in
 * `apps/effectcraft-android/src/bridge.rs`; keep the package, object and method names in step.
 */
object NativeBridge {
    /** [kind]: 0 = import media, 1 = open a project. [paths]: files inside the app's folder. */
    @JvmStatic
    external fun onPicked(kind: Int, paths: Array<String>)
}
