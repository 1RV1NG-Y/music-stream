package app.musicstream

import android.Manifest
import android.app.Activity
import android.content.Intent
import android.content.pm.PackageManager
import android.net.Uri
import android.os.Build
import android.os.PowerManager
import android.provider.Settings
import androidx.appcompat.app.AlertDialog
import androidx.core.app.ActivityCompat
import androidx.core.content.ContextCompat
import app.tauri.annotation.Command
import app.tauri.annotation.TauriPlugin
import app.tauri.plugin.Invoke
import app.tauri.plugin.Plugin

@TauriPlugin
class LanSyncPlugin(private val activity: Activity) : Plugin(activity) {
  @Command
  fun showControls(invoke: Invoke) {
    activity.runOnUiThread {
      val enabled = LanSyncService.enabled(activity)
      val exempt = (activity.getSystemService(Activity.POWER_SERVICE) as PowerManager).isIgnoringBatteryOptimizations(activity.packageName)
      val notifications = Build.VERSION.SDK_INT < 33 || ContextCompat.checkSelfPermission(activity, Manifest.permission.POST_NOTIFICATIONS) == PackageManager.PERMISSION_GRANTED
      val failure = LanSyncService.preferences(activity).getString("last-error", null)
      val choices = mutableListOf(if (enabled) "Pause background sync" else "Start / resume background sync")
      if (!exempt) choices.add("Allow listening during device idle…")
      if (!notifications) choices.add("Allow sync notifications…")
      AlertDialog.Builder(activity).setTitle("Background serving")
        .setMessage("${if (enabled) "Enabled" else "Paused"}. While enabled, music-stream keeps a local-network listener and CPU/Wi-Fi locks active so paired computers can play this phone's music with the screen off. This uses extra battery.\n\n${if (exempt) "Battery exemption is enabled." else "Android Doze can still block the network. For reliable receipt during idle, you can opt into a battery exemption."}\n\n${if (notifications) "Serving notifications are allowed." else "Notifications are blocked. Android can run the listener, but hides its notification; allow notifications below to see its status and Pause action."}\n\nPausing stops serving once the app leaves the screen. No cloud service is used.${if (failure == null) "" else "\n\n$failure"}")
        .setPositiveButton("Controls") { _, _ ->
          AlertDialog.Builder(activity).setTitle("Background serving")
            .setItems(choices.toTypedArray()) { _, index ->
              try {
                when (choices[index]) {
                  choices[0] -> if (enabled) LanSyncService.pause(activity) else {
                    LanSyncService.resume(activity)
                    if (!notifications && Build.VERSION.SDK_INT >= 33) ActivityCompat.requestPermissions(activity, arrayOf(Manifest.permission.POST_NOTIFICATIONS), 47110)
                  }
                  "Allow listening during device idle…" -> AlertDialog.Builder(activity)
                    .setTitle("Allow serving while idle?")
                    .setMessage("Android normally suspends network access during Doze, even with a foreground service. Exempting music-stream lets your paired computers play music while this phone is idle and can increase battery use. You can undo this in Android battery settings.")
                    .setPositiveButton("Open Android approval") { _, _ ->
                      activity.startActivity(Intent(Settings.ACTION_REQUEST_IGNORE_BATTERY_OPTIMIZATIONS, Uri.parse("package:${activity.packageName}")))
                    }.setNegativeButton("Cancel", null).show()
                  else -> if (Build.VERSION.SDK_INT >= 33) ActivityCompat.requestPermissions(activity, arrayOf(Manifest.permission.POST_NOTIFICATIONS), 47110)
                }
              } catch (e: Exception) {
                LanSyncService.preferences(activity).edit().putBoolean("enabled", false).putString("last-error", e.message).apply()
                AlertDialog.Builder(activity).setTitle("Could not change background serving").setMessage(e.message).setPositiveButton("OK", null).show()
              }
            }.setNegativeButton("Cancel", null).show()
        }.setNegativeButton("Done", null).show()
      invoke.resolve()
    }
  }
}
