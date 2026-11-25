// SPDX-License-Identifier: AGPL-3.0-or-later
import Foundation
import Security

/// Media-server and AlwyWatch tokens, in the iOS Keychain (this device only, never in backups).
/// The same keys as Android's TokenStore: "<account id>", "<account id>:plextv", "seerr"…
enum Keychain {
    private static let service = "be.alwyapps.watch.tokens"
    /// The profile whose tokens are read and written (each profile its own; "default" keeps the
    /// keys it always had).
    static var profile = ProfileStore.defaultId

    private static func prefix(_ profile: String) -> String { "p:\(profile):" }

    private static func scoped(_ key: String) -> String {
        profile == ProfileStore.defaultId ? key : prefix(profile) + key
    }

    static func get(_ key: String) -> String? {
        let query: [String: Any] = [
            kSecClass as String: kSecClassGenericPassword,
            kSecAttrService as String: service,
            kSecAttrAccount as String: scoped(key),
            kSecReturnData as String: true,
            kSecMatchLimit as String: kSecMatchLimitOne,
        ]
        var out: AnyObject?
        guard SecItemCopyMatching(query as CFDictionary, &out) == errSecSuccess,
              let data = out as? Data else { return nil }
        return String(data: data, encoding: .utf8)
    }

    static func put(_ key: String, _ value: String) {
        remove(key)
        let query: [String: Any] = [
            kSecClass as String: kSecClassGenericPassword,
            kSecAttrService as String: service,
            kSecAttrAccount as String: scoped(key),
            kSecAttrAccessible as String: kSecAttrAccessibleAfterFirstUnlockThisDeviceOnly,
            kSecValueData as String: Data(value.utf8),
        ]
        SecItemAdd(query as CFDictionary, nil)
    }

    static func remove(_ key: String) {
        let query: [String: Any] = [
            kSecClass as String: kSecClassGenericPassword,
            kSecAttrService as String: service,
            kSecAttrAccount as String: scoped(key),
        ]
        SecItemDelete(query as CFDictionary)
    }

    /// The profile in use's keys (to copy them to another device).
    static func keys() -> [String] { keys(profile: profile) }

    /// A profile's keys, without their profile prefix.
    static func keys(profile id: String) -> [String] {
        let all = allKeys()
        if id == ProfileStore.defaultId { return all.filter { !$0.hasPrefix("p:") } }
        let p = prefix(id)
        return all.filter { $0.hasPrefix(p) }.map { String($0.dropFirst(p.count)) }
    }

    /// A removed profile's tokens.
    static func removeAll(profile id: String) {
        guard id != ProfileStore.defaultId else { return }
        for k in allKeys() where k.hasPrefix(prefix(id)) {
            let query: [String: Any] = [
                kSecClass as String: kSecClassGenericPassword,
                kSecAttrService as String: service,
                kSecAttrAccount as String: k,
            ]
            SecItemDelete(query as CFDictionary)
        }
    }

    private static func allKeys() -> [String] {
        let query: [String: Any] = [
            kSecClass as String: kSecClassGenericPassword,
            kSecAttrService as String: service,
            kSecReturnAttributes as String: true,
            kSecMatchLimit as String: kSecMatchLimitAll,
        ]
        var out: AnyObject?
        guard SecItemCopyMatching(query as CFDictionary, &out) == errSecSuccess,
              let items = out as? [[String: Any]] else { return [] }
        return items.compactMap { $0[kSecAttrAccount as String] as? String }
    }
}
