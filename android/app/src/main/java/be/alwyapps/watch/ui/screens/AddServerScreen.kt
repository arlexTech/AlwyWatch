// SPDX-License-Identifier: AGPL-3.0-or-later
package be.alwyapps.watch.ui.screens

import android.content.Intent
import android.net.Uri
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.imePadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.safeDrawingPadding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.KeyboardActions
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.text.input.PasswordVisualTransformation
import androidx.compose.ui.unit.dp
import be.alwyapps.watch.core.CoreException
import be.alwyapps.watch.core.SignedIn
import be.alwyapps.watch.core.keep.ServerEntry
import be.alwyapps.watch.core.keep.ServerType
import be.alwyapps.watch.core.media.PlexServerResource
import be.alwyapps.watch.core.media.QuickConnectCode
import be.alwyapps.watch.services
import be.alwyapps.watch.ui.Accent
import be.alwyapps.watch.ui.ActionButton
import be.alwyapps.watch.ui.Chevron
import be.alwyapps.watch.ui.Danger
import be.alwyapps.watch.ui.Field
import be.alwyapps.watch.ui.FocusCard
import be.alwyapps.watch.ui.IconAction
import be.alwyapps.watch.ui.Icons
import be.alwyapps.watch.ui.LocalCore
import be.alwyapps.watch.ui.LocalDimens
import be.alwyapps.watch.ui.Muted
import be.alwyapps.watch.ui.Outline
import be.alwyapps.watch.ui.Subtle
import be.alwyapps.watch.ui.SurfaceColor
import be.alwyapps.watch.ui.SurfaceHigh
import be.alwyapps.watch.ui.ambientGlow
import be.alwyapps.watch.ui.dpadEscape
import be.alwyapps.watch.ui.initialFocus
import be.alwyapps.watch.ui.tr
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch

private sealed interface Step {
    data object Choose : Step

    /**
     * The AlwyWatch server's address, then how to sign in there (a code, a Jellyfin or Plex
     * login, username and password); or, to [create] an account, its sign-up.
     */
    data class AlwyWatch(
        val create: Boolean,
    ) : Step

    /** Creating an AlwyWatch account: invitation code, username and password. */
    data class Register(
        val url: String,
    ) : Step

    /** Accounts copied from a signed-in device, through this AlwyWatch server. */
    data class Receive(
        val url: String,
    ) : Step

    /** Signing in to an AlwyWatch account with username and password. */
    data class AccountLogin(
        val url: String,
    ) : Step

    /** Signed in to an AlwyWatch account: its servers to sign in to on this device. */
    data class Known(
        val url: String,
    ) : Step

    /** "I don't have one": Jellyfin or Plex directly. */
    data object Direct : Step

    data object JellyfinUrl : Step

    data class Jellyfin(
        val url: String,
        val quick: QuickConnectCode?,
    ) : Step

    data object Plex : Step

    /** Plex Home: who is watching (the account token stays for switching later). */
    data class PlexUsers(
        val accountToken: String,
        val users: List<be.alwyapps.watch.core.media.PlexHomeUser>,
    ) : Step

    data class PlexServers(
        val accountToken: String,
        val servers: List<PlexServerResource>,
    ) : Step
}

/**
 * A profile's first screen ("Welcome to AlwyWatch" / "Add a profile": sign in to AlwyWatch,
 * create an account, or Jellyfin or Plex only), "Add a server" from Settings, and with
 * [connect] Settings → Account → Connect to AlwyWatch (false: sign in, true: create an
 * account). Calls [onSignedIn] once the token is safely stored.
 */
@Composable
fun AddServerScreen(
    onSignedIn: (accountId: String) -> Unit,
    connect: Boolean? = null,
) {
    val core = LocalCore.current
    val ctx = LocalContext.current
    val d = LocalDimens.current
    val nav = be.alwyapps.watch.ui.LocalNavigator.current
    val scope = rememberCoroutineScope()
    // Steps visited, so Back returns to the previous one (AlwyWatch server list, or direct).
    // Already connected to an AlwyWatch server: the choice offers its servers not added yet.
    val keep = remember { core.keepConnection() }
    // No login yet: the profile's first screen (not "Add a server").
    val logins = remember { core.accounts() }
    val firstRun = logins.isEmpty()

    fun existingLogin(e: ServerEntry) = logins.firstOrNull { it.serverId == e.id }
    var history by remember {
        val signedIn = core.account()
        mutableStateOf(
            when {
                connect != null -> listOf<Step>(Step.AlwyWatch(connect))
                // Signed in to an AlwyWatch account, no login yet: on with its servers.
                firstRun && signedIn != null -> listOf(Step.Choose, Step.Known(signedIn.baseUrl))
                else -> listOf(Step.Choose)
            },
        )
    }
    val step = history.last()

    fun go(next: Step) {
        history = history + next
    }

    fun back() {
        if (history.size > 1) history = history.dropLast(1) else nav.pop()
    }
    var error by remember { mutableStateOf<String?>(null) }
    var busy by remember { mutableStateOf(false) }
    // Picked from an AlwyWatch server's list: Plex then selects that server by itself.
    var presetPlexId by remember { mutableStateOf<String?>(null) }
    androidx.activity.compose.ReportDrawn()
    androidx.activity.compose.BackHandler(enabled = history.size > 1) {
        back()
        error = null
    }

    // Plex Home: the account's own token, kept to switch user later from Settings.
    var plexHomeToken by remember { mutableStateOf<String?>(null) }

    fun finish(s: SignedIn) {
        ctx.services.tokens.put(s.account.id, s.token)
        plexHomeToken?.let { ctx.services.tokens.put("${s.account.id}:plexhome", it) }
        // Plex: the plex.tv account token is what Seerr and the keep service accept.
        s.plexAccountToken?.let { ctx.services.tokens.put("${s.account.id}:plextv", it) }
        (plexHomeToken ?: s.plexAccountToken)?.let { core.setPlexAccountToken(s.account.id, it) }
        core.setSetting("current_account", s.account.id)
        // Leaving soon + Seerr through the AlwyWatch server the user started from (if any).
        val keepToken = s.plexAccountToken ?: s.token
        ctx.services.scope.launch {
            runCatching { core.keepOnboard(s.account.id, keepToken) }
            // Signed in to an AlwyWatch account: this sign-in joins it.
            runCatching { core.accountLinkMedia(s.account.id, keepToken) }
        }
        onSignedIn(s.account.id)
    }

    // After the Plex sign-in (and the user choice): the server picked earlier, or the list.
    suspend fun plexServersFor(token: String) {
        val servers = core.plexServers(token)
        val preset = servers.firstOrNull { it.machineId == presetPlexId }
        if (preset != null) {
            finish(core.plexConnect(token, preset, null))
        } else {
            go(Step.PlexServers(token, servers))
        }
    }

    fun attempt(block: suspend () -> Unit) {
        if (busy) return
        busy = true
        error = null
        scope.launch {
            try {
                block()
            } catch (e: CoreException) {
                error = e.message
            } finally {
                busy = false
            }
        }
    }

    // A media server from an AlwyWatch server's list: sign in there, then connect leaving soon.
    // A login this profile already has there: just use it.
    fun pick(
        url: String,
        entry: ServerEntry,
    ) = attempt {
        val existing = core.accounts().firstOrNull { it.serverId == entry.id && keepToken(ctx, it) != null }
        if (existing != null) {
            useExistingLogin(core, ctx, url, existing)
            onSignedIn(existing.id)
            return@attempt
        }
        core.keepSetPreset(url)
        when (entry.kind) {
            ServerType.JELLYFIN -> {
                core.jellyfinServerInfo(entry.url)
                val quick = runCatching { core.jellyfinQuickConnectStart(entry.url) }.getOrNull()
                go(Step.Jellyfin(entry.url, quick))
            }
            ServerType.PLEX -> {
                presetPlexId = entry.id
                go(Step.Plex)
            }
        }
    }

    Column(
        Modifier
            .fillMaxSize()
            .ambientGlow(Accent.copy(alpha = 0.55f))
            .safeDrawingPadding()
            .imePadding()
            .verticalScroll(rememberScrollState())
            .padding(horizontal = d.gutter, vertical = 24.dp)
            .widthIn(max = 600.dp),
        verticalArrangement = Arrangement.spacedBy(18.dp),
    ) {
        if (step == Step.Choose && !firstRun) {
            be.alwyapps.watch.ui
                .PageTitle(tr("Add a server"))
        } else if (step == Step.Choose) {
            // A new profile starts here: a way back to the others.
            val profiles = remember { ctx.services.profiles.list() }
            Column(Modifier.padding(top = 24.dp, bottom = 8.dp)) {
                Text(if (profiles.size > 1) tr("Add a profile") else tr("Welcome to AlwyWatch"), style = MaterialTheme.typography.displaySmall)
                Text(tr("Your Jellyfin and Plex, fast and beautiful."), style = MaterialTheme.typography.bodyLarge, color = Muted)
            }
            if (profiles.size > 1) {
                val me = profiles.firstOrNull { it.id == ctx.services.profile }
                ActionButton(tr("{name} · switch profile", "name" to (me?.name ?: tr("Profile"))), {
                    ProfileSession.open = false
                    (ctx as android.app.Activity).recreate()
                }, icon = Icons.Person)
            }
        } else {
            Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(12.dp)) {
                IconAction(Icons.Back, tr("Back"), {
                    back()
                    error = null
                })
                Text(
                    when (step) {
                        is Step.AlwyWatch -> if (step.create) tr("Create an AlwyWatch account") else tr("Sign in to AlwyWatch")
                        is Step.Register -> tr("Create an AlwyWatch account")
                        is Step.Receive -> tr("Sign in with a code")
                        is Step.AccountLogin -> tr("AlwyWatch account")
                        is Step.Known -> tr("Your servers")
                        Step.Direct -> tr("Jellyfin or Plex only")
                        Step.Plex, is Step.PlexServers, is Step.PlexUsers -> "Plex"
                        else -> "Jellyfin"
                    },
                    style = MaterialTheme.typography.headlineMedium,
                )
            }
        }
        when (val s = step) {
            Step.Choose ->
                if (firstRun) {
                    FirstChoices(
                        onSignIn = { go(Step.AlwyWatch(create = false)) },
                        onCreate = { go(Step.AlwyWatch(create = true)) },
                        onDirect = { go(Step.Direct) },
                    )
                } else if (keep != null) {
                    ConnectedChoices(
                        keep.baseUrl,
                        busy,
                        onPick = { url, entry -> pick(url, entry) },
                        onJellyfin = {
                            core.keepClearPreset()
                            presetPlexId = null
                            go(Step.JellyfinUrl)
                        },
                        onPlex = {
                            core.keepClearPreset()
                            presetPlexId = null
                            go(Step.Plex)
                        },
                    )
                } else {
                    // Not on AlwyWatch: Jellyfin or Plex (AlwyWatch is connected from Settings → Account).
                    LaunchedEffect(Unit) {
                        core.keepClearPreset()
                        presetPlexId = null
                    }
                    ChoiceCard("Jellyfin", tr("Quick Connect or password"), Icons.Library, { go(Step.JellyfinUrl) }, Modifier.initialFocus())
                    ChoiceCard("Plex", tr("Sign in on plex.tv"), Icons.Play, { go(Step.Plex) })
                }

            is Step.AlwyWatch ->
                AlwyWatchServerPicker(
                    create = s.create,
                    initialUrl =
                        remember {
                            keep?.baseUrl ?: core.account()?.baseUrl ?: ctx.services.profiles.lastAlwyWatchUrl
                                .orEmpty()
                        },
                    busy = busy,
                    onPick = { url, entry -> pick(url, entry) },
                    onError = { error = it },
                    onCopy = { url -> go(Step.Receive(url)) },
                    onAccount = { url -> go(Step.AccountLogin(url)) },
                    onCreate = { url -> go(Step.Register(url)) },
                )

            is Step.Register -> RegisterStep(s.url, busy, ::attempt) { history = history.dropLast(1) + Step.Known(s.url) }

            is Step.Receive ->
                ReceiveFromDevice(s.url, onDone = onSignedIn, onAccount = {
                    history = history.dropLast(1) + Step.Known(s.url)
                })

            is Step.AccountLogin -> {
                var user by remember { mutableStateOf("") }
                var pass by remember { mutableStateOf("") }
                val submit = {
                    attempt {
                        val r = core.accountLogin(s.url, user, pass, android.os.Build.MODEL ?: "Android")
                        ctx.services.tokens.put(ACCOUNT_TOKEN, r.token)
                        ctx.services.profiles.lastAlwyWatchUrl = r.account.baseUrl
                        runCatching { core.syncSettings() }
                        history = history.dropLast(1) + Step.Known(s.url)
                    }
                }
                Text(
                    tr("Your AlwyWatch account keeps your profile, settings and server list. To watch, you then sign in to your server."),
                    color = Muted,
                    style = MaterialTheme.typography.bodyMedium,
                )
                Field(
                    user,
                    { user = it },
                    label = { Text(tr("Username")) },
                    singleLine = true,
                    modifier = Modifier.fillMaxWidth().dpadEscape().initialFocus(),
                )
                Field(
                    pass,
                    { pass = it },
                    label = { Text(tr("Password")) },
                    singleLine = true,
                    visualTransformation = PasswordVisualTransformation(),
                    keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Password, imeAction = ImeAction.Done),
                    keyboardActions = KeyboardActions(onDone = { submit() }),
                    modifier = Modifier.fillMaxWidth().dpadEscape(),
                )
                ActionButton(if (busy) tr("Signing in…") else tr("Sign in"), { submit() }, primary = true)
                Text(
                    tr("No password (passkey or single sign-on)? Go back, choose “Sign in with a code” and approve it on your phone."),
                    color = Subtle,
                    style = MaterialTheme.typography.bodySmall,
                )
            }

            is Step.Known -> {
                val me = remember { core.account() }
                var served by remember { mutableStateOf<List<ServerEntry>?>(null) }
                LaunchedEffect(s.url) { served = runCatching { core.keepServers(s.url) }.getOrDefault(emptyList()) }
                Text(
                    me?.name?.let { tr("Signed in as {name}. Now sign in to the server you watch on.", "name" to it) }
                        ?: tr("Signed in as you. Now sign in to the server you watch on."),
                    style = MaterialTheme.typography.titleMedium,
                )
                served?.forEach { e -> ServerCard(e, existing = existingLogin(e)) { if (!busy) pick(s.url, e) } }
                val known = remember(served) { core.knownServers().filter { k -> served.orEmpty().none { it.url.trimEnd('/') == k.url.trimEnd('/') } } }
                known.forEach { k ->
                    val from = tr("{kind} · from your other devices", "kind" to if (k.kind == "plex") "Plex" else "Jellyfin")
                    ChoiceCard(k.name.ifEmpty { k.url }, from, Icons.Library, {
                        if (k.kind == "plex") {
                            go(Step.Plex)
                        } else {
                            attempt {
                                val quick = runCatching { core.jellyfinQuickConnectStart(k.url) }.getOrNull()
                                go(Step.Jellyfin(k.url, quick))
                            }
                        }
                    })
                }
                ChoiceCard(tr("Another server"), tr("Jellyfin or Plex by its address"), Icons.Add, { go(Step.Direct) })
            }
            Step.Direct -> {
                // No AlwyWatch server on this path: forget one picked earlier then backed out of.
                LaunchedEffect(Unit) {
                    core.keepClearPreset()
                    presetPlexId = null
                }
                ChoiceCard("Jellyfin", tr("Quick Connect or password"), Icons.Library, { go(Step.JellyfinUrl) }, Modifier.initialFocus())
                ChoiceCard("Plex", tr("Sign in on plex.tv"), Icons.Play, { go(Step.Plex) })
            }

            Step.JellyfinUrl -> {
                var url by remember { mutableStateOf("") }
                val submit = {
                    attempt {
                        val normalized =
                            be.alwyapps.watch.core
                                .normalizeServerUrl(url)
                        core.jellyfinServerInfo(normalized)
                        val quick = runCatching { core.jellyfinQuickConnectStart(normalized) }.getOrNull()
                        go(Step.Jellyfin(normalized, quick))
                    }
                }
                Field(
                    url,
                    { url = it },
                    label = { Text(tr("Server address")) },
                    placeholder = { Text("jellyfin.example.org") },
                    singleLine = true,
                    keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Uri, imeAction = ImeAction.Go),
                    keyboardActions = KeyboardActions(onGo = { submit() }),
                    modifier = Modifier.fillMaxWidth().dpadEscape().initialFocus(),
                )
                Text(tr("Your Jellyfin server's address, as you use it in a browser."), color = Muted, style = MaterialTheme.typography.bodyMedium)
                ActionButton(if (busy) tr("Checking…") else tr("Continue"), { submit() }, primary = true)
            }

            is Step.Jellyfin -> {
                if (s.quick != null) {
                    Column(
                        Modifier
                            .fillMaxWidth()
                            .clip(RoundedCornerShape(24.dp))
                            .background(SurfaceColor)
                            .border(1.dp, Outline, RoundedCornerShape(24.dp))
                            .padding(24.dp),
                        verticalArrangement = Arrangement.spacedBy(10.dp),
                    ) {
                        Text(tr("QUICK CONNECT"), style = MaterialTheme.typography.labelSmall, color = Accent)
                        Text(
                            s.quick.code
                                .chunked(3)
                                .joinToString(" "),
                            style =
                                MaterialTheme.typography.displayLarge.copy(
                                    letterSpacing =
                                        androidx.compose.ui.unit
                                            .TextUnit(0.12f, androidx.compose.ui.unit.TextUnitType.Em),
                                ),
                        )
                        Text(
                            tr("In a Jellyfin app where you're signed in, open your profile → Quick Connect and enter this code."),
                            color = Muted,
                            style = MaterialTheme.typography.bodyMedium,
                        )
                        Text(tr("Waiting for approval…"), style = MaterialTheme.typography.bodySmall, color = Subtle)
                    }
                    LaunchedEffect(s.quick.secret) {
                        while (true) {
                            delay(3000)
                            try {
                                finish(core.jellyfinQuickConnectPoll(s.url, s.quick.secret))
                                break
                            } catch (_: CoreException.Pending) {
                            } catch (e: CoreException) {
                                // Codes expire after a few minutes (finding a phone takes time on a
                                // TV): get a fresh one instead of giving up.
                                val fresh = runCatching { core.jellyfinQuickConnectStart(s.url) }.getOrNull()
                                if (fresh != null) {
                                    history = history.dropLast(1) + Step.Jellyfin(s.url, fresh)
                                } else {
                                    error = e.message
                                }
                                break
                            }
                        }
                    }
                    Text(tr("Or with your password"), Modifier.padding(top = 8.dp), style = MaterialTheme.typography.titleMedium)
                }
                var user by remember { mutableStateOf("") }
                var pass by remember { mutableStateOf("") }
                Field(
                    user,
                    { user = it },
                    label = { Text(tr("Username")) },
                    singleLine = true,
                    modifier = Modifier.fillMaxWidth().dpadEscape(),
                )
                Field(
                    pass,
                    { pass = it },
                    label = { Text(tr("Password")) },
                    singleLine = true,
                    visualTransformation = PasswordVisualTransformation(),
                    keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Password, imeAction = ImeAction.Done),
                    keyboardActions = KeyboardActions(onDone = { attempt { finish(core.jellyfinSignIn(s.url, user, pass)) } }),
                    modifier = Modifier.fillMaxWidth().dpadEscape(),
                )
                ActionButton(if (busy) tr("Signing in…") else tr("Sign in"), {
                    attempt { finish(core.jellyfinSignIn(s.url, user, pass)) }
                }, primary = true)
            }

            Step.Plex -> {
                // Phone: a browser sign-in (long PIN) plus a short code for plex.tv/link on any
                // device; TV: the short code only. Whichever gets linked first wins.
                val browser = !d.isTv
                var shortCode by remember { mutableStateOf<String?>(null) }
                var authUrl by remember { mutableStateOf<String?>(null) }
                LaunchedEffect(Unit) {
                    try {
                        val short = core.plexCreatePin(false)
                        shortCode = short.code
                        val long = if (browser) runCatching { core.plexCreatePin(true) }.getOrNull() else null
                        authUrl = long?.authUrl?.takeIf { it.isNotEmpty() }
                        authUrl?.let { openInBrowser(ctx, it) }
                        val pins = listOfNotNull(short.id, long?.id)
                        while (true) {
                            delay(2000)
                            val token =
                                pins.firstNotNullOfOrNull { id ->
                                    try {
                                        core.plexCheckPin(id).also { error = null }
                                    } catch (_: CoreException.Pending) {
                                        null
                                    } catch (e: CoreException.Network) {
                                        // A blip while waiting: say so, keep waiting.
                                        error = e.message
                                        null
                                    }
                                } ?: continue
                            // Several people on this Plex account: ask who is watching first.
                            val users = runCatching { core.plexHomeUsers(token) }.getOrDefault(emptyList())
                            if (users.size > 1) {
                                plexHomeToken = token
                                go(Step.PlexUsers(token, users))
                            } else {
                                plexServersFor(token)
                            }
                            break
                        }
                    } catch (e: CoreException) {
                        error = e.message
                    }
                }
                Column(
                    Modifier
                        .fillMaxWidth()
                        .clip(RoundedCornerShape(24.dp))
                        .background(SurfaceColor)
                        .border(1.dp, Outline, RoundedCornerShape(24.dp))
                        .padding(24.dp),
                    verticalArrangement = Arrangement.spacedBy(12.dp),
                ) {
                    authUrl?.let { url ->
                        Text(tr("Sign in on plex.tv in your browser, then come back here."), style = MaterialTheme.typography.titleMedium)
                        ActionButton(tr("Open plex.tv"), { openInBrowser(ctx, url) }, Modifier.initialFocus(), primary = true)
                        Text(tr("Or, on any device"), Modifier.padding(top = 8.dp), style = MaterialTheme.typography.bodySmall, color = Subtle)
                    }
                    Text(tr("PLEX.TV/LINK"), style = MaterialTheme.typography.labelSmall, color = Accent)
                    Text(shortCode ?: "…", style = MaterialTheme.typography.displayLarge)
                    Text(tr("Go to plex.tv/link and enter this code."), color = Muted)
                    Text(tr("Waiting for Plex…"), style = MaterialTheme.typography.bodySmall, color = Subtle)
                }
            }

            is Step.PlexUsers ->
                PlexUserPicker(s.accountToken, s.users, busy) { userToken ->
                    attempt { plexServersFor(userToken) }
                }

            is Step.PlexServers -> {
                Text(tr("Choose a server"), style = MaterialTheme.typography.titleMedium)
                if (s.servers.isEmpty()) Text(tr("This Plex account has no servers."), color = Muted)
                s.servers.forEachIndexed { i, srv ->
                    ChoiceCard(
                        srv.name,
                        if (srv.owned) tr("Your server") else tr("Shared with you"),
                        Icons.Library,
                        { attempt { finish(core.plexConnect(s.accountToken, srv, null)) } },
                        if (i == 0) Modifier.initialFocus() else Modifier,
                    )
                }
            }
        }
        error?.let { Text(it, color = Danger) }
    }
}

/**
 * The AlwyWatch server's address (prefilled from this device's other profiles), then the ways
 * to sign in there: a code (first on TV), a Jellyfin or Plex login on one of its servers,
 * username and password. With [create]: on to the sign-up.
 */
@Composable
private fun AlwyWatchServerPicker(
    create: Boolean,
    initialUrl: String,
    busy: Boolean,
    onPick: (url: String, entry: ServerEntry) -> Unit,
    onError: (String?) -> Unit,
    onCopy: (url: String) -> Unit,
    onAccount: (url: String) -> Unit,
    onCreate: (url: String) -> Unit,
) {
    val core = LocalCore.current
    val tv = LocalDimens.current.isTv
    val scope = rememberCoroutineScope()
    val logins = remember { core.accounts() }
    var url by remember { mutableStateOf(initialUrl.substringAfter("://").trimEnd('/')) }
    var entries by remember { mutableStateOf<Pair<String, List<ServerEntry>>?>(null) }
    var checking by remember { mutableStateOf(false) }
    val load = {
        onError(null)
        if (!checking) {
            checking = true
            scope.launch {
                try {
                    val normalized =
                        be.alwyapps.watch.core
                            .normalizeServerUrl(url)
                    val list = core.keepServers(normalized)
                    if (create) onCreate(normalized) else entries = normalized to list
                } catch (e: CoreException) {
                    onError(e.message)
                } finally {
                    checking = false
                }
            }
        }
    }
    Text(
        if (create) {
            tr("Type the AlwyWatch address the person who runs your server gave you with your invitation.")
        } else {
            tr("Type the AlwyWatch address the person who runs your server gave you.")
        },
        color = Muted,
        style = MaterialTheme.typography.bodyMedium,
    )
    Field(
        url,
        {
            url = it
            entries = null
        },
        placeholder = { Text("watch.example.org") },
        pill = true,
        singleLine = true,
        keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Uri, imeAction = ImeAction.Go),
        keyboardActions = KeyboardActions(onGo = { load() }),
        modifier = Modifier.fillMaxWidth().dpadEscape().initialFocus(),
    )
    if (entries == null) ActionButton(if (checking) tr("Checking…") else tr("Continue"), { load() }, primary = true)
    entries?.let { (base, list) ->
        @Composable
        fun Code() =
            ChoiceCard(
                tr("Sign in with a code"),
                tr("Scan a QR code with your phone, or approve on a device where you're signed in"),
                Icons.Download,
                { onCopy(base) },
            )
        if (tv) Code()
        if (list.isNotEmpty()) {
            Text(tr("With your Jellyfin or Plex login"), Modifier.padding(top = 8.dp), style = MaterialTheme.typography.titleMedium)
            list.forEach { e -> ServerCard(e, existing = logins.firstOrNull { it.serverId == e.id }) { if (!busy) onPick(base, e) } }
        }
        Text(tr("Or"), Modifier.padding(top = 8.dp), style = MaterialTheme.typography.titleMedium)
        if (!tv) Code()
        ChoiceCard(
            tr("Username and password"),
            tr("Your AlwyWatch account's"),
            Icons.Person,
            { onAccount(base) },
        )
        Text(
            tr("Passkey or Pocket ID? Choose “Sign in with a code” and approve it on your phone."),
            color = Subtle,
            style = MaterialTheme.typography.bodySmall,
        )
    }
}

/**
 * Creating an AlwyWatch account: what the server allows (an invitation, open, closed), then
 * the invitation code, username and password. [onDone] goes on to "Your servers".
 */
@Composable
private fun RegisterStep(
    url: String,
    busy: Boolean,
    attempt: (suspend () -> Unit) -> Unit,
    onDone: () -> Unit,
) {
    val core = LocalCore.current
    val ctx = LocalContext.current
    var options by remember { mutableStateOf<be.alwyapps.watch.core.AccountOptions?>(null) }
    var failed by remember { mutableStateOf<String?>(null) }
    LaunchedEffect(url) {
        try {
            options = core.accountOptions(url)
        } catch (e: CoreException) {
            failed = e.message
        }
    }
    var invite by remember { mutableStateOf("") }
    var user by remember { mutableStateOf("") }
    var pass by remember { mutableStateOf("") }
    val o = options
    when {
        failed != null -> Text(failed!!, color = Danger)
        o == null -> Text(tr("Checking…"), color = Subtle)
        o.registration == "closed" ->
            Text(tr("This AlwyWatch server doesn't take new accounts. Ask the person who runs it."), color = Muted)
        else -> {
            val submit = {
                attempt {
                    val r =
                        core.accountRegister(
                            url,
                            user,
                            pass,
                            invite.takeIf { o.registration == "invite" },
                            android.os.Build.MODEL ?: "Android",
                        )
                    ctx.services.tokens.put(ACCOUNT_TOKEN, r.token)
                    ctx.services.profiles.lastAlwyWatchUrl = r.account.baseUrl
                    runCatching { core.syncSettings() }
                    onDone()
                }
            }
            Text(
                tr("Your AlwyWatch account keeps your name, settings and server list. To watch, you then add your Jellyfin or Plex login."),
                color = Muted,
                style = MaterialTheme.typography.bodyMedium,
            )
            if (o.registration == "invite") {
                Field(
                    invite,
                    { invite = it.trim() },
                    label = { Text(tr("Invitation code")) },
                    singleLine = true,
                    modifier = Modifier.fillMaxWidth().dpadEscape().initialFocus(),
                )
            }
            Field(
                user,
                { user = it },
                label = { Text(tr("Username")) },
                singleLine = true,
                modifier = Modifier.fillMaxWidth().dpadEscape().then(if (o.registration == "invite") Modifier else Modifier.initialFocus()),
            )
            Field(
                pass,
                { pass = it },
                label = { Text(tr("Password")) },
                singleLine = true,
                visualTransformation = PasswordVisualTransformation(),
                keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Password, imeAction = ImeAction.Done),
                keyboardActions = KeyboardActions(onDone = { submit() }),
                modifier = Modifier.fillMaxWidth().dpadEscape(),
            )
            ActionButton(if (busy) tr("Creating…") else tr("Create the account"), { submit() }, primary = true)
            if (o.passkeys || o.pocketid) {
                Text(
                    tr(
                        "Prefer a passkey or Pocket ID? Create the account on {address}, then come back and choose “Sign in with a code”.",
                        "address" to url.substringAfter("://"),
                    ),
                    color = Subtle,
                    style = MaterialTheme.typography.bodySmall,
                )
            }
        }
    }
}

@Composable
private fun ServerCard(
    e: ServerEntry,
    modifier: Modifier = Modifier,
    existing: be.alwyapps.watch.core.Account? = null,
    onClick: () -> Unit,
) {
    val plex = e.kind == ServerType.PLEX
    ChoiceCard(
        e.name,
        when {
            existing != null -> tr("Use your login {name}", "name" to existing.userName)
            plex -> tr("Plex · sign in on plex.tv")
            else -> tr("Jellyfin · Quick Connect or password")
        },
        if (plex) Icons.Play else Icons.Library,
        onClick,
        modifier,
    )
}

/** A profile's first screen: sign in to AlwyWatch, create an account, or Jellyfin or Plex only. */
@Composable
private fun FirstChoices(
    onSignIn: () -> Unit,
    onCreate: () -> Unit,
    onDirect: () -> Unit,
) {
    ChoiceCard(
        tr("Sign in to AlwyWatch"),
        tr("Your name, settings and servers come with you"),
        Icons.Hourglass,
        onSignIn,
        Modifier.initialFocus(),
    )
    ChoiceCard(
        tr("Create an AlwyWatch account"),
        tr("You have an invitation"),
        Icons.Add,
        onCreate,
    )
    ChoiceCard(
        tr("Jellyfin or Plex only"),
        tr("No account: nothing synced, no Leaving soon or watch together"),
        Icons.Person,
        onDirect,
    )
}

/**
 * Adding a server while connected to an AlwyWatch server: its servers not added yet (if any),
 * then Jellyfin or Plex directly.
 */
@Composable
private fun ConnectedChoices(
    keepUrl: String,
    busy: Boolean,
    onPick: (url: String, entry: ServerEntry) -> Unit,
    onJellyfin: () -> Unit,
    onPlex: () -> Unit,
) {
    val core = LocalCore.current
    val added = remember { core.accounts().map { it.serverId }.toSet() }
    // Cached per screen visit; failing (offline) just leaves the direct choices.
    var missing by remember { mutableStateOf<List<ServerEntry>?>(null) }
    LaunchedEffect(keepUrl) {
        missing = runCatching { core.keepServers(keepUrl) }.getOrNull()?.filter { it.id !in added }.orEmpty()
    }
    val list = missing
    if (list == null) {
        Text(tr("Checking your AlwyWatch server…"), color = Subtle, style = MaterialTheme.typography.bodyMedium)
    } else if (list.isNotEmpty()) {
        Text(tr("From your AlwyWatch server"), style = MaterialTheme.typography.titleMedium)
        list.forEachIndexed { i, e ->
            ServerCard(e, if (i == 0) Modifier.initialFocus() else Modifier) { if (!busy) onPick(keepUrl, e) }
        }
        Text(tr("Another server"), Modifier.padding(top = 8.dp), style = MaterialTheme.typography.titleMedium)
    }
    ChoiceCard(
        "Jellyfin",
        tr("Quick Connect or password"),
        Icons.Library,
        onJellyfin,
        if (list != null && list.isEmpty()) Modifier.initialFocus() else Modifier,
    )
    ChoiceCard("Plex", tr("Sign in on plex.tv"), Icons.Play, onPlex)
}

/**
 * Opens a web page in the browser, not in an app that claims the link: with the Plex app
 * installed, app.plex.tv would open there and the sign-in would never reach us.
 */
internal fun openInBrowser(
    ctx: android.content.Context,
    url: String,
) {
    val view = Intent(Intent.ACTION_VIEW, Uri.parse(url)).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
    val browser =
        ctx.packageManager
            .resolveActivity(Intent(Intent.ACTION_VIEW, Uri.parse("https://example.org")), android.content.pm.PackageManager.MATCH_DEFAULT_ONLY)
            ?.activityInfo
            ?.packageName
            ?.takeIf { it != "android" } // "android" = the chooser: no default browser set
    runCatching { ctx.startActivity(if (browser != null) Intent(view).setPackage(browser) else view) }
        .onFailure { runCatching { ctx.startActivity(view) } }
}

/**
 * "Who's watching?" for a Plex Home: one card per member; a protected one asks for its PIN.
 * [onUser] gets the token of the chosen user.
 */
@Composable
fun PlexUserPicker(
    accountToken: String,
    users: List<be.alwyapps.watch.core.media.PlexHomeUser>,
    busy: Boolean,
    onUser: (String) -> Unit,
) {
    val core = LocalCore.current
    val scope = rememberCoroutineScope()
    var asking by remember { mutableStateOf<be.alwyapps.watch.core.media.PlexHomeUser?>(null) }
    var pin by remember { mutableStateOf("") }
    var error by remember { mutableStateOf<String?>(null) }
    var switching by remember { mutableStateOf(false) }

    fun switch(
        u: be.alwyapps.watch.core.media.PlexHomeUser,
        pin: String?,
    ) {
        if (switching || busy) return
        switching = true
        error = null
        scope.launch {
            try {
                onUser(core.plexSwitchUser(accountToken, u.uuid, pin))
            } catch (_: CoreException.Unauthorized) {
                error = tr("Wrong PIN.")
            } catch (e: CoreException) {
                error = e.message
            } finally {
                switching = false
            }
        }
    }
    Text(tr("Which Plex user?"), style = MaterialTheme.typography.titleMedium)
    users.forEachIndexed { i, u ->
        ChoiceCard(
            u.title,
            listOfNotNull(if (u.admin) tr("Account owner") else null, if (u.protected) tr("PIN") else null).joinToString(" · ").ifEmpty { "Plex Home" },
            Icons.Person,
            {
                if (u.protected) {
                    asking = u
                    pin = ""
                } else {
                    switch(u, null)
                }
            },
            if (i == 0) Modifier.initialFocus() else Modifier,
        )
        if (asking == u) {
            Field(
                pin,
                { pin = it.filter(Char::isDigit).take(4) },
                label = { Text(tr("PIN for {name}", "name" to u.title)) },
                singleLine = true,
                visualTransformation = PasswordVisualTransformation(),
                keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.NumberPassword, imeAction = ImeAction.Done),
                keyboardActions = KeyboardActions(onDone = { switch(u, pin) }),
                modifier = Modifier.fillMaxWidth().dpadEscape().initialFocus(),
            )
            ActionButton(if (switching) tr("Checking…") else tr("Continue"), { switch(u, pin) }, primary = true)
        }
    }
    error?.let { Text(it, color = Danger) }
}

/** Big tappable card for the sign-in choices. */
@Composable
internal fun ChoiceCard(
    title: String,
    subtitle: String,
    icon: androidx.compose.ui.graphics.vector.ImageVector,
    onClick: () -> Unit,
    modifier: Modifier = Modifier,
) {
    FocusCard(onClick = onClick, modifier = modifier.fillMaxWidth(), shape = RoundedCornerShape(20.dp), glow = false) {
        Row(
            Modifier
                .fillMaxWidth()
                .background(SurfaceColor)
                .border(1.dp, Outline, RoundedCornerShape(20.dp))
                .padding(18.dp),
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.spacedBy(16.dp),
        ) {
            Box(
                Modifier
                    .size(48.dp)
                    .clip(RoundedCornerShape(14.dp))
                    .background(SurfaceHigh),
                contentAlignment = Alignment.Center,
            ) { androidx.compose.material3.Icon(icon, null, Modifier.size(26.dp), tint = Accent) }
            Column(Modifier.weight(1f)) {
                Text(title, style = MaterialTheme.typography.titleMedium)
                Text(subtitle, style = MaterialTheme.typography.bodySmall, color = Muted)
            }
            Chevron()
        }
    }
}

/**
 * Settings → switch Plex user: the members of this account's Plex Home; the chosen one replaces
 * the account (same server, their watch state), and keeps syncing with the AlwyWatch server.
 */
@Composable
fun PlexSwitchScreen(
    accountId: String,
    onSwitched: (String) -> Unit,
) {
    val core = LocalCore.current
    val ctx = LocalContext.current
    val d = LocalDimens.current
    val tokens = ctx.services.tokens
    val account = remember { core.accounts().firstOrNull { it.id == accountId } }
    // The Plex Home's own token (kept at sign-in), else this user's: both can list and switch.
    val homeToken = remember { tokens.get("$accountId:plexhome") ?: tokens.get("$accountId:plextv") }
    var users by remember { mutableStateOf<List<be.alwyapps.watch.core.media.PlexHomeUser>?>(null) }
    var error by remember { mutableStateOf<String?>(null) }
    var busy by remember { mutableStateOf(false) }
    val scope = rememberCoroutineScope()
    LaunchedEffect(homeToken) {
        if (homeToken == null) {
            error = tr("Sign in to Plex again to switch user.")
            return@LaunchedEffect
        }
        try {
            users = core.plexHomeUsers(homeToken)
        } catch (e: CoreException) {
            error = e.message
        }
    }
    Column(
        Modifier
            .fillMaxSize()
            .verticalScroll(rememberScrollState())
            .padding(horizontal = d.gutter, vertical = 16.dp)
            .widthIn(max = 600.dp),
        verticalArrangement = Arrangement.spacedBy(16.dp),
    ) {
        be.alwyapps.watch.ui
            .PageTitle(tr("Switch Plex user"), account?.let { tr("{name} on {server}", "name" to it.userName, "server" to it.serverName) })
        val list = users
        when {
            list == null && error == null -> Text(tr("Loading…"), color = Subtle)
            list != null && list.size < 2 -> Text(tr("This Plex account has no other users (it is not a Plex Home)."), color = Muted)
            list != null && homeToken != null && account != null ->
                PlexUserPicker(homeToken, list, busy) { userToken ->
                    busy = true
                    scope.launch {
                        try {
                            val server =
                                core.plexServers(userToken).firstOrNull { it.machineId == account.serverId }
                                    ?: throw CoreException.Invalid(tr("This user has no access to {server}.", "server" to account.serverName))
                            val s = core.plexConnect(userToken, server, null)
                            tokens.put(s.account.id, s.token)
                            s.plexAccountToken?.let { tokens.put("${s.account.id}:plextv", it) }
                            tokens.put("${s.account.id}:plexhome", homeToken)
                            // The AlwyWatch server followed the old user: follow the new one.
                            val keep = core.keepConnection()
                            if (keep?.accountId == accountId) {
                                runCatching { core.keepConnect(keep.baseUrl, s.account.id, s.plexAccountToken ?: s.token) }
                            }
                            if (s.account.id != accountId) {
                                core.removeAccount(accountId)
                                tokens.remove(accountId)
                                tokens.remove("$accountId:plextv")
                                tokens.remove("$accountId:plexhome")
                            }
                            core.setSetting("current_account", s.account.id)
                            onSwitched(s.account.id)
                        } catch (e: CoreException) {
                            error = e.message
                        } finally {
                            busy = false
                        }
                    }
                }
        }
        error?.let { Text(it, color = Danger) }
    }
}
