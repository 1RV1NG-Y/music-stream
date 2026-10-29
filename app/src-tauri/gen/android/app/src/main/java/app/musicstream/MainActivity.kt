package app.musicstream

import android.content.res.Configuration
import android.graphics.Color
import android.os.Bundle
import androidx.activity.enableEdgeToEdge
import androidx.core.view.ViewCompat
import androidx.core.view.WindowCompat
import androidx.core.view.WindowInsetsCompat

class MainActivity : TauriActivity() {
  override fun onCreate(savedInstanceState: Bundle?) {
    enableEdgeToEdge()
    super.onCreate(savedInstanceState)
    // Keep the web view clear of the status bar, navigation bar, cutout and keyboard (as in Bits).
    val content = findViewById<android.view.View>(android.R.id.content)
    ViewCompat.setOnApplyWindowInsetsListener(content) { view, insets ->
      val bars = insets.getInsets(
        WindowInsetsCompat.Type.systemBars() or WindowInsetsCompat.Type.displayCutout() or WindowInsetsCompat.Type.ime()
      )
      view.setPadding(bars.left, bars.top, bars.right, bars.bottom)
      WindowInsetsCompat.CONSUMED
    }
    applyBarColors(resources.configuration)
    requestAudioPermission()
  }

  /** The library is the phone's audio files: Android 13+ asks for audio only. */
  private fun requestAudioPermission() {
    val permission = if (android.os.Build.VERSION.SDK_INT >= 33) android.Manifest.permission.READ_MEDIA_AUDIO
      else android.Manifest.permission.READ_EXTERNAL_STORAGE
    if (checkSelfPermission(permission) != android.content.pm.PackageManager.PERMISSION_GRANTED) {
      requestPermissions(arrayOf(permission), 47)
    }
  }

  override fun onStart() {
    super.onStart()
    LanSyncRuntime.setVisible(this, true)
    try {
      LanSyncService.startIfEnabled(this)
    } catch (error: Exception) {
      // Background-start restrictions (or vendor policy) must never crash the app.
      android.util.Log.e("LanSync", "Android rejected the foreground service start", error)
      LanSyncService.preferences(this).edit().putBoolean("enabled", false)
        .putString("last-error", "Android could not start background sync: ${error.message}").apply()
      android.widget.Toast.makeText(this, "Background serving could not start. Open music-stream and tap Background serving to retry.", android.widget.Toast.LENGTH_LONG).show()
    }
  }

  override fun onStop() {
    LanSyncRuntime.setVisible(this, false)
    super.onStop()
  }

  // The manifest handles uiMode changes itself (no activity restart), so follow them here.
  override fun onConfigurationChanged(newConfig: Configuration) {
    super.onConfigurationChanged(newConfig)
    applyBarColors(newConfig)
  }

  /** The strips behind the system bars are black, like the app (OLED black). */
  private fun applyBarColors(config: Configuration) {
    val night = (config.uiMode and Configuration.UI_MODE_NIGHT_MASK) == Configuration.UI_MODE_NIGHT_YES
    val content = findViewById<android.view.View>(android.R.id.content)
    content.setBackgroundColor(Color.BLACK)
    WindowCompat.getInsetsController(window, content).apply {
      isAppearanceLightStatusBars = false
      isAppearanceLightNavigationBars = false
    }
  }
}
