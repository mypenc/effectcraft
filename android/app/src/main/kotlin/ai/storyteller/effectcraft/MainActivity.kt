package ai.storyteller.effectcraft

import android.content.Intent
import android.net.Uri
import android.os.Bundle
import android.provider.OpenableColumns
import android.util.Log
import android.view.View
import android.view.HapticFeedbackConstants
import androidx.activity.result.contract.ActivityResultContracts
import androidx.core.view.ViewCompat
import androidx.core.view.WindowCompat
import androidx.core.view.WindowInsetsCompat
import com.google.androidgamesdk.GameActivity
import java.io.File
import java.util.concurrent.Executors

/**
 * Hosts the Rust app. Rust calls the `@Suppress("unused")` methods below over JNI (see
 * `apps/effectcraft-android/src/bridge.rs`); results go back through [NativeBridge].
 *
 * Android gives apps `content://` URIs, but the engine reads and writes plain paths. So:
 *  - **import / open**: each picked document is copied into `filesDir/imports/…` and its path
 *    is handed to Rust;
 *  - **save / render**: Rust writes to `getExternalFilesDir()/exports/<name>` and [Exports]
 *    mirrors it to the document picked here.
 */
class MainActivity : GameActivity() {
    private val io = Executors.newSingleThreadExecutor()
    private lateinit var exports: Exports

    private var pickKind = 0
    private val pickOne = registerForActivityResult(ActivityResultContracts.OpenDocument()) { uri ->
        deliver(pickKind, listOfNotNull(uri))
    }
    private val pickMany = registerForActivityResult(ActivityResultContracts.OpenMultipleDocuments()) { uris ->
        deliver(pickKind, uris)
    }

    private class ExportRequest(val staged: File, val displayName: String, val keepSyncing: Boolean)

    private val exportQueue = ArrayDeque<ExportRequest>()
    private var exportActive: ExportRequest? = null
    private val createDocument = registerForActivityResult(ActivityResultContracts.CreateDocument("*/*")) { uri ->
        val request = exportActive
        exportActive = null
        if (uri != null && request != null) exports.watch(request.staged, uri, request.keepSyncing)
        launchNextExport()
    }

    override fun onCreate(savedInstanceState: Bundle?) {
        // Android 15 forces edge-to-edge, so the status/navigation bars would cover the top of the
        // app (the menu bar). Nothing on the Rust side reads the insets, so pad the content view by
        // them: the app then draws only inside the safe area.
        WindowCompat.setDecorFitsSystemWindows(window, false)
        super.onCreate(savedInstanceState)
        val content = findViewById<View>(android.R.id.content)
        ViewCompat.setOnApplyWindowInsetsListener(content) { view, insets ->
            val bars = insets.getInsets(WindowInsetsCompat.Type.systemBars() or WindowInsetsCompat.Type.displayCutout())
            view.setPadding(bars.left, bars.top, bars.right, bars.bottom)
            WindowInsetsCompat.CONSUMED
        }
        ViewCompat.requestApplyInsets(content)
        exports = Exports(applicationContext)
        DebugLog.install(this)
        handleViewIntent(intent)
    }

    override fun onDestroy() {
        if (isFinishing) DebugLog.markClean(this)
        super.onDestroy()
    }

    override fun onNewIntent(intent: Intent) {
        super.onNewIntent(intent)
        handleViewIntent(intent)
    }

    // ---- called from Rust --------------------------------------------------------------

    /** Open the document picker; the result arrives via [NativeBridge.onPicked]. */
    @Suppress("unused")
    fun pickFiles(kind: Int, multiple: Boolean) = runOnUiThread {
        pickKind = kind
        if (multiple) pickMany.launch(arrayOf("*/*")) else pickOne.launch(arrayOf("*/*"))
    }

    /** Ask where to save; [stagedPath] is what the engine is about to write. */
    @Suppress("unused")
    fun requestExport(stagedPath: String, displayName: String, keepSyncing: Boolean) = runOnUiThread {
        exportQueue.addLast(ExportRequest(File(stagedPath), displayName, keepSyncing))
        launchNextExport()
    }

    /** Help ▸ Show Debug Log: a dialog with this app's log (copy / share). */
    @Suppress("unused")
    fun showLog() = runOnUiThread { DebugLog.show(this) }

    @Suppress("unused")
    fun haptic() = runOnUiThread {
        window.decorView.performHapticFeedback(HapticFeedbackConstants.LONG_PRESS)
    }

    // ---- helpers -----------------------------------------------------------------------

    private fun launchNextExport() {
        if (exportActive != null) return
        val next = exportQueue.removeFirstOrNull() ?: return
        exportActive = next
        createDocument.launch(next.displayName)
    }

    private fun handleViewIntent(intent: Intent?) {
        val uri = intent?.takeIf { it.action == Intent.ACTION_VIEW }?.data ?: return
        deliver(1, listOf(uri))
    }

    /** Copy picked documents into the app's folder (off the UI thread) and tell Rust. */
    private fun deliver(kind: Int, uris: List<Uri>) {
        if (uris.isEmpty()) {
            NativeBridge.onPicked(kind, emptyArray())
            return
        }
        io.execute {
            val dir = File(filesDir, "imports/${System.currentTimeMillis()}").apply { mkdirs() }
            val paths = uris.mapNotNull { uri ->
                try {
                    val target = File(dir, uniqueName(dir, displayName(uri)))
                    contentResolver.openInputStream(uri)?.use { input ->
                        target.outputStream().use { input.copyTo(it, 1 shl 20) }
                    } ?: return@mapNotNull null
                    target.absolutePath
                } catch (t: Throwable) {
                    Log.w("EffectCraft", "could not read $uri", t)
                    null
                }
            }
            NativeBridge.onPicked(kind, paths.toTypedArray())
        }
    }

    private fun displayName(uri: Uri): String {
        if (uri.scheme == "content") {
            contentResolver.query(uri, arrayOf(OpenableColumns.DISPLAY_NAME), null, null, null)?.use { c ->
                if (c.moveToFirst()) c.getString(0)?.let { return it.replace('/', '_') }
            }
        }
        return uri.lastPathSegment?.substringAfterLast('/') ?: "file"
    }

    private fun uniqueName(dir: File, name: String): String {
        if (!File(dir, name).exists()) return name
        val stem = name.substringBeforeLast('.', name)
        val ext = name.substringAfterLast('.', "").let { if (it.isEmpty()) "" else ".$it" }
        var n = 2
        while (File(dir, "$stem ($n)$ext").exists()) n++
        return "$stem ($n)$ext"
    }
}
