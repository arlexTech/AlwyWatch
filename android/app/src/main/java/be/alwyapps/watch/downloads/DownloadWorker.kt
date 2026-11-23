// SPDX-License-Identifier: AGPL-3.0-or-later
package be.alwyapps.watch.downloads

import android.app.NotificationChannel
import android.app.NotificationManager
import android.content.Context
import android.content.pm.ServiceInfo
import android.os.Build
import androidx.core.app.NotificationCompat
import androidx.work.Constraints
import androidx.work.CoroutineWorker
import androidx.work.ExistingWorkPolicy
import androidx.work.ForegroundInfo
import androidx.work.NetworkType
import androidx.work.OneTimeWorkRequestBuilder
import androidx.work.WorkManager
import androidx.work.WorkerParameters
import be.alwyapps.watch.core.downloads.DownloadState
import be.alwyapps.watch.services
import be.alwyapps.watch.ui.tr
import kotlinx.coroutines.coroutineScope
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch

/**
 * Runs the core's download queue as a foreground job. WorkManager only starts it with a network
 * and starts it again (with backoff) when downloads are left waiting for the network.
 */
class DownloadWorker(
    ctx: Context,
    params: WorkerParameters,
) : CoroutineWorker(ctx, params) {
    override suspend fun doWork(): Result =
        coroutineScope {
            val core = applicationContext.services.core.await()
            runCatching { setForeground(foregroundInfo(tr("Starting downloads…"), 0, 0)) }
            val progress =
                launch {
                    while (true) {
                        val running = core.downloads().firstOrNull { it.state == DownloadState.RUNNING }
                        if (running != null) {
                            val total = running.total?.toLong() ?: 0L
                            val done = running.downloaded.toLong()
                            val pct = if (total > 0) (done * 100 / total).toInt() else 0
                            runCatching { setForeground(foregroundInfo(running.title, pct, if (total > 0) 100 else 0)) }
                        }
                        delay(1000)
                    }
                }
            try {
                core.runDownloads()
            } catch (_: Exception) {
                return@coroutineScope Result.retry()
            } finally {
                progress.cancel()
            }
            if (core.hasPendingDownloads()) Result.retry() else Result.success()
        }

    private fun foregroundInfo(
        title: String,
        progress: Int,
        max: Int,
    ): ForegroundInfo {
        val nm = applicationContext.getSystemService(NotificationManager::class.java)
        if (Build.VERSION.SDK_INT >= 26) {
            nm.createNotificationChannel(NotificationChannel(CHANNEL, tr("Downloads"), NotificationManager.IMPORTANCE_LOW))
        }
        val n =
            NotificationCompat
                .Builder(applicationContext, CHANNEL)
                .setSmallIcon(android.R.drawable.stat_sys_download)
                .setContentTitle(title)
                .setOngoing(true)
                .setOnlyAlertOnce(true)
                .setProgress(max, progress, max == 0)
                .build()
        return if (Build.VERSION.SDK_INT >= 29) {
            ForegroundInfo(NOTIFICATION_ID, n, ServiceInfo.FOREGROUND_SERVICE_TYPE_DATA_SYNC)
        } else {
            ForegroundInfo(NOTIFICATION_ID, n)
        }
    }

    companion object {
        private const val CHANNEL = "downloads"
        private const val NOTIFICATION_ID = 42
        private const val WORK = "downloads"

        fun start(ctx: Context) {
            val req =
                OneTimeWorkRequestBuilder<DownloadWorker>()
                    .setConstraints(Constraints.Builder().setRequiredNetworkType(NetworkType.CONNECTED).build())
                    .build()
            // APPEND_OR_REPLACE: a download added while the job ends is never missed.
            WorkManager.getInstance(ctx).enqueueUniqueWork(WORK, ExistingWorkPolicy.APPEND_OR_REPLACE, req)
        }
    }
}
