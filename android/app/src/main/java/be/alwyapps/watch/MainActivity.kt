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
import androidx.lifecycle.lifecycleScope
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
import be.alwyapps.watch.ui.screens.KeepForeverScreen
import be.alwyapps.watch.ui.screens.LeavingSoonScreen
import be.alwyapps.watch.ui.screens.LibraryGrid
import be.alwyapps.watch.ui.screens.LibraryScreen
import be.alwyapps.watch.ui.screens.RequestsScreen
import be.alwyapps.watch.ui.screens.SearchScreen
import be.alwyapps.watch.ui.screens.SeerrConnectScreen
import be.alwyapps.watch.ui.screens.SettingsScreen
import be.alwyapps.watch.ui.screens.TriageScreen
import kotlinx.coroutines.launch

class MainActivity : ComponentActivity() {
    private var lastBackAt = Long.MIN_VALUE / 2
    private var echoDown = false

    // Some remotes (Chromecast with Google TV) send Back twice per press, ~20 ms apart: drop
    // the echo, or one press would go back two screens.
    override fun dispatchKeyEvent(event: android.view.KeyEvent): Boolean {
        if (event.keyCode == android.view.KeyEvent.KEYCODE_BACK) {
            if (event.action == android.view.KeyEvent.ACTION_DOWN && event.repeatCount == 0) {
                echoDown = event.eventTime - lastBackAt < 300
                lastBackAt = event.eventTime
            }
            if (echoDown) {
                if (event.action == android.view.KeyEvent.ACTION_UP) echoDown = false
                return true
            }
        }
        if (event.keyCode == android.view.KeyEvent.KEYCODE_DPAD_LEFT) {
            be.alwyapps.watch.ui.HeldLeft.held = event.action == android.view.KeyEvent.ACTION_DOWN && event.repeatCount > 0
        }
        return super.dispatchKeyEvent(event)
    }

    override fun onResume() {
        super.onResume()
        // Back to the app: settings changed on another device apply here.
        val core = services.core
        if (core.isCompleted) {
            lifecycleScope.launch(kotlinx.coroutines.Dispatchers.IO) { SettingsSync.run(core.await()) }
        }
    }

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        enableEdgeToEdge()
        val services = services
        // Once Compose draws (its root paints the background), the window's own background is a
        // full-screen fill under it on every frame for nothing.
        window.decorView.post { window.setBackgroundDrawable(null) }
        setContent {
            AlwyTheme(isTv = services.isTv) {
                // The window background is already the app color, so waiting a few ms for the
                // native core to load shows no flash; then the first composition renders from cache.
                val core by produceState<AlwyWatch?>(null) { value = services.core.await() }
                Box(Modifier.fillMaxSize().background(Background)) {
                    core?.let { c ->
                        androidx.compose.runtime.CompositionLocalProvider(be.alwyapps.watch.ui.LocalCore provides c) {
                            be.alwyapps.watch.ui.screens
                                .ProfileGate { Root(c) }
                        }
                    }
                }
            }
        }
    }
}

@Composable
private fun Root(core: AlwyWatch) {
    // Settings from another device (server in use…) re-read when they arrive.
    val synced = SettingsSync.version
    var account by remember(synced) {
        mutableStateOf(
            core.getSetting("current_account")?.takeIf { id -> core.isActive(id) }
                ?: core.accounts().firstOrNull { core.isActive(it.id) }?.id,
        )
    }
    val nav = remember(core) { Navigator(core, Navigator.restore(core)) }
    BackHandler(enabled = nav.stack.size > 1) { nav.pop() }
    // Top of the stack: another tab goes back home first; home asks before quitting (setting).
    val activity = androidx.compose.ui.platform.LocalContext.current as? android.app.Activity
    val isTv = be.alwyapps.watch.ui.LocalDimens.current.isTv
    var askQuit by remember { mutableStateOf(false) }
    BackHandler(
        enabled =
            nav.stack.size <= 1 &&
                (
                    nav.current != Route.Home ||
                        be.alwyapps.watch.ui
                            .UiPrefs(core, isTv)
                            .confirmQuit
                ),
    ) {
        if (nav.current != Route.Home) nav.reset(Route.Home) else askQuit = true
    }
    if (askQuit) {
        be.alwyapps.watch.ui.ConfirmDialog(
            "Quit AlwyWatch?",
            "You can turn this question off in Settings.",
            confirm = "Quit",
            onConfirm = { activity?.finish() },
            onDismiss = { askQuit = false },
        )
    }

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
        key(acct, synced) {
            CompositionLocalProvider(LocalAccount provides acct) {
                AppScaffold {
                    // One initial focus per screen visit (see initialFocus).
                    val focusDone = remember(nav.current) { mutableStateOf(false) }
                    CompositionLocalProvider(be.alwyapps.watch.ui.LocalInitialFocusDone provides focusDone) {
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
                            is Route.SeerrTitle ->
                                be.alwyapps.watch.ui.screens
                                    .SeerrTitleScreen(r.mediaType, r.tmdbId)
                            is Route.SeerrSaga ->
                                be.alwyapps.watch.ui.screens
                                    .SeerrSagaScreen(r.id, r.name)
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
                            Route.Together ->
                                be.alwyapps.watch.ui.screens
                                    .TogetherScreen()
                            Route.Watchlist ->
                                be.alwyapps.watch.ui.screens
                                    .WatchlistScreen()
                            is Route.SettingsSection ->
                                be.alwyapps.watch.ui.screens.SettingsSectionScreen(r.id, onAccountChanged = {
                                    account = it
                                    nav.reset(Route.Home)
                                })
                            Route.HistoryCopy ->
                                be.alwyapps.watch.ui.screens
                                    .HistoryCopyScreen()
                            Route.Triage -> TriageScreen()
                            Route.KeepAdmin -> KeepAdminScreen()
                            Route.KeepForever -> KeepForeverScreen()
                            Route.SeerrConnect -> SeerrConnectScreen(onDone = { nav.pop() })
                            // Combined home: a page of another account's server runs as that account.
                            is Route.Library ->
                                CompositionLocalProvider(LocalAccount provides (r.account ?: acct)) { LibraryScreen(r.id, r.title) }
                            is Route.Filtered ->
                                CompositionLocalProvider(LocalAccount provides (r.account ?: acct)) {
                                    LibraryGrid(r.libraryId, r.title, r.genreId, r.collectionId)
                                }
                            is Route.Detail ->
                                CompositionLocalProvider(LocalAccount provides (r.account ?: acct)) { DetailScreen(r.itemId, r.season) }
                        }
                    }
                }
            }
        }
    }
}
