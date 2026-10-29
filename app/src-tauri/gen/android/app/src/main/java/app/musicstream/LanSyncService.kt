package app.musicstream

import android.app.*
import android.content.Context
import android.content.Intent
import android.content.pm.ServiceInfo
import android.net.*
import android.net.wifi.WifiManager
import android.os.Build
import android.os.IBinder
import android.os.PowerManager
import android.util.Log
import androidx.core.app.NotificationCompat
import androidx.core.content.ContextCompat

/** Keeps the phone serving its music on the local network, including when the Activity is gone. */
class LanSyncService : Service() {
  companion object {
    private const val CHANNEL = "lan-sync"
    private const val ID = 47110
    private const val PAUSE = "app.musicstream.PAUSE_LAN_SYNC"
    fun preferences(context: Context) = context.getSharedPreferences("lan-sync", Context.MODE_PRIVATE)
    fun enabled(context: Context) = preferences(context).getBoolean("enabled", true)
    fun startIfEnabled(context: Context) {
      if (enabled(context)) ContextCompat.startForegroundService(context, Intent(context, LanSyncService::class.java))
    }
    fun resume(context: Context) {
      preferences(context).edit().putBoolean("enabled", true).remove("last-error").apply()
      startIfEnabled(context)
    }
    fun pause(context: Context) {
      preferences(context).edit().putBoolean("enabled", false).apply()
      context.stopService(Intent(context, LanSyncService::class.java))
    }
  }

  private var cpu: PowerManager.WakeLock? = null
  private var wifi: WifiManager.WifiLock? = null
  private var multicast: WifiManager.MulticastLock? = null
  private var connectivity: ConnectivityManager? = null
  private var registered = false
  private var destroyed = false
  private val callback = object : ConnectivityManager.NetworkCallback() {
    override fun onAvailable(network: Network) = LanSyncRuntime.networkChanged()
    override fun onLost(network: Network) = LanSyncRuntime.networkChanged()
    override fun onLinkPropertiesChanged(network: Network, properties: LinkProperties) = LanSyncRuntime.networkChanged()
  }

  override fun onCreate() {
    super.onCreate()
    if (Build.VERSION.SDK_INT >= 26) {
      getSystemService(NotificationManager::class.java).createNotificationChannel(
        NotificationChannel(CHANNEL, "Serving music", NotificationManager.IMPORTANCE_LOW).apply {
          description = "Lets your paired computers play the music on this phone"
          setShowBadge(false)
        }
      )
    }
  }

  private fun notification(error: String? = null): Notification {
    val open = PendingIntent.getActivity(this, 0, Intent(this, MainActivity::class.java), PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE)
    val pause = PendingIntent.getService(this, 1, Intent(this, LanSyncService::class.java).setAction(PAUSE), PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE)
    val builder = NotificationCompat.Builder(this, CHANNEL)
      .setSmallIcon(R.drawable.ic_lan_sync)
      .setContentTitle(if (error == null) "Serving music" else "Serving music could not start")
      .setContentText(error ?: "Your paired computers can play this phone's music")
      .setContentIntent(open).setOngoing(error == null).setSilent(true)
    if (error == null) builder.addAction(0, "Pause", pause)
    return builder.build()
  }

  override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
    if (intent?.action == PAUSE) {
      preferences(this).edit().putBoolean("enabled", false).apply()
    }
    if (!enabled(this)) {
      stopSelf()
      return START_NOT_STICKY
    }
    try {
      if (Build.VERSION.SDK_INT >= 29) startForeground(ID, notification(), ServiceInfo.FOREGROUND_SERVICE_TYPE_CONNECTED_DEVICE)
      else startForeground(ID, notification())
      acquireResources()
      LanSyncRuntime.setService(this, true) { error ->
        if (error != null && !destroyed) {
          preferences(this).edit().putBoolean("enabled", false).putString("last-error", "The LAN runtime could not start: ${error.message}").apply()
          stopForeground(STOP_FOREGROUND_REMOVE)
          getSystemService(NotificationManager::class.java).notify(ID + 1, notification("Open music-stream and tap Background serving to retry."))
          stopSelf()
        }
      }
    } catch (e: Exception) {
      Log.e("LanSync", "Foreground listener failed", e)
      preferences(this).edit().putBoolean("enabled", false).putString("last-error", "Android could not start background serving: ${e.message}").apply()
      stopSelf()
      return START_NOT_STICKY
    }
    return START_STICKY
  }

  @Suppress("DEPRECATION")
  private fun acquireResources() {
    if (cpu != null) return
    cpu = (getSystemService(POWER_SERVICE) as PowerManager).newWakeLock(PowerManager.PARTIAL_WAKE_LOCK, "music-stream:lan").apply {
      setReferenceCounted(false)
      acquire()
    }
    val manager = applicationContext.getSystemService(WIFI_SERVICE) as WifiManager
    wifi = manager.createWifiLock(WifiManager.WIFI_MODE_FULL_HIGH_PERF, "music-stream:lan").apply {
      setReferenceCounted(false)
      acquire()
    }
    multicast = manager.createMulticastLock("music-stream:mdns").apply {
      setReferenceCounted(false)
      acquire()
    }
    connectivity = getSystemService(CONNECTIVITY_SERVICE) as ConnectivityManager
    // Do not require INTERNET/VALIDATED: a LAN without Internet must still work.
    connectivity!!.registerNetworkCallback(NetworkRequest.Builder().clearCapabilities()
      .addTransportType(NetworkCapabilities.TRANSPORT_WIFI)
      .addTransportType(NetworkCapabilities.TRANSPORT_ETHERNET).build(), callback)
    registered = true
  }

  override fun onDestroy() {
    destroyed = true
    if (registered) connectivity?.unregisterNetworkCallback(callback)
    multicast?.let { if (it.isHeld) it.release() }
    wifi?.let { if (it.isHeld) it.release() }
    cpu?.let { if (it.isHeld) it.release() }
    LanSyncRuntime.setService(this, false)
    stopForeground(STOP_FOREGROUND_REMOVE)
    super.onDestroy()
  }

  override fun onBind(intent: Intent?): IBinder? = null
}
