// SPDX-License-Identifier: AGPL-3.0-or-later
package be.alwyapps.watch.baselineprofile

import androidx.benchmark.macro.BaselineProfileMode
import androidx.benchmark.macro.CompilationMode
import androidx.benchmark.macro.StartupMode
import androidx.benchmark.macro.StartupTimingMetric
import androidx.benchmark.macro.junit4.MacrobenchmarkRule
import androidx.test.ext.junit.runners.AndroidJUnit4
import org.junit.Rule
import org.junit.Test
import org.junit.runner.RunWith

/** Cold start, without and with the Baseline Profile (timeToInitialDisplay / FullDisplay). */
@RunWith(AndroidJUnit4::class)
class StartupBenchmark {
    @get:Rule
    val rule = MacrobenchmarkRule()

    @Test
    fun coldStartNoProfile() = run(CompilationMode.None())

    @Test
    fun coldStartBaselineProfile() = run(CompilationMode.Partial(BaselineProfileMode.Require))

    private fun run(mode: CompilationMode) =
        rule.measureRepeated(
            packageName = PACKAGE,
            metrics = listOf(StartupTimingMetric()),
            compilationMode = mode,
            startupMode = StartupMode.COLD,
            iterations = 10,
        ) {
            startAndWait()
        }
}
