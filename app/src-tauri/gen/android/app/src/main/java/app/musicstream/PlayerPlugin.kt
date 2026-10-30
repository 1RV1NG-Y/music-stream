package app.musicstream

import android.app.Activity
import android.content.ComponentName
import android.net.Uri
import androidx.core.content.ContextCompat
import androidx.media3.common.C
import androidx.media3.common.MediaItem
import androidx.media3.common.MediaMetadata
import androidx.media3.common.Player
import androidx.media3.session.MediaController
import androidx.media3.session.SessionToken
import app.tauri.annotation.Command
import app.tauri.annotation.TauriPlugin
import app.tauri.plugin.Invoke
import app.tauri.plugin.JSArray
import app.tauri.plugin.JSObject
import app.tauri.plugin.Plugin
import java.io.File
import org.json.JSONObject

/**
 * The app's player commands on Android, run against the player service through a media
 * controller. Positions in the queue are in play order (shuffled when shuffle is on), like
 * the desktop player's, so the UI is the same on both.
 */
@TauriPlugin
class PlayerPlugin(private val activity: Activity) : Plugin(activity) {
  private var controller: MediaController? = null

  /** Runs `block` on the main thread with a connected controller (starting the service). */
  private fun withController(invoke: Invoke, block: (MediaController) -> Unit) {
    activity.runOnUiThread {
      val ready = controller?.takeIf { it.isConnected }
      if (ready != null) {
        run(invoke) { block(ready) }
        return@runOnUiThread
      }
      val token = SessionToken(activity, ComponentName(activity, PlayerService::class.java))
      val future = MediaController.Builder(activity, token).buildAsync()
      future.addListener({
        try {
          val c = future.get()
          controller = c
          run(invoke) { block(c) }
        } catch (e: Exception) {
          invoke.reject("Couldn't start the player: ${e.message}")
        }
      }, ContextCompat.getMainExecutor(activity))
    }
  }

  private fun run(invoke: Invoke, block: () -> Unit) {
    try {
      block()
    } catch (e: Exception) {
      invoke.reject(e.message ?: e.toString())
    }
  }

  private fun item(o: JSONObject): MediaItem {
    val meta = MediaMetadata.Builder()
      .setTitle(o.optString("title"))
      .setArtist(o.optString("artist"))
      .setAlbumTitle(o.optString("album"))
    o.optString("artwork").takeIf { it.isNotEmpty() }?.let { path ->
      // Thumbnails are small JPEGs in the app's own storage; the notification needs the bytes.
      runCatching { File(path).readBytes() }.getOrNull()?.let { meta.setArtworkData(it, MediaMetadata.PICTURE_TYPE_FRONT_COVER) }
    }
    val uri = Uri.fromFile(File(o.getString("path")))
    return MediaItem.Builder()
      .setMediaId(o.getString("id"))
      .setUri(uri)
      .setRequestMetadata(MediaItem.RequestMetadata.Builder().setMediaUri(uri).build())
      .setMediaMetadata(meta.build())
      .build()
  }

  private fun items(args: JSObject, key: String = "items"): List<MediaItem> {
    val arr = args.getJSONArray(key)
    return (0 until arr.length()).map { item(arr.getJSONObject(it)) }
  }

  /** Media item indices in the order they play. */
  private fun order(c: MediaController): List<Int> {
    val t = c.currentTimeline
    if (t.isEmpty) return emptyList()
    val out = mutableListOf<Int>()
    var i = t.getFirstWindowIndex(c.shuffleModeEnabled)
    while (i != C.INDEX_UNSET && out.size < t.windowCount) {
      out.add(i)
      i = t.getNextWindowIndex(i, Player.REPEAT_MODE_OFF, c.shuffleModeEnabled)
    }
    return out
  }

  private fun itemAt(c: MediaController, position: Int): Int = order(c).getOrNull(position) ?: position

  @Command
  fun setQueue(invoke: Invoke) {
    val args = invoke.getArgs()
    withController(invoke) { c ->
      val list = items(args)
      val start = args.optInt("start", 0).coerceIn(0, maxOf(0, list.size - 1))
      if (args.has("shuffle")) c.shuffleModeEnabled = args.getBoolean("shuffle")
      c.setMediaItems(list, start, 0L)
      c.prepare()
      if (args.optBoolean("play", true)) c.play()
      invoke.resolve()
    }
  }

  @Command
  fun insert(invoke: Invoke) {
    val args = invoke.getArgs()
    withController(invoke) { c ->
      val list = items(args)
      // "Play next" lands right after the current song. With shuffle on, Media3's shuffle
      // order puts added items at random places (a custom shuffle order would fix it).
      val at = if (args.optBoolean("next")) c.currentMediaItemIndex + 1 else c.mediaItemCount
      c.addMediaItems(at.coerceIn(0, c.mediaItemCount), list)
      if (c.mediaItemCount == list.size) c.prepare()
      invoke.resolve()
    }
  }

  @Command
  fun remove(invoke: Invoke) {
    val position = invoke.getArgs().getInt("index")
    withController(invoke) { c ->
      c.removeMediaItem(itemAt(c, position))
      invoke.resolve()
    }
  }

  @Command
  fun move(invoke: Invoke) {
    val args = invoke.getArgs()
    withController(invoke) { c ->
      if (c.shuffleModeEnabled) throw IllegalStateException("Turn shuffle off to reorder the queue.")
      c.moveMediaItem(args.getInt("from"), args.getInt("to"))
      invoke.resolve()
    }
  }

  @Command
  fun jump(invoke: Invoke) {
    val position = invoke.getArgs().getInt("index")
    withController(invoke) { c ->
      c.seekToDefaultPosition(itemAt(c, position))
      c.play()
      invoke.resolve()
    }
  }

  @Command
  fun clear(invoke: Invoke) = withController(invoke) { c -> c.clearMediaItems(); c.stop(); invoke.resolve() }

  @Command
  fun toggle(invoke: Invoke) = withController(invoke) { c -> if (c.isPlaying || c.playWhenReady) c.pause() else { if (c.playbackState == Player.STATE_ENDED) c.seekTo(0, 0); c.play() }; invoke.resolve() }

  @Command
  fun play(invoke: Invoke) = withController(invoke) { c -> c.play(); invoke.resolve() }

  @Command
  fun pause(invoke: Invoke) = withController(invoke) { c -> c.pause(); invoke.resolve() }

  @Command
  fun next(invoke: Invoke) = withController(invoke) { c -> c.seekToNext(); invoke.resolve() }

  @Command
  fun prev(invoke: Invoke) = withController(invoke) { c -> c.seekToPrevious(); invoke.resolve() }

  @Command
  fun seek(invoke: Invoke) {
    val ms = invoke.getArgs().getLong("ms")
    withController(invoke) { c -> c.seekTo(ms); invoke.resolve() }
  }

  @Command
  fun setShuffle(invoke: Invoke) {
    val on = invoke.getArgs().getBoolean("on")
    withController(invoke) { c -> c.shuffleModeEnabled = on; invoke.resolve() }
  }

  @Command
  fun setRepeat(invoke: Invoke) {
    val mode = when (invoke.getArgs().getString("mode")) {
      "all" -> Player.REPEAT_MODE_ALL
      "one" -> Player.REPEAT_MODE_ONE
      else -> Player.REPEAT_MODE_OFF
    }
    withController(invoke) { c -> c.repeatMode = mode; invoke.resolve() }
  }

  @Command
  fun state(invoke: Invoke) = withController(invoke) { c ->
    val order = order(c)
    val queue = JSArray()
    order.forEach { queue.put(c.getMediaItemAt(it).mediaId) }
    val position = order.indexOf(c.currentMediaItemIndex)
    val out = JSObject()
    out.put("index", if (c.mediaItemCount == 0 || position < 0) null else position)
    out.put("playing", c.playWhenReady && c.playbackState != Player.STATE_ENDED)
    out.put("positionMs", maxOf(0L, c.currentPosition))
    out.put("durationMs", c.duration.takeIf { it != C.TIME_UNSET } ?: 0L)
    out.put("buffering", c.playbackState == Player.STATE_BUFFERING)
    out.put("shuffle", c.shuffleModeEnabled)
    out.put("repeat", when (c.repeatMode) { Player.REPEAT_MODE_ALL -> "all"; Player.REPEAT_MODE_ONE -> "one"; else -> "off" })
    out.put("error", c.playerError?.message)
    out.put("queue", queue)
    invoke.resolve(out)
  }
}
