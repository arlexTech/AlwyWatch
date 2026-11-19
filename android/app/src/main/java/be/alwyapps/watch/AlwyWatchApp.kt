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
    val isTv: Boolean = app.packageManager.hasSystemFeature(PackageManager.FEATURE_LEANBACK)
    val tokens by lazy { TokenStore(app) }

    val core: Deferred<AlwyWatch> =
        scope.async(Dispatchers.IO, start = CoroutineStart.LAZY) {
            be.alwyapps.watch.core.NativeTls
                .ensure(app)
            val core = AlwyWatch(config())
            for (account in core.accounts()) {
                tokens.get(account.id)?.let { core.activateAccount(account.id, it) }
            }
            watchNetwork(core)
            // Enumerating codecs takes tens of ms: do it now, off the main thread.
            be.alwyapps.watch.player.DeviceProfiles
                .get(app)
            core
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
        )
    }

    /** Sends progress reports queued while offline as soon as a network is back. */
    private fun watchNetwork(core: AlwyWatch) {
        val cm = app.getSystemService(ConnectivityManager::class.java) ?: return
        cm.registerDefaultNetworkCallback(
            object : ConnectivityManager.NetworkCallback() {
                override fun onAvailable(network: Network) {
                    scope.launch { runCatching { core.flushPendingProgress() } }
                }
            },
        )
    }
}
