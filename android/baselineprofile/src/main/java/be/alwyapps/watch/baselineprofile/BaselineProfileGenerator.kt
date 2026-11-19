// SPDX-License-Identifier: AGPL-3.0-or-later
package be.alwyapps.watch.baselineprofile

import androidx.benchmark.macro.junit4.BaselineProfileRule
import androidx.test.ext.junit.runners.AndroidJUnit4
import org.junit.Rule
import org.junit.Test
import org.junit.runner.RunWith

/** Records the classes and methods used at startup so ART compiles them ahead of time. */
@RunWith(AndroidJUnit4::class)
class BaselineProfileGenerator {
    @get:Rule
    val rule = BaselineProfileRule()

    @Test
    fun startup() =
        rule.collect(packageName = PACKAGE, includeInStartupProfile = true) {
            startAndWait()
        }
}
