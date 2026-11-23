// SPDX-License-Identifier: AGPL-3.0-or-later
import org.gradle.api.DefaultTask
import org.gradle.api.file.ConfigurableFileCollection
import org.gradle.api.file.DirectoryProperty
import org.gradle.api.tasks.InputFiles
import org.gradle.api.tasks.OutputDirectory
import org.gradle.api.tasks.PathSensitive
import org.gradle.api.tasks.PathSensitivity
import org.gradle.api.tasks.TaskAction

/**
 * `res/xml/locales_config.xml` from the catalogs in /i18n, so Android's per-app language
 * setting lists exactly the languages the app has. Adding /i18n/<code>.po adds it here.
 */
abstract class LocalesConfigTask : DefaultTask() {
    @get:InputFiles
    @get:PathSensitive(PathSensitivity.NAME_ONLY)
    abstract val catalogs: ConfigurableFileCollection

    @get:OutputDirectory
    abstract val outDir: DirectoryProperty

    @TaskAction
    fun run() {
        val codes = listOf("en") + catalogs.files.map { it.nameWithoutExtension }.sorted()
        val xml = outDir.get().dir("xml").asFile.apply { mkdirs() }
        xml.resolve("locales_config.xml").writeText(
            buildString {
                append("<?xml version=\"1.0\" encoding=\"utf-8\"?>\n")
                append("<locale-config xmlns:android=\"http://schemas.android.com/apk/res/android\">\n")
                for (c in codes) append("    <locale android:name=\"${c.replace('_', '-')}\" />\n")
                append("</locale-config>\n")
            },
        )
    }
}
