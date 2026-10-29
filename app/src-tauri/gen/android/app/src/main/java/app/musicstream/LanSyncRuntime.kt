package app.musicstream

import android.content.Context
import android.os.Handler
import android.os.Looper
import android.util.Log
import java.io.File
import java.util.concurrent.Executors
import java.util.concurrent.TimeUnit

/** No Activity/WebView references: a sticky service can recreate the Rust node by itself. */
object LanSyncRuntime {
  private val executor = Executors.newSingleThreadScheduledExecutor()
  private val main = Handler(Looper.getMainLooper())
  // Accessed only on executor. A visible app still needs its node when background
  // listening is paused. Give its last visibility checkpoint five seconds to finish.
  private var visible = false
  private var service = false
  private var visibilityGeneration = 0L
  private var loaded = false

  private external fun nativeSetRunning(path: String, running: Boolean)
  private external fun nativeNetworkChanged()

  private fun reconcile(context: Context) {
    if (!loaded) {
      System.loadLibrary("music_stream_lib")
      loaded = true
    }
    // Tauri app_data_dir() is Android's dataDir, NOT filesDir. Reuse the paired identity.
    nativeSetRunning(File(context.applicationInfo.dataDir, "library").absolutePath, visible || service)
  }

  fun setVisible(context: Context, value: Boolean) {
    val app = context.applicationContext
    executor.execute {
      val generation = ++visibilityGeneration
      if (value) {
        visible = true
        try { reconcile(app) } catch (e: Throwable) { Log.e("LanSync", "Library start failed", e) }
      } else {
        executor.schedule({
          if (generation == visibilityGeneration) {
            visible = false
            try { reconcile(app) } catch (e: Throwable) { Log.e("LanSync", "Library stop failed", e) }
          }
        }, 5, TimeUnit.SECONDS)
      }
    }
  }

  fun setService(context: Context, value: Boolean, done: (Throwable?) -> Unit = {}) {
    val app = context.applicationContext
    executor.execute {
      service = value
      val error = try { reconcile(app); null } catch (e: Throwable) {
        Log.e("LanSync", "LAN runtime transition failed", e)
        e
      }
      main.post { done(error) }
    }
  }

  fun networkChanged() {
    executor.execute {
      if (loaded) try { nativeNetworkChanged() } catch (e: Throwable) { Log.w("LanSync", "Network change failed", e) }
    }
  }
}
