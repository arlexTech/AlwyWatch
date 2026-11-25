// SPDX-License-Identifier: AGPL-3.0-or-later
import AVFoundation
import PhotosUI
import SwiftUI
import UIKit

extension Account: Identifiable {}

/// My AlwyWatch account as the server last said (name, avatar, welcome step done).
@MainActor
final class MyAccount: ObservableObject {
    static let shared = MyAccount()
    @Published var me: Me?
    /// The welcome step was closed during this run (it comes back at the next start if unfinished).
    @Published var welcomeClosed = false

    func refresh(_ core: AlwyWatch) async {
        guard core.keepActive() else { me = nil; return }
        if let m = try? await core.keepMe() { me = m }
    }

    /// A new account was just linked: show the welcome step again if it isn't done.
    func linked(_ core: AlwyWatch) async {
        welcomeClosed = false
        await refresh(core)
    }

    /// The welcome step is on screen (other prompts wait).
    var welcomeShowing: Bool { me.map { !$0.profileSet && !welcomeClosed } ?? false }
}

/// The status in a failed call's message ("… HTTP 409 …").
private func httpStatus(_ error: Error) -> Int? {
    let text = message(error)
    guard let r = text.range(of: "HTTP ") else { return nil }
    return Int(String(text[r.upperBound...].prefix { $0.isNumber }))
}

private func capitalizedFirst(_ s: String) -> String { s.prefix(1).uppercased() + s.dropFirst() }

// ----- Linked logins (docs/identity-apps.md) ------------------------------------------------

/// One AlwyWatch account for a person's sign-ins: the sign-ins of my account, the device's
/// accounts that could join it, and the one-time "Are these both you?" offer.
@MainActor
final class Identity: ObservableObject {
    static let shared = Identity()
    /// The sign-ins of my AlwyWatch account as the server last said (nil: not read yet).
    @Published var links: [LinkedIdentity]?
    /// The device's accounts that can join it (read with the links).
    @Published var linkableNow: [Account] = []
    /// The device account offered by "Are these both you?" (shown once per run at most).
    @Published var offer: Account?
    private var looked = false
    private var served: (base: String, ids: Set<String>)?

    func refreshLinks(_ core: AlwyWatch) async {
        guard core.keepActive() else { links = nil; return }
        if let l = try? await core.keepLinks() { links = l }
    }

    /// The links and the linkable accounts again (Settings).
    func reload(_ core: AlwyWatch) async {
        await refreshLinks(core)
        linkableNow = await linkable(core)
    }

    private func servedIds(_ core: AlwyWatch) async -> Set<String>? {
        guard let base = core.keepConnection()?.baseUrl else { return nil }
        if let s = served, s.base == base { return s.ids }
        guard let list = try? await core.keepServers(url: base) else { return nil }
        let ids = Set(list.map(\.id))
        served = (base, ids)
        return ids
    }

    /// The device's other accounts that can join my AlwyWatch account: on a server it serves,
    /// not the synced account's server (one sign-in per server), not linked yet, with a token.
    func linkable(_ core: AlwyWatch) async -> [Account] {
        guard let keep = core.keepConnection() else { return [] }
        let accounts = (try? core.accounts()) ?? []
        guard accounts.count >= 2, let mine = accounts.first(where: { $0.id == keep.accountId }),
              let ids = await servedIds(core) else { return [] }
        if links == nil { await refreshLinks(core) }
        guard let linked = links.map({ Set($0.map(\.serverId)) }) else { return [] }
        return accounts.filter {
            $0.id != mine.id && $0.serverId != mine.serverId && ids.contains($0.serverId)
                && !linked.contains($0.serverId) && AppModel.keepToken($0) != nil
        }
    }

    /// Makes `account` part of my AlwyWatch account, then refreshes what depends on who I am.
    func link(_ core: AlwyWatch, _ account: Account) async throws {
        guard let token = AppModel.keepToken(account) else {
            throw CoreError.Unauthorized(message: "no token for \(account.serverName)")
        }
        links = try await core.keepLink(accountId: account.id, token: token)
        await afterChange(core)
    }

    /// Takes a sign-in out of my account; it becomes a new, empty one.
    func unlink(_ core: AlwyWatch, _ identity: LinkedIdentity) async throws {
        links = try await core.keepUnlink(userKey: identity.userKey)
        // This device's own sign-in went: it is a fresh account now (the welcome step shows).
        if identity.current { await MyAccount.shared.linked(core) } else { await afterChange(core) }
        linkableNow = await linkable(core)
    }

    /// After a link or an unlink: who I am, the links, rooms and invitations, leaving soon.
    private func afterChange(_ core: AlwyWatch) async {
        await MyAccount.shared.refresh(core)
        await refreshLinks(core)
        await TogetherInbox.shared.refresh(core)
        _ = try? await core.refreshLeavingSoon(force: true)
    }

    // "Not now", remembered on this device per pair.
    private static let declinedKey = "identity.declined"

    func declined(_ myKey: String, _ account: Account) -> Bool {
        (UserDefaults.standard.stringArray(forKey: Self.declinedKey) ?? []).contains("\(myKey)|\(account.id)")
    }

    func decline(_ myKey: String, _ account: Account) {
        var set = UserDefaults.standard.stringArray(forKey: Self.declinedKey) ?? []
        set.append("\(myKey)|\(account.id)")
        UserDefaults.standard.set(set, forKey: Self.declinedKey)
    }

    /// An account was signed out: once signed in again, it may be offered again.
    func forgetAccount(_ accountId: String) {
        let set = (UserDefaults.standard.stringArray(forKey: Self.declinedKey) ?? []).filter { !$0.hasSuffix("|\(accountId)") }
        UserDefaults.standard.set(set, forKey: Self.declinedKey)
        links = nil
        looked = false
    }

    /// Looks once per run for a pair to offer.
    func lookForOffer(_ core: AlwyWatch) async {
        if looked { return }
        // Most devices have one account: nothing to look up, no network.
        guard ((try? core.accounts())?.count ?? 0) >= 2, core.keepConnection() != nil else { return }
        looked = true
        guard let me = MyAccount.shared.me else { return }
        offer = await linkable(core).first { !declined(me.userKey, $0) }
    }
}

/// A failed link or unlink in plain words.
func linkProblem(_ error: Error, _ serverName: String?) -> String {
    switch error as? CoreError {
    case .some(.Unauthorized):
        if let serverName {
            return tr("Your {server} sign-in has expired: sign in again on that account, then link.", ["server": serverName])
        }
        return tr("Your other sign-in has expired: sign in again on that account, then link.")
    case .some(.Network): return tr("Can't reach your AlwyWatch server right now.")
    default: break
    }
    switch httpStatus(error) {
    case 409: return tr("These are two different people on the same server: they can't be one AlwyWatch account.")
    case 429: return tr("Too many tries. Try again in a few minutes.")
    case 502: return tr("The media server is unreachable right now. Try again later.")
    default:
        let text = message(error)
        return text.isEmpty ? tr("Something went wrong.") : text
    }
}

private func kindLabel(_ kind: ServerKind) -> String { kind == .plex ? "Plex" : "Jellyfin" }
private func kindLabel(_ kind: ServerType) -> String { kind == .plex ? "Plex" : "Jellyfin" }

/// "Also signed in as …": the other sign-ins of my account, for a second line.
func alsoSignedInAs(_ links: [LinkedIdentity]?) -> String? {
    guard let others = links?.filter({ !$0.current }), !others.isEmpty else { return nil }
    let names = others.map { "\($0.mediaName) (\($0.serverName))" }.joined(separator: ", ")
    return tr("Also signed in as {names}", ["names": names])
}

// ----- Choosing an avatar -------------------------------------------------------------------

enum AvatarPick: Equatable {
    case initial
    case preset(String)
    /// The picture already uploaded.
    case current
    /// A new photo (square JPEG), not sent yet.
    case upload(Data)
}

extension Me {
    var pick: AvatarPick {
        if avatar.customVersion != nil { return .current }
        if let p = avatar.preset, AvatarPresets.of(p) != nil { return .preset(p) }
        return .initial
    }
}

/// The default avatar: the initial of the name on a colour derived from it.
struct InitialAvatar: View {
    let name: String
    var size: CGFloat = 40

    var body: some View {
        ZStack {
            hexColor(avatarDefaultColor(name: name))
            Text(avatarInitial(name: name)).font(.system(size: size * 0.45, weight: .bold)).foregroundStyle(.white)
        }
        .frame(width: size, height: size)
        .clipShape(Circle())
    }
}

/// A photo as the square JPEG the server takes (centre crop, 512 px).
private func squareJpeg(_ data: Data, side: CGFloat = 512) -> Data? {
    guard let img = UIImage(data: data), img.size.width > 0, img.size.height > 0 else { return nil }
    let scale = side / min(img.size.width, img.size.height)
    let w = img.size.width * scale, h = img.size.height * scale
    let format = UIGraphicsImageRendererFormat()
    format.scale = 1
    let out = UIGraphicsImageRenderer(size: CGSize(width: side, height: side), format: format).image { _ in
        img.draw(in: CGRect(x: (side - w) / 2, y: (side - h) / 2, width: w, height: h))
    }
    return out.jpegData(compressionQuality: 0.85)
}

/// The avatars to choose from: the initial, the built-in ones, the uploaded picture if any, a
/// new photo if one was picked, and "Upload a picture".
struct AvatarGrid: View {
    let me: Me
    let name: String
    let pick: AvatarPick
    let onPick: (AvatarPick) -> Void
    /// A photo was chosen (cropped to a square JPEG).
    let onUpload: (Data) -> Void
    var upload: Data?
    private let size: CGFloat = 54
    @State private var photo: PhotosPickerItem?

    var body: some View {
        FlowLayout(spacing: 10) {
            tile(pick == .initial, tr("Your initial"), { onPick(.initial) }) {
                InitialAvatar(name: name.trimmingCharacters(in: .whitespaces).isEmpty ? me.name : name, size: size)
            }
            ForEach(AvatarPresets.all, id: \.id) { p in
                tile(pick == .preset(p.id), tr("Picture {shape}", ["shape": p.shape]), { onPick(.preset(p.id)) }) {
                    PresetAvatar(preset: p)
                }
            }
            if me.avatar.customVersion != nil {
                tile(pick == .current, tr("Your picture"), { onPick(.current) }) {
                    PersonAvatar(avatar: me.avatar, name: me.name, userKey: me.userKey, size: size)
                }
            }
            if let upload, let img = UIImage(data: upload) {
                tile(pick == .upload(upload), tr("New picture"), { onPick(.upload(upload)) }) {
                    Image(uiImage: img).resizable().scaledToFill()
                }
            }
            PhotosPicker(selection: $photo, matching: .images) {
                ring(false) {
                    ZStack {
                        Palette.surface
                        Image(systemName: "plus").foregroundStyle(Palette.muted)
                    }
                }
            }
            .accessibilityLabel(tr("Upload a picture"))
        }
        .onChange(of: photo) { item in
            guard let item else { return }
            Task {
                if let data = try? await item.loadTransferable(type: Data.self), let jpeg = squareJpeg(data) {
                    onUpload(jpeg)
                }
                photo = nil
            }
        }
    }

    private func tile<C: View>(_ selected: Bool, _ label: String, _ action: @escaping () -> Void, @ViewBuilder _ content: () -> C) -> some View {
        Button(action: action) { ring(selected, content) }
            .buttonStyle(.plain)
            .accessibilityLabel(label)
            .accessibilityAddTraits(selected ? .isSelected : [])
    }

    private func ring<C: View>(_ selected: Bool, _ content: () -> C) -> some View {
        content()
            .frame(width: size, height: size)
            .clipShape(Circle())
            .padding(4)
            .overlay(Circle().stroke(selected ? Palette.accent : Color.clear, lineWidth: 2.5))
    }
}

// ----- Settings → Account -------------------------------------------------------------------

/// Settings → Account: with AlwyWatch, how others see you, the AlwyWatch server, password and
/// passkeys, linked logins and signing out; without, "Connect to AlwyWatch".
struct AccountSettings: View {
    @EnvironmentObject var model: AppModel
    @State private var version = 0

    var body: some View {
        let _ = version
        let keep = model.core.keepConnection()
        let account = model.core.account()
        if keep == nil, account == nil {
            Section {
                NavigationLink(value: Route.addServer) {
                    TitledRow(title: tr("Connect to AlwyWatch"), subtitle: tr("No AlwyWatch account · connect to AlwyWatch"), icon: "person.crop.circle.badge.plus")
                }
            }
            .listRowBackground(Palette.surface)
        } else {
            if keep != nil {
                AlwyWatchAccountGroup()
            } else if let account {
                // Signed in to the account only (no login on a server it serves yet).
                Section(tr("Your AlwyWatch account")) {
                    HStack(spacing: 12) {
                        InitialAvatar(name: account.name, size: 46)
                        TitledRow(title: account.name, subtitle: tr("How others see you"))
                    }
                }
                .listRowBackground(Palette.surface)
            }
            AlwyWatchConnectionGroup(version: $version)
        }
    }
}

/// My display name and avatar on the AlwyWatch server, and changing them.
private struct AlwyWatchAccountGroup: View {
    @EnvironmentObject var model: AppModel
    @ObservedObject private var my = MyAccount.shared
    @ObservedObject private var identity = Identity.shared
    @State private var editing = false
    @State private var choosing = false
    @State private var error: String?

    var body: some View {
        if let me = my.me {
            Section {
                HStack(spacing: 12) {
                    PersonAvatar(avatar: me.avatar, name: me.name, userKey: me.userKey, size: 46)
                    TitledRow(title: me.name, subtitle: subtitle(me))
                }
                Button { editing = true } label: {
                    TitledRow(title: tr("Change name"), icon: "person")
                }
                if me.nameIsCustom, !me.mediaName.isEmpty {
                    Button {
                        change { try await $0.keepResetDisplayName() }
                    } label: {
                        TitledRow(title: tr("Reset to {name}", ["name": me.mediaName]), subtitle: tr("Your name on your media server"), icon: "arrow.counterclockwise")
                    }
                }
                Button { choosing = true } label: {
                    TitledRow(title: tr("Change picture"), subtitle: tr("A built-in one, or a photo"), icon: "person.2")
                }
                ErrorText(text: error)
            } header: {
                Text(tr("Your AlwyWatch account"))
            } footer: {
                Text(tr("Others see this name and picture when you watch together."))
            }
            .listRowBackground(Palette.surface)
            .sheet(isPresented: $editing) { EditNameSheet(me: me).environmentObject(model) }
            .sheet(isPresented: $choosing) {
                ChoosePictureSheet(me: me) { apply($0, me) }
            }
        }
    }

    private func subtitle(_ me: Me) -> String {
        let first = me.nameIsCustom && !me.mediaName.isEmpty
            ? tr("Signed in as {name}", ["name": me.mediaName])
            : tr("How others see you")
        return ([first] + [alsoSignedInAs(identity.links)].compactMap { $0 }).joined(separator: "\n")
    }

    private func apply(_ p: AvatarPick, _ me: Me) {
        switch p {
        case let .preset(id): change { try await $0.keepSetAvatarPreset(preset: id) }
        case .initial: if me.pick != .initial { change { try await $0.keepRemoveAvatar() } }
        case let .upload(data): change(uploaded: data) { try await $0.keepUploadAvatar(bytes: data, contentType: "image/jpeg") }
        case .current: break
        }
    }

    private func change(uploaded: Data? = nil, _ block: @escaping (AlwyWatch) async throws -> Me) {
        error = nil
        let core = model.core
        Task {
            do {
                let m = try await block(core)
                if let uploaded, let v = m.avatar.customVersion { AvatarImages.put(userKey: m.userKey, version: v, data: uploaded) }
                my.me = m
            } catch {
                self.error = friendly(error)
            }
        }
    }
}

/// The display name, checked as it is typed.
private struct EditNameSheet: View {
    @EnvironmentObject var model: AppModel
    @Environment(\.dismiss) private var dismiss
    let me: Me
    @State private var name: String
    @State private var busy = false
    @State private var error: String?

    init(me: Me) {
        self.me = me
        _name = State(initialValue: me.name)
    }

    var body: some View {
        let problem = displayNameProblem(name: name)
        NavigationStack {
            Form {
                Section {
                    TextField(tr("Your name"), text: $name)
                        .submitLabel(.done)
                        .onSubmit(save)
                        .onChange(of: name) { _ in error = nil }
                } footer: {
                    VStack(alignment: .leading, spacing: 6) {
                        Text(tr("This is the name others see."))
                        ErrorText(text: error ?? problem.map(capitalizedFirst))
                    }
                }
                .listRowBackground(Palette.surface)
            }
            .scrollContentBackground(.hidden)
            .background(Palette.background.ignoresSafeArea())
            .navigationTitle(tr("Your name"))
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) { Button(tr("Cancel")) { dismiss() } }
                ToolbarItem(placement: .confirmationAction) {
                    Button(busy ? tr("Saving…") : tr("Save"), action: save).disabled(problem != nil || busy)
                }
            }
        }
        .presentationDetents([.medium])
    }

    private func save() {
        guard !busy, displayNameProblem(name: name) == nil else { return }
        if name.trimmingCharacters(in: .whitespaces) == me.name {
            dismiss()
            return
        }
        busy = true
        let core = model.core
        let chosen = name
        Task {
            do {
                MyAccount.shared.me = try await core.keepSetDisplayName(name: chosen)
                dismiss()
            } catch {
                self.error = friendly(error)
            }
            busy = false
        }
    }
}

/// "Choose a picture": a tap picks and closes; Remove goes back to the initial.
private struct ChoosePictureSheet: View {
    @Environment(\.dismiss) private var dismiss
    let me: Me
    let onPick: (AvatarPick) -> Void

    var body: some View {
        NavigationStack {
            ScrollView {
                AvatarGrid(me: me, name: me.name, pick: me.pick, onPick: { p in
                    dismiss()
                    onPick(p)
                }, onUpload: { data in
                    dismiss()
                    onPick(.upload(data))
                })
                .padding(20)
            }
            .background(Palette.background.ignoresSafeArea())
            .navigationTitle(tr("Choose a picture"))
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .confirmationAction) { Button(tr("Close")) { dismiss() } }
                if me.pick != .initial {
                    ToolbarItem(placement: .cancellationAction) {
                        Button(tr("Remove"), role: .destructive) {
                            dismiss()
                            onPick(.initial)
                        }
                    }
                }
            }
        }
    }
}

/// The AlwyWatch server (address and state; a tap changes it), password and passkeys (on the
/// server's web page), linked logins and "Sign out of AlwyWatch".
private struct AlwyWatchConnectionGroup: View {
    @EnvironmentObject var model: AppModel
    @ObservedObject private var my = MyAccount.shared
    @Binding var version: Int
    @State private var problem: String?
    @State private var checked = false
    @State private var signingOut = false

    var body: some View {
        let keep = model.core.keepConnection()
        let account = model.core.account()
        if let base = keep?.baseUrl ?? account?.baseUrl {
            Section(tr("AlwyWatch server")) {
                NavigationLink(value: Route.addServer) {
                    TitledRow(title: host(base), subtitle: status(keep, account), icon: "hourglass", tint: problem != nil ? Palette.danger : Palette.accent)
                }
                .task(id: "\(base)|\(keep?.accountId ?? "")") { await check(keep != nil) }
                // The account itself is managed on the server's website (one place for every app).
                if let account, let page = URL(string: trimmed(account.baseUrl) + "/account") {
                    Link(destination: page) {
                        TitledRow(
                            title: tr("Password & passkeys"),
                            subtitle: tr("Also deleting the account: opens {address}", ["address": host(page.absoluteString) + "/account"]),
                            icon: "gearshape"
                        )
                    }
                }
            }
            .listRowBackground(Palette.surface)
            if let me = my.me { LinkedSignInsGroup(me: me) }
            Section {
                Button { signingOut = true } label: {
                    TitledRow(title: tr("Sign out of AlwyWatch"), subtitle: tr("This profile keeps its Jellyfin and Plex logins"), icon: "xmark", tint: Palette.danger)
                }
                .confirmationDialog(tr("Sign out of AlwyWatch?"), isPresented: $signingOut, titleVisibility: .visible) {
                    Button(tr("Sign out"), role: .destructive) { Task { await signOut() } }
                    Button(tr("Cancel"), role: .cancel) {}
                } message: {
                    Text(tr("This profile keeps its Jellyfin and Plex logins and becomes a profile without an AlwyWatch account. Leaving soon and watch together disappear from it. Your keep choices stay on the server."))
                }
            }
            .listRowBackground(Palette.surface)
        }
    }

    private func trimmed(_ url: String) -> String {
        var u = url
        while u.hasSuffix("/") { u.removeLast() }
        return u
    }

    private func status(_ keep: KeepConnection?, _ account: AlwyAccount?) -> String {
        if let problem { return problem }
        if !checked { return tr("Checking…") }
        if let keep, account == nil { return tr("Connected with your login {name}", ["name": keep.userName]) }
        return tr("Connected")
    }

    /// Can the server be reached, and does it still accept this login? (Also who I am.)
    private func check(_ hasKeep: Bool) async {
        guard hasKeep else { checked = true; return }
        do {
            MyAccount.shared.me = try await model.core.keepMe()
            problem = nil
        } catch CoreError.Unauthorized {
            problem = tr("Sign-in expired: sign in again")
        } catch {
            problem = tr("Can't reach it")
        }
        checked = true
        await Identity.shared.reload(model.core)
    }

    private func signOut() async {
        let core = model.core
        try? core.keepDisconnect()
        MyAccount.shared.me = nil
        Identity.shared.links = nil
        Identity.shared.linkableNow = []
        if core.account() != nil {
            try? await core.accountSignOut()
            Keychain.remove(accountTokenKey)
        }
        version += 1
        model.reload()
    }
}

/// Settings → Account → Linked logins: the sign-ins of my account ("Main", "This device"),
/// Unlink for the others, and "Link another login" when an account of this device can join.
private struct LinkedSignInsGroup: View {
    @EnvironmentObject var model: AppModel
    @ObservedObject private var identity = Identity.shared
    let me: Me
    @State private var unlinking: LinkedIdentity?
    @State private var choosing = false
    @State private var linking: Account?
    @State private var error: String?

    var body: some View {
        let links = identity.links ?? []
        let linkable = identity.linkableNow
        if links.count >= 2 || !linkable.isEmpty {
            Section {
                ForEach(links, id: \.userKey) { id in
                    Button {
                        // The main sign-in holds the account: it can't be unlinked.
                        if !id.canonical { unlinking = id }
                    } label: {
                        HStack(spacing: 14) {
                            ServerMark(kind: id.kind == .plex ? "plex" : "jellyfin", size: 30)
                            VStack(alignment: .leading, spacing: 2) {
                                Text(id.mediaName).foregroundStyle(Palette.onSurface)
                                Text(([ "\(id.serverName) · \(kindLabel(id.kind))" ] + (id.current ? [tr("This device")] : [])).joined(separator: " · "))
                                    .font(.footnote).foregroundStyle(Palette.muted)
                            }
                            Spacer()
                            Text(id.canonical ? tr("Main") : tr("Unlink")).foregroundStyle(Palette.muted)
                        }
                    }
                    .buttonStyle(.plain)
                }
                if !linkable.isEmpty {
                    Button {
                        if linkable.count == 1 { linking = linkable[0] } else { choosing = true }
                    } label: {
                        TitledRow(
                            title: tr("Link another login"),
                            subtitle: linkable.count == 1
                                ? tr("{name} on {server}", ["name": linkable[0].userName, "server": linkable[0].serverName])
                                : trn("{n} account on this device", "{n} accounts on this device", linkable.count),
                            icon: "plus"
                        )
                    }
                }
                ErrorText(text: error)
            } header: {
                Text(tr("Linked logins"))
            } footer: {
                Text(tr("Your logins on different servers, as one AlwyWatch account: the same name, picture, keep choices and rooms."))
            }
            .listRowBackground(Palette.surface)
            .confirmationDialog(
                unlinking.map { tr("Unlink {name}?", ["name": $0.mediaName]) } ?? "",
                isPresented: Binding(get: { unlinking != nil }, set: { if !$0 { unlinking = nil } }),
                titleVisibility: .visible,
                presenting: unlinking
            ) { id in
                Button(tr("Unlink"), role: .destructive) { unlink(id) }
                Button(tr("Cancel"), role: .cancel) {}
            } message: { id in
                Text(tr("{name} on {server} becomes a separate, new AlwyWatch account with no votes, name or history. Continue?", ["name": id.mediaName, "server": id.serverName]))
            }
            .confirmationDialog(tr("Link another login"), isPresented: $choosing, titleVisibility: .visible) {
                ForEach(linkable) { a in
                    Button("\(a.userName) · \(a.serverName) (\(kindLabel(a.kind)))") { linking = a }
                }
                Button(tr("Cancel"), role: .cancel) {}
            }
            .sheet(item: $linking) { a in
                LinkSheet(other: a, onDone: { linking = nil }, onNotNow: { linking = nil }).environmentObject(model)
            }
        }
    }

    private func unlink(_ id: LinkedIdentity) {
        error = nil
        let core = model.core
        Task {
            do {
                try await Identity.shared.unlink(core, id)
            } catch {
                self.error = httpStatus(error) == 409 ? tr("The main sign-in can't be unlinked.") : linkProblem(error, id.serverName)
            }
        }
    }
}

/// "Are these both you?": the synced account (me) and `other`, with names, servers and
/// pictures so a family member's account on a shared device is not linked by mistake.
struct LinkSheet: View {
    @EnvironmentObject var model: AppModel
    @ObservedObject private var my = MyAccount.shared
    let other: Account
    let onDone: () -> Void
    let onNotNow: () -> Void
    @State private var busy = false
    @State private var error: String?

    var body: some View {
        let me = my.me
        let mine = model.core.keepConnection().flatMap { k in model.accounts.first { $0.id == k.accountId } }
        let nameA = mine?.userName ?? me.flatMap { $0.mediaName.isEmpty ? $0.name : $0.mediaName } ?? tr("this account")
        NavigationStack {
            ScrollView {
                VStack(alignment: .leading, spacing: 16) {
                    Text(tr("Use one AlwyWatch account for both of these sign-ins:")).font(.body)
                    person(nameA, mine.map { "\($0.serverName), \(kindLabel($0.kind))" } ?? "") {
                        if let me {
                            PersonAvatar(avatar: me.avatar, name: me.name, userKey: me.userKey, size: 48)
                        } else {
                            InitialAvatar(name: nameA, size: 48)
                        }
                    }
                    person(other.userName, "\(other.serverName), \(kindLabel(other.kind))") {
                        InitialAvatar(name: other.userName, size: 48)
                    }
                    Text(tr("They will share one name and picture, the same keep choices and the same watch-together rooms, on every device."))
                    Text(tr("Only link them if both are you, not someone else who uses this device. It can't simply be undone: unlinking later turns that sign-in into a new, empty account."))
                        .foregroundStyle(Palette.muted)
                    ErrorText(text: error)
                }
                .padding(20)
                .frame(maxWidth: .infinity, alignment: .leading)
            }
            .background(Palette.background.ignoresSafeArea())
            .navigationTitle(tr("Are these both you?"))
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) { Button(tr("Not now"), action: onNotNow) }
                ToolbarItem(placement: .confirmationAction) {
                    Button(busy ? tr("Linking…") : tr("Link"), action: link).disabled(busy)
                }
            }
        }
    }

    private func person<A: View>(_ name: String, _ server: String, @ViewBuilder _ avatar: () -> A) -> some View {
        HStack(spacing: 14) {
            avatar()
            VStack(alignment: .leading, spacing: 2) {
                Text(name).font(.headline)
                Text(server).font(.subheadline).foregroundStyle(Palette.muted)
            }
        }
    }

    private func link() {
        guard !busy else { return }
        busy = true
        error = nil
        let core = model.core
        let account = other
        Task {
            do {
                try await Identity.shared.link(core, account)
                onDone()
            } catch {
                self.error = linkProblem(error, account.serverName)
            }
            busy = false
        }
    }
}

// ----- Welcome step -------------------------------------------------------------------------

/// Once per account (while the server says it isn't done): how others see you, name and
/// picture. Continue saves what changed, Skip saves nothing; both end the step. Nothing here
/// ever blocks the app: a failed call just closes it (it comes back at the next start).
struct WelcomeView: View {
    @EnvironmentObject var model: AppModel
    let me: Me
    @State private var name: String
    @State private var pick: AvatarPick
    @State private var uploaded: Data?
    @State private var busy = false

    init(me: Me) {
        self.me = me
        _name = State(initialValue: me.name)
        _pick = State(initialValue: me.pick)
    }

    var body: some View {
        let problem = displayNameProblem(name: name)
        ScrollView {
            VStack(alignment: .leading, spacing: 18) {
                HStack(spacing: 18) {
                    Group {
                        switch pick {
                        case let .upload(data):
                            if let img = UIImage(data: data) { Image(uiImage: img).resizable().scaledToFill() }
                        case let .preset(id):
                            if let p = AvatarPresets.of(id) { PresetAvatar(preset: p) }
                        case .current:
                            PersonAvatar(avatar: me.avatar, name: me.name, userKey: me.userKey, size: 76)
                        case .initial:
                            InitialAvatar(name: name.trimmingCharacters(in: .whitespaces).isEmpty ? me.name : name, size: 76)
                        }
                    }
                    .frame(width: 76, height: 76)
                    .clipShape(Circle())
                    VStack(alignment: .leading, spacing: 4) {
                        Text(tr("Welcome!")).font(.largeTitle.weight(.bold))
                        Text(tr("This is how others see you")).foregroundStyle(Palette.muted)
                    }
                }
                VStack(alignment: .leading, spacing: 6) {
                    Text(tr("Your name")).font(.subheadline).foregroundStyle(Palette.muted)
                    TextField(tr("Your name"), text: $name)
                        .submitLabel(.done)
                        .padding(12)
                        .background(Palette.surface, in: RoundedRectangle(cornerRadius: 10))
                    ErrorText(text: problem.map(capitalizedFirst))
                }
                Text(tr("Your picture")).font(.headline)
                AvatarGrid(me: me, name: name, pick: pick, onPick: { pick = $0 }, onUpload: { data in
                    uploaded = data
                    pick = .upload(data)
                }, upload: uploaded)
                HStack(spacing: 12) {
                    Button { finish(save: true) } label: {
                        Label(tr("Continue"), systemImage: "checkmark").frame(minWidth: 120)
                    }
                    .buttonStyle(.borderedProminent)
                    .tint(Palette.accent)
                    .foregroundStyle(.black)
                    Button(tr("Skip")) { finish(save: false) }.buttonStyle(.bordered)
                }
                .padding(.top, 6)
                Text(tr("You can change this later in Settings.")).font(.footnote).foregroundStyle(Palette.muted)
            }
            .padding(.horizontal, 20)
            .padding(.vertical, 24)
            .frame(maxWidth: 720, alignment: .leading)
            .frame(maxWidth: .infinity)
        }
        .background(Palette.background.ignoresSafeArea())
    }

    /// Not tied to this view: it closes at once, the calls go on.
    private func finish(save: Bool) {
        if busy { return }
        busy = true
        let core = model.core
        let me = me
        let chosenName = name.trimmingCharacters(in: .whitespaces)
        let chosen = pick
        MyAccount.shared.welcomeClosed = true
        Task {
            var latest: Me?
            if save {
                if chosenName != me.name, displayNameProblem(name: chosenName) == nil {
                    if let m = try? await core.keepSetDisplayName(name: chosenName) { latest = m }
                }
                switch chosen {
                case let .preset(id):
                    if me.avatar.preset != id || me.avatar.customVersion != nil {
                        if let m = try? await core.keepSetAvatarPreset(preset: id) { latest = m }
                    }
                case .initial:
                    if me.pick != .initial, let m = try? await core.keepRemoveAvatar() { latest = m }
                case let .upload(data):
                    if let m = try? await core.keepUploadAvatar(bytes: data, contentType: "image/jpeg") {
                        if let v = m.avatar.customVersion { AvatarImages.put(userKey: m.userKey, version: v, data: data) }
                        latest = m
                    }
                case .current: break
                }
            }
            if let m = try? await core.keepMarkProfileSet() { latest = m }
            if let latest { MyAccount.shared.me = latest }
        }
    }
}

// ----- Other devices: sending this device's logins ------------------------------------------

/// Settings that follow the user to a new device (preferences, not caches).
private let syncedSettings = [combinedKey, preferredKey, "current_account", "downloads.delete_rule"]

/// What a signed-in device sends: the core's accounts and settings, plus their tokens.
private func buildBundle(_ core: AlwyWatch) throws -> String {
    var tokens: [String: String] = [:]
    for a in (try? core.accounts()) ?? [] {
        for k in [a.id, "\(a.id):plextv", "\(a.id):plexhome"] {
            if let v = Keychain.get(k) { tokens[k] = v }
        }
    }
    if let s = Keychain.get("seerr") { tokens["seerr"] = s }
    let config = try core.exportConfig(settingKeys: syncedSettings)
    let data = try JSONSerialization.data(withJSONObject: ["core": config, "tokens": tokens] as [String: Any])
    return String(decoding: data, as: UTF8.self)
}

/// Settings → This device → Other devices (profiles with AlwyWatch).
struct OtherDevicesGroup: View {
    @EnvironmentObject var model: AppModel
    @State private var adding = false

    var body: some View {
        if model.core.keepConnection() != nil || model.core.account() != nil {
            Section(tr("Other devices")) {
                Button { adding = true } label: {
                    TitledRow(
                        title: tr("Sign in another device"),
                        subtitle: tr("On the new phone, tablet or TV, choose “Sign in with a code”, then type its code here"),
                        icon: "plus"
                    )
                }
            }
            .listRowBackground(Palette.surface)
            .sheet(isPresented: $adding) { SendToDeviceSheet().environmentObject(model) }
        }
    }
}

/// Type the new device's code (or scan its QR code), compare the check number, send the
/// accounts sealed to that device.
private struct SendToDeviceSheet: View {
    @EnvironmentObject var model: AppModel
    @Environment(\.dismiss) private var dismiss
    @State private var code = ""
    @State private var request: TransferRequest?
    @State private var error: String?
    @State private var busy = false
    @State private var sent = false
    @State private var scanning = false

    var body: some View {
        NavigationStack {
            Form {
                Section {
                    if sent {
                        Text(tr("Your accounts and settings are on their way. The other device signs in by itself.")).foregroundStyle(Palette.muted)
                    } else if let r = request {
                        Text(tr("Send your accounts and settings to “{device}”?", ["device": r.device])).font(.headline)
                        VStack(alignment: .leading, spacing: 4) {
                            Text(tr("Check number")).font(.footnote).foregroundStyle(Palette.muted)
                            Text(r.check).font(.system(size: 40, weight: .bold, design: .rounded)).foregroundStyle(Palette.accent)
                        }
                        Text(tr("Only send if the new device shows the same number.")).font(.footnote).foregroundStyle(Palette.muted)
                    } else {
                        Text(tr("On the new device, choose “Sign in with a code”, then type the code it shows here.")).foregroundStyle(Palette.muted)
                        TextField("123456", text: $code)
                            .keyboardType(.numberPad)
                            .textContentType(.oneTimeCode)
                            .font(.title2.monospacedDigit())
                            .onChange(of: code) { c in
                                let digits = String(c.filter(\.isNumber).prefix(6))
                                if digits != c { code = digits }
                            }
                        if UIImagePickerController.isSourceTypeAvailable(.camera) {
                            Button { scan() } label: { Label(tr("Scan the QR code"), systemImage: "qrcode.viewfinder") }
                        }
                    }
                    ErrorText(text: error)
                }
                .listRowBackground(Palette.surface)
            }
            .scrollContentBackground(.hidden)
            .background(Palette.background.ignoresSafeArea())
            .navigationTitle(sent ? tr("Sent") : tr("Sign in another device"))
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                if !sent {
                    ToolbarItem(placement: .cancellationAction) { Button(tr("Cancel")) { dismiss() } }
                }
                ToolbarItem(placement: .confirmationAction) {
                    if sent {
                        Button(tr("Done")) { dismiss() }
                    } else if request == nil {
                        Button(busy ? tr("Checking…") : tr("Continue"), action: lookUp).disabled(busy || code.count != 6)
                    } else {
                        Button(busy ? tr("Sending…") : tr("Send"), action: send).disabled(busy)
                    }
                }
            }
            .fullScreenCover(isPresented: $scanning) {
                QrScannerView(onCode: { c in
                    scanning = false
                    code = c
                    lookUp()
                }, onClose: { scanning = false })
            }
        }
    }

    private func scan() {
        error = nil
        switch AVCaptureDevice.authorizationStatus(for: .video) {
        case .authorized: scanning = true
        case .notDetermined:
            AVCaptureDevice.requestAccess(for: .video) { ok in
                Task { @MainActor in
                    if ok { scanning = true } else { error = tr("Without the camera, type the code instead.") }
                }
            }
        default: error = tr("Without the camera, type the code instead.")
        }
    }

    private func lookUp() {
        guard !busy, code.count == 6 else { return }
        busy = true
        let core = model.core
        let c = code
        Task {
            do {
                request = try await core.transferPeer(code: c)
                error = nil
            } catch CoreError.NotFound {
                error = tr("Unknown or expired code.")
            } catch {
                self.error = message(error)
            }
            busy = false
        }
    }

    private func send() {
        guard !busy else { return }
        busy = true
        let core = model.core
        let c = code
        Task {
            do {
                try await core.transferSend(code: c, bundle: buildBundle(core))
                sent = true
            } catch {
                self.error = message(error)
            }
            busy = false
        }
    }
}

/// `…/q/123456` (the address in the QR code), or six digits on their own (the core's rule).
private func signInCode(_ text: String) -> String? {
    let t = text.trimmingCharacters(in: .whitespacesAndNewlines)
    let tail = t.range(of: "/q/", options: .backwards).map { String(t[$0.upperBound...]) } ?? t
    let code = String(tail.prefix { $0 != "?" && $0 != "#" && $0 != "/" })
    return code.count == 6 && code.allSatisfy({ $0.isASCII && $0.isNumber }) ? code : nil
}

/// Full-screen camera that reads another device's "Sign in with a code" QR code.
private struct QrScannerView: View {
    let onCode: (String) -> Void
    let onClose: () -> Void
    @State private var done = false

    var body: some View {
        ZStack(alignment: .top) {
            QrCamera { text in
                guard !done, let c = signInCode(text) else { return }
                done = true
                onCode(c)
            }
            .ignoresSafeArea()
            HStack {
                Text(tr("Point at the QR code on the other device")).font(.headline).foregroundStyle(.white)
                Spacer()
                Button(action: onClose) {
                    Image(systemName: "xmark").font(.headline).foregroundStyle(.white).padding(10)
                }
                .accessibilityLabel(tr("Close"))
            }
            .padding()
            .background(Color.black.opacity(0.55))
        }
        .background(Color.black.ignoresSafeArea())
    }
}

private struct QrCamera: UIViewControllerRepresentable {
    let onText: (String) -> Void

    func makeUIViewController(context: Context) -> QrCameraController {
        let c = QrCameraController()
        c.onText = onText
        return c
    }

    func updateUIViewController(_ c: QrCameraController, context: Context) { c.onText = onText }
}

/// The camera with iOS's own QR reader (no library).
final class QrCameraController: UIViewController, AVCaptureMetadataOutputObjectsDelegate {
    var onText: ((String) -> Void)?
    private let session = AVCaptureSession()
    private var preview: AVCaptureVideoPreviewLayer?

    override func viewDidLoad() {
        super.viewDidLoad()
        view.backgroundColor = .black
        guard let device = AVCaptureDevice.default(for: .video),
              let input = try? AVCaptureDeviceInput(device: device),
              session.canAddInput(input) else { return }
        session.addInput(input)
        let output = AVCaptureMetadataOutput()
        guard session.canAddOutput(output) else { return }
        session.addOutput(output)
        output.setMetadataObjectsDelegate(self, queue: .main)
        output.metadataObjectTypes = [.qr]
        let layer = AVCaptureVideoPreviewLayer(session: session)
        layer.videoGravity = .resizeAspectFill
        view.layer.addSublayer(layer)
        preview = layer
    }

    override func viewDidLayoutSubviews() {
        super.viewDidLayoutSubviews()
        preview?.frame = view.bounds
    }

    override func viewWillAppear(_ animated: Bool) {
        super.viewWillAppear(animated)
        let s = session
        DispatchQueue.global(qos: .userInitiated).async { s.startRunning() }
    }

    override func viewWillDisappear(_ animated: Bool) {
        super.viewWillDisappear(animated)
        let s = session
        DispatchQueue.global(qos: .userInitiated).async { s.stopRunning() }
    }

    nonisolated func metadataOutput(_ output: AVCaptureMetadataOutput, didOutput metadataObjects: [AVMetadataObject], from connection: AVCaptureConnection) {
        let texts = metadataObjects.compactMap { ($0 as? AVMetadataMachineReadableCodeObject)?.stringValue }
        guard !texts.isEmpty else { return }
        Task { @MainActor in
            for t in texts { self.onText?(t) }
        }
    }
}
