// SPDX-License-Identifier: AGPL-3.0-or-later
package be.alwyapps.watch.ui.screens

import android.content.Intent
import android.net.Uri
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.rememberScrollState
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
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.text.input.PasswordVisualTransformation
import androidx.compose.ui.unit.dp
import be.alwyapps.watch.core.CoreException
import be.alwyapps.watch.core.SeerrSignedIn
import be.alwyapps.watch.core.normalizeServerUrl
import be.alwyapps.watch.core.seerr.SeerrInfo
import be.alwyapps.watch.core.seerr.SeerrLogin
import be.alwyapps.watch.services
import be.alwyapps.watch.ui.ActionButton
import be.alwyapps.watch.ui.Danger
import be.alwyapps.watch.ui.Field
import be.alwyapps.watch.ui.LocalCore
import be.alwyapps.watch.ui.LocalDimens
import be.alwyapps.watch.ui.Muted
import be.alwyapps.watch.ui.dpadEscape
import be.alwyapps.watch.ui.initialFocus
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch

/** Connects a Seerr instance with whatever sign-in it accepts. */
@Composable
fun SeerrConnectScreen(onDone: () -> Unit) {
    val core = LocalCore.current
    val ctx = LocalContext.current
    val d = LocalDimens.current
    val scope = rememberCoroutineScope()
    var info by remember { mutableStateOf<SeerrInfo?>(null) }
    var method by remember { mutableStateOf<SeerrLogin?>(null) }
    var error by remember { mutableStateOf<String?>(null) }
    var url by remember { mutableStateOf("") }

    fun finish(s: SeerrSignedIn) {
        ctx.services.tokens.put("seerr", s.cookie)
        onDone()
    }

    fun attempt(block: suspend () -> Unit) {
        error = null
        scope.launch {
            try {
                block()
            } catch (e: CoreException) {
                error = e.message
            }
        }
    }

    Column(
        Modifier
            .fillMaxSize()
            .verticalScroll(rememberScrollState())
            .padding(horizontal = d.gutter, vertical = 16.dp)
            .widthIn(max = 640.dp),
        verticalArrangement = Arrangement.spacedBy(16.dp),
    ) {
        be.alwyapps.watch.ui
            .PageTitle("Requests", "Connect Seerr to ask for movies and shows")
        val i = info
        if (i == null && core.keepActive()) {
            // Works whatever media server the user signed in with (Seerr itself serves one type).
            be.alwyapps.watch.ui.Group(null, footer = "No extra sign-in: your AlwyWatch server links your Seerr account.") {
                be.alwyapps.watch.ui.ListRow(
                    "Use my AlwyWatch server",
                    subtitle = "Recommended",
                    icon = be.alwyapps.watch.ui.Icons.Hourglass,
                    onClick = {
                        attempt {
                            core.seerrConnectBridge()
                            onDone()
                        }
                    },
                    trailing = {
                        be.alwyapps.watch.ui
                            .Chevron()
                    },
                )
            }
            Text("Or connect Seerr directly", style = MaterialTheme.typography.titleSmall, color = Muted)
        }
        if (i == null) {
            val submit = { attempt { info = core.seerrInfo(normalizeServerUrl(url)) } }
            Field(
                url,
                { url = it },
                Modifier.fillMaxWidth().dpadEscape().initialFocus(),
                placeholder = { Text("requests.example.org") },
                pill = true,
                singleLine = true,
                keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Uri, imeAction = ImeAction.Go),
                keyboardActions = KeyboardActions(onGo = { submit() }),
            )
            ActionButton("Continue", { submit() }, primary = true)
        } else {
            Text("${i.title} · ${i.baseUrl}", color = Muted)
            when (method) {
                null ->
                    androidx.compose.foundation.layout.FlowRow(
                        horizontalArrangement = Arrangement.spacedBy(10.dp),
                        verticalArrangement = Arrangement.spacedBy(10.dp),
                    ) {
                        for (m in i.logins) {
                            when (m) {
                                SeerrLogin.PLEX -> ActionButton("Sign in with Plex", { method = m }, Modifier.initialFocus(), primary = true)
                                SeerrLogin.JELLYFIN -> ActionButton("Sign in with Jellyfin", { method = m }, Modifier.initialFocus(), primary = true)
                                SeerrLogin.EMBY -> ActionButton("Sign in with Emby", { method = m }, primary = true)
                                SeerrLogin.LOCAL -> ActionButton("Seerr account", { method = m })
                                SeerrLogin.NOT_CONFIGURED -> Text("This Seerr has no sign-in method enabled.", color = Danger)
                            }
                        }
                    }

                SeerrLogin.PLEX -> {
                    var code by remember { mutableStateOf<String?>(null) }
                    LaunchedEffect(Unit) {
                        try {
                            val pin = core.plexCreatePin(!d.isTv)
                            code = pin.code
                            if (pin.authUrl.isNotEmpty()) runCatching { ctx.startActivity(Intent(Intent.ACTION_VIEW, Uri.parse(pin.authUrl))) }
                            while (true) {
                                delay(2000)
                                try {
                                    finish(core.seerrSignInPlex(i.baseUrl, core.plexCheckPin(pin.id)))
                                    break
                                } catch (_: CoreException.Pending) {
                                }
                            }
                        } catch (e: CoreException) {
                            error = e.message
                        }
                    }
                    Text(if (d.isTv) "Go to plex.tv/link and enter:" else "Finish signing in to Plex in the browser.", color = Muted)
                    if (d.isTv) Text(code ?: "…", style = MaterialTheme.typography.displayMedium)
                }

                SeerrLogin.JELLYFIN, SeerrLogin.EMBY, SeerrLogin.LOCAL -> {
                    if (method == SeerrLogin.JELLYFIN) {
                        var code by remember { mutableStateOf<String?>(null) }
                        LaunchedEffect(Unit) {
                            val qc = runCatching { core.seerrQuickConnectStart(i.baseUrl) }.getOrNull() ?: return@LaunchedEffect
                            code = qc.code
                            while (true) {
                                delay(3000)
                                try {
                                    finish(core.seerrQuickConnectPoll(i.baseUrl, qc.secret))
                                    break
                                } catch (_: CoreException.Pending) {
                                } catch (e: CoreException) {
                                    break
                                }
                            }
                        }
                        code?.let {
                            Text("Quick Connect: enter this code in a signed-in Jellyfin app", color = Muted)
                            Text(it, style = MaterialTheme.typography.displayMedium)
                            Text("or sign in with your password:", color = Muted)
                        }
                    }
                    var user by remember { mutableStateOf("") }
                    var pass by remember { mutableStateOf("") }
                    val local = method == SeerrLogin.LOCAL
                    val signIn = {
                        attempt {
                            finish(
                                if (local) core.seerrSignInLocal(i.baseUrl, user, pass) else core.seerrSignInJellyfin(i.baseUrl, user, pass),
                            )
                        }
                    }
                    Field(
                        user,
                        { user = it },
                        Modifier.fillMaxWidth().dpadEscape(),
                        label = { Text(if (local) "Email" else "Username") },
                        singleLine = true,
                    )
                    Field(
                        pass,
                        { pass = it },
                        Modifier.fillMaxWidth().dpadEscape(),
                        label = { Text("Password") },
                        singleLine = true,
                        visualTransformation = PasswordVisualTransformation(),
                        keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Password, imeAction = ImeAction.Done),
                        keyboardActions = KeyboardActions(onDone = { signIn() }),
                    )
                    ActionButton("Sign in", { signIn() }, primary = true)
                }

                SeerrLogin.NOT_CONFIGURED -> {}
            }
        }
        error?.let { Text(it, color = Danger) }
    }
}
