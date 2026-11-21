// SPDX-License-Identifier: AGPL-3.0-or-later
package be.alwyapps.watch

import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.setValue
import be.alwyapps.watch.core.AlwyWatch

/**
 * Settings that follow the viewer across devices (the core's `settings_sync`, kept with their
 * Jellyfin user). Run at start and whenever the app comes back; [version] bumps when settings
 * changed here, so the screens re-read them.
 */
object SettingsSync {
    var version by mutableIntStateOf(0)

    suspend fun run(core: AlwyWatch) {
        val changed = runCatching { core.syncSettings() }.getOrDefault(0u)
        if (changed > 0u) {
            kotlinx.coroutines.withContext(kotlinx.coroutines.Dispatchers.Main) { version++ }
        }
    }
}
