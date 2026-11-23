// SPDX-License-Identifier: AGPL-3.0-or-later
package be.alwyapps.watch.keep

import android.app.NotificationChannel
import android.app.NotificationManager
import android.content.Context
import android.os.Build
import androidx.core.app.NotificationCompat
import androidx.work.CoroutineWorker
import androidx.work.ExistingPeriodicWorkPolicy
import androidx.work.PeriodicWorkRequestBuilder
import androidx.work.WorkManager
import androidx.work.WorkerParameters
import be.alwyapps.watch.services
import be.alwyapps.watch.ui.tr
import java.util.concurrent.TimeUnit

/** Daily: "you wanted to watch X before it leaves" for keep-until-watched with a reminder. */
class KeepReminderWorker(
    ctx: Context,
    params: WorkerParameters,
) : CoroutineWorker(ctx, params) {
    override suspend fun doWork(): Result {
        val core = applicationContext.services.core.await()
        if (!core.keepActive()) return Result.success()
        runCatching { core.refreshLeavingSoon(false) }
        val due = core.keepDueReminders(System.currentTimeMillis() / 1000)
        if (due.isEmpty()) return Result.success()
        val nm = applicationContext.getSystemService(NotificationManager::class.java)
        if (Build.VERSION.SDK_INT >= 26) {
            nm.createNotificationChannel(NotificationChannel(CHANNEL, tr("Watch reminders"), NotificationManager.IMPORTANCE_DEFAULT))
        }
        for ((i, item) in due.withIndex()) {
            val n =
                NotificationCompat
                    .Builder(applicationContext, CHANNEL)
                    .setSmallIcon(android.R.drawable.ic_media_play)
                    .setContentTitle(tr("Still want to watch {title}?", "title" to item.title))
                    .setContentText(tr("You asked to keep it until you have watched it."))
                    .setAutoCancel(true)
                    .build()
            runCatching { nm.notify(1000 + i, n) }
        }
        return Result.success()
    }

    companion object {
        private const val CHANNEL = "keep-reminders"

        fun schedule(ctx: Context) {
            WorkManager.getInstance(ctx).enqueueUniquePeriodicWork(
                "keep-reminders",
                ExistingPeriodicWorkPolicy.KEEP,
                PeriodicWorkRequestBuilder<KeepReminderWorker>(1, TimeUnit.DAYS).build(),
            )
        }
    }
}
