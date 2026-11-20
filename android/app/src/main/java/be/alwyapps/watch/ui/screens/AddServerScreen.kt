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
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch

private sealed interface Step {
    data object Choose : Step

    /** "I have an AlwyWatch server": address, then the media servers it serves. */
    data object AlwyWatch : Step

    /** Accounts copied from a signed-in device, through this AlwyWatch server. */
    data class Receive(
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

/** Adds a Jellyfin or Plex account. Calls [onSignedIn] once the token is safely stored. */
@Composable
fun AddServerScreen(onSignedIn: (accountId: String) -> Unit) {
    val core = LocalCore.current
    val ctx = LocalContext.current
    val d = LocalDimens.current
    val scope = rememberCoroutineScope()
    // Steps visited, so Back returns to the previous one (AlwyWatch server list, or direct).
    // Already connected to an AlwyWatch server: the choice offers its servers not added yet.
    val keep = remember { core.keepConnection() }
    var history by remember { mutableStateOf(listOf<Step>(Step.Choose)) }
    val step = history.last()

    fun go(next: Step) {
        history = history + next
    }

    fun back() {
        if (history.size > 1) history = history.dropLast(1)
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
        core.setSetting("current_account", s.account.id)
        // Leaving soon + Seerr through the AlwyWatch server the user started from (if any).
        val keepToken = s.plexAccountToken ?: s.token
        ctx.services.scope.launch { runCatching { core.keepOnboard(s.account.id, keepToken) } }
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
    fun pick(
        url: String,
        entry: ServerEntry,
    ) = attempt {
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
        if (step == Step.Choose && keep != null) {
            be.alwyapps.watch.ui
                .PageTitle("Add a server")
        } else if (step == Step.Choose) {
            Column(Modifier.padding(top = 24.dp, bottom = 8.dp)) {
                Text("AlwyWatch", style = MaterialTheme.typography.displayLarge)
                Text("Your Jellyfin and Plex, fast and beautiful.", style = MaterialTheme.typography.bodyLarge, color = Muted)
            }
        } else {
            Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(12.dp)) {
                IconAction(Icons.Back, "Back", {
                    back()
                    error = null
                })
                Text(
                    when (step) {
                        Step.AlwyWatch -> "AlwyWatch server"
                        is Step.Receive -> "Copy from a device"
                        Step.Direct -> "Sign in"
                        Step.Plex, is Step.PlexServers, is Step.PlexUsers -> "Plex"
                        else -> "Jellyfin"
                    },
                    style = MaterialTheme.typography.headlineMedium,
                )
            }
        }
        when (val s = step) {
            Step.Choose ->
                if (keep != null) {
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
                    FirstChoices(onAlwyWatch = { go(Step.AlwyWatch) }, onDirect = { go(Step.Direct) })
                }

            Step.AlwyWatch ->
                AlwyWatchServerPicker(
                    busy = busy,
                    onPick = { url, entry -> pick(url, entry) },
                    onError = { error = it },
                    onCopy = { url -> go(Step.Receive(url)) },
                )

            is Step.Receive -> ReceiveFromDevice(s.url, onDone = onSignedIn)
            Step.Direct -> {
                // No AlwyWatch server on this path: forget one picked earlier then backed out of.
                LaunchedEffect(Unit) {
                    core.keepClearPreset()
                    presetPlexId = null
                }
                ChoiceCard("Jellyfin", "Quick Connect or password", Icons.Library, { go(Step.JellyfinUrl) }, Modifier.initialFocus())
                ChoiceCard("Plex", "Sign in on plex.tv", Icons.Play, { go(Step.Plex) })
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
                    label = { Text("Server address") },
                    placeholder = { Text("jellyfin.example.org") },
                    singleLine = true,
                    keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Uri, imeAction = ImeAction.Go),
                    keyboardActions = KeyboardActions(onGo = { submit() }),
                    modifier = Modifier.fillMaxWidth().dpadEscape().initialFocus(),
                )
                Text("Your Jellyfin server's address, as you use it in a browser.", color = Muted, style = MaterialTheme.typography.bodyMedium)
                ActionButton(if (busy) "Checking…" else "Continue", { submit() }, primary = true)
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
                        Text("QUICK CONNECT", style = MaterialTheme.typography.labelSmall, color = Accent)
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
                            "In a Jellyfin app where you're signed in, open your profile → Quick Connect and enter this code.",
                            color = Muted,
                            style = MaterialTheme.typography.bodyMedium,
                        )
                        Text("Waiting for approval…", style = MaterialTheme.typography.bodySmall, color = Subtle)
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
                    Text("Or with your password", Modifier.padding(top = 8.dp), style = MaterialTheme.typography.titleMedium)
                }
                var user by remember { mutableStateOf("") }
                var pass by remember { mutableStateOf("") }
                Field(
                    user,
                    { user = it },
                    label = { Text("Username") },
                    singleLine = true,
                    modifier = Modifier.fillMaxWidth().dpadEscape(),
                )
                Field(
                    pass,
                    { pass = it },
                    label = { Text("Password") },
                    singleLine = true,
                    visualTransformation = PasswordVisualTransformation(),
                    keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Password, imeAction = ImeAction.Done),
                    keyboardActions = KeyboardActions(onDone = { attempt { finish(core.jellyfinSignIn(s.url, user, pass)) } }),
                    modifier = Modifier.fillMaxWidth().dpadEscape(),
                )
                ActionButton(if (busy) "Signing in…" else "Sign in", {
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
                        Text("Sign in on plex.tv in your browser, then come back here.", style = MaterialTheme.typography.titleMedium)
                        ActionButton("Open plex.tv", { openInBrowser(ctx, url) }, Modifier.initialFocus(), primary = true)
                        Text("Or, on any device", Modifier.padding(top = 8.dp), style = MaterialTheme.typography.bodySmall, color = Subtle)
                    }
                    Text("PLEX.TV/LINK", style = MaterialTheme.typography.labelSmall, color = Accent)
                    Text(shortCode ?: "…", style = MaterialTheme.typography.displayLarge)
                    Text("Go to plex.tv/link and enter this code.", color = Muted)
                    Text("Waiting for Plex…", style = MaterialTheme.typography.bodySmall, color = Subtle)
                }
            }

            is Step.PlexUsers ->
                PlexUserPicker(s.accountToken, s.users, busy) { userToken ->
                    attempt { plexServersFor(userToken) }
                }

            is Step.PlexServers -> {
                Text("Choose a server", style = MaterialTheme.typography.titleMedium)
                if (s.servers.isEmpty()) Text("This Plex account has no servers.", color = Muted)
                s.servers.forEachIndexed { i, srv ->
                    ChoiceCard(
                        srv.name,
                        if (srv.owned) "Your server" else "Shared with you",
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

/** "Got an AlwyWatch server address?": lists its servers so nobody types media server URLs. */
@Composable
private fun AlwyWatchServerPicker(
    busy: Boolean,
    onPick: (url: String, entry: ServerEntry) -> Unit,
    onError: (String?) -> Unit,
    onCopy: (url: String) -> Unit = {},
) {
    val core = LocalCore.current
    val scope = rememberCoroutineScope()
    var url by remember { mutableStateOf("") }
    var entries by remember { mutableStateOf<Pair<String, List<ServerEntry>>?>(null) }
    val load = {
        onError(null)
        scope.launch {
            try {
                val normalized =
                    be.alwyapps.watch.core
                        .normalizeServerUrl(url)
                entries = normalized to core.keepServers(normalized)
            } catch (e: CoreException) {
                onError(e.message)
            }
        }
    }
    Text(
        "Enter the address your server's owner gave you, then pick the server you watch on.",
        color = Muted,
        style = MaterialTheme.typography.bodyMedium,
    )
    Field(
        url,
        { url = it },
        placeholder = { Text("watch.example.org") },
        pill = true,
        singleLine = true,
        keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Uri, imeAction = ImeAction.Go),
        keyboardActions = KeyboardActions(onGo = { load() }),
        modifier = Modifier.fillMaxWidth().dpadEscape().initialFocus(),
    )
    ActionButton("Show servers", { load() }, primary = entries == null)
    entries?.let { (base, list) ->
        if (list.isEmpty()) Text("This AlwyWatch server lists no media servers.", color = Muted)
        list.forEach { e -> ServerCard(e) { if (!busy) onPick(base, e) } }
        // Already signed in elsewhere: copy everything instead of signing in again.
        Text("Already using AlwyWatch on another device?", Modifier.padding(top = 8.dp), style = MaterialTheme.typography.titleMedium)
        ChoiceCard("Copy from another device", "Your accounts and settings, approved on a device where you're signed in", Icons.Download, {
            onCopy(base)
        })
    }
}

@Composable
private fun ServerCard(
    e: ServerEntry,
    modifier: Modifier = Modifier,
    onClick: () -> Unit,
) {
    val plex = e.kind == ServerType.PLEX
    ChoiceCard(
        e.name,
        if (plex) "Plex · sign in on plex.tv" else "Jellyfin · Quick Connect or password",
        if (plex) Icons.Play else Icons.Library,
        onClick,
        modifier,
    )
}

/** First run: with or without an AlwyWatch server. */
@Composable
private fun FirstChoices(
    onAlwyWatch: () -> Unit,
    onDirect: () -> Unit,
) {
    Text("How do you watch?", style = MaterialTheme.typography.titleMedium)
    ChoiceCard(
        "I have an AlwyWatch server",
        "Its address comes from your server's owner. Includes what's leaving soon and requests.",
        Icons.Hourglass,
        onAlwyWatch,
        Modifier.initialFocus(),
    )
    ChoiceCard(
        "I don't have one",
        "Sign in to Jellyfin or Plex directly. You can add an AlwyWatch server later in Settings.",
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
        Text("Checking your AlwyWatch server…", color = Subtle, style = MaterialTheme.typography.bodyMedium)
    } else if (list.isNotEmpty()) {
        Text("From your AlwyWatch server", style = MaterialTheme.typography.titleMedium)
        list.forEachIndexed { i, e ->
            ServerCard(e, if (i == 0) Modifier.initialFocus() else Modifier) { if (!busy) onPick(keepUrl, e) }
        }
        Text("Another server", Modifier.padding(top = 8.dp), style = MaterialTheme.typography.titleMedium)
    }
    ChoiceCard(
        "Jellyfin",
        "Quick Connect or password",
        Icons.Library,
        onJellyfin,
        if (list != null && list.isEmpty()) Modifier.initialFocus() else Modifier,
    )
    ChoiceCard("Plex", "Sign in on plex.tv", Icons.Play, onPlex)
}

/**
 * Opens a web page in the browser, not in an app that claims the link: with the Plex app
 * installed, app.plex.tv would open there and the sign-in would never reach us.
 */
private fun openInBrowser(
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
                error = "Wrong PIN."
            } catch (e: CoreException) {
                error = e.message
            } finally {
                switching = false
            }
        }
    }
    Text("Who's watching?", style = MaterialTheme.typography.titleMedium)
    users.forEachIndexed { i, u ->
        ChoiceCard(
            u.title,
            listOfNotNull(if (u.admin) "Account owner" else null, if (u.protected) "PIN" else null).joinToString(" · ").ifEmpty { "Plex Home" },
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
                label = { Text("PIN for ${u.title}") },
                singleLine = true,
                visualTransformation = PasswordVisualTransformation(),
                keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.NumberPassword, imeAction = ImeAction.Done),
                keyboardActions = KeyboardActions(onDone = { switch(u, pin) }),
                modifier = Modifier.fillMaxWidth().dpadEscape().initialFocus(),
            )
            ActionButton(if (switching) "Checking…" else "Continue", { switch(u, pin) }, primary = true)
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
    FocusCard(onClick = onClick, modifier = modifier.fillMaxWidth(), shape = RoundedCornerShape(20.dp)) {
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
            error = "Sign in to Plex again to switch user."
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
            .PageTitle("Switch Plex user", account?.let { "${it.userName} on ${it.serverName}" })
        val list = users
        when {
            list == null && error == null -> Text("Loading…", color = Subtle)
            list != null && list.size < 2 -> Text("This Plex account has no other users (it is not a Plex Home).", color = Muted)
            list != null && homeToken != null && account != null ->
                PlexUserPicker(homeToken, list, busy) { userToken ->
                    busy = true
                    scope.launch {
                        try {
                            val server =
                                core.plexServers(userToken).firstOrNull { it.machineId == account.serverId }
                                    ?: throw CoreException.Invalid("This user has no access to ${account.serverName}.")
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
