// SPDX-License-Identifier: AGPL-3.0-or-later
import SwiftUI

/// The pages of Settings, in the same order on every app (Android's Section).
enum SettingsSection: String, CaseIterable, Hashable {
    case account, servers, requests, playback, audio, interface, downloads, device, admin

    var label: String {
        switch self {
        case .account: return tr("Account")
        case .servers: return tr("Servers")
        case .requests: return tr("Requests")
        case .playback: return tr("Playback")
        case .audio: return tr("Audio & subtitles")
        case .interface: return tr("Menu & pages")
        case .downloads: return tr("Downloads")
        case .device: return tr("This device")
        case .admin: return tr("Admin")
        }
    }

    var icon: String {
        switch self {
        case .account: return "person"
        case .servers: return "square.stack"
        case .requests: return "plus"
        case .playback: return "play"
        case .audio: return "captions.bubble"
        case .interface: return "house"
        case .downloads: return "arrow.down.circle"
        case .device: return "iphone"
        case .admin: return "gearshape"
        }
    }
}

/// The player's settings, stored in the core (the same keys as Android: they follow the
/// viewer to their other devices).
struct PlayerSettings {
    let core: AlwyWatch

    private func get(_ k: String) -> String? { (try? core.getSetting(key: k)) ?? nil }
    private func set(_ k: String, _ v: String) { try? core.setSetting(key: k, value: v) }
    private func flag(_ k: String, _ v: Bool) { set(k, v ? "1" : "0") }
    private func int(_ k: String, _ d: Int) -> Int { get(k).flatMap(Int.init) ?? d }

    var autoSkipIntro: Bool { get { get("player.auto_skip_intro") == "1" } nonmutating set { flag("player.auto_skip_intro", newValue) } }
    var autoPlayNext: Bool { get { get("player.auto_next") != "0" } nonmutating set { flag("player.auto_next", newValue) } }
    var countdownS: Int { get { int("player.countdown", 10) } nonmutating set { set("player.countdown", String(newValue)) } }
    var seekBackS: Int { get { int("player.seek_back", 10) } nonmutating set { set("player.seek_back", String(newValue)) } }
    var seekForwardS: Int { get { int("player.seek_forward", 30) } nonmutating set { set("player.seek_forward", String(newValue)) } }
    var resumeRewindS: Int { get { int("player.resume_rewind", 0) } nonmutating set { set("player.resume_rewind", String(newValue)) } }
    var stillWatchingH: Int { get { int("player.still_watching", 2) } nonmutating set { set("player.still_watching", String(newValue)) } }
    var audioLanguage: String { get { get("player.audio_lang") ?? "" } nonmutating set { set("player.audio_lang", newValue) } }
    var subtitleMode: String { get { get("player.sub_mode") ?? "auto" } nonmutating set { set("player.sub_mode", newValue) } }
    var subtitleLanguage: String { get { get("player.sub_lang") ?? "" } nonmutating set { set("player.sub_lang", newValue) } }
    var subtitleSize: String { get { get("player.sub_size") ?? "normal" } nonmutating set { set("player.sub_size", newValue) } }
    var subtitleColor: String { get { get("player.sub_color") ?? "white" } nonmutating set { set("player.sub_color", newValue) } }
    var subtitleBackground: Bool { get { get("player.sub_bg") == "1" } nonmutating set { flag("player.sub_bg", newValue) } }
    var subtitleRaised: Bool { get { get("player.sub_raised") == "1" } nonmutating set { flag("player.sub_raised", newValue) } }
    /// Show pages play the show's theme song quietly (like Plex); off by default on phones.
    var themeMusic: Bool { get { get("ui.theme_music") == "1" } nonmutating set { flag("ui.theme_music", newValue) } }
}

private func capitalizedFirstWord(_ s: String) -> String { s.prefix(1).uppercased() + s.dropFirst() }

/// Settings → Menu & pages.
private struct InterfaceSettings: View {
    let settings: PlayerSettings

    var body: some View {
        Section(tr("Pages and app")) {
            SettingToggle(title: tr("Theme music on show pages"), subtitle: tr("Quietly, when the server has the show's theme."), value: settings.themeMusic) {
                settings.themeMusic = $0
            }
        }
    }
}

/// "French" for "fra"/"fre"/"fr", in the app's language.
func languageName(_ code: String?) -> String? {
    guard let code, !code.isEmpty else { return nil }
    let name = Locale(identifier: I18n.language).localizedString(forLanguageCode: code) ?? code
    return name.prefix(1).uppercased() + name.dropFirst()
}

private func seconds(_ n: Int) -> String { trn("{n} second", "{n} seconds", n) }
private func onOff(_ b: Bool) -> String { b ? tr("on") : tr("off") }

/// Settings: one row per section, each saying what is set there now (Android's SettingsScreen).
struct SettingsScreen: View {
    @EnvironmentObject var model: AppModel

    var body: some View {
        let admin = model.core.keepConnection()?.isAdmin == true
        List {
            ForEach(SettingsSection.allCases.filter { $0 != .admin || admin }, id: \.self) { s in
                NavigationLink(value: Route.settingsSection(s.rawValue)) {
                    HStack(spacing: 14) {
                        Image(systemName: s.icon).foregroundStyle(Palette.accent).frame(width: 26)
                        VStack(alignment: .leading, spacing: 2) {
                            Text(s.label).foregroundStyle(Palette.onSurface)
                            Text(summary(s)).font(.footnote).foregroundStyle(Palette.muted).lineLimit(2)
                        }
                    }
                }
            }
        }
        .scrollContentBackground(.hidden)
        .background(Palette.background.ignoresSafeArea())
        .navigationTitle(tr("Settings"))
    }

    private func summary(_ s: SettingsSection) -> String {
        let core = model.core
        let settings = PlayerSettings(core: core)
        switch s {
        case .account:
            let keep = core.keepConnection()
            let account = core.account()
            guard let address = (keep?.baseUrl ?? account?.baseUrl).map(host) else {
                return tr("No AlwyWatch account · connect to AlwyWatch")
            }
            return [account?.name ?? keep?.userName, address].compactMap { $0 }.joined(separator: " · ")
        case .servers:
            let list = model.accounts.map { "\($0.serverName) (\(kindName($0.kind)))" }.joined(separator: " · ")
            return list.isEmpty ? tr("No server yet") : list
        case .requests:
            guard let seerr = core.seerrConnection() else { return tr("Off") }
            return seerr.viaKeep ? tr("Through your AlwyWatch server") : tr("Request site {address}", ["address": host(seerr.baseUrl)])
        case .playback:
            return tr("Skip intros {intros} · next episode {next} · −{back} s / +{forward} s", [
                "intros": onOff(settings.autoSkipIntro), "next": onOff(settings.autoPlayNext),
                "back": settings.seekBackS, "forward": settings.seekForwardS,
            ])
        case .audio:
            let mode: String
            switch settings.subtitleMode {
            case "forced": mode = tr("subtitles forced only")
            case "always": mode = tr("subtitles always")
            case "off": mode = tr("subtitles off")
            default: mode = tr("subtitles automatic")
            }
            return [languageName(settings.audioLanguage) ?? tr("Original audio"), mode, sizeName(settings.subtitleSize).lowercased()]
                .joined(separator: " · ")
        case .interface:
            return capitalizedFirstWord(tr("theme music"))
        case .downloads:
            return tr("When to delete, manage downloads")
        case .device:
            let lang = i18nLanguages().first { $0.code == I18n.language }?.name
            let version = Bundle.main.object(forInfoDictionaryKey: "CFBundleShortVersionString") as? String ?? ""
            return [lang, "AlwyWatch \(version)"].compactMap { $0 }.joined(separator: " · ")
        case .admin:
            return tr("People, countdown, invitations, server status")
        }
    }
}

func host(_ url: String) -> String {
    var s = url
    if let r = s.range(of: "://") { s = String(s[r.upperBound...]) }
    while s.hasSuffix("/") { s.removeLast() }
    return s
}

private func sizeName(_ s: String) -> String {
    switch s {
    case "small": return tr("Small")
    case "large": return tr("Large")
    case "huge": return tr("Huge")
    default: return tr("Normal")
    }
}

/// One section of Settings on its own page.
struct SettingsSectionView: View {
    @EnvironmentObject var model: AppModel
    let section: SettingsSection

    var body: some View {
        List {
            switch section {
            case .account: AccountSettings()
            case .servers: ServersSettings()
            case .requests: RequestsSettings()
            case .playback: PlaybackSettings(settings: PlayerSettings(core: model.core))
            case .audio: AudioSettings(settings: PlayerSettings(core: model.core))
            case .interface: InterfaceSettings(settings: PlayerSettings(core: model.core))
            case .downloads: DownloadSettings()
            case .device: DeviceSettings()
            case .admin: AdminSettings()
            }
        }
        .scrollContentBackground(.hidden)
        .background(Palette.background.ignoresSafeArea())
        .navigationTitle(section.label)
    }
}

/// A row with a title and its explanation below.
private struct TitledLabel: View {
    let title: String
    var subtitle: String?

    var body: some View {
        VStack(alignment: .leading, spacing: 2) {
            Text(title).foregroundStyle(Palette.onSurface)
            if let subtitle { Text(subtitle).font(.footnote).foregroundStyle(Palette.muted) }
        }
    }
}

/// A switch stored by `set` (its state kept here so it moves at once).
private struct SettingToggle: View {
    let title: String
    var subtitle: String?
    @State var value: Bool
    let set: (Bool) -> Void

    var body: some View {
        Toggle(isOn: Binding(get: { value }, set: { value = $0; set($0) })) {
            TitledLabel(title: title, subtitle: subtitle)
        }
        .tint(Palette.accent)
    }
}

/// A choice among a few values, stored by `set`.
private struct SettingChoice<V: Hashable>: View {
    let title: String
    var subtitle: String?
    let options: [(V, String)]
    @State var value: V
    let set: (V) -> Void

    var body: some View {
        Picker(selection: Binding(get: { value }, set: { value = $0; set($0) })) {
            ForEach(options, id: \.0) { Text($0.1).tag($0.0) }
        } label: {
            TitledLabel(title: title, subtitle: subtitle)
        }
    }
}

// MARK: Servers

private struct ServersSettings: View {
    @EnvironmentObject var model: AppModel
    @State private var together = true
    @State private var removing: Account?
    @State private var switching: Account?
    @ObservedObject private var identity = Identity.shared

    var body: some View {
        let accounts = model.accounts
        let many = accounts.count > 1
        let preferred = (try? model.core.getSetting(key: preferredKey)) ?? nil
        // Together: the preferred account is also the one in use.
        let main = together && many ? (accounts.first { $0.id == preferred }?.id ?? model.account) : model.account
        Section {
            ForEach(accounts, id: \.id) { a in
                Button {
                    guard a.id != main else { return }
                    try? model.core.setSetting(key: preferredKey, value: a.id)
                    model.use(a.id)
                } label: {
                    HStack(spacing: 12) {
                        ServerMark(kind: a.kind == .plex ? "plex" : "jellyfin", size: 34)
                        TitledLabel(title: tr("{name} on {server}", ["name": a.userName, "server": a.serverName]), subtitle: kindName(a.kind))
                        Spacer()
                        if a.id == main {
                            Text(together && many ? tr("Preferred") : tr("Showing now")).font(.footnote).foregroundStyle(Palette.accent)
                        }
                        if a.kind == .plex {
                            Button { switching = a } label: {
                                Image(systemName: "person.crop.circle").font(.title3).foregroundStyle(Palette.muted)
                            }
                            .buttonStyle(.borderless)
                            .accessibilityLabel(tr("Switch Plex user"))
                        }
                    }
                }
                .swipeActions {
                    Button(tr("Sign out"), role: .destructive) { removing = a }
                }
                .contextMenu {
                    if a.kind == .plex {
                        Button { switching = a } label: { Label(tr("Switch Plex user"), systemImage: "person.crop.circle") }
                    }
                    Button(tr("Sign out of {server}", ["server": a.serverName]), role: .destructive) { removing = a }
                }
            }
            // Logins linked to the AlwyWatch account but not signed in on this device: a way to.
            ForEach((identity.links ?? []).filter { l in !accounts.contains { $0.serverId == l.serverId } }, id: \.userKey) { l in
                NavigationLink(value: Route.addServer) {
                    HStack(spacing: 12) {
                        Image(systemName: "plus").foregroundStyle(Palette.accent).frame(width: 34)
                        TitledLabel(title: tr("{name} on {server}", ["name": l.mediaName, "server": l.serverName]), subtitle: tr("Linked to your account, not signed in on this device"))
                        Spacer()
                        Text(tr("Sign in")).font(.footnote).foregroundStyle(Palette.muted)
                    }
                }
            }
            NavigationLink(value: Route.addServer) {
                Label(tr("Add a server"), systemImage: "plus")
            }
        } header: {
            Text(tr("Your servers"))
        } footer: {
            if many, model.core.keepConnection() != nil { Text(tr("AlwyWatch uses the login showing now.")) }
        }
        .onAppear { together = ((try? model.core.getSetting(key: combinedKey)) ?? nil) != "0" }
        .task { if model.core.keepActive() { await identity.refreshLinks(model.core) } }
        .sheet(item: $switching) { PlexSwitchSheet(account: $0).environmentObject(model) }
        .confirmationDialog(
            tr("Sign out of {server}?", ["server": removing?.serverName ?? ""]),
            isPresented: Binding(get: { removing != nil }, set: { if !$0 { removing = nil } }),
            titleVisibility: .visible
        ) {
            Button(tr("Sign out"), role: .destructive) {
                if let a = removing { model.signOut(a.id) }
                removing = nil
            }
        } message: {
            if let a = removing {
                Text(tr("{name} is signed out of {server} on this device. Its downloads stay until you delete them.", ["name": a.userName, "server": a.serverName]))
            }
        }
        if many {
            Section {
                Toggle(isOn: Binding(get: { together }, set: { v in
                    together = v
                    try? model.core.setSetting(key: combinedKey, value: v ? "1" : "0")
                    model.reload()
                })) {
                    TitledLabel(
                        title: tr("Show all servers together"),
                        subtitle: together ? tr("Tap a login above to make it the preferred one") : tr("One server at a time: tap a login above to switch")
                    )
                }
                .tint(Palette.accent)
            } header: {
                Text(tr("Home"))
            } footer: {
                Text(tr("Together, the home mixes every server; a title on several is shown once, from the preferred one."))
            }
            SyncSection(jobs: model.sync)
            Section {
                NavigationLink(value: Route.historyCopy) {
                    TitledLabel(title: tr("Copy watch history"), subtitle: tr("What you watched and where you stopped, from one server to another (Plex to Jellyfin)"))
                }
            } header: {
                Text(tr("Moving between servers"))
            } footer: {
                Text(tr("A one-time copy, one way. Each person copies their own history, with their own logins."))
            }
        }
    }
}

// MARK: Account, requests

// MARK: Playback, audio

private struct PlaybackSettings: View {
    @EnvironmentObject var model: AppModel
    let settings: PlayerSettings

    var body: some View {
        Section(tr("Intros and credits")) {
            SettingToggle(title: tr("Skip intros automatically"), subtitle: tr("Otherwise a Skip intro button appears."),
                          value: settings.autoSkipIntro) { settings.autoSkipIntro = $0 }
            SettingToggle(title: tr("Play the next episode automatically"),
                          subtitle: tr("A countdown during the credits, which Back cancels. Off: playback stops at the end of the episode."),
                          value: settings.autoPlayNext) { settings.autoPlayNext = $0 }
            SettingChoice(title: tr("Next episode countdown"), options: [5, 10, 15, 30].map { ($0, seconds($0)) },
                          value: settings.countdownS) { settings.countdownS = $0 }
        }
        Section(tr("Remote and resuming")) {
            SettingChoice(title: tr("Skip back"), options: [5, 10, 15, 30].map { ($0, seconds($0)) },
                          value: settings.seekBackS) { settings.seekBackS = $0 }
            SettingChoice(title: tr("Skip forward"), options: [(10, seconds(10)), (15, seconds(15)), (30, seconds(30)), (60, trn("{n} minute", "{n} minutes", 1))],
                          value: settings.seekForwardS) { settings.seekForwardS = $0 }
            SettingChoice(title: tr("Rewind when resuming"), options: [(0, tr("Off")), (5, seconds(5)), (10, seconds(10)), (30, seconds(30))],
                          value: settings.resumeRewindS) { settings.resumeRewindS = $0 }
            SettingChoice(title: tr("Are you still watching?"), subtitle: tr("Pauses when the remote has not been touched for that long."),
                          options: [(0, tr("Never"))] + [2, 3, 4].map { ($0, trn("After {n} hour", "After {n} hours", $0)) },
                          value: settings.stillWatchingH) { settings.stillWatchingH = $0 }
        }
        if model.core.keepActive() { TogetherRightsSettings() }
    }
}

/// In watch together rooms I create, what the others may do.
private struct TogetherRightsSettings: View {
    @EnvironmentObject var model: AppModel
    @State private var rights: TogetherRights?

    var body: some View {
        Section {
            if let r = rights {
                toggle(tr("Others can change what's playing"), tr("Next episode, another title."), r.changeTitle) { var n = r; n.changeTitle = $0; return n }
                toggle(tr("Others can pause and play"), nil, r.playPause) { var n = r; n.playPause = $0; return n }
                toggle(tr("Others can move forward or back"), nil, r.seek) { var n = r; n.seek = $0; return n }
            }
        } header: {
            Text(tr("Watch together"))
        } footer: {
            Text(tr("In rooms you create, what the others may do. You can always do everything."))
        }
        .onAppear { rights = model.core.togetherRights() }
    }

    private func toggle(_ title: String, _ subtitle: String?, _ on: Bool, _ change: @escaping (Bool) -> TogetherRights) -> some View {
        Toggle(isOn: Binding(get: { on }, set: { v in
            let n = change(v)
            rights = n
            try? model.core.setTogetherRights(rights: n)
        })) {
            TitledLabel(title: title, subtitle: subtitle)
        }
        .tint(Palette.accent)
    }
}

private struct AudioSettings: View {
    let settings: PlayerSettings
    private let codes = ["eng", "fra", "nld", "deu", "spa", "ita", "por", "jpn", "kor", "zho", "ara", "rus", "pol", "swe", "tur", "hin"]

    var body: some View {
        let languages = codes.map { ($0, languageName($0) ?? $0) }
        let device = languageName(Locale.current.language.languageCode?.identifier) ?? "?"
        Section {
            SettingChoice(title: tr("Audio language"), options: [("", tr("Original (the file's default)"))] + languages,
                          value: settings.audioLanguage) { settings.audioLanguage = $0 }
            SettingChoice(title: tr("Subtitles"), subtitle: tr("Automatic: when the audio is not in your subtitle language."),
                          options: [("auto", tr("Automatic")), ("forced", tr("Only foreign-language parts")), ("always", tr("Always")), ("off", tr("Off"))],
                          value: settings.subtitleMode) { settings.subtitleMode = $0 }
            SettingChoice(title: tr("Subtitle language"), options: [("", tr("Device language ({language})", ["language": device]))] + languages,
                          value: settings.subtitleLanguage) { settings.subtitleLanguage = $0 }
        } header: {
            Text(tr("Languages"))
        } footer: {
            Text(tr("A movie or show remembers the tracks picked for it (on its page or while watching), and its next episodes follow."))
        }
        Section(tr("Subtitle look")) {
            SettingChoice(title: tr("Size"), options: ["small", "normal", "large", "huge"].map { ($0, sizeName($0)) },
                          value: settings.subtitleSize) { settings.subtitleSize = $0 }
            SettingChoice(title: tr("Colour"), options: [("white", tr("White")), ("yellow", tr("Yellow"))],
                          value: settings.subtitleColor) { settings.subtitleColor = $0 }
            SettingToggle(title: tr("Background box"), subtitle: tr("Otherwise a dark outline keeps them readable."),
                          value: settings.subtitleBackground) { settings.subtitleBackground = $0 }
            SettingToggle(title: tr("Raise subtitles"), subtitle: tr("Higher on the screen (TVs that crop the edges)."),
                          value: settings.subtitleRaised) { settings.subtitleRaised = $0 }
        }
    }
}

// MARK: This device, admin

private struct DeviceSettings: View {
    @EnvironmentObject var model: AppModel
    @State private var language = I18n.choice
    @State private var cacheBytes: UInt64 = 0

    var body: some View {
        Section(tr("Language")) {
            Picker(tr("Language"), selection: $language) {
                Text(tr("Same as the device")).tag("")
                ForEach(i18nLanguages(), id: \.code) { l in Text(l.name).tag(l.code) }
            }
            .onChange(of: language) { code in
                I18n.choose(code)
                model.reload()
            }
        }
        Section {
            HStack {
                Text(tr("Image cache"))
                Spacer()
                Text(tr("{size} MB", ["size": cacheBytes / (1024 * 1024)])).foregroundStyle(Palette.muted)
                Button(tr("Clear")) {
                    try? model.core.clearImageCache()
                    cacheBytes = model.core.imageCacheBytes()
                }
                .buttonStyle(.borderless)
            }
        } header: {
            Text(tr("Storage"))
        } footer: {
            Text(tr("Posters and backdrops kept to show pages instantly; they come back when needed."))
        }
        .onAppear { cacheBytes = model.core.imageCacheBytes() }
        ProfilesGroup()
        OtherDevicesGroup()
        Section {
            let version = Bundle.main.object(forInfoDictionaryKey: "CFBundleShortVersionString") as? String ?? ""
            TitledLabel(title: "AlwyWatch \(version)", subtitle: tr("Core {version}", ["version": coreVersion()]))
        } header: {
            Text(tr("About"))
        } footer: {
            Text(tr("Free software under the GNU AGPL v3 or later."))
        }
    }
}

