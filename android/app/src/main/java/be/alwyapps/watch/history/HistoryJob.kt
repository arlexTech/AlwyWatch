// SPDX-License-Identifier: AGPL-3.0-or-later
package be.alwyapps.watch.history

import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.content.Context
import android.content.pm.ServiceInfo
import android.os.Build
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import androidx.core.app.NotificationCompat
import androidx.work.CoroutineWorker
import androidx.work.ExistingWorkPolicy
import androidx.work.ForegroundInfo
import androidx.work.OneTimeWorkRequestBuilder
import androidx.work.WorkManager
import androidx.work.WorkerParameters
import androidx.work.workDataOf
import be.alwyapps.watch.MainActivity
import be.alwyapps.watch.core.CoreException
import be.alwyapps.watch.core.HistoryCopyResult
import be.alwyapps.watch.core.HistoryPlan
import be.alwyapps.watch.services
import be.alwyapps.watch.ui.tr
import be.alwyapps.watch.ui.trn
import kotlinx.coroutines.coroutineScope
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch

/**
 * The watch history check or copy in progress, for the whole app process. It runs as a
 * foreground job (a notification with its progress), so leaving the app doesn't stop it and
 * battery savers (OnePlus "smart mode"…) leave it alone; the page shows where it is on return.
 */
object HistoryJob {
    var from by mutableStateOf<String?>(null)
    var to by mutableStateOf<String?>(null)
    var plan by mutableStateOf<HistoryPlan?>(null)
    var result by mutableStateOf<HistoryCopyResult?>(null)
    var running by mutableStateOf(false)
    var error by mutableStateOf<String?>(null)

    fun reset() {
        plan = null
        result = null
        error = null
    }

    /** Reads both histories and builds the plan (writes nothing). */
    fun check(
        ctx: Context,
        from: String,
        to: String,
    ) {
        if (running) return
        reset()
        enqueue(ctx, MODE_CHECK, from, to)
    }

    /** Writes the checked plan's changes. */
    fun copy(ctx: Context) {
        val p = plan ?: return
        val to = to ?: return
        if (running || p.changes.isEmpty()) return
        enqueue(ctx, MODE_COPY, from.orEmpty(), to)
    }

    private fun enqueue(
        ctx: Context,
        mode: String,
        from: String,
        to: String,
    ) {
        running = true
        error = null
        val req =
            OneTimeWorkRequestBuilder<HistoryWorker>()
                .setInputData(workDataOf(KEY_MODE to mode, KEY_FROM to from, KEY_TO to to))
                .build()
        WorkManager.getInstance(ctx).enqueueUniqueWork(WORK, ExistingWorkPolicy.KEEP, req)
    }

    internal const val MODE_CHECK = "check"
    internal const val MODE_COPY = "copy"
    internal const val KEY_MODE = "mode"
    internal const val KEY_FROM = "from"
    internal const val KEY_TO = "to"
    private const val WORK = "history-copy"
}

class HistoryWorker(
    ctx: Context,
    params: WorkerParameters,
) : CoroutineWorker(ctx, params) {
    override suspend fun doWork(): Result =
        coroutineScope {
            val job = HistoryJob
            val mode = inputData.getString(HistoryJob.KEY_MODE)
            val from = inputData.getString(HistoryJob.KEY_FROM).orEmpty()
            val to = inputData.getString(HistoryJob.KEY_TO).orEmpty()
            val core = applicationContext.services.core.await()
            runCatching { setForeground(info(tr("Copy watch history"), tr("Starting…"), 0, 0)) }
            val progress =
                launch {
                    // Posted again only when it changes: a repost every second made the
                    // notification jump in the shade (and woke the screen on some phones).
                    var last: Any? = null
                    while (true) {
                        val p = runCatching { core.historyCopyProgress() }.getOrNull()
                        if (p != null && p.running) {
                            val total = p.total.toInt()
                            val done = p.done.toInt()
                            val text =
                                if (total > 0) {
                                    tr("{step} · {done} of {total}", "step" to p.step, "done" to done, "total" to total)
                                } else {
                                    tr("{step}…", "step" to p.step)
                                }
                            val shown = Triple(text, done, total)
                            if (shown != last) {
                                last = shown
                                runCatching { setForeground(info(tr("Copy watch history"), text, done, total)) }
                            }
                        }
                        delay(1000)
                    }
                }
            try {
                if (mode == HistoryJob.MODE_COPY) {
                    val changes = job.plan?.changes.orEmpty()
                    val r = core.historyCopyApply(to, changes)
                    job.result = r
                    job.plan = null
                    done(
                        tr("Watch history copied"),
                        if (r.failed > 0u) {
                            tr("{copied} titles, {failed} failed", "copied" to r.applied, "failed" to r.failed)
                        } else {
                            trn("{n} title", "{n} titles", r.applied.toLong())
                        },
                    )
                } else {
                    val p = core.historyCopyPlan(from, to)
                    job.plan = p
                    done(
                        tr("Watch history checked"),
                        if (p.changes.isEmpty()) {
                            tr("Nothing to copy")
                        } else {
                            trn("{n} title to copy: open AlwyWatch to confirm", "{n} titles to copy: open AlwyWatch to confirm", p.changes.size)
                        },
                    )
                }
            } catch (e: CoreException) {
                job.error = e.message
                done(tr("Copy watch history stopped"), e.message.orEmpty())
            } finally {
                progress.cancel()
                job.running = false
            }
            Result.success()
        }

    private fun channel(nm: NotificationManager) {
        if (Build.VERSION.SDK_INT >= 26) {
            nm.createNotificationChannel(NotificationChannel(CHANNEL, tr("Watch history"), NotificationManager.IMPORTANCE_LOW))
        }
    }

    private fun openApp(): PendingIntent =
        PendingIntent.getActivity(
            applicationContext,
            0,
            // As the launcher does: the running app comes to the front (no second copy).
            MainActivity.bringToFront(applicationContext),
            PendingIntent.FLAG_IMMUTABLE,
        )

    private fun info(
        title: String,
        text: String,
        done: Int,
        total: Int,
    ): ForegroundInfo {
        val nm = applicationContext.getSystemService(NotificationManager::class.java)
        channel(nm)
        val n =
            NotificationCompat
                .Builder(applicationContext, CHANNEL)
                .setSmallIcon(android.R.drawable.stat_notify_sync)
                .setContentTitle(title)
                .setContentText(text)
                .setContentIntent(openApp())
                .setOngoing(true)
                .setOnlyAlertOnce(true)
                .setProgress(total, done, total == 0)
                .build()
        return if (Build.VERSION.SDK_INT >= 29) {
            ForegroundInfo(ONGOING_ID, n, ServiceInfo.FOREGROUND_SERVICE_TYPE_DATA_SYNC)
        } else {
            ForegroundInfo(ONGOING_ID, n)
        }
    }

    /** The outcome, left in the notification shade after the job ends. */
    private fun done(
        title: String,
        text: String,
    ) {
        val nm = applicationContext.getSystemService(NotificationManager::class.java)
        channel(nm)
        val n =
            NotificationCompat
                .Builder(applicationContext, CHANNEL)
                .setSmallIcon(android.R.drawable.stat_notify_sync_noanim)
                .setContentTitle(title)
                .setContentText(text)
                .setContentIntent(openApp())
                .setAutoCancel(true)
                .build()
        runCatching { nm.notify(DONE_ID, n) }
    }

    private companion object {
        const val CHANNEL = "history"
        const val ONGOING_ID = 43
        const val DONE_ID = 44
    }
}
