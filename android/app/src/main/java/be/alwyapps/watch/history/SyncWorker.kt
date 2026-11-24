// SPDX-License-Identifier: AGPL-3.0-or-later
package be.alwyapps.watch.history

import android.app.NotificationChannel
import android.app.NotificationManager
import android.content.Context
import android.content.pm.ServiceInfo
import android.os.Build
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import androidx.core.app.NotificationCompat
import androidx.work.Constraints
import androidx.work.CoroutineWorker
import androidx.work.ExistingPeriodicWorkPolicy
import androidx.work.ExistingWorkPolicy
import androidx.work.ForegroundInfo
import androidx.work.NetworkType
import androidx.work.OneTimeWorkRequestBuilder
import androidx.work.PeriodicWorkRequestBuilder
import androidx.work.WorkManager
import androidx.work.WorkerParameters
import be.alwyapps.watch.core.AlwyWatch
import be.alwyapps.watch.services
import be.alwyapps.watch.ui.tr
import kotlinx.coroutines.coroutineScope
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch
import java.util.concurrent.TimeUnit

/**
 * Keeps paired servers' watch state in sync (the core's `sync`): every few hours in the
 * background, and at once when asked. What is watched in AlwyWatch is mirrored right away by
 * the core; this catches what other apps did. A foreground job while it runs, like the copy.
 */
class SyncWorker(
    ctx: Context,
    params: WorkerParameters,
) : CoroutineWorker(ctx, params) {
    override suspend fun doWork(): Result =
        coroutineScope {
            val core = applicationContext.services.core.await()
            if (core.syncPairs().isEmpty()) {
                SyncState.running = false
                return@coroutineScope Result.success()
            }
            SyncState.running = true
            runCatching { setForeground(info(tr("Syncing watch history…"), 0, 0)) }
            val progress =
                launch {
                    // Posted again only when it changes: a repost every second made the
                    // notification jump in the shade (and woke the screen on some phones).
                    var last: Any? = null
                    while (true) {
                        val p = runCatching { core.historyCopyProgress() }.getOrNull()
                        if (p != null && p.running) {
                            val text =
                                if (p.total > 0u) {
                                    tr("{step} · {done} of {total}", "step" to p.step, "done" to p.done, "total" to p.total)
                                } else {
                                    tr("{step}…", "step" to p.step)
                                }
                            val shown = Triple(text, p.done, p.total)
                            if (shown != last) {
                                last = shown
                                runCatching { setForeground(info(text, p.done.toInt(), p.total.toInt())) }
                            }
                        }
                        delay(1000)
                    }
                }
            try {
                core.syncNow()
            } catch (_: Exception) {
                return@coroutineScope Result.retry()
            } finally {
                progress.cancel()
                SyncState.running = false
                SyncState.version++
            }
            Result.success()
        }

    private fun info(
        text: String,
        done: Int,
        total: Int,
    ): ForegroundInfo {
        val nm = applicationContext.getSystemService(NotificationManager::class.java)
        if (Build.VERSION.SDK_INT >= 26) {
            nm.createNotificationChannel(NotificationChannel(CHANNEL, tr("Watch history"), NotificationManager.IMPORTANCE_LOW))
        }
        val n =
            NotificationCompat
                .Builder(applicationContext, CHANNEL)
                .setSmallIcon(android.R.drawable.stat_notify_sync)
                .setContentTitle(tr("Syncing watch history"))
                .setContentText(text)
                .setOngoing(true)
                .setOnlyAlertOnce(true)
                .setSilent(true)
                .setProgress(total, done, total == 0)
                .build()
        return if (Build.VERSION.SDK_INT >= 29) {
            ForegroundInfo(ID, n, ServiceInfo.FOREGROUND_SERVICE_TYPE_DATA_SYNC)
        } else {
            ForegroundInfo(ID, n)
        }
    }

    companion object {
        private const val CHANNEL = "history"
        private const val ID = 45
        private const val NOW = "sync-now"
        private const val PERIODIC = "sync-periodic"

        private fun network() = Constraints.Builder().setRequiredNetworkType(NetworkType.CONNECTED).build()

        /** Syncs now (turning a pair on, "Sync now"). */
        fun now(ctx: Context) {
            SyncState.running = true
            val req = OneTimeWorkRequestBuilder<SyncWorker>().setConstraints(network()).build()
            WorkManager.getInstance(ctx).enqueueUniqueWork(NOW, ExistingWorkPolicy.KEEP, req)
        }

        /** Every 6 hours while a pair is on; nothing otherwise. Cheap to call at every start. */
        fun schedule(
            ctx: Context,
            core: AlwyWatch,
        ) {
            val wm = WorkManager.getInstance(ctx)
            if (runCatching { core.syncPairs() }.getOrDefault(emptyList()).isEmpty()) {
                wm.cancelUniqueWork(PERIODIC)
                return
            }
            val req =
                PeriodicWorkRequestBuilder<SyncWorker>(6, TimeUnit.HOURS)
                    .setConstraints(network())
                    .build()
            wm.enqueueUniquePeriodicWork(PERIODIC, ExistingPeriodicWorkPolicy.KEEP, req)
        }
    }
}

/** For the Settings rows: whether a sync runs, and a bump when one ends (status changed). */
object SyncState {
    var running by mutableStateOf(false)
    var version by mutableIntStateOf(0)
}
