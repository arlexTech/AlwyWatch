// SPDX-License-Identifier: AGPL-3.0-or-later
package be.alwyapps.watch.ui

import be.alwyapps.watch.core.AlwyWatch

/** App-level preferences (menu, clock, quitting), kept in the core settings. */
class UiPrefs(
    private val core: AlwyWatch,
    private val isTv: Boolean,
) {
    /** Back on the home asks before closing the app (TV: easy to press once too often). */
    var confirmQuit: Boolean
        get() = core.getSetting("ui.confirm_quit")?.let { it == "1" } ?: isTv
        set(v) = core.setSetting("ui.confirm_quit", if (v) "1" else "0")

    /** A clock in the top-right corner of TV screens. */
    var clock: Boolean
        get() = core.getSetting("ui.clock")?.let { it == "1" } ?: isTv
        set(v) = core.setSetting("ui.clock", if (v) "1" else "0")

    /** The menu's libraries: their order, and the hidden ones (by id). */
    var libraryOrder: List<String>
        get() =
            core
                .getSetting("ui.menu_libs")
                ?.split(',')
                ?.filter { it.isNotEmpty() }
                .orEmpty()
        set(v) = core.setSetting("ui.menu_libs", v.joinToString(","))

    var hiddenLibraries: Set<String>
        get() =
            core
                .getSetting("ui.menu_hidden")
                ?.split(',')
                ?.filter { it.isNotEmpty() }
                ?.toSet()
                .orEmpty()
        set(v) = core.setSetting("ui.menu_hidden", v.joinToString(","))

    /** [libs] in the user's order (new ones last), without the hidden ones unless [all]. */
    fun <T> arrange(
        libs: List<T>,
        id: (T) -> String,
        all: Boolean = false,
    ): List<T> {
        val order = libraryOrder
        val hidden = hiddenLibraries
        return libs
            .sortedBy { l -> order.indexOf(id(l)).let { if (it < 0) Int.MAX_VALUE else it } }
            .filter { all || id(it) !in hidden }
    }
}
