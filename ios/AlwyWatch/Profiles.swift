// SPDX-License-Identifier: AGPL-3.0-or-later
import CommonCrypto
import Foundation
import Security
import SwiftUI

/// A person on this device: their own accounts, AlwyWatch server, Seerr and settings.
struct Profile: Identifiable, Equatable {
    let id: String
    let name: String
    let hasPin: Bool
    /// No name of its own yet: it takes its account's (a new one shows as "New profile").
    let unnamed: Bool
}

/// The device's profiles (Android's ProfileStore). "default" is the profile that existed before
/// profiles (its data stays where it was). A PIN is stored as a salted PBKDF2 hash: it keeps
/// people out of each other's profile on a shared device; tokens stay in the Keychain.
enum ProfileStore {
    static let defaultId = "default"
    /// The first profile's name until it takes its account's (or is renamed).
    static let unnamed = "Me"
    private static let listKey = "profiles.list"
    private static let currentKey = "profiles.current"
    private static let autoKey = "profiles.auto_sign_in"
    private static var d: UserDefaults { .standard }

    private static func raw() -> [[String: Any]] {
        (d.array(forKey: listKey) as? [[String: Any]]) ?? [["id": defaultId, "name": unnamed]]
    }

    private static func save(_ a: [[String: Any]]) { d.set(a, forKey: listKey) }

    private static func edit(_ id: String, _ change: (inout [String: Any]) -> Void) {
        save(raw().map { o in
            guard o["id"] as? String == id else { return o }
            var n = o
            change(&n)
            return n
        })
    }

    static func list() -> [Profile] {
        raw().compactMap { o in
            guard let id = o["id"] as? String, let stored = o["name"] as? String else { return nil }
            let isUnnamed = stored == unnamed
            // Only the first profile is "Me": an added one is "New profile" until it has a login.
            let name = isUnnamed && id != defaultId ? tr("New profile") : stored
            return Profile(id: id, name: name, hasPin: o["pin"] != nil, unnamed: isUnnamed)
        }
    }

    /// The profile the app opens (the last one used).
    static var current: String {
        get {
            let c = d.string(forKey: currentKey)
            return list().contains { $0.id == c } ? c! : defaultId
        }
        set { d.set(newValue, forKey: currentKey) }
    }

    /// A new, empty profile (one person): it opens on the sign-in and takes its name from the
    /// first account there.
    static func add() -> String {
        let id = String(UUID().uuidString.lowercased().prefix(8))
        save(raw() + [["id": id, "name": unnamed]])
        return id
    }

    static func rename(_ id: String, _ name: String) {
        let n = name.trimmingCharacters(in: .whitespaces)
        edit(id) { $0["name"] = n.isEmpty ? "Profile" : String(n.prefix(24)) }
    }

    static func remove(_ id: String) {
        guard id != defaultId else { return }
        save(raw().filter { $0["id"] as? String != id })
    }

    /// Sets (or with nil, removes) a profile's PIN.
    static func setPin(_ id: String, _ pin: String?) {
        edit(id) { o in
            guard let pin else { o.removeValue(forKey: "pin"); return }
            var salt = Data(count: 16)
            _ = salt.withUnsafeMutableBytes { SecRandomCopyBytes(kSecRandomDefault, 16, $0.baseAddress!) }
            o["pin"] = ["salt": salt.base64EncodedString(), "hash": hash(pin, salt).base64EncodedString()]
        }
    }

    static func checkPin(_ id: String, _ pin: String) -> Bool {
        guard let o = raw().first(where: { $0["id"] as? String == id }) else { return false }
        guard let p = o["pin"] as? [String: String] else { return true }
        guard let s = p["salt"].flatMap({ Data(base64Encoded: $0) }), let h = p["hash"].flatMap({ Data(base64Encoded: $0) }) else { return false }
        let got = hash(pin, s)
        // Constant time.
        return got.count == h.count && zip(got, h).reduce(0) { $0 | ($1.0 ^ $1.1) } == 0
    }

    private static func hash(_ pin: String, _ salt: Data) -> Data {
        var out = [UInt8](repeating: 0, count: 32)
        let pw = Array(pin.utf8).map { CChar(bitPattern: $0) }
        _ = salt.withUnsafeBytes { s in
            CCKeyDerivationPBKDF(
                CCPBKDFAlgorithm(kCCPBKDF2), pw, pw.count,
                s.bindMemory(to: UInt8.self).baseAddress, salt.count,
                CCPseudoRandomAlgorithm(kCCPRFHmacAlgSHA256), 100_000, &out, out.count
            )
        }
        return Data(out)
    }

    /// Several profiles: open the last one used at launch instead of asking who's watching.
    static var autoSignIn: Bool {
        get { d.bool(forKey: autoKey) }
        set { d.set(newValue, forKey: autoKey) }
    }

    /// Where a profile keeps its data ("default": where it always was).
    static func dataDir(_ id: String) -> URL {
        let support = FileManager.default.urls(for: .applicationSupportDirectory, in: .userDomainMask)[0]
        return id == defaultId ? support : support.appendingPathComponent("profiles/\(id)")
    }

    /// Added profiles left before any login (nothing in the Keychain): removed with their few
    /// files, so starting to add a profile and going back leaves nothing behind. Not `keep`.
    static func pruneEmpty(keep: String) {
        for p in list() where p.id != defaultId && p.id != keep && p.unnamed {
            if !Keychain.keys(profile: p.id).isEmpty { continue }
            try? FileManager.default.removeItem(at: dataDir(p.id))
            remove(p.id)
        }
    }

    /// The profile's name the first time it has an account: the AlwyWatch account's, else
    /// the media account's.
    @MainActor
    static func adoptName(_ model: AppModel) {
        guard let p = list().first(where: { $0.id == model.profile }), p.unnamed else { return }
        let name = (model.core.account()?.name).flatMap { $0.trimmingCharacters(in: .whitespaces).isEmpty ? nil : $0 }
            ?? model.current?.userName
        guard let name, !name.isEmpty else { return }
        rename(p.id, name)
    }
}

// ----- Switching -----------------------------------------------------------------------------

/// The app for the profile in use: its core (one per profile) and whether "Who's watching?" was
/// answered. Switching builds the new profile's core and redraws every screen (Android restarts
/// the app; iOS apps can't).
@MainActor
final class ProfileHost: ObservableObject {
    @Published private(set) var model: AppModel
    /// The profile was picked (and its PIN checked): the app shows.
    @Published var open: Bool

    init() {
        let current = ProfileStore.current
        ProfileStore.pruneEmpty(keep: current)
        model = AppModel(profile: current)
        let profiles = ProfileStore.list()
        let p = profiles.first { $0.id == current }
        // Automatic sign-in: straight into the last profile (its PIN is still asked).
        let auto = ProfileStore.autoSignIn && p?.hasPin != true
        open = auto || (profiles.count < 2 && p?.hasPin != true)
    }

    /// Into another profile (its PIN already checked).
    func switchTo(_ id: String) {
        if id == model.profile {
            open = true
            return
        }
        model.stop()
        TogetherSession.shared.reset()
        TogetherInbox.shared.reset()
        MyAccount.shared.reset()
        Identity.shared.reset()
        ProfileStore.current = id
        model = AppModel(profile: id)
        open = true
        model.foreground()
    }

    /// A new profile, opened on the sign-in.
    func addProfile() { switchTo(ProfileStore.add()) }
}

// ----- Views ---------------------------------------------------------------------------------

/// Java's String.hashCode, so a name has the same colour as on Android.
private func javaHash(_ s: String) -> Int32 {
    var h: Int32 = 0
    for u in s.utf16 { h = h &* 31 &+ Int32(u) }
    return h
}

/// A profile's round initial on a gradient derived from its name.
struct ProfileAvatar: View {
    let name: String
    var size: CGFloat = 96

    private static let palette: [[Color]] = [
        [Color(red: 1, green: 0xB2 / 255, blue: 0x24 / 255), Color(red: 1, green: 0x6A / 255, blue: 0x3D / 255)],
        [Color(red: 0x5B / 255, green: 0x8C / 255, blue: 1), Color(red: 0x9B / 255, green: 0x5B / 255, blue: 1)],
        [Color(red: 0x3D / 255, green: 0xDC / 255, blue: 0x97 / 255), Color(red: 0x14 / 255, green: 0xA3 / 255, blue: 0xC7 / 255)],
        [Color(red: 1, green: 0x5A / 255, blue: 0x8A / 255), Color(red: 1, green: 0x9F / 255, blue: 0x5A / 255)],
    ]

    var body: some View {
        let colors = Self.palette[Int(javaHash(name) & 0x7FFF_FFFF) % Self.palette.count]
        Text(name.first.map { String($0).uppercased() } ?? "?")
            .font(.system(size: size < 48 ? size * 0.5 : size * 0.36, weight: .bold))
            .foregroundStyle(Color.black)
            .frame(width: size, height: size)
            .background(LinearGradient(colors: colors, startPoint: .topLeading, endPoint: .bottomTrailing), in: Circle())
    }
}

/// A profile's PIN, checked when the fourth digit is typed.
struct PinEntry: View {
    let profile: Profile
    var onBack: (() -> Void)?
    let onOk: () -> Void
    @State private var pin = ""
    @State private var wrong = 0
    @FocusState private var focused: Bool

    var body: some View {
        VStack(spacing: 18) {
            ProfileAvatar(name: profile.name)
            Text(tr("PIN for {name}", ["name": profile.name])).font(.title2.weight(.semibold))
            SecureField("••••", text: $pin)
                .keyboardType(.numberPad)
                .multilineTextAlignment(.center)
                .font(.title2)
                .frame(width: 200)
                .padding(.vertical, 12)
                .background(Palette.surface, in: Capsule())
                .focused($focused)
                .onChange(of: pin) { v in
                    let digits = String(v.filter(\.isNumber).prefix(4))
                    if digits != v { pin = digits; return }
                    if digits.count == 4 { check() }
                }
            if wrong > 0 { Text(tr("Wrong PIN")).foregroundStyle(Palette.danger) }
            if let onBack { Button(tr("Back"), action: onBack).buttonStyle(ActionButtonStyle()) }
        }
        .onAppear { focused = true }
    }

    private func check() {
        if ProfileStore.checkPin(profile.id, pin) {
            onOk()
        } else {
            wrong += 1
            pin = ""
        }
    }
}

private struct ProfileTile: View {
    let name: String
    let note: String
    var size: CGFloat = 96
    var add = false
    let action: () -> Void

    var body: some View {
        Button(action: action) {
            VStack(spacing: 8) {
                if add {
                    Image(systemName: "plus").font(.title).foregroundStyle(.white)
                        .frame(width: size, height: size)
                        .background(Palette.surfaceHigh, in: Circle())
                } else {
                    ProfileAvatar(name: name, size: size)
                }
                Text(name).font(.subheadline.weight(.semibold)).lineLimit(1)
                Text(note.isEmpty ? " " : note).font(.caption2).foregroundStyle(Palette.muted).lineLimit(1)
            }
            .frame(width: size + 24)
        }
        .buttonStyle(.plain)
    }
}

/// Before the app: "Who's watching?" when the device has several profiles, or the PIN of the
/// only (or the automatically opened) profile when it has one.
struct WhoIsWatchingView: View {
    @EnvironmentObject var host: ProfileHost
    @State private var asking: Profile?
    private let profiles = ProfileStore.list()

    var body: some View {
        ZStack {
            Palette.background.ignoresSafeArea()
            RadialGradient(colors: [Palette.accent.opacity(0.25), .clear], center: .top, startRadius: 0, endRadius: 500).ignoresSafeArea()
            ScrollView {
                VStack(spacing: 28) {
                    if let a = asking {
                        PinEntry(profile: a, onBack: profiles.count > 1 ? { asking = nil } : nil) { host.switchTo(a.id) }
                    } else {
                        Text(tr("Who's watching?")).font(.largeTitle.weight(.bold))
                        FlowLayout(spacing: 20) {
                            ForEach(profiles) { p in
                                ProfileTile(name: p.name, note: p.hasPin ? tr("PIN") : "") {
                                    if p.hasPin { asking = p } else { host.switchTo(p.id) }
                                }
                            }
                        }
                    }
                }
                .padding(24)
                .frame(maxWidth: .infinity, minHeight: 500)
            }
        }
        .onAppear {
            let current = profiles.first { $0.id == host.model.profile }
            // One profile, or automatic sign-in into a PIN-protected one: just its PIN.
            if profiles.count == 1 {
                asking = profiles.first
            } else if ProfileStore.autoSignIn, current?.hasPin == true {
                asking = current
            }
        }
    }
}

/// "Who's watching?" from the home's avatar: one tap on a profile switches to it (its PIN is
/// asked here), with a way to add one and to the settings.
struct ProfileSwitcherSheet: View {
    @EnvironmentObject var host: ProfileHost
    @Environment(\.dismiss) private var dismiss
    let onSettings: () -> Void
    @State private var asking: Profile?
    private let profiles = ProfileStore.list()

    var body: some View {
        NavigationStack {
            ScrollView {
                if let a = asking {
                    PinEntry(profile: a) { host.switchTo(a.id) }.padding(24)
                } else {
                    FlowLayout(spacing: 12) {
                        ForEach(profiles) { p in
                            let current = p.id == host.model.profile
                            ProfileTile(name: p.name, note: current ? tr("Current") : p.hasPin ? tr("PIN") : "", size: 64) {
                                if current { dismiss() } else if p.hasPin { asking = p } else { host.switchTo(p.id) }
                            }
                        }
                        ProfileTile(name: tr("Add"), note: "", size: 64, add: true) { host.addProfile() }
                    }
                    .padding(20)
                }
            }
            .background(Palette.background.ignoresSafeArea())
            .navigationTitle(asking.map { tr("Switch to {name}", ["name": $0.name]) } ?? tr("Who's watching?"))
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) {
                    if asking != nil {
                        Button(tr("Back")) { asking = nil }
                    } else {
                        Button(tr("Close")) { dismiss() }
                    }
                }
                ToolbarItem(placement: .confirmationAction) {
                    if asking == nil { Button { onSettings() } label: { Label(tr("Settings"), systemImage: "gearshape") } }
                }
            }
        }
        .presentationDetents([.medium, .large])
    }
}

/// Settings → This device: the device's profiles (switch, add, rename, PIN, remove).
struct ProfilesGroup: View {
    @EnvironmentObject var host: ProfileHost
    @State private var version = 0
    @State private var renaming: Profile?
    @State private var pinFor: Profile?
    @State private var switching: Profile?
    @State private var removing: Profile?
    @State private var auto = ProfileStore.autoSignIn

    var body: some View {
        let _ = version
        let profiles = ProfileStore.list()
        let current = host.model.profile
        Section {
            ForEach(profiles) { p in
                Button {
                    if p.id == current { renaming = p } else if p.hasPin { switching = p } else { host.switchTo(p.id) }
                } label: {
                    HStack(spacing: 12) {
                        ProfileAvatar(name: p.name, size: 34)
                        VStack(alignment: .leading, spacing: 2) {
                            Text(p.name).foregroundStyle(Palette.onSurface)
                            if p.hasPin { Text(tr("Protected by a PIN")).font(.footnote).foregroundStyle(Palette.muted) }
                        }
                        Spacer()
                        if p.id == current {
                            Text(tr("In use")).font(.footnote).foregroundStyle(Palette.muted)
                            Button { pinFor = p } label: { Image(systemName: "lock").foregroundStyle(Palette.muted) }
                                .buttonStyle(.borderless)
                                .accessibilityLabel(tr("PIN"))
                        } else {
                            Button { removing = p } label: { Image(systemName: "trash").foregroundStyle(Palette.muted) }
                                .buttonStyle(.borderless)
                                .accessibilityLabel(tr("Remove {name}", ["name": p.name]))
                        }
                    }
                }
            }
            Button { host.addProfile() } label: {
                TitledRow(title: tr("Add a profile"), subtitle: tr("Someone else on this device"), icon: "plus")
            }
            if profiles.count > 1 {
                Toggle(isOn: Binding(get: { auto }, set: { auto = $0; ProfileStore.autoSignIn = $0 })) {
                    VStack(alignment: .leading, spacing: 2) {
                        Text(tr("Skip “Who's watching?” at start")).foregroundStyle(Palette.onSurface)
                        Text(tr("Open the last profile used instead of asking who's watching (a PIN is still asked)")).font(.footnote).foregroundStyle(Palette.muted)
                    }
                }
                .tint(Palette.accent)
            }
        } header: {
            Text(tr("Profiles"))
        } footer: {
            Text(tr("A profile is one person: their own AlwyWatch account or logins, history and settings. Downloads are shared."))
        }
        .listRowBackground(Palette.surface)
        .onAppear {
            ProfileStore.adoptName(host.model)
            version += 1
        }
        .sheet(item: $renaming) { p in
            RenameProfileSheet(profile: p) { version += 1 }
        }
        .sheet(item: $pinFor) { p in
            ProfilePinSheet(profile: p) { version += 1 }
        }
        .sheet(item: $switching) { p in
            NavigationStack {
                PinEntry(profile: p) { host.switchTo(p.id) }
                    .padding(24)
                    .frame(maxWidth: .infinity, maxHeight: .infinity)
                    .background(Palette.background.ignoresSafeArea())
                    .navigationTitle(tr("Switch to {name}", ["name": p.name]))
                    .navigationBarTitleDisplayMode(.inline)
                    .toolbar { ToolbarItem(placement: .cancellationAction) { Button(tr("Cancel")) { switching = nil } } }
            }
            .presentationDetents([.medium])
        }
        .confirmationDialog(
            removing.map { tr("Remove {name}?", ["name": $0.name]) } ?? "",
            isPresented: Binding(get: { removing != nil }, set: { if !$0 { removing = nil } }),
            titleVisibility: .visible,
            presenting: removing
        ) { p in
            Button(tr("Remove"), role: .destructive) { remove(p) }
            Button(tr("Cancel"), role: .cancel) {}
        } message: { _ in
            Text(tr("Their servers, history and settings on this device are deleted. Downloads only they wanted are deleted too."))
        }
    }

    private func remove(_ p: Profile) {
        try? host.model.core.releaseProfileDownloads(profile: p.id)
        try? FileManager.default.removeItem(at: ProfileStore.dataDir(p.id))
        Keychain.removeAll(profile: p.id)
        ProfileStore.remove(p.id)
        removing = nil
        version += 1
    }
}

private struct RenameProfileSheet: View {
    @Environment(\.dismiss) private var dismiss
    let profile: Profile
    let onSaved: () -> Void
    @State private var name = ""

    var body: some View {
        NavigationStack {
            Form {
                TextField(tr("Name"), text: $name)
                    .onChange(of: name) { v in if v.count > 24 { name = String(v.prefix(24)) } }
                    .listRowBackground(Palette.surface)
            }
            .scrollContentBackground(.hidden)
            .background(Palette.background.ignoresSafeArea())
            .navigationTitle(tr("Rename"))
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) { Button(tr("Cancel")) { dismiss() } }
                ToolbarItem(placement: .confirmationAction) {
                    Button(tr("Save")) {
                        ProfileStore.rename(profile.id, name)
                        onSaved()
                        dismiss()
                    }
                }
            }
        }
        .presentationDetents([.medium])
        .onAppear { name = profile.name }
    }
}

private struct ProfilePinSheet: View {
    @Environment(\.dismiss) private var dismiss
    let profile: Profile
    let onSaved: () -> Void
    @State private var pin = ""

    var body: some View {
        NavigationStack {
            Form {
                Section {
                    SecureField(profile.hasPin ? tr("New 4-digit PIN") : tr("4-digit PIN"), text: $pin)
                        .keyboardType(.numberPad)
                        .onChange(of: pin) { v in
                            let digits = String(v.filter(\.isNumber).prefix(4))
                            if digits != v { pin = digits }
                        }
                } footer: {
                    Text(tr("Asked when opening this profile."))
                }
                .listRowBackground(Palette.surface)
                if profile.hasPin {
                    Button(tr("Remove PIN"), role: .destructive) {
                        ProfileStore.setPin(profile.id, nil)
                        onSaved()
                        dismiss()
                    }
                    .listRowBackground(Palette.surface)
                }
            }
            .scrollContentBackground(.hidden)
            .background(Palette.background.ignoresSafeArea())
            .navigationTitle(profile.hasPin ? tr("PIN for {name}", ["name": profile.name]) : tr("Protect {name} with a PIN", ["name": profile.name]))
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) { Button(tr("Cancel")) { dismiss() } }
                ToolbarItem(placement: .confirmationAction) {
                    Button(tr("Save")) {
                        ProfileStore.setPin(profile.id, pin)
                        onSaved()
                        dismiss()
                    }
                    .disabled(pin.count != 4)
                }
            }
        }
        .presentationDetents([.medium])
    }
}
