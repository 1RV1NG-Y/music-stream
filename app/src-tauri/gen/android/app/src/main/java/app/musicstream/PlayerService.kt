package app.musicstream

import android.content.Intent
import android.os.Handler
import android.os.Looper
import android.util.Log
import androidx.media3.common.AudioAttributes
import androidx.media3.common.C
import androidx.media3.common.MediaItem
import androidx.media3.exoplayer.ExoPlayer
import androidx.media3.session.MediaSession
import androidx.media3.session.MediaSessionService
import com.google.common.util.concurrent.Futures
import com.google.common.util.concurrent.ListenableFuture
import java.util.concurrent.Executors

/**
 * The phone's player (PLAN.md §7): ExoPlayer in a media session, so the lock screen, the
 * notification, headset and Bluetooth buttons, audio focus and "headphones unplugged" all
 * work. It also counts plays (half the song, or 4 minutes) and records them through the node.
 */
class PlayerService : MediaSessionService() {
  private var session: MediaSession? = null
  private val main = Handler(Looper.getMainLooper())
  private var entry: String? = null
  private var heardMs = 0L
  private var lastMs = 0L
  private var counted = false

  override fun onCreate() {
    super.onCreate()
    val player = ExoPlayer.Builder(this)
      .setAudioAttributes(AudioAttributes.Builder().setUsage(C.USAGE_MEDIA).setContentType(C.AUDIO_CONTENT_TYPE_MUSIC).build(), true)
      .setHandleAudioBecomingNoisy(true)
      .setWakeMode(C.WAKE_MODE_LOCAL)
      .build()
    session = MediaSession.Builder(this, player).setCallback(object : MediaSession.Callback {
      // Controllers can't pass a file URI across processes; the app sends it as request metadata.
      override fun onAddMediaItems(
        mediaSession: MediaSession,
        controller: MediaSession.ControllerInfo,
        mediaItems: MutableList<MediaItem>,
      ): ListenableFuture<MutableList<MediaItem>> =
        Futures.immediateFuture(mediaItems.map { it.buildUpon().setUri(it.requestMetadata.mediaUri).build() }.toMutableList())
    }).build()
    main.post(tick)
  }

  override fun onGetSession(controllerInfo: MediaSession.ControllerInfo): MediaSession? = session

  // Swiping the app away keeps the music going; with nothing playing, the service ends.
  override fun onTaskRemoved(rootIntent: Intent?) {
    val p = session?.player
    if (p == null || !p.playWhenReady || p.mediaItemCount == 0) stopSelf()
  }

  override fun onDestroy() {
    main.removeCallbacks(tick)
    session?.run {
      player.release()
      release()
    }
    session = null
    super.onDestroy()
  }

  private val tick = object : Runnable {
    override fun run() {
      count()
      main.postDelayed(this, 1000)
    }
  }

  /** A play counts once half the song, or 4 minutes, has been heard (PLAN.md §4). */
  private fun count() {
    val p = session?.player ?: return
    val item = p.currentMediaItem ?: return
    val key = "${p.currentMediaItemIndex}:${item.mediaId}"
    val pos = p.currentPosition
    // A new entry, or the same one started over (repeat one, previous).
    if (key != entry || (pos + 2000 < lastMs && pos < 3000)) {
      entry = key
      heardMs = 0
      counted = false
      lastMs = pos
      return
    }
    // Real progress only: a seek forward adds at most one tick.
    if (p.isPlaying && pos > lastMs) heardMs += minOf(pos - lastMs, 1500)
    lastMs = pos
    val duration = p.duration.takeIf { it != C.TIME_UNSET && it > 0 } ?: return
    if (!counted && heardMs >= minOf(duration / 2, 240_000L)) {
      counted = true
      PlayerRuntime.recordPlay(item.mediaId)
    }
  }
}

/** Calls into the Rust node, off the main thread. */
object PlayerRuntime {
  private val executor = Executors.newSingleThreadExecutor()
  private external fun nativeRecordPlay(trackId: String)

  fun recordPlay(trackId: String) {
    executor.execute {
      try {
        System.loadLibrary("music_stream_lib")
        nativeRecordPlay(trackId)
      } catch (e: Throwable) {
        Log.e("Player", "Couldn't count a play", e)
      }
    }
  }
}
