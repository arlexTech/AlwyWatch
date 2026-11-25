// SPDX-License-Identifier: AGPL-3.0-or-later
import Foundation
import SwiftUI
import UIKit

/// The Rust core (one per process) and the app-wide state the screens share. Like Android's
/// `Services`: nothing here touches the network at startup; screens render from the core's
/// cache, then refresh.
@MainActor
final class AppModel: ObservableObject {
    let core: AlwyWatch
    /// The account the screens show ("current_account" in the core settings, as on Android).
    @Published var account: String?
    @Published var accounts: [Account] = []
    /// Watch-state sync and the history copy (Settings).
    let sync: SyncJobs
    let downloads: DownloadRunner
    /// What the AlwyWatch server offers (Leaving soon, requests): which tabs show.
    @Published var services: Services
    /// A tab to bring to the front (the room's "Choose what to watch": Home).
    @Published var tabRequest: Int?

    func chooseForRoom() { tabRequest = 0 }

    init() {
        I18n.apply()
        core = try! AlwyWatch(config: AppModel.config())
        sync = SyncJobs(core: core)
        downloads = DownloadRunner(core: core)
        services = core.keepServices()
        Self.activate(core)
        reload()
        // A playback closed: watched downloads may go, "next episodes" may come.
        NotificationCenter.default.addObserver(forName: .playbackEnded, object: nil, queue: .main) { [weak self] _ in
            Task { @MainActor in self?.downloads.sync() }
        }
        Task.detached { [core] in
            _ = try? await core.flushPendingProgress()
            _ = try? await core.syncSettings()
        }
    }

    /// Back in the app (and at start): settings that follow the viewer, then the watch-state
    /// sync when it is due (Android's SettingsSync and SyncWorker).
    func foreground() {
        Task.detached { [core] in
            if ((try? await core.syncSettings()) ?? 0) > 0 {
                await MainActor.run { self.reload() }
            }
        }
        sync.syncIfDue()
        sync.schedule()
        // Invitations to watch together arrive while the app is in front.
        TogetherInbox.shared.start(core)
        // Rules due, "next episodes" topped up, and the queue resumed where it stopped.
        downloads.sync()
        Task {
            if core.keepActive(), let s = try? await core.refreshKeepServices() { services = s }
        }
    }

    func reload() {
        accounts = (try? core.accounts()) ?? []
        let saved = (try? core.getSetting(key: "current_account")) ?? nil
        account = accounts.first { $0.id == saved }?.id ?? accounts.first?.id
    }

    func use(_ id: String) {
        try? core.setSetting(key: "current_account", value: id)
        reload()
    }

    var current: Account? { accounts.first { $0.id == account } }

    /// After a sign-in: the tokens to the Keychain, the account opened, the AlwyWatch server
    /// (leaving soon, Seerr) connected through it when the user started from one.
    func finish(_ s: SignedIn, plexHomeToken: String? = nil) {
        Keychain.put(s.account.id, s.token)
        if let t = plexHomeToken { Keychain.put("\(s.account.id):plexhome", t) }
        if let t = s.plexAccountToken { Keychain.put("\(s.account.id):plextv", t) }
        if let t = plexHomeToken ?? s.plexAccountToken { core.setPlexAccountToken(accountId: s.account.id, token: t) }
        use(s.account.id)
        let keepToken = s.plexAccountToken ?? s.token
        Task.detached { [core] in
            _ = try? await core.keepOnboard(accountId: s.account.id, token: keepToken)
            _ = try? await core.accountLinkMedia(accountId: s.account.id, token: keepToken)
        }
    }

    func signOut(_ id: String) {
        try? core.removeAccount(accountId: id)
        for k in [id, "\(id):plextv", "\(id):plexhome"] { Keychain.remove(k) }
        Identity.shared.forgetAccount(id)
        reload()
    }

    /// The token the AlwyWatch server (and Seerr through it) accepts for a login: plex.tv's
    /// for Plex, the server's own for Jellyfin (Android's keepToken).
    nonisolated static func keepToken(_ a: Account) -> String? {
        a.kind == .plex ? Keychain.get("\(a.id):plextv") : Keychain.get(a.id)
    }

    /// Connects (or switches) the AlwyWatch server as [account], then Seerr through it if unset.
    @discardableResult
    func connectKeep(url: String, account: Account) async throws -> KeepConnection {
        guard let token = Self.keepToken(account) else {
            throw CoreError.Invalid(message: tr("Sign in to {server} again first.", ["server": account.serverName]))
        }
        let conn = try await core.keepConnect(url: url, accountId: account.id, token: token)
        lastAlwyWatchUrl = conn.baseUrl
        let off = (try? core.getSetting(key: seerrOffKey)) ?? nil
        if core.seerrConnection() == nil, off != "1" { await connectSeerrThroughKeep() }
        // Who I am there (the welcome step shows if it isn't done).
        await MyAccount.shared.linked(core)
        return conn
    }

    /// Seerr through the AlwyWatch server, trying each login it serves (Plex first: Seerr
    /// usually runs in Plex mode). The first that works is kept.
    func connectSeerrThroughKeep() async {
        guard let keep = core.keepConnection() else { return }
        let served = (try? await core.keepServers(url: keep.baseUrl)).map { Set($0.map(\.id)) }
        let candidates = ((try? core.accounts()) ?? [])
            .filter { served == nil || served!.contains($0.serverId) }
            .sorted { ($0.kind == .plex ? 0 : 1) < ($1.kind == .plex ? 0 : 1) }
        for a in candidates {
            guard let token = Self.keepToken(a) else { continue }
            if (try? await core.seerrConnectBridgeAs(accountId: a.id, token: token)) != nil { return }
        }
    }

    /// Connect to AlwyWatch with a login this device already has: no new sign-in.
    func useExistingLogin(url: String, account: Account) async throws {
        try await connectKeep(url: url, account: account)
        if let t = Self.keepToken(account) { _ = try? await core.accountLinkMedia(accountId: account.id, token: t) }
    }

    /// The AlwyWatch address typed last (prefilled next time).
    var lastAlwyWatchUrl: String? {
        get { UserDefaults.standard.string(forKey: "alwywatch.last_url") }
        set { UserDefaults.standard.set(newValue, forKey: "alwywatch.last_url") }
    }

    /// Attaches stored accounts and connections to their tokens (Keychain). At startup, and
    /// after accounts arrive from another device.
    nonisolated static func activate(_ core: AlwyWatch) {
        let accounts = (try? core.accounts()) ?? []
        for a in accounts {
            if let t = Keychain.get(a.id) { try? core.activateAccount(accountId: a.id, token: t) }
            if let t = Keychain.get("\(a.id):plexhome") ?? Keychain.get("\(a.id):plextv") {
                core.setPlexAccountToken(accountId: a.id, token: t)
            }
        }
        if let k = core.keepConnection() {
            let plex = accounts.first { $0.id == k.accountId }?.kind == .plex
            if let t = plex ? Keychain.get("\(k.accountId):plextv") : Keychain.get(k.accountId) {
                try? core.keepActivate(token: t)
            }
        }
        if core.account() != nil, let t = Keychain.get(accountTokenKey) { try? core.accountActivate(token: t) }
        if let seerr = core.seerrConnection() {
            if let bridge = seerr.bridgeAccount, let a = accounts.first(where: { $0.id == bridge }) {
                if let t = a.kind == .plex ? Keychain.get("\(a.id):plextv") : Keychain.get(a.id) {
                    try? core.seerrActivateBridge(token: t)
                }
            } else if let t = Keychain.get("seerr") {
                try? core.seerrActivate(cookie: t)
            }
        }
    }

    private static func config() -> CoreConfig {
        let fm = FileManager.default
        let support = fm.urls(for: .applicationSupportDirectory, in: .userDomainMask)[0]
        let caches = fm.urls(for: .cachesDirectory, in: .userDomainMask)[0]
        let downloads = support.appendingPathComponent("downloads")
        for d in [support, caches, downloads] { try? fm.createDirectory(at: d, withIntermediateDirectories: true) }
        // Downloads can be fetched again: not in the phone's backups.
        var noBackup = URLResourceValues()
        noBackup.isExcludedFromBackup = true
        var dir = downloads
        try? dir.setResourceValues(noBackup)
        let defaults = UserDefaults.standard
        let id = defaults.string(forKey: "device.id") ?? {
            let new = UUID().uuidString
            defaults.set(new, forKey: "device.id")
            return new
        }()
        let version = Bundle.main.object(forInfoDictionaryKey: "CFBundleShortVersionString") as? String ?? "0"
        let pad = UIDevice.current.userInterfaceIdiom == .pad
        return CoreConfig(
            dataDir: support.path,
            cacheDir: caches.path,
            client: ClientInfo(
                product: "AlwyWatch",
                version: version,
                deviceName: UIDevice.current.name,
                deviceId: id,
                platform: pad ? "iPadOS" : "iOS"
            ),
            imageCacheMb: 300,
            downloadsDir: downloads.path,
            profile: nil,
            downloadsDb: nil
        )
    }
}

/// The AlwyWatch account token, in the Keychain like media tokens (Android's ACCOUNT_TOKEN).
let accountTokenKey = "alwywatch_account"

/// An error's sentence for the screen (the core's are already translated).
func message(_ error: Error) -> String {
    guard let e = error as? CoreError else { return error.localizedDescription }
    switch e {
    case let .Unauthorized(m), let .Forbidden(m), let .NotFound(m), let .Server(m), let .Network(m),
         let .Pending(m), let .Invalid(m), let .NotSignedIn(m), let .Storage(m), let .Internal(m):
        return m
    }
}

/// True for "not approved yet" while polling a sign-in.
func isPending(_ error: Error) -> Bool {
    if case .Pending = error as? CoreError { return true }
    return false
}
