// SPDX-License-Identifier: AGPL-3.0-or-later
package be.alwyapps.watch

import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.compose.BackHandler
import androidx.activity.compose.setContent
import androidx.activity.enableEdgeToEdge
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.runtime.Composable
import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.runtime.getValue
import androidx.compose.runtime.key
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.produceState
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import be.alwyapps.watch.core.AlwyWatch
import be.alwyapps.watch.ui.AlwyTheme
import be.alwyapps.watch.ui.AppScaffold
import be.alwyapps.watch.ui.Background
import be.alwyapps.watch.ui.LocalAccount
import be.alwyapps.watch.ui.LocalCore
import be.alwyapps.watch.ui.LocalNavigator
import be.alwyapps.watch.ui.Navigator
import be.alwyapps.watch.ui.Route
import be.alwyapps.watch.ui.screens.AddServerScreen
import be.alwyapps.watch.ui.screens.DetailScreen
import be.alwyapps.watch.ui.screens.DownloadsScreen
import be.alwyapps.watch.ui.screens.HomeScreen
import be.alwyapps.watch.ui.screens.KeepAdminScreen
import be.alwyapps.watch.ui.screens.LeavingSoonScreen
import be.alwyapps.watch.ui.screens.LibraryGrid
import be.alwyapps.watch.ui.screens.LibraryScreen
import be.alwyapps.watch.ui.screens.RequestsScreen
import be.alwyapps.watch.ui.screens.SearchScreen
import be.alwyapps.watch.ui.screens.SeerrConnectScreen
import be.alwyapps.watch.ui.screens.SettingsScreen
import be.alwyapps.watch.ui.screens.TriageScreen

class MainActivity : ComponentActivity() {
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        enableEdgeToEdge()
        val services = services
        setContent {
            AlwyTheme(isTv = services.isTv) {
                // The window background is already the app color, so waiting a few ms for the
                // native core to load shows no flash; then the first composition renders from cache.
                val core by produceState<AlwyWatch?>(null) { value = services.core.await() }
                Box(Modifier.fillMaxSize().background(Background)) {
                    core?.let { Root(it) }
                }
            }
        }
    }
}

@Composable
private fun Root(core: AlwyWatch) {
    var account by remember {
        mutableStateOf(
            core.getSetting("current_account")?.takeIf { id -> core.isActive(id) }
                ?: core.accounts().firstOrNull { core.isActive(it.id) }?.id,
        )
    }
    val nav = remember(core) { Navigator(core, Navigator.restore(core)) }
    BackHandler(enabled = nav.stack.size > 1) { nav.pop() }

    CompositionLocalProvider(LocalCore provides core, LocalNavigator provides nav) {
        val acct = account
        if (acct == null) {
            Box(Modifier.fillMaxSize()) {
                AddServerScreen(onSignedIn = {
                    account = it
                    nav.reset(Route.Home)
                })
            }
            return@CompositionLocalProvider
        }
        // Re-create screens when the account changes (different server, different cache keys).
        key(acct) {
            CompositionLocalProvider(LocalAccount provides acct) {
                AppScaffold {
                    when (val r = nav.current) {
                        Route.Home -> HomeScreen()
                        Route.Search -> SearchScreen()
                        Route.Settings ->
                            SettingsScreen(onAccountChanged = {
                                account = it
                                nav.reset(Route.Home)
                            })
                        Route.AddServer ->
                            AddServerScreen(onSignedIn = {
                                account = it
                                nav.reset(Route.Home)
                            })
                        Route.Downloads ->
                            androidx.compose.foundation.layout.Column {
                                be.alwyapps.watch.ui
                                    .PageTitle("Downloads")
                                be.alwyapps.watch.ui.screens
                                    .DownloadsHome()
                            }
                        Route.ManageDownloads -> DownloadsScreen()
                        is Route.PlexSwitch ->
                            be.alwyapps.watch.ui.screens.PlexSwitchScreen(r.accountId, onSwitched = {
                                account = it
                                nav.reset(Route.Home)
                            })
                        is Route.DownloadedShow ->
                            be.alwyapps.watch.ui.screens
                                .DownloadedShowScreen(r.seriesId, r.title)
                        Route.Requests -> RequestsScreen()
                        Route.LeavingSoon -> LeavingSoonScreen()
                        Route.Triage -> TriageScreen()
                        Route.KeepAdmin -> KeepAdminScreen()
                        Route.SeerrConnect -> SeerrConnectScreen(onDone = { nav.reset(Route.Requests) })
                        is Route.Library -> LibraryScreen(r.id, r.title)
                        is Route.Filtered -> LibraryGrid(r.libraryId, r.title, r.genreId, r.collectionId)
                        is Route.Detail -> DetailScreen(r.itemId, r.season)
                    }
                }
            }
        }
    }
}
