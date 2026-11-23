// SPDX-License-Identifier: AGPL-3.0-or-later
package be.alwyapps.watch.ui

import android.content.Context
import be.alwyapps.watch.core.i18nBest
import be.alwyapps.watch.core.i18nLanguages
import be.alwyapps.watch.core.i18nPluralIndex
import be.alwyapps.watch.core.i18nSetLanguage
import be.alwyapps.watch.core.i18nTable

/**
 * Translations (docs/i18n.md). The text in the code is English and is its own key:
 * `tr("Sign in")`, `tr("Leaves in {days} days", "days" to 3)`, `trn("{n} title", "{n} titles", n)`.
 * The catalogs are the shared /i18n/<lang>.po files, compiled into the core; this keeps the
 * selected language's table in a HashMap, loaded once before the first frame.
 */
object I18n {
    private const val PREFS = "device"
    private const val KEY = "language"

    @Volatile private var table: Map<String, List<String>> = emptyMap()
    private val plurals = java.util.concurrent.ConcurrentHashMap<Long, Int>()

    /** The language in use ("en": the text in the code). */
    @Volatile var language: String = "en"
        private set

    /** The choice in Settings: "" follows the system. */
    fun choice(ctx: Context): String = ctx.getSharedPreferences(PREFS, Context.MODE_PRIVATE).getString(KEY, "") ?: ""

    /** Every language, English first: (code, own name). */
    fun languages(): List<Pair<String, String>> = i18nLanguages().map { it.code to it.name }

    /** Selects the language from the setting or the system; needs the native core loaded. */
    fun apply(ctx: Context) {
        val chosen = choice(ctx)
        val code =
            chosen.ifEmpty {
                val locales = ctx.resources.configuration.locales
                i18nBest((0 until locales.size()).map { locales[it].toLanguageTag() })
            }
        i18nSetLanguage(code)
        table = if (code == "en") emptyMap() else i18nTable(code).associate { it.id to it.forms }
        plurals.clear()
        language = code
    }

    /** Saves the choice ("" = the system's) and applies it; the caller recreates the screens. */
    fun choose(
        ctx: Context,
        code: String,
    ) {
        ctx
            .getSharedPreferences(PREFS, Context.MODE_PRIVATE)
            .edit()
            .putString(KEY, code)
            .apply()
        apply(ctx)
    }

    internal fun lookup(text: String): String = table[text]?.firstOrNull() ?: text

    internal fun plural(
        one: String,
        many: String,
        n: Long,
    ): String {
        val forms = table[one] ?: return if (n == 1L) one else many
        val i = plurals.getOrPut(n) { i18nPluralIndex(n.toULong()).toInt() }
        return forms.getOrNull(i) ?: forms.last()
    }
}

private fun fill(
    text: String,
    args: Array<out Pair<String, Any?>>,
): String {
    var out = text
    for ((k, v) in args) out = out.replace("{$k}", v.toString())
    return out
}

/** The translation of [text], with each `{name}` replaced by its value from [args]. */
fun tr(
    text: String,
    vararg args: Pair<String, Any?>,
): String = fill(I18n.lookup(text), args)

/** The plural form of [one]/[many] for [n], `{n}` replaced by [n], then [args]. */
fun trn(
    one: String,
    many: String,
    n: Number,
    vararg args: Pair<String, Any?>,
): String = fill(I18n.plural(one, many, n.toLong()).replace("{n}", n.toString()), args)
