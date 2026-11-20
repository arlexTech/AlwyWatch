// SPDX-License-Identifier: AGPL-3.0-or-later
package be.alwyapps.watch.ui.screens

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.text.KeyboardActions
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.material3.AlertDialog
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
import androidx.compose.ui.semantics.contentType
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.unit.dp
import be.alwyapps.watch.core.AlwyWatch
import be.alwyapps.watch.core.CoreException
import be.alwyapps.watch.core.TransferCode
import be.alwyapps.watch.core.TransferRequest
import be.alwyapps.watch.data.TokenStore
import be.alwyapps.watch.services
import be.alwyapps.watch.ui.Accent
import be.alwyapps.watch.ui.ActionButton
import be.alwyapps.watch.ui.Background
import be.alwyapps.watch.ui.Danger
import be.alwyapps.watch.ui.Field
import be.alwyapps.watch.ui.LocalCore
import be.alwyapps.watch.ui.Muted
import be.alwyapps.watch.ui.Subtle
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch
import org.json.JSONObject

/** Settings that follow the user to a new device (preferences, not caches). */
private val SyncedSettings = listOf(COMBINED_KEY, PREFERRED_KEY, "current_account", "downloads.delete_rule")

/** What a signed-in device sends: the core's accounts and settings, plus their tokens. */
private fun buildBundle(
    core: AlwyWatch,
    tokens: TokenStore,
): String {
    val t = JSONObject()
    for (a in core.accounts()) {
        for (k in listOf(a.id, "${a.id}:plextv", "${a.id}:plexhome")) tokens.get(k)?.let { t.put(k, it) }
    }
    tokens.get("seerr")?.let { t.put("seerr", it) }
    return JSONObject()
        .put("core", core.exportConfig(SyncedSettings))
        .put("tokens", t)
        .toString()
}

/** New device: stores what arrived and signs in with it. Returns the account to open. */
private fun restoreBundle(
    core: AlwyWatch,
    tokens: TokenStore,
    json: String,
): String? {
    val o = JSONObject(json)
    val ids = core.importConfig(o.getString("core"))
    val t = o.getJSONObject("tokens")
    for (k in t.keys()) tokens.put(k, t.getString(k))
    return core.getSetting("current_account")?.takeIf { it in ids } ?: ids.firstOrNull()
}

/**
 * New device, from the AlwyWatch server step: shows a code to type on a signed-in device and
 * waits for the accounts (end-to-end encrypted: the server only relays them).
 */
@Composable
fun ReceiveFromDevice(
    url: String,
    onDone: (String) -> Unit,
) {
    val core = LocalCore.current
    val ctx = LocalContext.current
    var code by remember { mutableStateOf<TransferCode?>(null) }
    var error by remember { mutableStateOf<String?>(null) }
    LaunchedEffect(url) {
        while (true) {
            try {
                val c = core.transferStart(url, android.os.Build.MODEL ?: "Android")
                code = c
                error = null
                val until = System.currentTimeMillis() + c.expiresIn.toLong() * 1000
                while (System.currentTimeMillis() < until) {
                    delay(2000)
                    val bundle = runCatching { core.transferReceive(url, c.handle) }.getOrNull() ?: continue
                    val account = restoreBundle(core, ctx.services.tokens, bundle)
                    ctx.services.activate(core)
                    if (account != null) {
                        core.setSetting("current_account", account)
                        onDone(account)
                    } else {
                        error = "Nothing to copy: that device has no accounts."
                    }
                    return@LaunchedEffect
                }
                // Expired: a new code.
            } catch (e: CoreException) {
                error = e.message
                delay(5000)
            }
        }
    }
    Column(verticalArrangement = Arrangement.spacedBy(12.dp)) {
        Text("COPY FROM ANOTHER DEVICE", style = MaterialTheme.typography.labelSmall, color = Accent)
        Text(code?.code?.chunked(3)?.joinToString(" ") ?: "…", style = MaterialTheme.typography.displayLarge)
        Text(
            "On a device where you're signed in: Settings → AlwyWatch server → Add a device, and type this code.",
            color = Muted,
            style = MaterialTheme.typography.bodyMedium,
        )
        code?.let { Text("Check number: ${it.check}. It must match the one shown on the other device.", color = Subtle) }
        Text("Waiting for the other device…", style = MaterialTheme.typography.bodySmall, color = Subtle)
        error?.let { Text(it, color = Danger) }
    }
}

/**
 * Signed-in device: "Add a device". Type the new device's code, compare the check number,
 * send the accounts sealed to that device.
 */
@Composable
fun SendToDeviceDialog(onDismiss: () -> Unit) {
    val core = LocalCore.current
    val ctx = LocalContext.current
    val scope = rememberCoroutineScope()
    var code by remember { mutableStateOf("") }
    var request by remember { mutableStateOf<TransferRequest?>(null) }
    var error by remember { mutableStateOf<String?>(null) }
    var busy by remember { mutableStateOf(false) }
    var sent by remember { mutableStateOf(false) }

    fun lookUp() {
        if (busy || code.length != 6) return
        busy = true
        scope.launch {
            try {
                request = core.transferPeer(code)
                error = null
            } catch (e: CoreException) {
                error = if (e is CoreException.NotFound) "Unknown or expired code." else e.message
            } finally {
                busy = false
            }
        }
    }
    AlertDialog(
        onDismissRequest = onDismiss,
        containerColor = Background,
        title = { Text(if (sent) "Sent" else "Add a device") },
        text = {
            Column(verticalArrangement = Arrangement.spacedBy(12.dp)) {
                val r = request
                when {
                    sent -> Text("Your accounts and settings are on their way. The other device signs in by itself.", color = Muted)
                    r == null -> {
                        Text("On the new device, choose \"Copy from another device\" and type its code here.", color = Muted)
                        Field(
                            code,
                            { code = it.filter(Char::isDigit).take(6) },
                            Modifier.fillMaxWidth().semantics {
                                contentType =
                                    androidx.compose.ui.autofill
                                        .ContentType("oneTimeCode")
                            },
                            placeholder = { Text("123456") },
                            singleLine = true,
                            // A plain number: as a password field, password managers offer to fill
                            // and save it.
                            keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Number, imeAction = ImeAction.Done),
                            keyboardActions = KeyboardActions(onDone = { lookUp() }),
                        )
                    }
                    else -> {
                        Text("Send your accounts and settings to “${r.device}”?", style = MaterialTheme.typography.titleSmall)
                        Text("Check number", color = Muted, style = MaterialTheme.typography.bodySmall)
                        Text(r.check, style = MaterialTheme.typography.displaySmall, color = Accent)
                        Text("Only send if the new device shows the same number.", color = Muted, style = MaterialTheme.typography.bodySmall)
                    }
                }
                error?.let { Text(it, color = Danger, style = MaterialTheme.typography.bodySmall) }
            }
        },
        confirmButton = {
            val r = request
            when {
                sent -> ActionButton("Done", onDismiss, primary = true)
                r == null -> ActionButton(if (busy) "Checking…" else "Continue", { lookUp() }, primary = true)
                else ->
                    ActionButton(if (busy) "Sending…" else "Send", {
                        if (busy) return@ActionButton
                        busy = true
                        scope.launch {
                            try {
                                core.transferSend(code, buildBundle(core, ctx.services.tokens))
                                sent = true
                            } catch (e: CoreException) {
                                error = e.message
                            } finally {
                                busy = false
                            }
                        }
                    }, primary = true)
            }
        },
        dismissButton = { if (!sent) ActionButton("Cancel", onDismiss) },
    )
}
