// SPDX-License-Identifier: AGPL-3.0-or-later
package be.alwyapps.watch

import android.app.Application
import android.content.Context
import android.content.pm.PackageManager
import android.net.ConnectivityManager
import android.net.Network
import android.os.Build
import be.alwyapps.watch.core.AlwyWatch
import be.alwyapps.watch.core.CoreConfig
import be.alwyapps.watch.core.media.ClientInfo
import be.alwyapps.watch.data.TokenStore
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.CoroutineStart
import kotlinx.coroutines.Deferred
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.async
import kotlinx.coroutines.launch
import java.util.UUID

class AlwyWatchApp : Application() {
    val services by lazy { Services(this) }

    override fun onCreate() {
        super.onCreate()
        // Start loading the native core right away, off the main thread.
        services.core.start()
    }
}

val Context.services: Services get() = (applicationContext as AlwyWatchApp).services

/** Process-wide singletons, created lazily. Nothing here touches the network at startup. */
class Services(
    private val app: Application,
) {
    val scope = CoroutineScope(SupervisorJob() + Dispatchers.Default)
    val isTv: Boolean =
        app.packageManager.hasSystemFeature(PackageManager.FEATURE_LEANBACK) &&
            // Debug builds: `adb push x /sdcard/Android/data/be.alwyapps.watch/files/force-touch-ui`
            // renders the phone/tablet UI on a TV device (layout testing).
            !(BuildConfig.DEBUG && java.io.File(app.getExternalFilesDir(null), "force-touch-ui").exists())
    val tokens by lazy { TokenStore(app) }

    val core: Deferred<AlwyWatch> =
        scope.async(Dispatchers.IO, start = CoroutineStart.LAZY) {
            be.alwyapps.watch.core.NativeTls
                .ensure(app)
            val core = AlwyWatch(config())
            activate(core)
            be.alwyapps.watch.keep.KeepReminderWorker
                .schedule(app)
            watchNetwork(core)
            // Deletion rules, "next N episodes", and downloads interrupted by a reboot or a
            // killed process (they continue on their own).
            if (!isTv) syncDownloads(core)
            // Enumerating codecs takes tens of ms: do it now, off the main thread.
            be.alwyapps.watch.player.DeviceProfiles
                .get(app)
            core
        }

    /**
     * Attaches stored accounts and connections to their tokens (secure storage). At startup,
     * and after accounts arrive from another device.
     */
    fun activate(core: AlwyWatch) {
        for (account in core.accounts()) {
            tokens.get(account.id)?.let { runCatching { core.activateAccount(account.id, it) } }
        }
        core.keepConnection()?.let { k ->
            val account = core.accounts().firstOrNull { it.id == k.accountId }
            val token =
                if (account?.kind == be.alwyapps.watch.core.media.ServerKind.PLEX) tokens.get("${k.accountId}:plextv") else tokens.get(k.accountId)
            token?.let { runCatching { core.keepActivate(it) } }
        }
        val seerr = core.seerrConnection()
        val bridgeAccount = seerr?.bridgeAccount?.let { id -> core.accounts().firstOrNull { it.id == id } }
        if (bridgeAccount != null) {
            // Seerr through the AlwyWatch server as one of the accounts.
            val token =
                if (bridgeAccount.kind == be.alwyapps.watch.core.media.ServerKind.PLEX) {
                    tokens.get("${bridgeAccount.id}:plextv")
                } else {
                    tokens.get(bridgeAccount.id)
                }
            token?.let { runCatching { core.seerrActivateBridge(it) } }
        } else if (seerr != null) {
            tokens.get("seerr")?.let { runCatching { core.seerrActivate(it) } }
        }
    }

    private fun config(): CoreConfig {
        val prefs = app.getSharedPreferences("device", Context.MODE_PRIVATE)
        val id =
            prefs.getString("id", null) ?: UUID.randomUUID().toString().also {
                prefs.edit().putString("id", it).apply()
            }
        return CoreConfig(
            dataDir = app.filesDir.path,
            cacheDir = app.cacheDir.path,
            client =
                ClientInfo(
                    product = "AlwyWatch",
                    version = BuildConfig.VERSION_NAME,
                    deviceName = Build.MODEL ?: "Android",
                    deviceId = id,
                    platform = if (isTv) "Android TV" else "Android",
                ),
            imageCacheMb = 300u,
            // App-specific external storage: no permission needed, removed with the app.
            downloadsDir = (app.getExternalFilesDir(android.os.Environment.DIRECTORY_MOVIES) ?: java.io.File(app.filesDir, "downloads")).path,
        )
    }

    /**
     * Download housekeeping: deletes what its rule says (offline too), tops up shows set to
     * "keep the next N episodes", then runs the queue if anything is waiting.
     */
    fun syncDownloads(core: AlwyWatch) {
        scope.launch {
            runCatching { core.syncDownloads() }
            if (core.hasPendingDownloads()) {
                be.alwyapps.watch.downloads.DownloadWorker
                    .start(app)
            }
        }
    }

    /** Sends progress reports queued while offline as soon as a network is back. */
    private fun watchNetwork(core: AlwyWatch) {
        val cm = app.getSystemService(ConnectivityManager::class.java) ?: return
        cm.registerDefaultNetworkCallback(
            object : ConnectivityManager.NetworkCallback() {
                override fun onAvailable(network: Network) {
                    scope.launch { runCatching { core.flushPendingProgress() } }
                    if (!isTv) syncDownloads(core)
                }
            },
        )
    }
}
