// SPDX-License-Identifier: AGPL-3.0-or-later
// Baseline Profile generator + startup Macrobenchmark (run on a connected device):
//   ./gradlew :app:generateBaselineProfile
//   ./gradlew :baselineprofile:connectedBenchmarkReleaseAndroidTest
plugins {
    alias(libs.plugins.android.test)
    alias(libs.plugins.baselineprofile)
    alias(libs.plugins.ktlint)
}

android {
    namespace = "be.alwyapps.watch.baselineprofile"
    compileSdk = 37

    defaultConfig {
        minSdk = 28
        targetSdk = 36
        testInstrumentationRunner = "androidx.test.runner.AndroidJUnitRunner"
        // CI runs on an emulator: numbers are for spotting regressions, not absolute values.
        testInstrumentationRunnerArguments["androidx.benchmark.suppressErrors"] = "EMULATOR"
    }

    targetProjectPath = ":app"
}

baselineProfile {
    useConnectedDevices = true
}

dependencies {
    implementation(libs.androidx.junit)
    implementation(libs.uiautomator)
    implementation(libs.benchmark.macro)
}
