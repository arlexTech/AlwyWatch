// SPDX-License-Identifier: AGPL-3.0-or-later
package be.alwyapps.watch.core

import android.content.Context

/** Gives the Rust core's TLS verifier access to Android's trust store. Call once, first thing. */
object NativeTls {
    @JvmStatic
    private external fun init(context: Context)

    @Volatile private var done = false

    @Synchronized
    fun ensure(context: Context) {
        if (done) return
        System.loadLibrary("alwywatch_core")
        init(context.applicationContext)
        done = true
    }
}
