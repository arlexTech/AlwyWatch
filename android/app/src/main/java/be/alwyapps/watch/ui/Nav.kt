// SPDX-License-Identifier: AGPL-3.0-or-later
package be.alwyapps.watch.ui

import androidx.compose.runtime.mutableStateListOf
import androidx.compose.runtime.staticCompositionLocalOf
import be.alwyapps.watch.core.AlwyWatch

/** App destinations. Encoded as strings so the back stack survives process death. */
sealed interface Route {
    data object Home : Route

    data object Search : Route

    data object Settings : Route

    data object AddServer : Route

    data object Downloads : Route

    data object Requests : Route

    data object SeerrConnect : Route

    data object LeavingSoon : Route

    data object Triage : Route

    data object KeepAdmin : Route

    data class Library(
        val id: String,
        val title: String,
    ) : Route

    /** [season]: for a show, the season to open (from an episode's "Season N" link). */
    data class Detail(
        val itemId: String,
        val season: String? = null,
    ) : Route

    /** A show's downloaded episodes (works offline). */
    data class DownloadedShow(
        val seriesId: String,
        val title: String,
    ) : Route

    data object ManageDownloads : Route

    /** A library narrowed to a genre or a collection (from its Categories / Collections tabs). */
    data class Filtered(
        val libraryId: String,
        val title: String,
        val genreId: String?,
        val collectionId: String?,
    ) : Route

    fun encode(): String =
        when (this) {
            Home -> "home"
            Search -> "search"
            Settings -> "settings"
            AddServer -> "add"
            Downloads -> "downloads"
            Requests -> "requests"
            SeerrConnect -> "seerr-connect"
            LeavingSoon -> "leaving"
            Triage -> "triage"
            KeepAdmin -> "keep-admin"
            is Library -> "lib\u001f$id\u001f$title"
            is Detail -> if (season != null) "item\u001f$itemId\u001f$season" else "item\u001f$itemId"
            is DownloadedShow -> "dlshow\u001f$seriesId\u001f$title"
            ManageDownloads -> "dlmanage"
            is Filtered -> "filtered\u001f$libraryId\u001f$title\u001f${genreId.orEmpty()}\u001f${collectionId.orEmpty()}"
        }

    companion object {
        fun decode(s: String): Route? {
            val p = s.split('\u001f')
            return when (p[0]) {
                "home" -> Home
                "search" -> Search
                "settings" -> Settings
                "add" -> AddServer
                "downloads" -> Downloads
                "requests" -> Requests
                "seerr-connect" -> SeerrConnect
                "leaving" -> LeavingSoon
                "triage" -> Triage
                "keep-admin" -> KeepAdmin
                "lib" -> if (p.size == 3) Library(p[1], p[2]) else null
                "item" -> if (p.size >= 2) Detail(p[1], p.getOrNull(2)) else null
                "dlshow" -> if (p.size == 3) DownloadedShow(p[1], p[2]) else null
                "dlmanage" -> ManageDownloads
                "filtered" -> if (p.size == 5) Filtered(p[1], p[2], p[3].ifEmpty { null }, p[4].ifEmpty { null }) else null
                else -> null
            }
        }
    }
}

/**
 * A tiny back stack (no navigation library): a list of routes, persisted in the core settings
 * so a cold start reopens the last screen instantly.
 */
class Navigator(
    private val core: AlwyWatch,
    initial: List<Route>,
) {
    val stack = mutableStateListOf<Route>().apply { addAll(initial.ifEmpty { listOf(Route.Home) }) }
    val current: Route get() = stack.last()

    fun push(route: Route) {
        if (current != route) stack.add(route)
        save()
    }

    /** Replaces the whole stack (e.g. after sign-in). */
    fun reset(route: Route) {
        stack.clear()
        stack.add(route)
        save()
    }

    /** Returns false when there is nothing to pop (let the system close the app). */
    fun pop(): Boolean {
        if (stack.size <= 1) return false
        stack.removeAt(stack.lastIndex)
        save()
        return true
    }

    private fun save() {
        runCatching { core.setSetting(KEY, stack.joinToString("\u001e") { it.encode() }) }
    }

    companion object {
        private const val KEY = "ui.back_stack"

        fun restore(core: AlwyWatch): List<Route> =
            runCatching { core.getSetting(KEY) }
                .getOrNull()
                ?.split('\u001e')
                ?.mapNotNull(Route::decode)
                .orEmpty()
    }
}

val LocalNavigator = staticCompositionLocalOf<Navigator> { error("no navigator") }
val LocalCore = staticCompositionLocalOf<AlwyWatch> { error("no core") }
val LocalAccount = staticCompositionLocalOf<String> { error("no account") }
