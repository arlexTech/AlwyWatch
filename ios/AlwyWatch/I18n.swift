// SPDX-License-Identifier: AGPL-3.0-or-later
import Foundation

/// Translations (docs/i18n.md), the same catalogs as Android, desktop and web (/i18n/*.po,
/// compiled into the core). The English text is its own key:
/// `tr("Sign in")`, `tr("Leaves in {days} days", ["days": 3])`, `trn("{n} title", "{n} titles", n)`.
enum I18n {
    private static var table: [String: [String]] = [:]
    private(set) static var language = "en"
    private static let key = "language"

    /// The choice in Settings: "" follows the system.
    static var choice: String { UserDefaults.standard.string(forKey: key) ?? "" }

    static func apply() {
        let code = choice.isEmpty ? i18nBest(locales: Locale.preferredLanguages) : choice
        i18nSetLanguage(code: code)
        var t: [String: [String]] = [:]
        if code != "en" {
            for e in i18nTable(code: code) { t[e.id] = e.forms }
        }
        table = t
        language = code
    }

    static func choose(_ code: String) {
        UserDefaults.standard.set(code, forKey: key)
        apply()
    }

    static func lookup(_ text: String) -> String { table[text]?.first ?? text }

    static func plural(_ one: String, _ many: String, _ n: Int) -> String {
        guard let forms = table[one] else { return n == 1 ? one : many }
        let i = Int(i18nPluralIndex(n: UInt64(max(n, 0))))
        return i < forms.count ? forms[i] : (forms.last ?? many)
    }
}

private func fill(_ text: String, _ args: [String: Any]) -> String {
    var out = text
    for (k, v) in args { out = out.replacingOccurrences(of: "{\(k)}", with: "\(v)") }
    return out
}

/// The translation of `text`, with each `{name}` replaced by its value from `args`.
func tr(_ text: String, _ args: [String: Any] = [:]) -> String { fill(I18n.lookup(text), args) }

/// The plural form of `one`/`many` for `n`, `{n}` replaced by `n`, then `args`.
func trn(_ one: String, _ many: String, _ n: Int, _ args: [String: Any] = [:]) -> String {
    fill(I18n.plural(one, many, n).replacingOccurrences(of: "{n}", with: "\(n)"), args)
}
