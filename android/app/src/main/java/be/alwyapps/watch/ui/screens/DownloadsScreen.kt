// SPDX-License-Identifier: AGPL-3.0-or-later
package be.alwyapps.watch.ui.screens

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.ui.Modifier
import androidx.compose.ui.unit.dp
import be.alwyapps.watch.core.downloads.DownloadState
import be.alwyapps.watch.ui.LocalDimens
import be.alwyapps.watch.ui.Muted
import be.alwyapps.watch.ui.tr

/** Every download, newest first, to pause, resume or delete (all accounts). */
@Composable
fun DownloadsScreen() {
    val d = LocalDimens.current
    val (list, refresh) = rememberDownloads()
    val bytes = list.filter { it.state == DownloadState.COMPLETED }.sumOf { (it.total ?: it.downloaded).toLong() }
    LazyColumn(
        Modifier.fillMaxSize(),
        contentPadding = PaddingValues(horizontal = d.gutter, vertical = 16.dp),
        verticalArrangement = Arrangement.spacedBy(10.dp),
    ) {
        item {
            be.alwyapps.watch.ui
                .PageTitle(tr("Manage downloads"), tr("{size} GB on this device", "size" to "%.1f".format(bytes / 1e9)))
        }
        if (list.isEmpty()) item { Text(tr("Nothing downloaded."), color = Muted) }
        items(list, key = { it.id }) { DownloadRow(it, refresh) }
    }
}
