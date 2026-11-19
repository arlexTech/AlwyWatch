// SPDX-License-Identifier: AGPL-3.0-or-later
plugins {
    alias(libs.plugins.android.application)
    alias(libs.plugins.kotlin.compose)
    alias(libs.plugins.ktlint)
}

val rustRoot: File = rootDir.parentFile
val ndkVersionPinned = "29.0.14206865"
val rustProfile = providers.gradleProperty("alwywatch.rustProfile").getOrElse("release")
val rustAbis = providers.gradleProperty("alwywatch.abis").getOrElse("arm64-v8a,armeabi-v7a,x86_64").split(",")

val rustSources =
    fileTree(rustRoot) {
        include("core/**/*.rs", "core/**/Cargo.toml", "core/**/uniffi.toml", "tools/**/*.rs", "Cargo.toml", "Cargo.lock")
    }

val cargoNdk =
    tasks.register<CargoNdkTask>("cargoNdk") {
        group = "rust"
        description = "Builds libalwywatch_core.so for each ABI with cargo-ndk"
        workspace.set(rustRoot)
        sources.from(rustSources)
        abis.set(rustAbis)
        profile.set(rustProfile)
        minSdk.set(26)
        ndkDir.set(androidComponents.sdkComponents.sdkDirectory.map { it.dir("ndk/$ndkVersionPinned") })
        outDir.set(layout.buildDirectory.dir("generated/rustJniLibs"))
    }

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
    }

    buildTypes {
        release {
            isMinifyEnabled = true
            isShrinkResources = true
            proguardFiles(getDefaultProguardFile("proguard-android-optimize.txt"), "proguard-rules.pro")
            // Signing is configured from ~/.config/alwywatch (outside the repo) once the release key exists.
        }
    }

    splits {
        abi {
            isEnable = true
            reset()
            include(*rustAbis.toTypedArray())
            isUniversalApk = true
        }
    }

    buildFeatures {
        compose = true
        buildConfig = true
    }

    packaging {
        resources.excludes += setOf("/META-INF/{AL2.0,LGPL2.1}", "DebugProbesKt.bin", "kotlin-tooling-metadata.json")
    }
}

androidComponents {
    onVariants { variant ->
        variant.sources.jniLibs?.addGeneratedSourceDirectory(cargoNdk, CargoNdkTask::outDir)
        variant.sources.kotlin?.addGeneratedSourceDirectory(uniffiBindgen, UniffiBindgenTask::outDir)
    }
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
    debugImplementation(libs.compose.ui.tooling)
}
