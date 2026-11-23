// SPDX-License-Identifier: AGPL-3.0-or-later
package be.alwyapps.watch.ui

import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import be.alwyapps.watch.core.CoreException

/** State of a cache-first resource. `value` is shown as soon as it exists, even while refreshing. */
class Swr<T : Any>(
    value: T?,
) {
    var value by mutableStateOf(value)
        internal set
    var refreshing by mutableStateOf(true)
        internal set
    var error by mutableStateOf<String?>(null)
        internal set

    /** The exception behind [error], to tell kinds apart without reading the (translated) text. */
    var failure by mutableStateOf<CoreException?>(null)
        internal set
    internal var nonce by mutableIntStateOf(0)
    internal var force = false

    /** User-triggered refresh (pull, retry button): bypasses the freshness TTL. */
    fun reload() {
        force = true
        nonce++
    }
}

/**
 * Renders from the local cache in the very first composition (a synchronous SQLite read, well
 * under a millisecond), then revalidates in the background. Errors never hide cached data.
 */
@Composable
fun <T : Any> rememberSwr(
    vararg keys: Any?,
    cached: () -> T?,
    refresh: suspend (force: Boolean) -> T,
): Swr<T> {
    val state = remember(*keys) { Swr(runCatching(cached).getOrNull()) }
    LaunchedEffect(state, state.nonce) {
        state.refreshing = true
        try {
            state.value = refresh(state.force)
            state.error = null
            state.failure = null
        } catch (e: CoreException) {
            state.error = e.message ?: e.javaClass.simpleName
            state.failure = e
        } finally {
            state.force = false
            state.refreshing = false
        }
    }
    return state
}
