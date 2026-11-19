// SPDX-License-Identifier: AGPL-3.0-or-later
package be.alwyapps.watch.ui.screens

import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.LinearProgressIndicator
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.unit.dp
import be.alwyapps.watch.core.downloads.Download
import be.alwyapps.watch.core.downloads.DownloadState
import be.alwyapps.watch.downloads.DownloadWorker
import be.alwyapps.watch.player.PlayerActivity
import be.alwyapps.watch.ui.Accent
import be.alwyapps.watch.ui.ActionButton
import be.alwyapps.watch.ui.LocalCore
import be.alwyapps.watch.ui.LocalDimens
import be.alwyapps.watch.ui.Muted
import be.alwyapps.watch.ui.SurfaceColor
import kotlinx.coroutines.delay

private fun Download.status(): String {
    val mb = { b: ULong -> "%.1f GB".format(b.toDouble() / 1e9) }
    val size = total?.let { "${mb(downloaded)} / ${mb(it)}" } ?: mb(downloaded)
    return when (state) {
        DownloadState.QUEUED -> "Queued · $size"
        DownloadState.RUNNING -> "Downloading · $size"
        DownloadState.PAUSED -> "Paused · $size"
        DownloadState.PAUSED_NO_SPACE -> "Paused: storage full. Free some space, then resume."
        DownloadState.WAITING -> "Waiting for the network · $size"
        DownloadState.FAILED -> "Failed: ${error ?: "unknown error"}"
        DownloadState.COMPLETED -> "Downloaded · ${total?.let(mb) ?: ""}"
    }
}

/** Downloads (phone and tablet only). Polls the core once a second while visible. */
@Composable
fun DownloadsScreen() {
    val core = LocalCore.current
    val ctx = LocalContext.current
    val d = LocalDimens.current
    var list by remember { mutableStateOf(runCatching { core.downloads() }.getOrDefault(emptyList())) }
    var tick by remember { mutableStateOf(0) }
    LaunchedEffect(tick) {
        while (true) {
            list = runCatching { core.downloads() }.getOrDefault(list)
            delay(1000)
        }
    }
    LazyColumn(Modifier.fillMaxSize().padding(horizontal = d.gutter), verticalArrangement = Arrangement.spacedBy(10.dp)) {
        item { Text("Downloads", Modifier.padding(vertical = 12.dp), style = MaterialTheme.typography.titleLarge) }
        if (list.isEmpty()) item { Text("Nothing downloaded yet. Use \"Download\" on a movie or an episode.", color = Muted) }
        items(list, key = { it.id }) { dl ->
            Column(
                Modifier
                    .fillMaxWidth()
                    .clip(RoundedCornerShape(8.dp))
                    .background(SurfaceColor)
                    .padding(12.dp),
            ) {
                Text(dl.title, style = MaterialTheme.typography.titleMedium)
                Text(dl.status(), color = Muted, style = MaterialTheme.typography.bodySmall)
                val total = dl.total?.toLong() ?: 0L
                if (dl.state != DownloadState.COMPLETED && total > 0) {
                    LinearProgressIndicator(
                        progress = { (dl.downloaded.toLong().toFloat() / total).coerceIn(0f, 1f) },
                        modifier = Modifier.fillMaxWidth().padding(vertical = 8.dp).height(4.dp),
                        color = Accent,
                    )
                }
                Row(Modifier.padding(top = 8.dp), horizontalArrangement = Arrangement.spacedBy(10.dp), verticalAlignment = Alignment.CenterVertically) {
                    when (dl.state) {
                        DownloadState.COMPLETED ->
                            ActionButton("Play", { ctx.startActivity(PlayerActivity.intent(ctx, dl.accountId, dl.itemId, 0L)) }, primary = true)
                        DownloadState.RUNNING, DownloadState.QUEUED, DownloadState.WAITING ->
                            ActionButton("Pause", {
                                core.pauseDownload(dl.id)
                                tick++
                            })
                        else ->
                            ActionButton("Resume", {
                                core.resumeDownload(dl.id)
                                DownloadWorker.start(ctx)
                                tick++
                            }, primary = true)
                    }
                    ActionButton("Delete", {
                        core.removeDownload(dl.id)
                        tick++
                    })
                }
            }
        }
    }
}
