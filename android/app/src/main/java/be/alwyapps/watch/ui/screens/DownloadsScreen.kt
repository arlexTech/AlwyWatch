// SPDX-License-Identifier: AGPL-3.0-or-later
package be.alwyapps.watch.ui.screens

import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.aspectRatio
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.width
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
import be.alwyapps.watch.ui.LocalCore
import be.alwyapps.watch.ui.LocalDimens
import be.alwyapps.watch.ui.Muted
import be.alwyapps.watch.ui.SurfaceColor
import be.alwyapps.watch.ui.thumb
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
    val account = be.alwyapps.watch.ui.LocalAccount.current
    val totalBytes = list.filter { it.state == DownloadState.COMPLETED }.sumOf { (it.total ?: it.downloaded).toLong() }
    LazyColumn(
        Modifier.fillMaxSize(),
        contentPadding =
            androidx.compose.foundation.layout
                .PaddingValues(horizontal = d.gutter, vertical = 16.dp),
        verticalArrangement = Arrangement.spacedBy(10.dp),
    ) {
        item {
            be.alwyapps.watch.ui.PageTitle(
                "Downloads",
                if (list.isEmpty()) "Watch without a connection" else "Available offline · %.1f GB".format(totalBytes / 1e9),
            )
        }
        if (list.isEmpty()) {
            item {
                Column(
                    Modifier
                        .fillMaxWidth()
                        .padding(top = 40.dp),
                    horizontalAlignment = Alignment.CenterHorizontally,
                    verticalArrangement = Arrangement.spacedBy(12.dp),
                ) {
                    androidx.compose.material3.Icon(be.alwyapps.watch.ui.Icons.Download, null, Modifier.height(56.dp).width(56.dp), tint = be.alwyapps.watch.ui.Subtle)
                    Text("Nothing downloaded yet", style = MaterialTheme.typography.titleMedium)
                    Text("Tap the download button on a movie or an episode.", color = Muted, style = MaterialTheme.typography.bodyMedium)
                }
            }
        }
        items(list, key = { it.id }) { dl ->
            // Artwork from the cache when the download belongs to the current account.
            val item = remember(dl.itemId) { if (dl.accountId == account) runCatching { core.cachedItem(account, dl.itemId)?.item }.getOrNull() else null }
            Row(
                Modifier
                    .fillMaxWidth()
                    .clip(RoundedCornerShape(d.radius + 4.dp))
                    .background(SurfaceColor)
                    .padding(10.dp),
                verticalAlignment = Alignment.CenterVertically,
                horizontalArrangement = Arrangement.spacedBy(14.dp),
            ) {
                androidx.compose.foundation.layout.Box(Modifier.width(120.dp).clip(RoundedCornerShape(10.dp))) {
                    be.alwyapps.watch.ui
                        .RemoteImage(dl.itemId, item?.let { it.thumb() }, 240.dp, Modifier.fillMaxWidth().aspectRatio(16f / 9f))
                }
                Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(4.dp)) {
                    Text(dl.title, style = MaterialTheme.typography.titleSmall, maxLines = 2, overflow = androidx.compose.ui.text.style.TextOverflow.Ellipsis)
                    Text(
                        dl.status(),
                        color = if (dl.state == DownloadState.FAILED || dl.state == DownloadState.PAUSED_NO_SPACE) be.alwyapps.watch.ui.Danger else Muted,
                        style = MaterialTheme.typography.bodySmall,
                    )
                    val total = dl.total?.toLong() ?: 0L
                    if (dl.state != DownloadState.COMPLETED && total > 0) {
                        LinearProgressIndicator(
                            progress = { (dl.downloaded.toLong().toFloat() / total).coerceIn(0f, 1f) },
                            modifier =
                                Modifier
                                    .fillMaxWidth()
                                    .padding(top = 4.dp)
                                    .height(4.dp)
                                    .clip(RoundedCornerShape(50)),
                            color = Accent,
                            trackColor = be.alwyapps.watch.ui.SurfaceHigh,
                        )
                    }
                }
                when (dl.state) {
                    DownloadState.COMPLETED ->
                        be.alwyapps.watch.ui.IconAction(be.alwyapps.watch.ui.Icons.Play, "Play", {
                            ctx.startActivity(PlayerActivity.intent(ctx, dl.accountId, dl.itemId, 0L))
                        })
                    DownloadState.RUNNING, DownloadState.QUEUED, DownloadState.WAITING ->
                        be.alwyapps.watch.ui.IconAction(be.alwyapps.watch.ui.Icons.Pause, "Pause", {
                            core.pauseDownload(dl.id)
                            tick++
                        })
                    else ->
                        be.alwyapps.watch.ui.IconAction(be.alwyapps.watch.ui.Icons.Refresh, "Resume", {
                            core.resumeDownload(dl.id)
                            DownloadWorker.start(ctx)
                            tick++
                        })
                }
                be.alwyapps.watch.ui.IconAction(be.alwyapps.watch.ui.Icons.Delete, "Delete", {
                    core.removeDownload(dl.id)
                    tick++
                })
            }
        }
    }
}
