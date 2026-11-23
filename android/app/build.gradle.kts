// SPDX-License-Identifier: AGPL-3.0-or-later
import java.util.Properties

plugins {
    alias(libs.plugins.android.application)
    alias(libs.plugins.kotlin.compose)
    alias(libs.plugins.ktlint)
    alias(libs.plugins.baselineprofile)
}

val rustRoot: File = rootDir.parentFile
val ndkVersionPinned = "29.0.14206865"
val rustProfile = providers.gradleProperty("alwywatch.rustProfile").getOrElse("release")
val rustAbis = providers.gradleProperty("alwywatch.abis").getOrElse("arm64-v8a,armeabi-v7a,x86_64").split(",")

val rustSources =
    fileTree(rustRoot) {
        include("core/**/*.rs", "core/**/Cargo.toml", "core/**/uniffi.toml", "tools/**/*.rs", "Cargo.toml", "Cargo.lock")
    }

// Release variants: the configured profile (fat LTO). Debug variants: "android-dev" (no LTO,
// incremental), so a core change rebuilds in seconds when testing.
fun cargoNdkTask(
    name: String,
    taskProfile: String,
) = tasks.register<CargoNdkTask>(name) {
    group = "rust"
    description = "Builds libalwywatch_core.so ($taskProfile) for each ABI with cargo-ndk"
    workspace.set(rustRoot)
    sources.from(rustSources)
    abis.set(rustAbis)
    profile.set(taskProfile)
    minSdk.set(26)
    ndkDir.set(androidComponents.sdkComponents.sdkDirectory.map { it.dir("ndk/$ndkVersionPinned") })
    outDir.set(layout.buildDirectory.dir("generated/rustJniLibs/$taskProfile"))
}

val cargoNdk = cargoNdkTask("cargoNdk", rustProfile)
val cargoNdkDev = cargoNdkTask("cargoNdkDev", "android-dev")

val uniffiBindgen =
    tasks.register<UniffiBindgenTask>("uniffiBindgen") {
        group = "rust"
        description = "Generates Kotlin bindings for the Rust core"
        workspace.set(rustRoot)
        sources.from(rustSources)
        outDir.set(layout.buildDirectory.dir("generated/uniffi/kotlin"))
    }

val rustlsVerifierVersion =
    providers.of(RustlsVerifierVersion::class.java) {
        parameters.lockFile.set(File(rustRoot, "Cargo.lock"))
    }

// Release key lives outside the repo: ~/.config/alwywatch/signing.properties (storeFile,
// storePassword, keyAlias, keyPassword). Without it, release APKs are built unsigned.
val signingProps =
    File(System.getProperty("user.home"), ".config/alwywatch/signing.properties")
        .takeIf { it.isFile }
        ?.let { f -> Properties().apply { f.inputStream().use { load(it) } } }

// `./gradlew bundleRelease`: the Play Store bundle (.aab) instead of direct-download APKs.
val playBundle = gradle.startParameter.taskNames.any { it.contains("bundle", ignoreCase = true) }

android {
    namespace = "be.alwyapps.watch"
    compileSdk = 37
    ndkVersion = ndkVersionPinned

    defaultConfig {
        applicationId = "be.alwyapps.watch"
        minSdk = 26
        targetSdk = 36
        versionCode = 1
        versionName = "0.1.0"
        // Libraries bring native code for CPUs the Rust core isn't built for: a bundle with
        // those would install there and crash. (APK builds get this from the ABI splits.)
        if (playBundle) ndk { abiFilters += rustAbis }
    }

    signingConfigs {
        if (signingProps != null) {
            create("release") {
                storeFile = File(signingProps.getProperty("storeFile"))
                storePassword = signingProps.getProperty("storePassword")
                keyAlias = signingProps.getProperty("keyAlias")
                keyPassword = signingProps.getProperty("keyPassword")
            }
        }
    }

    buildTypes {
        debug {
            // Same id and, when the release key is present, same signature as release: a dev
            // build installs over the release app and keeps its sign-in and cache.
            signingConfigs.findByName("release")?.let { signingConfig = it }
            resValue("string", "app_name", "AlwyWatch Dev")
        }
        release {
            resValue("string", "app_name", "AlwyWatch")
            signingConfig = signingConfigs.findByName("release")
            isMinifyEnabled = true
            isShrinkResources = true
            proguardFiles(getDefaultProguardFile("proguard-android-optimize.txt"), "proguard-rules.pro")
        }
    }

    splits {
        abi {
            // A bundle is split by Play itself; the build refuses both at once.
            isEnable = !playBundle
            reset()
            include(*rustAbis.toTypedArray())
            isUniversalApk = true
        }
    }

    buildFeatures {
        compose = true
        buildConfig = true
        resValues = true
    }

    packaging {
        // Direct-download APKs store native libraries compressed: ~40% smaller download, the
        // system extracts them once at install time. A Play bundle (`bundleRelease`) keeps them
        // uncompressed and page-aligned instead: Play compresses the download on its own.
        jniLibs.useLegacyPackaging = !playBundle
        resources.excludes += setOf("/META-INF/{AL2.0,LGPL2.1}", "DebugProbesKt.bin", "kotlin-tooling-metadata.json")
    }
}

androidComponents {
    onVariants { variant ->
        val rust = if (variant.buildType == "debug") cargoNdkDev else cargoNdk
        variant.sources.jniLibs?.addGeneratedSourceDirectory(rust, CargoNdkTask::outDir)
        variant.sources.kotlin?.addGeneratedSourceDirectory(uniffiBindgen, UniffiBindgenTask::outDir)
    }
}

baselineProfile {
    // Regenerated on demand (needs a device): ./gradlew :app:generateBaselineProfile
    automaticGenerationDuringBuild = false
    saveInSrc = true
}

ktlint {
    filter {
        exclude { it.file.path.contains("/build/") }
    }
}

dependencies {
    implementation(platform(libs.compose.bom))
    implementation(libs.compose.ui)
    implementation(libs.compose.foundation)
    implementation(libs.compose.material3)
    implementation(libs.compose.ui.tooling.preview)
    implementation(libs.activity.compose)
    // UniFFI's Kotlin bindings call the Rust library through JNA.
    implementation("${libs.jna.get()}@aar")
    // Kotlin half of rustls-platform-verifier; version must equal the Rust crate's (Cargo.lock).
    implementation("org.rustls:rustls-platform-verifier:${rustlsVerifierVersion.get()}")
    // The generated bindings expose async Rust functions as suspend functions.
    implementation(libs.coroutines.android)
    // Resumable downloads run as a foreground job that WorkManager restarts when the network returns.
    implementation(libs.work.runtime)
    implementation(libs.media3.exoplayer)
    // Only for the server-transcode fallback (Jellyfin/Plex serve HLS).
    implementation(libs.media3.exoplayer.hls)
    implementation(libs.media3.ui)
    // Software decoding of DTS/DTS-HD/TrueHD/(E-)AC3 on devices without those decoders,
    // so audio never needs a server transcode.
    implementation(libs.media3.ffmpeg)
    // libass rendering for ASS/SSA subtitles (styles, positioning, karaoke).
    implementation(libs.ass.media)
    // Installs the Baseline Profile on devices where the store does not (sideloads, F-Droid).
    implementation(libs.profileinstaller)
    baselineProfile(project(":baselineprofile"))
    debugImplementation(libs.compose.ui.tooling)
}
