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
