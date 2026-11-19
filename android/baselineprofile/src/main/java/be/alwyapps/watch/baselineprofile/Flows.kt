// SPDX-License-Identifier: AGPL-3.0-or-later
package be.alwyapps.watch.baselineprofile

import androidx.benchmark.macro.MacrobenchmarkScope
import androidx.test.uiautomator.By
import androidx.test.uiautomator.Until

const val PACKAGE = "be.alwyapps.watch"

/** Cold start until the first screen has real content (not just the first frame). */
fun MacrobenchmarkScope.startAndWait() {
    pressHome()
    startActivityAndWait()
    // First run: "Add a server". Signed in: the home rails.
    device.wait(Until.hasObject(By.textContains("server").pkg(PACKAGE)), 5_000) ||
        device.wait(Until.hasObject(By.scrollable(true).pkg(PACKAGE)), 5_000)
}
