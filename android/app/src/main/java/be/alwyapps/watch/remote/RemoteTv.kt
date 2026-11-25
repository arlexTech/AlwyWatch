// SPDX-License-Identifier: AGPL-3.0-or-later
package be.alwyapps.watch.remote

import android.content.Context
import android.content.Intent
import android.os.Build
import android.provider.Settings
import be.alwyapps.watch.core.AlwyWatch
import be.alwyapps.watch.core.CoreException
import be.alwyapps.watch.core.keep.RemoteAction
import be.alwyapps.watch.core.keep.RemoteCommand
import be.alwyapps.watch.core.keep.RemoteReport
import be.alwyapps.watch.player.PlayerActivity
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.Job
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.MutableSharedFlow
import kotlinx.coroutines.flow.SharedFlow
import kotlinx.coroutines.isActive
import kotlinx.coroutines.launch

/**
 * Play on the TV, the TV side (docs/keep-protocol.md "Play on the TV"): while AlwyWatch is open
 * on an Android TV with a profile connected to an AlwyWatch server, it listens there, so the
 * person's phones see it and can start a title on it. A Play opens the player; the other
 * commands go to the player that is open ([commands]), which reports what it plays ([report]).
 */
object RemoteTv {
    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.Main.immediate)
    private val commandFlow = MutableSharedFlow<RemoteCommand>(extraBufferCapacity = 8)

    /** Pause, seek, tracks, stop: for the player in front. */
    val commands: SharedFlow<RemoteCommand> = commandFlow

    private var started = 0
    private var job: Job? = null
    private var core: AlwyWatch? = null
    private var context: Context? = null

    /** What the player said last (sent again with each report, and cleared when it closes). */
    @Volatile private var last: RemoteReport? = null

    /** MainActivity or PlayerActivity in front, on a TV. */
    fun activityStarted(
        ctx: Context,
        core: AlwyWatch?,
    ) {
        context = ctx.applicationContext
        if (core != null) this.core = core
        started++
        ensure()
    }

    fun activityStopped() {
        started = (started - 1).coerceAtLeast(0)
        if (started > 0) return
        job?.cancel()
        job = null
        // Gone from the phones' list at once (else after a minute without listening).
        val c = core ?: return
        if (runCatching { c.keepActive() }.getOrDefault(false)) {
            CoroutineScope(Dispatchers.IO).launch { runCatching { c.remoteLeave() } }
        }
    }

    fun setCore(core: AlwyWatch) {
        this.core = core
        ensure()
    }

    /** The player's state (an empty title: nothing playing). Sent when it changes. */
    fun report(r: RemoteReport) {
        if (r == last) return
        last = r
        val c = core ?: return
        scope.launch(Dispatchers.IO) {
            runCatching { if (c.keepActive()) c.remoteReport(r) }
        }
    }

    /** The player closed. */
    fun stopped() = report(RemoteReport("", null, null, null, false, false, 0, 0, emptyList(), emptyList(), null, null, null))

    /** "Living room TV": the name set in the TV's settings, else its model. */
    private fun deviceName(ctx: Context): String =
        runCatching { Settings.Global.getString(ctx.contentResolver, "device_name") }.getOrNull()?.takeIf { it.isNotBlank() }
            ?: Build.MODEL

    private fun ensure() {
        if (started == 0 || job?.isActive == true) return
        job =
            scope.launch {
                var after = 0uL
                var failures = 0
                while (isActive) {
                    val c = core
                    val ctx = context
                    if (c == null || ctx == null || !runCatching { c.keepActive() }.getOrDefault(false)) {
                        delay(10_000)
                        continue
                    }
                    val r =
                        try {
                            c.remoteListen(deviceName(ctx), after)
                        } catch (_: CoreException) {
                            failures++
                            delay((failures * 3_000L).coerceAtMost(30_000))
                            continue
                        }
                    failures = 0
                    // The first answer only says where the queue stands: nothing old is replayed.
                    val fresh = after != 0uL
                    after = r.lastSeq
                    if (fresh) r.commands.forEach { take(c, ctx, it) }
                }
            }
    }

    private suspend fun take(
        core: AlwyWatch,
        ctx: Context,
        cmd: RemoteCommand,
    ) {
        if (cmd.action != RemoteAction.PLAY) {
            commandFlow.tryEmit(cmd)
            return
        }
        val play =
            try {
                core.remoteResolve(cmd)
            } catch (_: CoreException) {
                null
            }
        if (play == null) {
            val title = cmd.media?.title ?: ""
            report(
                RemoteReport(
                    title,
                    null,
                    null,
                    null,
                    false,
                    false,
                    0,
                    0,
                    emptyList(),
                    emptyList(),
                    null,
                    null,
                    be.alwyapps.watch.ui
                        .tr("Not on this TV's servers"),
                ),
            )
            return
        }
        // No position from the phone: where this TV's server says (resume, or the start).
        val start =
            play.positionMs ?: runCatching {
                core.refreshItem(play.accountId, play.itemId, false).item.user.let { u ->
                    if (u.played) 0L else u.positionMs.toLong()
                }
            }.getOrDefault(0L)
        ctx.startActivity(
            PlayerActivity
                .intent(ctx, play.accountId, play.itemId, start)
                .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK),
        )
    }
}
