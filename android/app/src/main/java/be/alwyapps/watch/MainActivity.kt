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
import androidx.compose.ui.focus.focusRequester
import androidx.compose.ui.focus.onFocusChanged
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
import be.alwyapps.watch.ui.tr
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
        if (event.action == android.view.KeyEvent.ACTION_DOWN) {
            be.alwyapps.watch.ui.HeldLeft.pressed = event.keyCode == android.view.KeyEvent.KEYCODE_DPAD_LEFT
        }
        if (event.keyCode == android.view.KeyEvent.KEYCODE_DPAD_LEFT) {
            be.alwyapps.watch.ui.HeldLeft.held = event.action == android.view.KeyEvent.ACTION_DOWN && event.repeatCount > 0
        }
        return super.dispatchKeyEvent(event)
    }

    override fun onStart() {
        super.onStart()
        be.alwyapps.watch.together.TogetherInbox
            .activityStarted(null, player = false)
        val core = services.core
        if (core.isCompleted) {
            lifecycleScope.launch {
                be.alwyapps.watch.together.TogetherInbox
                    .setCore(core.await())
            }
        }
    }

    override fun onStop() {
        super.onStop()
        be.alwyapps.watch.together.TogetherInbox
            .activityStopped(player = false)
    }

    override fun onNewIntent(intent: android.content.Intent) {
        super.onNewIntent(intent)
        takeRoom(intent)
    }

    /** The player's "Watch together" opened a room: show it here. */
    private fun takeRoom(intent: android.content.Intent?) {
        intent?.getStringExtra(EXTRA_ROOM)?.let {
            be.alwyapps.watch.together.TogetherInbox.openRoom = it
            intent.removeExtra(EXTRA_ROOM)
        }
    }

    companion object {
        private const val EXTRA_ROOM = "open_room"

        /** Brings the app to the front on a watch together room. */
        fun openRoom(
            ctx: android.content.Context,
            code: String,
        ) = android.content
            .Intent(ctx, MainActivity::class.java)
            .putExtra(EXTRA_ROOM, code)
            .addFlags(
                android.content.Intent.FLAG_ACTIVITY_NEW_TASK or android.content.Intent.FLAG_ACTIVITY_CLEAR_TOP or
                    android.content.Intent.FLAG_ACTIVITY_SINGLE_TOP,
            )
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
        takeRoom(intent)
        // Recreated after the per-app language changed in Android's settings: read it again.
        if (services.core.isCompleted) {
            be.alwyapps.watch.ui.I18n
                .apply(this)
        }
        // Once Compose draws (its root paints the background), the window's own background is a
        // full-screen fill under it on every frame for nothing.
        window.decorView.post { window.setBackgroundDrawable(null) }
        setContent {
            AlwyTheme(isTv = services.isTv) {
                // The window background is already the app color, so waiting a few ms for the
                // native core to load shows no flash; then the first composition renders from cache.
                val core by produceState<AlwyWatch?>(null) {
                    value =
                        services.core.await().also {
                            be.alwyapps.watch.together.TogetherInbox
                                .setCore(it)
                        }
                }
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
            tr("Quit AlwyWatch?"),
            tr("You can turn this question off in Settings."),
            confirm = tr("Quit"),
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
                // Over every screen: the one-time welcome step, then invitations to watch together.
                be.alwyapps.watch.ui.screens
                    .ProfileWelcome()
                if (!be.alwyapps.watch.ui.screens
                        .welcomeShowing()
                ) {
                    be.alwyapps.watch.ui.screens
                        .InvitationPrompt()
                }
                // "Are these both you?" (two sign-ins of one person), looked for after the start.
                be.alwyapps.watch.ui.screens
                    .IdentityPrompt()
                val openRoom = be.alwyapps.watch.together.TogetherInbox.openRoom
                androidx.compose.runtime.LaunchedEffect(openRoom) {
                    if (openRoom != null) {
                        be.alwyapps.watch.together.TogetherInbox.openRoom = null
                        if (nav.current != Route.Room(openRoom)) nav.push(Route.Room(openRoom))
                    }
                }
                // Watch together: every start of the room this device sits in opens the player,
                // wherever the app is (the host may have picked the title from any screen).
                val roomState = be.alwyapps.watch.together.TogetherSession.state
                val ctx = androidx.compose.ui.platform.LocalContext.current
                androidx.compose.runtime.LaunchedEffect(roomState?.code, roomState?.round, roomState?.started) {
                    val s = roomState ?: return@LaunchedEffect
                    val session = be.alwyapps.watch.together.TogetherSession
                    if (!s.started || s.round <= session.handledRound) return@LaunchedEffect
                    if (be.alwyapps.watch.together.TogetherInbox.playerVisible) return@LaunchedEffect
                    be.alwyapps.watch.ui.screens
                        .openRoomPlayer(ctx, core)
                        ?.let { problem ->
                            if (nav.current != Route.Room(s.code)) nav.push(Route.Room(s.code))
                            session.problem = problem
                        }
                }
                AppScaffold {
                    // One initial focus per screen visit (see initialFocus).
                    val focusDone = remember(nav.current) { mutableStateOf(false) }
                    // TV: a screen where nothing took the focus by its first frame (no initial
                    // focus, or the remembered element isn't there) gets it on its first element,
                    // so the D-pad is never dead and the side menu never gets it by default.
                    val screen =
                        remember {
                            androidx.compose.ui.focus
                                .FocusRequester()
                        }
                    var screenHasFocus by remember { mutableStateOf(false) }
                    if (isTv) {
                        androidx.compose.runtime.LaunchedEffect(nav.current) {
                            androidx.compose.runtime.withFrameNanos { }
                            if (!focusDone.value && !screenHasFocus && runCatching { screen.requestFocus() }.getOrDefault(false)) {
                                focusDone.value = true
                            }
                        }
                    }
                    CompositionLocalProvider(be.alwyapps.watch.ui.LocalInitialFocusDone provides focusDone) {
                        Box(
                            Modifier
                                .fillMaxSize()
                                .focusRequester(screen)
                                .onFocusChanged { screenHasFocus = it.hasFocus },
                        ) {
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
                                            .PageTitle(tr("Downloads"))
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
                                is Route.Room ->
                                    be.alwyapps.watch.ui.screens
                                        .RoomScreen(r.code)
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
                                    CompositionLocalProvider(LocalAccount provides (r.account ?: acct)) { LibraryScreen(r.id, r.title, r.recent) }
                                Route.ContinueWatching ->
                                    be.alwyapps.watch.ui.screens
                                        .ContinueWatchingScreen()
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
}
