// SPDX-License-Identifier: AGPL-3.0-or-later
package be.alwyapps.watch.remote

import android.content.Context
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import be.alwyapps.watch.core.AlwyWatch
import be.alwyapps.watch.core.keep.RemoteDevice
import be.alwyapps.watch.player.PlayerActivity
import kotlinx.coroutines.delay

/**
 * Play on the TV, the phone side (docs/keep-protocol.md "Play on the TV"): the person's TVs
 * where AlwyWatch is open now ([devices], asked every 20 s while the app is in front), and a
 * Play that asks where to play when there is one ([start], [asking]).
 */
object RemotePhone {
    /** My TVs there now (empty: Play starts here, as before). */
    var devices by mutableStateOf<List<RemoteDevice>>(emptyList())
        private set

    /** A title to play, waiting for "This phone" or a TV (the chooser in MainActivity shows it). */
    data class Pending(
        val account: String,
        val itemId: String,
        /** Where to start (0: from the start), on the TV too: what the button said. */
        val startMs: Long,
    )

    var asking by mutableStateOf<Pending?>(null)

    /** Polls while the app is in front (called in a repeatOnLifecycle(STARTED) block). */
    suspend fun poll(core: AlwyWatch) {
        while (true) {
            refresh(core)
            delay(20_000)
        }
    }

    suspend fun refresh(core: AlwyWatch) {
        devices =
            if (runCatching { core.keepActive() }.getOrDefault(false)) {
                runCatching { core.remoteDevices() }.getOrDefault(devices)
            } else {
                emptyList()
            }
    }

    /** Play [itemId]: here at once when no TV is there, else ask where. */
    fun start(
        ctx: Context,
        account: String,
        itemId: String,
        startMs: Long,
    ) {
        if (devices.isEmpty()) {
            here(ctx, account, itemId, startMs)
        } else {
            asking = Pending(account, itemId, startMs)
        }
    }

    fun here(
        ctx: Context,
        account: String,
        itemId: String,
        startMs: Long,
    ) = ctx.startActivity(PlayerActivity.intent(ctx, account, itemId, startMs))
}

/**
 * Where the TV is, on the phone: a local clock set by each report and moved on here while
 * the TV plays (smooth, no jumps between reports), and moved at once by this phone's own
 * buttons (a pause stops it now, not when the TV's report comes back). Reports the TV sent
 * before the last button press are stale for the position and play state.
 */
class RemoteClock {
    private var base = 0L
    private var baseAt = 0L
    private var duration = 0L
    private var receivedAt = 0L
    private var serverNow = 0L
    private var actedAt = Long.MIN_VALUE

    var playing = false
        private set

    private fun mono() = android.os.SystemClock.elapsedRealtime()

    fun take(s: be.alwyapps.watch.core.keep.RemoteState) {
        val t = mono()
        serverNow = s.nowMs
        receivedAt = t
        duration = s.report.durationMs
        if (s.atMs < actedAt) return
        base =
            be.alwyapps.watch.core
                .remotePositionNow(s)
        baseAt = t
        playing = s.report.playing && !s.report.loading
    }

    fun now(): Long {
        val p = if (playing) base + (mono() - baseAt) else base
        return if (duration > 0) p.coerceIn(0, duration) else p.coerceAtLeast(0)
    }

    /** A button here: [playing] and [position] as they will be on the TV. */
    fun acted(
        playing: Boolean? = null,
        position: Long? = null,
    ) {
        base = position ?: now()
        baseAt = mono()
        playing?.let { this.playing = it }
        // The server's time now, give or take the trip: reports from before it are stale.
        actedAt = serverNow + (mono() - receivedAt) + 250
    }
}
