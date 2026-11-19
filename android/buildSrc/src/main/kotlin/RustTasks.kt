// SPDX-License-Identifier: AGPL-3.0-or-later
import org.gradle.api.DefaultTask
import org.gradle.api.file.ConfigurableFileCollection
import org.gradle.api.file.DirectoryProperty
import org.gradle.api.provider.ListProperty
import org.gradle.api.provider.Property
import org.gradle.api.tasks.Input
import org.gradle.api.tasks.InputFiles
import org.gradle.api.tasks.Internal
import org.gradle.api.tasks.OutputDirectory
import org.gradle.api.tasks.PathSensitive
import org.gradle.api.tasks.PathSensitivity
import org.gradle.api.tasks.TaskAction
import org.gradle.process.ExecOperations
import java.io.File
import javax.inject.Inject

/** Resolves `cargo` from ~/.cargo/bin when Gradle runs without the user's shell PATH (Android Studio). */
fun cargoExecutable(): String {
    val home = File(System.getProperty("user.home"), ".cargo/bin/cargo")
    return if (home.canExecute()) home.absolutePath else "cargo"
}

abstract class RustTask : DefaultTask() {
    @get:Inject
    abstract val exec: ExecOperations

    /** Cargo workspace root. Not an input itself: [sources] lists the files that matter. */
    @get:Internal
    abstract val workspace: DirectoryProperty

    @get:InputFiles
    @get:PathSensitive(PathSensitivity.RELATIVE)
    abstract val sources: ConfigurableFileCollection

    @get:OutputDirectory
    abstract val outDir: DirectoryProperty

    protected fun cargo(
        args: List<String>,
        env: Map<String, String> = emptyMap(),
    ) {
        exec.exec {
            workingDir = workspace.get().asFile
            executable = cargoExecutable()
            args(args)
            environment(env)
        }
    }
}

/** Builds `libalwywatch_core.so` for each ABI with cargo-ndk into [outDir] (jniLibs layout). */
abstract class CargoNdkTask : RustTask() {
    @get:Input
    abstract val abis: ListProperty<String>

    @get:Input
    abstract val profile: Property<String>

    @get:Input
    abstract val minSdk: Property<Int>

    @get:Internal
    abstract val ndkDir: DirectoryProperty

    @TaskAction
    fun build() {
        // cargo-ndk only copies a library when it is newer than the one in -o: after switching
        // profiles (or ABIs) a stale .so would survive. Start from an empty directory.
        outDir.get().asFile.deleteRecursively()
        val args = mutableListOf("ndk", "--platform", minSdk.get().toString())
        abis.get().forEach { args += listOf("-t", it) }
        args += listOf("-o", outDir.get().asFile.absolutePath, "build", "--profile", profile.get(), "-p", "alwywatch-core")
        cargo(args, mapOf("ANDROID_NDK_HOME" to ndkDir.get().asFile.absolutePath))
    }
}

/**
 * Generates the Kotlin bindings. Builds the unstripped host library first (release `.so` files are
 * stripped and lose UniFFI metadata), then runs the workspace-pinned `uniffi-bindgen` in library mode.
 */
abstract class UniffiBindgenTask : RustTask() {
    @TaskAction
    fun generate() {
        cargo(listOf("build", "-q", "-p", "alwywatch-core", "-p", "uniffi-bindgen"))
        val root = workspace.get().asFile
        val os = System.getProperty("os.name").lowercase()
        val lib =
            when {
                os.contains("win") -> "alwywatch_core.dll"
                os.contains("mac") -> "libalwywatch_core.dylib"
                else -> "libalwywatch_core.so"
            }
        val bindgen = File(root, "target/debug/uniffi-bindgen" + if (os.contains("win")) ".exe" else "")
        val out = outDir.get().asFile
        out.deleteRecursively()
        exec.exec {
            workingDir = root
            executable = bindgen.absolutePath
            args(
                "generate",
                "--library",
                File(root, "target/debug/$lib").absolutePath,
                "--language",
                "kotlin",
                "--no-format",
                "--out-dir",
                out.absolutePath,
            )
        }
    }
}

/**
 * Version of the rustls-platform-verifier Android component, read from Cargo.lock so the Kotlin
 * half always matches the Rust half (as required by that crate).
 */
abstract class RustlsVerifierVersion : org.gradle.api.provider.ValueSource<String, RustlsVerifierVersion.Params> {
    interface Params : org.gradle.api.provider.ValueSourceParameters {
        val lockFile: org.gradle.api.file.RegularFileProperty
    }

    override fun obtain(): String {
        val lines = parameters.lockFile.get().asFile.readLines()
        val idx = lines.indexOfFirst { it.trim() == "name = \"rustls-platform-verifier-android\"" }
        check(idx >= 0) { "rustls-platform-verifier-android not found in Cargo.lock" }
        return lines
            .drop(idx + 1)
            .first { it.trimStart().startsWith("version = ") }
            .substringAfter('"')
            .substringBefore('"')
    }
}
