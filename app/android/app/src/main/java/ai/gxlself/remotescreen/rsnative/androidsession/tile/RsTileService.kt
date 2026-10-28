package ai.gxlself.remotescreen.rsnative.androidsession.tile

import android.annotation.SuppressLint
import android.app.PendingIntent
import android.content.Intent
import android.graphics.drawable.Icon
import android.os.Build
import android.service.quicksettings.Tile
import android.service.quicksettings.TileService
import ai.gxlself.remotescreen.MainActivity
import ai.gxlself.remotescreen.R
import ai.gxlself.remotescreen.logic.androidsession.TileClick
import ai.gxlself.remotescreen.logic.androidsession.TileSubtitle
import ai.gxlself.remotescreen.logic.androidsession.tileClick
import ai.gxlself.remotescreen.logic.androidsession.tileView
import ai.gxlself.remotescreen.rsnative.common.RsLog

class RsTileService : TileService() {
  override fun onCreate() {
    super.onCreate()
    TileBridge.attach(this)
  }

  override fun onStartListening() {
    super.onStartListening()
    refreshTile()
  }

  override fun onClick() {
    super.onClick()
    if (isLocked) {
      unlockAndRun { handleClick() }
    } else {
      handleClick()
    }
  }

  private fun refreshTile() {
    val tile = qsTile ?: return
    val target = TileBridge.target()
    val running = TileBridge.hasRunningSessions()
    val view = tileView(running, target.hostId, target.label)
    val subtitle = when (val item = view.subtitle) {
      TileSubtitle.ReturnToSession -> TileStrings.RETURN_TO_SESSION
      is TileSubtitle.Target -> item.label
      TileSubtitle.OpenApp -> TileStrings.OPEN_APP
    }
    tile.label = "Remote Screen"
    tile.icon = Icon.createWithResource(this, R.drawable.rs_tile)
    tile.state = if (view.active) Tile.STATE_ACTIVE else Tile.STATE_INACTIVE
    if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q) {
      tile.subtitle = subtitle
    }
    tile.updateTile()
    RsLog.marker("TILE", mapOf("state" to view.marker))
  }

  private fun handleClick() {
    val target = TileBridge.target()
    val running = TileBridge.hasRunningSessions()
    when (val click = tileClick(running, target.hostId)) {
      TileClick.Resume -> {
        RsLog.marker("TILE", mapOf("click" to "resume"))
        TileBridge.request("")
      }
      is TileClick.Connect -> {
        RsLog.marker("TILE", mapOf("click" to "connect"))
        TileBridge.request(click.hostId)
      }
      TileClick.Open -> RsLog.marker("TILE", mapOf("click" to "open"))
    }
    bringActivityToFront()
  }

  @SuppressLint("StartActivityAndCollapseDeprecated")
  private fun bringActivityToFront() {
    val intent = Intent(this, MainActivity::class.java).apply {
      flags = Intent.FLAG_ACTIVITY_NEW_TASK or Intent.FLAG_ACTIVITY_SINGLE_TOP
    }
    if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.UPSIDE_DOWN_CAKE) {
      val pendingIntent = PendingIntent.getActivity(
        this,
        0,
        intent,
        PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT,
      )
      startActivityAndCollapse(pendingIntent)
    } else {
      @Suppress("DEPRECATION")
      startActivityAndCollapse(intent)
    }
  }
}
