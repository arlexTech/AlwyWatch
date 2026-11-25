// SPDX-License-Identifier: AGPL-3.0-or-later
import SwiftUI
import UIKit

/// "{n} minutes" / "{n} hours" / "{n} days" since `at` (Unix seconds).
private func since(_ at: Int64) -> String {
    let s = max(0, Int64(Date().timeIntervalSince1970) - at)
    if s < 3600 { return trn("{n} minute", "{n} minutes", Int(max(1, s / 60))) }
    if s < 86400 { return trn("{n} hour", "{n} hours", Int(s / 3600)) }
    return trn("{n} day", "{n} days", Int(s / 86400))
}

/// "12 Dec 2026" for Unix seconds, in the app's language.
private func day(_ at: Int64) -> String {
    let f = DateFormatter()
    f.locale = Locale(identifier: I18n.language)
    f.dateStyle = .medium
    return f.string(from: Date(timeIntervalSince1970: TimeInterval(at)))
}

/// The AlwyWatch web app's Server settings page (nil when not connected to an AlwyWatch server).
@MainActor
private func serverSettingsPage(_ core: AlwyWatch) -> URL? {
    guard var base = core.keepConnection()?.baseUrl ?? core.account()?.baseUrl else { return nil }
    while base.hasSuffix("/") { base.removeLast() }
    return URL(string: base + "/settings/admin/server")
}

/// A settings row: an icon, a title and its explanation below.
struct TitledRow: View {
    let title: String
    var subtitle: String?
    var icon: String?
    var tint: Color = Palette.accent

    var body: some View {
        HStack(spacing: 14) {
            if let icon { Image(systemName: icon).foregroundStyle(tint).frame(width: 26) }
            VStack(alignment: .leading, spacing: 2) {
                Text(title).foregroundStyle(Palette.onSurface)
                if let subtitle { Text(subtitle).font(.footnote).foregroundStyle(Palette.muted) }
            }
        }
        .padding(.vertical, 2)
    }
}

/// Settings → Admin (admins of the AlwyWatch server only): the dashboard, people, the Leaving
/// soon countdown, kept forever, invitations, and what the server is connected to (read only).
struct AdminSettings: View {
    @EnvironmentObject var model: AppModel

    var body: some View {
        Section {
            NavigationLink(value: Route.dashboard) {
                TitledRow(title: tr("Dashboard"), subtitle: tr("What plays now, direct play or converted, bandwidth, CPU, RAM and who watched what"), icon: "tv")
            }
        }
        .listRowBackground(Palette.surface)
        if let page = serverSettingsPage(model.core) {
            Section(tr("AlwyWatch server")) {
                Link(destination: page) {
                    TitledRow(title: tr("Server settings"), subtitle: tr("Media servers, Seerr, Maintainerr, Tautulli, email and sign-up are set up in the web app."), icon: "gearshape")
                }
            }
            .listRowBackground(Palette.surface)
        }
        Section(tr("People")) {
            NavigationLink(value: Route.adminPeople) {
                TitledRow(title: tr("People"), subtitle: tr("What each person may keep, and whose request-site user they request as"), icon: "person.2")
            }
            NavigationLink(value: Route.adminInvites) {
                TitledRow(title: tr("Invitations"), subtitle: tr("Invite someone by email or with a link, also to Jellyfin"), icon: "plus")
            }
        }
        .listRowBackground(Palette.surface)
        Section(tr("Leaving soon")) {
            NavigationLink(value: Route.adminCountdown) {
                TitledRow(title: tr("Leaving soon countdown"), subtitle: tr("Restart the deletion countdown of a collection"), icon: "clock")
            }
            NavigationLink(value: Route.keepForever) {
                TitledRow(title: tr("Kept forever"), subtitle: tr("Per person, and how much of it someone else keeps too"), icon: "heart.fill")
            }
        }
        .listRowBackground(Palette.surface)
        ServerStatusSection()
    }
}

/// What the AlwyWatch server is connected to, each line with what to change when it is off.
private struct ServerStatusSection: View {
    @EnvironmentObject var model: AppModel
    @State private var status: ServerStatus?
    @State private var error: String?

    var body: some View {
        Section {
            if let st = status {
                if st.dryRun {
                    Text(tr("Test mode: nothing is changed in Maintainerr or Seerr. Turn it off in Server settings (web app)."))
                        .foregroundStyle(Palette.danger)
                }
                line("Maintainerr", st.maintainerr == "ok", maintainerrText(st))
                line(tr("Requests"), st.seerrBridge, st.seerrBridge
                    ? tr("On: every app requests as each person's Seerr user.")
                    : tr("Off: only direct request-site logins work. Set up Seerr in Server settings (web app)."))
                line("Jellyfin", st.jellyfinKey, st.jellyfinKey
                    ? tr("Keeps end by themselves once watched.")
                    : tr("Jellyfin keeps don't end by themselves. Add the Jellyfin server's key in Server settings (web app)."))
                line("Plex (Tautulli)", st.tautulli, st.tautulli
                    ? tr("Keeps end by themselves once watched.")
                    : tr("Plex keeps don't end by themselves. Set up Tautulli in Server settings (web app)."))
                line(tr("Webhooks"), st.webhooks, st.webhooks
                    ? tr("On: watched titles are noticed at once.")
                    : tr("Optional: watched titles are noticed within 15 min. For instant, add the webhooks shown in Server settings (web app)."))
                line(tr("Passkeys"), st.passkeys, st.passkeys ? tr("On") : tr("Off: set this site's address (https) in Server settings (web app)"))
                line("Pocket ID", st.pocketid, st.pocketid ? tr("On") : tr("Off"))
                line(tr("New accounts"), st.registration != "closed", registrationText(st.registration))
                TitledRow(title: tr("AlwyWatch server"), subtitle: st.version, icon: "hourglass", tint: Palette.onSurface)
            } else if let error {
                Text(error).foregroundStyle(Palette.danger).font(.footnote)
            } else {
                Text(tr("Checking…")).foregroundStyle(Palette.subtle)
            }
        } header: {
            Text(tr("Server status"))
        } footer: {
            Text(tr("Read only: change these in Server settings (web app)."))
        }
        .listRowBackground(Palette.surface)
        .task {
            do {
                status = try await model.core.keepAdminStatus()
            } catch {
                self.error = message(error)
            }
        }
    }

    private func line(_ title: String, _ ok: Bool, _ text: String) -> some View {
        TitledRow(title: title, subtitle: text, icon: ok ? "checkmark" : "info.circle", tint: ok ? Palette.accent : Palette.onSurface)
    }

    private func maintainerrText(_ st: ServerStatus) -> String {
        switch st.maintainerr {
        case "off": return tr("Off. Set up Maintainerr in Server settings (web app).")
        case "starting": return tr("Reading Maintainerr for the first time…")
        case "ok":
            return trn(
                "Maintainerr {version} · read {time} ago · {n} title leaving",
                "Maintainerr {version} · read {time} ago · {n} titles leaving",
                Int(st.leavingItems),
                ["version": st.maintainerrVersion ?? "", "time": st.lastSync.map(since) ?? "?"]
            )
        default: return tr("Can't reach Maintainerr. Check its address and API key in Server settings (web app).")
        }
    }

    private func registrationText(_ r: String) -> String {
        switch r {
        case "open": return tr("Anyone with the address")
        case "invite": return tr("With an invitation")
        default: return tr("Closed")
        }
    }
}

// ----- People and the countdown --------------------------------------------------------------

/// One line for a person's limits: "No limits", or what differs from the default.
private func rightsSummary(_ r: Rights, _ seerr: String?) -> String {
    var parts: [String] = []
    if !r.canKeepForever {
        parts.append(tr("Can't keep forever"))
    } else if let most = r.maxForever {
        parts.append(tr("Up to {n} forever", ["n": most]))
    }
    if let d = r.maxKeepDays { parts.append(trn("{n} day at most", "{n} days at most", Int(d))) }
    if let seerr { parts.append(tr("Requests as {name}", ["name": seerr])) }
    return parts.isEmpty ? tr("No limits") : parts.joined(separator: " · ")
}

extension AdminUser: Identifiable {
    public var id: String { userKey }
}

/// Admins: per-user rights and the collection countdown (Leaving soon's admin shortcut shows
/// both; Settings → Admin opens the people or the countdown alone).
struct KeepAdminView: View {
    @EnvironmentObject var model: AppModel
    var people = true
    var countdown = true
    @State private var users: [AdminUser]?
    @State private var note: String?
    @State private var armed: String?
    @State private var seerrUsers: [SeerrUser] = []
    @State private var editing: String?
    @State private var linking: AdminUser?

    var body: some View {
        let collections = leavingCollections()
        List {
            if people && countdown {
                Section(tr("Kept forever")) {
                    NavigationLink(value: Route.keepForever) {
                        TitledRow(title: tr("Who keeps what forever"), subtitle: tr("Per person, and how much of it someone else keeps too"), icon: "heart.fill")
                    }
                }
                .listRowBackground(Palette.surface)
            }
            if let note { Text(note).foregroundStyle(Palette.accent).listRowBackground(Color.clear) }
            if countdown {
                Section {
                    ForEach(collections) { c in
                        VStack(alignment: .leading, spacing: 10) {
                            Label(c.title, systemImage: "clock")
                            FlowLayout(spacing: 8) {
                                ForEach([UInt32(0), 14, 30], id: \.self) { stagger in
                                    let key = "\(c.id)/\(stagger)"
                                    let isArmed = armed == key
                                    Button(isArmed ? tr("Confirm") : stagger == 0 ? tr("Restart now") : trn("Spread over {n} day", "Spread over {n} days", Int(stagger))) {
                                        restart(c.id, c.title, stagger, key)
                                    }
                                    .buttonStyle(ActionButtonStyle(primary: isArmed))
                                }
                            }
                        }
                        .padding(.vertical, 6)
                    }
                } header: {
                    Text(tr("Start countdown"))
                } footer: {
                    Text(tr("Every item is already past its date. Before activating a collection in Maintainerr, restart its countdown so people get the full delay to vote. This changes deletion dates in Maintainerr (press twice to confirm)."))
                }
                .listRowBackground(Palette.surface)
            }
            if people {
                Section {
                    if users == nil, note == nil { Text(tr("Loading…")).foregroundStyle(Palette.subtle) }
                    ForEach(users ?? []) { u in
                        Button { editing = u.userKey } label: { personRow(u) }
                    }
                } header: {
                    Text(tr("People"))
                } footer: {
                    Text(tr("No limits by default. Tap a person to change theirs."))
                }
                .listRowBackground(Palette.surface)
            }
        }
        .scrollContentBackground(.hidden)
        .background(Palette.background.ignoresSafeArea())
        .navigationTitle(people && countdown ? tr("Admin") : people ? tr("People") : tr("Leaving soon countdown"))
        .sheet(isPresented: Binding(get: { editing != nil }, set: { if !$0 { editing = nil } })) {
            if let u = users?.first(where: { $0.userKey == editing }) {
                PersonRightsSheet(
                    user: u,
                    seerrName: seerrUsers.first { $0.id == u.seerrUserId }?.name,
                    hasSeerr: !seerrUsers.isEmpty,
                    seerrUsers: seerrUsers,
                    onRights: { update(u, $0) },
                    onSeerr: { link(u, $0) }
                )
                .environmentObject(model)
            }
        }
        .task {
            do {
                users = try await model.core.keepAdminUsers()
            } catch {
                note = message(error)
            }
            seerrUsers = (try? await model.core.keepAdminSeerrUsers()) ?? []
        }
    }

    private struct Collection: Identifiable {
        let id: Int64
        let title: String
    }

    /// The collections Leaving soon draws from.
    private func leavingCollections() -> [Collection] {
        var seen = Set<Int64>()
        var out: [Collection] = []
        for i in ((try? model.core.cachedLeavingSoon()) ?? nil) ?? [] {
            guard let id = i.collectionId, seen.insert(id).inserted else { continue }
            out.append(Collection(id: id, title: i.collectionTitle ?? tr("Collection {id}", ["id": id])))
        }
        return out
    }

    private func personRow(_ u: AdminUser) -> some View {
        let all = users ?? []
        let sameName = all.filter { $0.name.lowercased() == u.name.lowercased() }.count > 1
        let seerrName = seerrUsers.first { $0.id == u.seerrUserId }?.name
        let parts = [
            !u.mediaName.isEmpty && (sameName || u.mediaName.lowercased() != u.name.lowercased()) ? u.mediaName : nil,
            u.userKey.hasPrefix("plex:") ? "Plex" : "Jellyfin",
            rightsSummary(u.rights, seerrName),
        ].compactMap { $0 }
        return HStack(spacing: 12) {
            PersonAvatar(avatar: u.avatar, name: u.name, userKey: u.userKey, size: 34)
            VStack(alignment: .leading, spacing: 2) {
                Text(u.isAdmin ? tr("{name} · admin", ["name": u.name]) : u.name).foregroundStyle(Palette.onSurface)
                Text(parts.joined(separator: " · ")).font(.footnote).foregroundStyle(Palette.muted)
            }
            Spacer()
            if u.foreverCount > 0 {
                Image(systemName: "heart.fill").font(.caption).foregroundStyle(Palette.accent)
                Text("\(u.foreverCount)").foregroundStyle(Palette.muted)
            }
            Image(systemName: "chevron.right").font(.caption).foregroundStyle(Palette.subtle)
        }
    }

    private func restart(_ id: Int64, _ title: String, _ stagger: UInt32, _ key: String) {
        guard armed == key else {
            armed = key
            return
        }
        armed = nil
        Task {
            do {
                let r = try await model.core.keepAdminCountdown(collectionId: id, staggerDays: stagger)
                note = tr("{collection}: {reset} restarted, {skipped} skipped", ["collection": title, "reset": r.reset, "skipped": r.skipped])
            } catch {
                note = message(error)
            }
        }
    }

    private func update(_ u: AdminUser, _ r: Rights) {
        Task {
            do {
                let nu = try await model.core.keepAdminSetRights(userKey: u.userKey, rights: r)
                users = users?.map { $0.userKey == nu.userKey ? nu : $0 }
            } catch {
                note = message(error)
            }
        }
    }

    private func link(_ u: AdminUser, _ seerrId: Int64?) {
        Task {
            do {
                let nu = try await model.core.keepAdminLinkSeerr(userKey: u.userKey, seerrUserId: seerrId)
                users = users?.map { $0.userKey == nu.userKey ? nu : $0 }
            } catch {
                note = message(error)
            }
        }
    }
}

/// A person's limits, one clear row each (opened from the People list).
private struct PersonRightsSheet: View {
    @Environment(\.dismiss) private var dismiss
    let user: AdminUser
    let seerrName: String?
    let hasSeerr: Bool
    let seerrUsers: [SeerrUser]
    let onRights: (Rights) -> Void
    let onSeerr: (Int64?) -> Void

    var body: some View {
        let r = user.rights
        NavigationStack {
            List {
                Section {
                    HStack(spacing: 14) {
                        PersonAvatar(avatar: user.avatar, name: user.name, userKey: user.userKey, size: 48)
                        VStack(alignment: .leading, spacing: 2) {
                            Text(user.name).font(.headline)
                            Text(trn("Keeps {n} title forever.", "Keeps {n} titles forever.", Int(user.foreverCount))).font(.footnote).foregroundStyle(Palette.muted)
                        }
                    }
                    Toggle(tr("May keep forever"), isOn: Binding(get: { r.canKeepForever }, set: { v in
                        var n = r
                        n.canKeepForever = v
                        onRights(n)
                    }))
                    if r.canKeepForever {
                        Picker(tr("Titles kept forever"), selection: Binding(get: { r.maxForever ?? 0 }, set: { v in
                            var n = r
                            n.maxForever = v == 0 ? nil : v
                            onRights(n)
                        })) {
                            Text(tr("No limit")).tag(UInt32(0))
                            ForEach([UInt32(5), 10, 25, 50], id: \.self) { Text(tr("Up to {n}", ["n": $0])).tag($0) }
                        }
                    }
                    Picker(selection: Binding(get: { r.maxKeepDays ?? 0 }, set: { v in
                        var n = r
                        n.maxKeepDays = v == 0 ? nil : v
                        onRights(n)
                    })) {
                        Text(tr("No limit")).tag(UInt32(0))
                        ForEach([UInt32(7), 30, 90, 180], id: \.self) { Text(trn("{n} day", "{n} days", Int($0))).tag($0) }
                    } label: {
                        VStack(alignment: .leading, spacing: 2) {
                            Text(tr("Keep for at most"))
                            Text(tr("How long one “keep for a while” can last")).font(.caption).foregroundStyle(Palette.muted)
                        }
                    }
                }
                .listRowBackground(Palette.surface)
                if hasSeerr {
                    Section(tr("Requests as")) {
                        Button {
                            onSeerr(nil)
                        } label: {
                            HStack {
                                TitledRow(title: tr("Automatic"), subtitle: tr("Their own Seerr account (created if needed)"))
                                Spacer()
                                if user.seerrUserId == nil { Image(systemName: "checkmark").foregroundStyle(Palette.accent) }
                            }
                        }
                        ForEach(seerrUsers, id: \.id) { su in
                            Button {
                                onSeerr(su.id)
                            } label: {
                                HStack {
                                    Text(su.name).foregroundStyle(Palette.onSurface)
                                    Spacer()
                                    if user.seerrUserId == su.id { Image(systemName: "checkmark").foregroundStyle(Palette.accent) }
                                }
                            }
                        }
                    }
                    .listRowBackground(Palette.surface)
                }
            }
            .scrollContentBackground(.hidden)
            .background(Palette.background.ignoresSafeArea())
            .navigationTitle(user.name)
            .navigationBarTitleDisplayMode(.inline)
            .toolbar { ToolbarItem(placement: .confirmationAction) { Button(tr("Done")) { dismiss() } } }
        }
    }
}

// ----- Kept forever --------------------------------------------------------------------------

/// Admins: who keeps what forever, and above all how much of it someone else keeps too.
struct KeepForeverView: View {
    @EnvironmentObject var model: AppModel
    @State private var stats: ForeverStats?
    @State private var note: String?
    @State private var person: String?
    @State private var spread = "all"

    var body: some View {
        let s = stats
        let names = Dictionary((s?.people ?? []).map { ($0.userKey, $0.name) }, uniquingKeysWith: { a, _ in a })
        let shown = (s?.items ?? [])
            .filter { person == nil || $0.keepers.contains(person!) }
            .filter { spread == "all" || (spread == "shared" ? $0.keepers.count > 1 : $0.keepers.count == 1) }
        List {
            if let s, s.titles > 0 {
                Text(tr("{titles} titles · {shared} kept by several people · {alone} by one person", ["titles": s.titles, "shared": s.shared, "alone": s.titles - s.shared]))
                    .foregroundStyle(Palette.muted).listRowBackground(Color.clear)
            } else if s == nil, note == nil {
                Text(tr("Loading…")).foregroundStyle(Palette.muted).listRowBackground(Color.clear)
            }
            if let note { Text(note).foregroundStyle(Palette.accent).listRowBackground(Color.clear) }
            if let s, s.people.isEmpty {
                Text(tr("Nobody keeps anything forever yet.")).foregroundStyle(Palette.muted).listRowBackground(Color.clear)
            }
            if let s, !s.people.isEmpty {
                Section {
                    ForEach(s.people, id: \.userKey) { p in
                        Button { person = person == p.userKey ? nil : p.userKey } label: { personRow(p) }
                    }
                } header: {
                    Text(tr("People"))
                } footer: {
                    Text(tr("Shared: someone else keeps it forever too. Choose a person to see their titles."))
                }
                .listRowBackground(Palette.surface)
                ScrollView(.horizontal, showsIndicators: false) {
                    HStack(spacing: 8) {
                        ForEach(["all", "shared", "alone"], id: \.self) { v in
                            Button(spreadLabel(v)) { spread = v }.buttonStyle(ChipStyle(selected: spread == v))
                        }
                        if let k = person {
                            Button("\(names[k] ?? k) ✕") { person = nil }.buttonStyle(ChipStyle(selected: true))
                        }
                    }
                }
                .listRowBackground(Color.clear)
                .listRowInsets(EdgeInsets())
                Section([person.flatMap { names[$0] }, trn("{n} title", "{n} titles", shown.count)].compactMap { $0 }.joined(separator: " · ")) {
                    if shown.isEmpty { Text(tr("Nothing here.")).foregroundStyle(Palette.muted) }
                    ForEach(shown, id: \.key) { t in
                        let others = t.keepers.filter { $0 != person }.map { names[$0] ?? $0 }
                        HStack(spacing: 12) {
                            UrlImage(url: t.posterUrl)
                                .frame(width: 40, height: 60)
                                .clipShape(RoundedRectangle(cornerRadius: 4))
                            VStack(alignment: .leading, spacing: 2) {
                                Text(t.title + (t.year.map { " (\($0))" } ?? ""))
                                Text(person != nil && others.isEmpty ? tr("Only them")
                                    : person != nil ? tr("Also {names}", ["names": others.joined(separator: ", ")])
                                    : t.keepers.map { names[$0] ?? $0 }.joined(separator: ", "))
                                    .font(.footnote).foregroundStyle(Palette.muted)
                            }
                            Spacer()
                            Text(trn("{n} person", "{n} people", t.keepers.count)).font(.footnote).foregroundStyle(Palette.muted)
                        }
                    }
                }
                .listRowBackground(Palette.surface)
            }
        }
        .scrollContentBackground(.hidden)
        .background(Palette.background.ignoresSafeArea())
        .navigationTitle(tr("Kept forever"))
        .task {
            do {
                stats = try await model.core.keepAdminForever()
            } catch {
                note = message(error)
            }
        }
    }

    private func spreadLabel(_ v: String) -> String {
        switch v {
        case "shared": return tr("Kept by several")
        case "alone": return tr("Kept by one person")
        default: return tr("All")
        }
    }

    private func personRow(_ p: ForeverPerson) -> some View {
        let parts = [
            !p.mediaName.isEmpty && p.mediaName.lowercased() != p.name.lowercased() ? p.mediaName : nil,
            tr("{shared} shared · {alone} only them", ["shared": p.shared, "alone": p.alone]),
            p.overlaps.isEmpty ? nil : tr("most with {names}", ["names": p.overlaps.prefix(3).map { "\($0.name) (\($0.both))" }.joined(separator: ", ")]),
        ].compactMap { $0 }
        return VStack(alignment: .leading, spacing: 8) {
            HStack(spacing: 12) {
                PersonAvatar(avatar: p.avatar, name: p.name, userKey: p.userKey, size: 36)
                    .padding(person == p.userKey ? 3 : 0)
                    .background(person == p.userKey ? Palette.accent : .clear, in: Circle())
                VStack(alignment: .leading, spacing: 2) {
                    Text(p.name).foregroundStyle(Palette.onSurface)
                    Text(parts.joined(separator: " · ")).font(.footnote).foregroundStyle(Palette.muted)
                }
                Spacer()
                Text("\(p.count)").foregroundStyle(Palette.muted)
            }
            // Shared (accent) against alone (muted) at a glance.
            if p.count > 0 {
                GeometryReader { g in
                    HStack(spacing: 0) {
                        Rectangle().fill(Palette.accent).frame(width: g.size.width * CGFloat(p.shared) / CGFloat(max(1, p.shared + p.alone)))
                        Rectangle().fill(Palette.muted.opacity(0.5))
                    }
                }
                .frame(height: 6)
                .clipShape(Capsule())
            }
        }
    }
}

// ----- Invitations ---------------------------------------------------------------------------

/// Settings → Admin → Invitations: invite someone by email (one use, sent by the server) or
/// make a link, optionally with a Jellyfin login copying an existing user's access; then the
/// invitations, each can be ended.
struct InvitationsView: View {
    @EnvironmentObject var model: AppModel
    @State private var data: Invitations?
    @State private var loadError: String?
    @State private var version = 0
    @State private var email = ""
    @State private var note = ""
    @State private var makeJellyfin = true
    @State private var target: String?
    @State private var like: String?
    @State private var days: Int64 = 7
    @State private var uses: Int64 = 1
    @State private var result: AccountInvite?
    @State private var error: String?
    @State private var busy = false
    @State private var ending: InviteItem?

    var body: some View {
        let inv = data
        let targets = inv?.jellyfin ?? []
        let t = targets.first { $0.serverId == target } ?? targets.first
        let likeShown = like ?? t?.lastLike.flatMap { l in t?.users.contains { $0.id == l } == true ? l : nil } ?? t?.users.first?.id ?? ""
        let withEmail = inv?.mail == true && !email.trimmingCharacters(in: .whitespaces).isEmpty
        List {
            Text(tr("A new person gets an AlwyWatch account, and a Jellyfin login if you like")).foregroundStyle(Palette.muted).listRowBackground(Color.clear)
            if let inv {
                Section(tr("Invite someone")) {
                    if inv.mail {
                        VStack(alignment: .leading, spacing: 6) {
                            TextField(tr("Email (optional)"), text: $email)
                                .keyboardType(.emailAddress).textInputAutocapitalization(.never).autocorrectionDisabled()
                            Text(email.isEmpty ? tr("Empty: you get a link and a code to send them yourself.") : tr("AlwyWatch emails them a link that works once."))
                                .font(.footnote).foregroundStyle(Palette.muted)
                        }
                    } else if let page = serverSettingsPage(model.core) {
                        Link(destination: page) {
                            TitledRow(title: tr("Set up email in Server settings to send invitations by email"), subtitle: tr("For now, you get a link and a code to send them yourself."), icon: "info.circle")
                        }
                    }
                    if let t {
                        Toggle(isOn: $makeJellyfin) {
                            TitledRow(title: tr("Also make a Jellyfin login on {server}", ["server": t.name]), subtitle: tr("Same username and password as their AlwyWatch account"))
                        }
                        if makeJellyfin {
                            if targets.count > 1 {
                                Picker(tr("Jellyfin server"), selection: Binding(get: { t.serverId }, set: { target = $0; like = nil })) {
                                    ForEach(targets, id: \.serverId) { Text($0.name).tag($0.serverId) }
                                }
                            }
                            Picker(selection: Binding(get: { likeShown }, set: { like = $0 })) {
                                ForEach(t.users, id: \.id) { Text($0.name).tag($0.id) }
                                Text(tr("Jellyfin's defaults")).tag("")
                            } label: {
                                TitledRow(title: tr("With the same access as"), subtitle: tr("Libraries, parental rating and permissions are copied from this user"))
                            }
                        }
                    }
                    if withEmail {
                        TextField(tr("Personal note (optional)"), text: Binding(get: { note }, set: { note = String($0.prefix(500)) }), prompt: Text(tr("A few words in the email")))
                    }
                    VStack(alignment: .leading, spacing: 10) {
                        Text(tr("Valid for")).font(.footnote).foregroundStyle(Palette.muted)
                        HStack(spacing: 8) {
                            ForEach([Int64(1), 7, 30], id: \.self) { n in
                                Button(trn("{n} day", "{n} days", Int(n))) { days = n }.buttonStyle(ChipStyle(selected: days == n))
                            }
                        }
                        if !withEmail {
                            Text(tr("How many people")).font(.footnote).foregroundStyle(Palette.muted)
                            HStack(spacing: 8) {
                                ForEach([Int64(1), 5, 10], id: \.self) { n in
                                    Button(trn("{n} person", "{n} people", Int(n))) { uses = n }.buttonStyle(ChipStyle(selected: uses == n))
                                }
                            }
                        }
                    }
                    .buttonStyle(.plain)
                    .padding(.vertical, 6)
                }
                .listRowBackground(Palette.surface)
                Section {
                    Button {
                        send(inv, t, likeShown, withEmail)
                    } label: {
                        Label(busy ? (withEmail ? tr("Sending…") : tr("Creating…")) : (withEmail ? tr("Send invitation") : tr("Create link")), systemImage: "plus")
                    }
                    .buttonStyle(ActionButtonStyle(primary: true))
                    ErrorText(text: error)
                }
                .listRowBackground(Color.clear)
                if let result { InviteResult(invite: result) }
                if !inv.invites.isEmpty {
                    Section(tr("Invitations")) {
                        ForEach(inv.invites, id: \.id) { inviteRow($0) }
                    }
                    .listRowBackground(Palette.surface)
                }
            } else if let loadError {
                Text(loadError).foregroundStyle(Palette.danger).listRowBackground(Color.clear)
            } else {
                Text(tr("Loading…")).foregroundStyle(Palette.subtle).listRowBackground(Color.clear)
            }
        }
        .scrollContentBackground(.hidden)
        .background(Palette.background.ignoresSafeArea())
        .navigationTitle(tr("Invitations"))
        .task(id: version) {
            do {
                data = try await model.core.accountInvitations()
                loadError = nil
            } catch {
                if case .NotFound = error as? CoreError {
                    loadError = tr("Invitations are made with an AlwyWatch account: sign in to yours on this device (Settings → Account), or use the web app.")
                } else {
                    loadError = message(error)
                }
            }
        }
        .confirmationDialog(tr("End this invitation?"), isPresented: Binding(get: { ending != nil }, set: { if !$0 { ending = nil } }), titleVisibility: .visible) {
            Button(tr("End"), role: .destructive) {
                guard let item = ending else { return }
                Task {
                    do {
                        try await model.core.accountRevokeInvite(id: item.id)
                        version += 1
                    } catch {
                        self.error = message(error)
                    }
                }
            }
        } message: {
            Text(tr("Its code and link stop working. Accounts already made with it stay."))
        }
    }

    private func inviteRow(_ it: InviteItem) -> some View {
        let now = Int64(Date().timeIntervalSince1970)
        let active = it.usesLeft > 0 && it.expires > now
        let status: String? = active ? tr("Until {date}", ["date": day(it.expires)]) : it.usedBy.isEmpty ? tr("Ended") : nil
        let parts = [
            it.jellyfin.map { tr("with a login on {server}", ["server": $0]) },
            tr("by {name}", ["name": it.createdBy]),
            it.usedBy.isEmpty ? nil : tr("used by {names}", ["names": it.usedBy.joined(separator: ", ")]),
            status,
        ].compactMap { $0 }
        return HStack {
            TitledRow(title: it.email ?? tr("Link"), subtitle: parts.joined(separator: " · "), icon: it.email != nil ? "person" : "plus", tint: active ? Palette.accent : Palette.subtle)
            Spacer()
            if active {
                Button(tr("End")) { ending = it }.buttonStyle(ActionButtonStyle())
            }
        }
    }

    private func send(_ inv: Invitations, _ t: JellyfinTarget?, _ likeShown: String, _ withEmail: Bool) {
        guard !busy else { return }
        busy = true
        error = nil
        let jf = makeJellyfin ? t : nil
        Task {
            do {
                result = try await model.core.accountInviteSend(
                    email: withEmail ? email.trimmingCharacters(in: .whitespaces) : nil,
                    jellyfin: jf?.serverId,
                    like: jf != nil && !likeShown.isEmpty ? likeShown : nil,
                    note: withEmail ? note : nil,
                    days: days,
                    uses: withEmail ? 1 : uses
                )
                email = ""
                note = ""
                version += 1
            } catch {
                self.error = message(error)
            }
            busy = false
        }
    }
}

/// What was just made: "Sent to …", or the code and link to pass on (Copy / Share).
private struct InviteResult: View {
    let invite: AccountInvite

    var body: some View {
        let i = invite
        Section {
            VStack(alignment: .leading, spacing: 10) {
                if let to = i.sentTo {
                    Text(tr("Sent to {email}", ["email": to])).font(.headline).foregroundStyle(Palette.accent)
                }
                Text(tr("Invitation code")).font(.footnote).foregroundStyle(Palette.muted)
                Text(i.code).font(.title.weight(.bold)).foregroundStyle(i.sentTo == nil ? Palette.accent : Palette.onSurface).textSelection(.enabled)
                if let url = i.url {
                    Text(tr("Or the link")).font(.footnote).foregroundStyle(Palette.muted)
                    Text(host(url)).font(.subheadline.weight(.semibold)).textSelection(.enabled)
                }
                Text(trn(
                    "Valid {n} day, for {count}. In AlwyWatch: Add a profile → Create an AlwyWatch account.",
                    "Valid {n} days, for {count}. In AlwyWatch: Add a profile → Create an AlwyWatch account.",
                    Int(i.expiresInDays),
                    ["count": trn("{n} person", "{n} people", Int(i.uses))]
                ))
                .font(.footnote).foregroundStyle(Palette.muted)
                if i.sentTo == nil {
                    let text = [tr("Your AlwyWatch invitation code: {code}", ["code": i.code]), i.url].compactMap { $0 }.joined(separator: "\n")
                    HStack(spacing: 8) {
                        Button(tr("Copy")) { UIPasteboard.general.string = i.url ?? i.code }.buttonStyle(ActionButtonStyle())
                        ShareLink(item: text) { Text(tr("Share")) }.buttonStyle(ActionButtonStyle(primary: true))
                    }
                }
            }
            .padding(.vertical, 6)
        }
        .listRowBackground(Palette.surface)
    }
}
