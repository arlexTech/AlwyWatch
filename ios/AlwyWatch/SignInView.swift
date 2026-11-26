// SPDX-License-Identifier: AGPL-3.0-or-later
import SwiftUI
import UIKit

/// The first screen ("Welcome to AlwyWatch": sign in to AlwyWatch, create an account, or
/// Jellyfin or Plex only) and "Add a server" from Settings: the same steps as Android's
/// AddServerScreen.
struct SignInView: View {
    @EnvironmentObject var model: AppModel
    @Environment(\.dismiss) private var dismiss
    @Environment(\.openURL) private var openURL

    enum Step {
        case choose
        /// The AlwyWatch address, then its ways in (create: on to the sign-up).
        case alwyWatch(create: Bool)
        case register(url: String)
        /// Accounts copied from a signed-in device, through this AlwyWatch server.
        case receive(url: String)
        case accountLogin(url: String)
        /// Signed in to an AlwyWatch account: its servers to sign in to on this device.
        case known(url: String)
        /// "Jellyfin or Plex only".
        case direct
        case jellyfinUrl
        case jellyfin(url: String, quick: QuickConnectCode?)
        case plex
        case plexUsers(token: String, users: [PlexHomeUser])
        case plexServers(token: String, servers: [PlexServerResource])
    }

    /// Steps visited, so Back returns to the previous one.
    @State private var history: [Step] = [.choose]
    @State private var started = false
    @State private var firstRun = true
    @State private var keep: KeepConnection?
    @State private var busy = false
    @State private var error: String?
    @State private var url = ""
    @State private var username = ""
    @State private var password = ""
    @State private var plexCode: String?
    @State private var plexAuthUrl: URL?
    @State private var plexHomeToken: String?
    /// Picked from an AlwyWatch server's list: Plex then selects that server by itself.
    @State private var presetPlexId: String?
    /// The AlwyWatch address and what it serves: kept while going on and back.
    @State private var alwyUrl = ""
    @State private var alwyFound: (base: String, list: [ServerEntry])?
    @State private var checking = false

    private var step: Step { history.last ?? .choose }
    private var core: AlwyWatch { model.core }

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 18) {
                header
                content
                ErrorText(text: error)
            }
            .padding(.horizontal, 24)
            .padding(.vertical, 16)
            .frame(maxWidth: 600, alignment: .leading)
            .frame(maxWidth: .infinity)
        }
        .background(alignment: .top) {
            // Android's ambient glow.
            RadialGradient(colors: [Palette.accent.opacity(0.2), .clear], center: UnitPoint(x: 0.3, y: 0), startRadius: 0, endRadius: 600)
                .ignoresSafeArea()
        }
        .background(Palette.background.ignoresSafeArea())
        .scrollDismissesKeyboard(.interactively)
        .toolbar {
            if history.count > 1 {
                ToolbarItem(placement: .navigationBarLeading) {
                    Button { back() } label: { Image(systemName: "chevron.left") }
                        .accessibilityLabel(tr("Back"))
                }
            }
        }
        .navigationBarBackButtonHidden(history.count > 1)
        .onAppear(perform: start)
    }

    private func start() {
        guard !started else { return }
        started = true
        firstRun = ((try? core.accounts()) ?? []).isEmpty
        keep = core.keepConnection()
        alwyUrl = (keep?.baseUrl ?? core.account()?.baseUrl ?? model.lastAlwyWatchUrl ?? "").bare
        // Signed in to an AlwyWatch account: straight to its servers.
        if let me = core.account() { history = [.choose, .known(url: me.baseUrl)] }
    }

    @ViewBuilder private var header: some View {
        switch step {
        case .choose where firstRun:
            VStack(alignment: .leading, spacing: 8) {
                Text(tr("Welcome to AlwyWatch")).font(.largeTitle.weight(.bold))
                Text(tr("Your Jellyfin and Plex, fast and beautiful.")).foregroundStyle(Palette.muted)
            }
            .padding(.top, 24)
            .padding(.bottom, 8)
        case .choose:
            Text(tr("Add a server")).font(.largeTitle.weight(.bold))
        default:
            Text(title).font(.title.weight(.bold))
        }
    }

    private var title: String {
        switch step {
        case let .alwyWatch(create): return create ? tr("Create an AlwyWatch account") : tr("Sign in to AlwyWatch")
        case .register: return tr("Create an AlwyWatch account")
        case .receive: return tr("Sign in with a code")
        case .accountLogin: return tr("AlwyWatch account")
        case .known: return tr("Your servers")
        case .direct: return tr("Jellyfin or Plex only")
        case .plex, .plexUsers, .plexServers: return "Plex"
        default: return "Jellyfin"
        }
    }

    @ViewBuilder private var content: some View {
        switch step {
        case .choose:
            if firstRun {
                ChoiceCard(title: tr("Sign in to AlwyWatch"), subtitle: tr("Your name, settings and servers come with you"), icon: "hourglass") {
                    go(.alwyWatch(create: false))
                }
                ChoiceCard(title: tr("Create an AlwyWatch account"), subtitle: tr("If someone invited you"), icon: "plus") {
                    go(.alwyWatch(create: true))
                }
                ChoiceCard(title: tr("Jellyfin or Plex only"), subtitle: tr("No account: nothing synced, no Leaving soon or watch together"), icon: "person") {
                    go(.direct)
                }
            } else if let keep {
                ConnectedChoices(keepUrl: keep.baseUrl) { pick(keep.baseUrl, $0) } onJellyfin: {
                    clearPreset()
                    go(.jellyfinUrl)
                } onPlex: {
                    clearPreset()
                    go(.plex)
                }
            } else {
                // Not on AlwyWatch: Jellyfin or Plex (AlwyWatch is connected from Settings).
                directChoices
            }

        case let .alwyWatch(create):
            alwyWatchPicker(create: create)

        case let .register(u):
            RegisterStep(url: u, busy: busy, attempt: attempt) { signedIn in
                // The invitation made a Jellyfin login, already signed in: on to the home.
                try? core.keepSetPreset(url: u)
                finish(signedIn)
            } onDone: { replaceLast(.known(url: u)) }

        case let .receive(u):
            ReceiveCode(url: u) { account in
                if let account { done(account) } else { replaceLast(.known(url: u)) }
            }

        case let .accountLogin(u):
            AccountLoginStep(url: u, busy: busy, attempt: attempt, onError: { error = $0 }) {
                replaceLast(.known(url: u))
            } onNoPassword: { back() } onJellyfin: { signedIn in
                // The Jellyfin login an invitation made, same password: on to the home.
                try? core.keepSetPreset(url: u)
                finish(signedIn)
            }

        case let .known(u):
            KnownStep(url: u, busy: busy) { pick(u, $0) } onLinked: { all in
                // Signed in through the AlwyWatch account: a Jellyfin login first (it also
                // connects Leaving soon), the others stored as they are.
                try? core.keepSetPreset(url: u)
                let main = all.first { $0.account.kind == .jellyfin } ?? all[0]
                for s in all where s.account.id != main.account.id { Keychain.put(s.account.id, s.token) }
                finish(main)
            } onKnown: { k in
                if k.kind == "plex" {
                    go(.plex)
                } else {
                    attempt {
                        let quick = try? await core.jellyfinQuickConnectStart(url: k.url)
                        go(.jellyfin(url: k.url, quick: quick))
                    }
                }
            } onOther: { go(.direct) }

        case .direct:
            directChoices

        case .jellyfinUrl:
            field(tr("Server address"), text: $url, url: true, placeholder: "jellyfin.example.org")
                .onSubmit(checkJellyfin)
            Text(tr("Your Jellyfin server's address, as you use it in a browser.")).foregroundStyle(Palette.muted).font(.subheadline)
            Button(busy ? tr("Checking…") : tr("Continue"), action: checkJellyfin)
                .buttonStyle(ActionButtonStyle(primary: true))

        case let .jellyfin(u, quick):
            if let quick {
                VStack(alignment: .leading, spacing: 10) {
                    Text(tr("QUICK CONNECT")).font(.caption.weight(.bold)).foregroundStyle(Palette.accent)
                    Text(chunked(quick.code)).font(.system(size: 44, weight: .bold, design: .rounded)).kerning(4).textSelection(.enabled)
                    Text(tr("In a Jellyfin app where you're signed in, open your profile → Quick Connect and enter this code."))
                        .foregroundStyle(Palette.muted)
                    Text(tr("Waiting for approval…")).font(.footnote).foregroundStyle(Palette.subtle)
                }
                .frame(maxWidth: .infinity, alignment: .leading)
                .padding(24)
                .background(Palette.surface, in: RoundedRectangle(cornerRadius: 24))
                .overlay(RoundedRectangle(cornerRadius: 24).stroke(Palette.outline))
                .task(id: quick.secret) { await pollQuickConnect(url: u, secret: quick.secret) }
                Text(tr("Or with your password")).font(.headline).padding(.top, 8)
            }
            field(tr("Username"), text: $username)
            secure(tr("Password"), text: $password).onSubmit { signInJellyfin(u) }
            Button(busy ? tr("Signing in…") : tr("Sign in")) { signInJellyfin(u) }
                .buttonStyle(ActionButtonStyle(primary: true))

        case .plex:
            VStack(alignment: .leading, spacing: 12) {
                if let auth = plexAuthUrl {
                    Text(tr("Sign in on plex.tv in your browser, then come back here.")).font(.headline)
                    Button(tr("Open plex.tv")) { openURL(auth) }
                        .buttonStyle(ActionButtonStyle(primary: true))
                    Text(tr("Or, on any device")).font(.footnote).foregroundStyle(Palette.subtle).padding(.top, 8)
                }
                Text(tr("PLEX.TV/LINK")).font(.caption.weight(.bold)).foregroundStyle(Palette.accent)
                Text(plexCode ?? "…").font(.system(size: 44, weight: .bold, design: .rounded)).textSelection(.enabled)
                Text(tr("Go to plex.tv/link and enter this code.")).foregroundStyle(Palette.muted)
                Text(tr("Waiting for Plex…")).font(.footnote).foregroundStyle(Palette.subtle)
            }
            .frame(maxWidth: .infinity, alignment: .leading)
            .padding(24)
            .background(Palette.surface, in: RoundedRectangle(cornerRadius: 24))
            .overlay(RoundedRectangle(cornerRadius: 24).stroke(Palette.outline))
            .task { await plexSignIn() }

        case let .plexUsers(token, users):
            PlexUsers(token: token, users: users) { userToken in
                attempt { try await plexServersFor(userToken) }
            }

        case let .plexServers(token, servers):
            Text(tr("Choose a server")).font(.headline)
            if servers.isEmpty { Text(tr("This Plex account has no servers.")).foregroundStyle(Palette.muted) }
            ForEach(servers, id: \.machineId) { srv in
                ChoiceCard(title: srv.name, subtitle: srv.owned ? tr("Your server") : tr("Shared with you"), icon: "play.rectangle.on.rectangle") {
                    attempt { finish(try await core.plexConnect(accountToken: token, server: srv, preferredUrl: nil)) }
                }
            }
        }
    }

    @ViewBuilder private var directChoices: some View {
        ChoiceCard(title: "Jellyfin", subtitle: tr("Quick Connect or password"), icon: "play.rectangle.on.rectangle") { go(.jellyfinUrl) }
            // No AlwyWatch server on this path: forget one picked earlier then backed out of.
            .onAppear(perform: clearPreset)
        ChoiceCard(title: "Plex", subtitle: tr("Sign in on plex.tv"), icon: "play.circle") { go(.plex) }
    }

    /// The AlwyWatch address (prefilled), then the ways to sign in there: a Jellyfin or Plex
    /// login on one of its servers, a code, username and password. Creating: the sign-up.
    @ViewBuilder private func alwyWatchPicker(create: Bool) -> some View {
        Text(create
            ? tr("Type the AlwyWatch address the person who runs your server gave you with your invitation.")
            : tr("Type the AlwyWatch address the person who runs your server gave you."))
            .foregroundStyle(Palette.muted).font(.subheadline)
        TextField("watch.example.org", text: $alwyUrl)
            .textInputAutocapitalization(.never)
            .autocorrectionDisabled()
            .keyboardType(.URL)
            .submitLabel(.go)
            .padding(.horizontal, 20)
            .padding(.vertical, 14)
            .background(Palette.surface, in: Capsule())
            .overlay(Capsule().stroke(Palette.outline))
            .onChange(of: alwyUrl) { _ in alwyFound = nil }
            .onSubmit { loadAlwyWatch(create: create) }
        if let found = alwyFound {
            Label(found.list.isEmpty ? tr("AlwyWatch found") : tr("AlwyWatch found · {servers}", ["servers": found.list.map(\.name).joined(separator: ", ")]),
                  systemImage: "checkmark")
                .foregroundStyle(Palette.accent).font(.subheadline)
            if !found.list.isEmpty {
                Text(tr("With your Jellyfin or Plex login")).font(.headline).padding(.top, 8)
                ForEach(found.list, id: \.id) { e in
                    ServerCard(entry: e, existing: existingLogin(e)) { if !busy { pick(found.base, e) } }
                }
            }
            Text(tr("Or")).font(.headline).padding(.top, 8)
            ChoiceCard(title: tr("Sign in with a code"), subtitle: tr("Scan a QR code with your phone, or approve on a device where you're signed in"), icon: "qrcode") {
                go(.receive(url: found.base))
            }
            ChoiceCard(title: tr("Username and password"), subtitle: tr("Your AlwyWatch account's"), icon: "person") {
                go(.accountLogin(url: found.base))
            }
            Text(tr("Passkey or Pocket ID? Choose “Sign in with a code” and approve it on your phone."))
                .foregroundStyle(Palette.subtle).font(.footnote)
        } else {
            Button(checking ? tr("Checking…") : tr("Continue")) { loadAlwyWatch(create: create) }
                .buttonStyle(ActionButtonStyle(primary: true))
        }
    }

    private func loadAlwyWatch(create: Bool) {
        guard !checking else { return }
        error = nil
        checking = true
        UIApplication.shared.sendAction(#selector(UIResponder.resignFirstResponder), to: nil, from: nil, for: nil)
        Task {
            do {
                let base = try normalizeServerUrl(input: alwyUrl)
                let list = try await core.keepServers(url: base)
                // Remembered at once: next time it's already filled in.
                model.lastAlwyWatchUrl = base
                if create { go(.register(url: base)) } else { alwyFound = (base, list) }
            } catch {
                self.error = message(error)
            }
            checking = false
        }
    }

    private func existingLogin(_ e: ServerEntry) -> Account? {
        ((try? core.accounts()) ?? []).first { $0.serverId == e.id }
    }

    /// A media server from an AlwyWatch server's list: sign in there, then connect leaving soon.
    /// A login this device already has there: just use it.
    private func pick(_ base: String, _ entry: ServerEntry) {
        attempt {
            let logins = (try? core.accounts()) ?? []
            if let existing = logins.first(where: { $0.serverId == entry.id && AppModel.keepToken($0) != nil }) {
                try await model.useExistingLogin(url: base, account: existing)
                done(existing.id)
                return
            }
            try core.keepSetPreset(url: base)
            switch entry.kind {
            case .jellyfin:
                _ = try await core.jellyfinServerInfo(url: entry.url)
                let quick = try? await core.jellyfinQuickConnectStart(url: entry.url)
                go(.jellyfin(url: entry.url, quick: quick))
            case .plex:
                presetPlexId = entry.id
                go(.plex)
            }
        }
    }

    private func clearPreset() {
        try? core.keepClearPreset()
        presetPlexId = nil
    }

    private func field(_ label: String, text: Binding<String>, url: Bool = false, placeholder: String? = nil) -> some View {
        VStack(alignment: .leading, spacing: 6) {
            if placeholder != nil { Text(label).font(.footnote).foregroundStyle(Palette.muted) }
            TextField(placeholder ?? label, text: text)
                .textInputAutocapitalization(.never)
                .autocorrectionDisabled()
                .keyboardType(url ? .URL : .default)
                .textContentType(url ? .URL : .username)
                .padding(14)
                .background(Palette.surface, in: RoundedRectangle(cornerRadius: 14))
        }
    }

    private func checkJellyfin() {
        attempt {
            let u = try normalizeServerUrl(input: url)
            _ = try await core.jellyfinServerInfo(url: u)
            let quick = try? await core.jellyfinQuickConnectStart(url: u)
            go(.jellyfin(url: u, quick: quick))
        }
    }

    private func go(_ next: Step) {
        error = nil
        history.append(next)
    }

    private func replaceLast(_ next: Step) {
        error = nil
        history[history.count - 1] = next
    }

    private func back() {
        error = nil
        if history.count > 1 { history.removeLast() } else { dismiss() }
    }

    private func attempt(_ block: @escaping () async throws -> Void) {
        guard !busy else { return }
        busy = true
        error = nil
        Task {
            do { try await block() } catch { self.error = message(error) }
            busy = false
        }
    }

    private func finish(_ s: SignedIn) {
        model.finish(s, plexHomeToken: plexHomeToken)
        dismiss()
    }

    /// A login already stored (copied from another device, or reused): open it.
    private func done(_ accountId: String) {
        model.use(accountId)
        dismiss()
    }

    private func signInJellyfin(_ u: String) {
        attempt { finish(try await core.jellyfinSignIn(url: u, username: username, password: password)) }
    }

    private func pollQuickConnect(url u: String, secret: String) async {
        while !Task.isCancelled {
            try? await Task.sleep(nanoseconds: 3_000_000_000)
            if Task.isCancelled { return }
            do {
                finish(try await core.jellyfinQuickConnectPoll(url: u, secret: secret))
                return
            } catch {
                if isPending(error) { continue }
                // Codes expire after a few minutes: a fresh one instead of giving up.
                if let fresh = try? await core.jellyfinQuickConnectStart(url: u) {
                    replaceLast(.jellyfin(url: u, quick: fresh))
                } else {
                    self.error = message(error)
                }
                return
            }
        }
    }

    private func plexSignIn() async {
        do {
            // A browser sign-in (strong PIN) plus a short code for plex.tv/link on any device.
            // Whichever gets linked first wins.
            let short = try await core.plexCreatePin(strong: false)
            plexCode = short.code
            let long = try? await core.plexCreatePin(strong: true)
            if let auth = long?.authUrl, !auth.isEmpty, let u = URL(string: auth) {
                plexAuthUrl = u
                openURL(u)
            }
            let pins = [short.id] + (long.map { [$0.id] } ?? [])
            while !Task.isCancelled {
                try await Task.sleep(nanoseconds: 2_000_000_000)
                var token: String?
                for id in pins {
                    do {
                        token = try await core.plexCheckPin(pinId: id)
                        error = nil
                        break
                    } catch {
                        // A blip while waiting: say so, keep waiting.
                        if !isPending(error) { self.error = message(error) }
                    }
                }
                guard let token else { continue }
                // Several people on this Plex account: ask who is watching first.
                let users = (try? await core.plexHomeUsers(accountToken: token)) ?? []
                if users.count > 1 {
                    plexHomeToken = token
                    go(.plexUsers(token: token, users: users))
                } else {
                    try await plexServersFor(token)
                }
                return
            }
        } catch {
            if !(error is CancellationError) { self.error = message(error) }
        }
    }

    /// After the Plex sign-in (and the user choice): the server picked earlier, or the list.
    private func plexServersFor(_ token: String) async throws {
        let servers = try await core.plexServers(accountToken: token)
        if let preset = servers.first(where: { $0.machineId == presetPlexId }) {
            finish(try await core.plexConnect(accountToken: token, server: preset, preferredUrl: nil))
        } else {
            go(.plexServers(token: token, servers: servers))
        }
    }
}

extension String {
    /// Without "https://" and the trailing slash, as typed.
    var bare: String {
        var s = self
        if let r = s.range(of: "://") { s = String(s[r.upperBound...]) }
        while s.hasSuffix("/") { s.removeLast() }
        return s
    }
}

/// "123456" → "123 456".
func chunked(_ s: String) -> String {
    stride(from: 0, to: s.count, by: 3).map { i in
        let start = s.index(s.startIndex, offsetBy: i)
        return String(s[start ..< s.index(start, offsetBy: min(3, s.count - i))])
    }.joined(separator: " ")
}

func secure(_ label: String, text: Binding<String>) -> some View {
    SecureField(label, text: text)
        .textContentType(.password)
        .padding(14)
        .background(Palette.surface, in: RoundedRectangle(cornerRadius: 14))
}

/// A server an AlwyWatch server serves.
private struct ServerCard: View {
    let entry: ServerEntry
    var existing: Account?
    let action: () -> Void

    var body: some View {
        let plex = entry.kind == .plex
        let subtitle = existing.map { tr("Use your login {name}", ["name": $0.userName]) }
            ?? (plex ? tr("Plex · sign in on plex.tv") : tr("Jellyfin · Quick Connect or password"))
        ChoiceCard(title: entry.name, subtitle: subtitle, icon: plex ? "play.circle" : "play.rectangle.on.rectangle", action: action)
    }
}

/// Adding a server while connected to an AlwyWatch server: its servers not added yet (if
/// any), then Jellyfin or Plex directly.
private struct ConnectedChoices: View {
    @EnvironmentObject var model: AppModel
    let keepUrl: String
    let onPick: (ServerEntry) -> Void
    let onJellyfin: () -> Void
    let onPlex: () -> Void
    @State private var missing: [ServerEntry]?

    var body: some View {
        Group {
            if let list = missing {
                if !list.isEmpty {
                    Text(tr("From your AlwyWatch server")).font(.headline)
                    ForEach(list, id: \.id) { e in ServerCard(entry: e, action: { onPick(e) }) }
                    Text(tr("Another server")).font(.headline).padding(.top, 8)
                }
            } else {
                Text(tr("Checking your AlwyWatch server…")).foregroundStyle(Palette.subtle).font(.subheadline)
            }
            ChoiceCard(title: "Jellyfin", subtitle: tr("Quick Connect or password"), icon: "play.rectangle.on.rectangle", action: onJellyfin)
            ChoiceCard(title: "Plex", subtitle: tr("Sign in on plex.tv"), icon: "play.circle", action: onPlex)
        }
        .task(id: keepUrl) {
            let core = model.core
            let added = Set(((try? core.accounts()) ?? []).map(\.serverId))
            // Failing (offline) just leaves the direct choices.
            missing = ((try? await core.keepServers(url: keepUrl)) ?? []).filter { !added.contains($0.id) }
        }
    }
}

/// Signed in to an AlwyWatch account: the servers it serves, the ones from the user's other
/// devices, or another one by its address.
private struct KnownStep: View {
    @EnvironmentObject var model: AppModel
    let url: String
    let busy: Bool
    let onPick: (ServerEntry) -> Void
    let onLinked: ([SignedIn]) -> Void
    let onKnown: (KnownServer) -> Void
    let onOther: () -> Void
    @State private var served: [ServerEntry]?
    /// Signed in to the account: its linked servers sign in by themselves first.
    @State private var linking = true

    var body: some View {
        let core = model.core
        let me = core.account()
        let logins = (try? core.accounts()) ?? []
        let known = core.knownServers().filter { k in
            (served ?? []).allSatisfy { $0.url.bare != k.url.bare }
        }
        Group {
            Text(linking && me != nil ? tr("Signing you in to your servers…")
                : me.map { tr("Signed in as {name}. Now sign in to the server you watch on.", ["name": $0.name]) }
                ?? tr("Signed in as you. Now sign in to the server you watch on."))
                .font(.headline)
            ForEach(served ?? [], id: \.id) { e in
                ServerCard(entry: e, existing: logins.first { $0.serverId == e.id }) { if !busy { onPick(e) } }
            }
            ForEach(known, id: \.url) { k in
                ChoiceCard(title: k.name.isEmpty ? k.url : k.name,
                           subtitle: tr("{kind} · from your other devices", ["kind": k.kind == "plex" ? "Plex" : "Jellyfin"]),
                           icon: "play.rectangle.on.rectangle") { onKnown(k) }
            }
            ChoiceCard(title: tr("Another server"), subtitle: tr("Jellyfin or Plex by its address"), icon: "plus", action: onOther)
        }
        .task(id: url) {
            if model.core.account() != nil {
                let all = (try? await model.core.accountSignInLinked()) ?? []
                if !all.isEmpty {
                    onLinked(all)
                    return
                }
            }
            linking = false
            served = (try? await model.core.keepServers(url: url)) ?? []
        }
    }
}

/// An AlwyWatch account's username and password.
private struct AccountLoginStep: View {
    @EnvironmentObject var model: AppModel
    let url: String
    let busy: Bool
    let attempt: (@escaping () async throws -> Void) -> Void
    let onError: (String?) -> Void
    let onDone: () -> Void
    let onNoPassword: () -> Void
    let onJellyfin: (SignedIn) -> Void
    @State private var user = ""
    @State private var pass = ""
    @State private var resetSent = false

    var body: some View {
        Text(tr("Your AlwyWatch account keeps your profile, settings and server list. To watch, you then sign in to your server."))
            .foregroundStyle(Palette.muted).font(.subheadline)
        TextField(tr("Username"), text: $user)
            .textInputAutocapitalization(.never)
            .autocorrectionDisabled()
            .textContentType(.username)
            .padding(14)
            .background(Palette.surface, in: RoundedRectangle(cornerRadius: 14))
        secure(tr("Password"), text: $pass).onSubmit(submit)
        Button(busy ? tr("Signing in…") : tr("Sign in"), action: submit)
            .buttonStyle(ActionButtonStyle(primary: true))
        Button(tr("Forgot password?")) {
            resetSent = false
            if user.trimmingCharacters(in: .whitespaces).isEmpty {
                onError(tr("Type your username or email first, then choose “Forgot password?”."))
            } else {
                attempt {
                    try await model.core.accountForgotPassword(url: url, login: user)
                    resetSent = true
                }
            }
        }
        .buttonStyle(ActionButtonStyle())
        if resetSent {
            Text(tr("If this account has an email address, a link to set a new password is on its way. It works for an hour."))
                .font(.subheadline)
        }
        // Most people have no AlwyWatch password: their Jellyfin or Plex login is the way in.
        Button(tr("I don't have a password"), action: onNoPassword)
            .buttonStyle(ActionButtonStyle())
        Text(tr("Sign in with your Jellyfin or Plex login instead, or with a code approved on your phone (passkey, Pocket ID)."))
            .foregroundStyle(Palette.subtle).font(.footnote)
    }

    private func submit() {
        attempt {
            let core = model.core
            let r = try await core.accountLogin(url: url, username: user, password: pass, device: UIDevice.current.name)
            Keychain.put(accountTokenKey, r.token)
            model.lastAlwyWatchUrl = r.account.baseUrl
            _ = try? await core.syncSettings()
            if let jellyfin = r.jellyfin { onJellyfin(jellyfin) } else { onDone() }
        }
    }
}

/// Creating an AlwyWatch account: what the server allows (an invitation, open, closed), then
/// the invitation code (checked as typed), username, password and email. [onJellyfin] gets
/// the Jellyfin login the invitation made; otherwise [onDone] goes on to "Your servers".
private struct RegisterStep: View {
    @EnvironmentObject var model: AppModel
    let url: String
    let busy: Bool
    let attempt: (@escaping () async throws -> Void) -> Void
    let onJellyfin: (SignedIn) -> Void
    let onDone: () -> Void
    @State private var options: AccountOptions?
    @State private var failed: String?
    @State private var invite = ""
    @State private var user = ""
    @State private var pass = ""
    @State private var email = ""
    @State private var info: InviteInfo?
    @State private var inviteProblem: String?

    /// A pasted link works too: its last part is the code.
    private var code: String {
        var s = invite.trimmingCharacters(in: .whitespacesAndNewlines)
        while s.hasSuffix("/") { s.removeLast() }
        return s.split(separator: "/").last.map(String.init) ?? s
    }

    var body: some View {
        Group {
            if let failed {
                ErrorText(text: failed)
            } else if let o = options {
                if o.registration == "closed" {
                    Text(tr("This AlwyWatch server doesn't take new accounts. Ask the person who runs it.")).foregroundStyle(Palette.muted)
                } else {
                    form(o)
                }
            } else {
                Text(tr("Checking…")).foregroundStyle(Palette.subtle)
            }
        }
        .task(id: url) {
            do { options = try await model.core.accountOptions(url: url) } catch { failed = message(error) }
        }
        .task(id: "\(code)|\(options?.registration ?? "")") { await checkInvite() }
    }

    @ViewBuilder private func form(_ o: AccountOptions) -> some View {
        if let i = info { Text(tr("{name} invited you", ["name": i.inviter])).font(.headline) }
        Text(info?.jellyfin.map { tr("This username and password will also be your {server} (Jellyfin) login.", ["server": $0]) }
            ?? tr("Your AlwyWatch account keeps your name, settings and server list. To watch, you then add your Jellyfin or Plex login."))
            .foregroundStyle(Palette.muted).font(.subheadline)
        if o.registration == "invite" {
            input(tr("Invitation code"), $invite)
            if let p = inviteProblem {
                Text(tr("{problem} Ask the person who invited you for a new one.", ["problem": p]))
                    .foregroundStyle(Palette.danger).font(.footnote)
            }
        }
        input(tr("Username"), $user).textContentType(.username)
        secure(tr("Password"), text: $pass).textContentType(.newPassword)
        input(tr("Email (to reset a forgotten password)"), $email)
            .keyboardType(.emailAddress)
            .textContentType(.emailAddress)
            .onSubmit { submit(o) }
        Button(busy ? tr("Creating…") : tr("Create the account")) { submit(o) }
            .buttonStyle(ActionButtonStyle(primary: true))
        if o.passkeys || o.pocketid {
            Text(tr("Prefer a passkey or Pocket ID? Create the account on {address}, then come back and choose “Sign in with a code”.", ["address": url.bare]))
                .foregroundStyle(Palette.subtle).font(.footnote)
        }
    }

    private func input(_ label: String, _ text: Binding<String>) -> some View {
        TextField(label, text: text)
            .textInputAutocapitalization(.never)
            .autocorrectionDisabled()
            .padding(14)
            .background(Palette.surface, in: RoundedRectangle(cornerRadius: 14))
    }

    private func checkInvite() async {
        info = nil
        inviteProblem = nil
        guard options?.registration == "invite", code.count >= 6 else { return }
        try? await Task.sleep(nanoseconds: 400_000_000)
        if Task.isCancelled { return }
        do {
            let i = try await model.core.accountInviteInfo(url: url, code: code)
            info = i
            // Its address, for "forgot password" (the username is theirs to choose).
            if email.isEmpty, let e = i.email { email = e }
        } catch {
            if error is CancellationError { return }
            // Offline for a moment: the sign-up itself will say.
            if case .Network = error as? CoreError { return }
            inviteProblem = message(error)
        }
    }

    private func submit(_ o: AccountOptions) {
        attempt {
            let core = model.core
            let trimmed = email.trimmingCharacters(in: .whitespaces)
            let r = try await core.accountRegister(
                url: url, username: user, password: pass,
                invite: o.registration == "invite" ? code : nil,
                email: trimmed.isEmpty ? nil : trimmed,
                device: UIDevice.current.name
            )
            Keychain.put(accountTokenKey, r.token)
            model.lastAlwyWatchUrl = r.account.baseUrl
            _ = try? await core.syncSettings()
            // The invitation's Jellyfin login: stored like any Jellyfin sign-in.
            if let jellyfin = r.jellyfin { onJellyfin(jellyfin) } else { onDone() }
        }
    }
}

/// "Which Plex user?" for a Plex Home, with the PIN of protected users.
struct PlexUsers: View {
    @EnvironmentObject var model: AppModel
    let token: String
    let users: [PlexHomeUser]
    let onToken: (String) -> Void
    @State private var chosen: PlexHomeUser?
    @State private var pin = ""
    @State private var error: String?

    var body: some View {
        Text(tr("Which Plex user?")).font(.headline)
        ForEach(users, id: \.uuid) { u in
            let detail = [u.admin ? tr("Account owner") : nil, u.protected ? tr("PIN") : nil].compactMap { $0 }.joined(separator: " · ")
            ChoiceCard(title: u.title, subtitle: detail.isEmpty ? "Plex Home" : detail, icon: "person.crop.circle") {
                if u.protected { chosen = u } else { switchTo(u, pin: nil) }
            }
        }
        if let u = chosen {
            SecureField(tr("PIN for {name}", ["name": u.title]), text: $pin)
                .keyboardType(.numberPad)
                .padding(14)
                .background(Palette.surface, in: RoundedRectangle(cornerRadius: 14))
            Button(tr("Continue")) { switchTo(u, pin: pin) }.buttonStyle(ActionButtonStyle(primary: true))
        }
        ErrorText(text: error)
    }

    private func switchTo(_ u: PlexHomeUser, pin: String?) {
        Task {
            do {
                onToken(try await model.core.plexSwitchUser(accountToken: token, uuid: u.uuid, pin: pin))
            } catch {
                self.error = tr("Wrong PIN.")
            }
        }
    }
}

/// New device: a code (and its QR code) to approve on a signed-in device, which then sends
/// its accounts end-to-end encrypted (Android's ReceiveFromDevice).
struct ReceiveCode: View {
    @EnvironmentObject var model: AppModel
    let url: String
    let onDone: (String?) -> Void
    @State private var code: TransferCode?
    @State private var error: String?

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            Text(tr("SIGN IN WITH A CODE")).font(.caption.weight(.bold)).foregroundStyle(Palette.accent)
            let page = code.map { quickConnectUrl(url: url, code: $0.code) }
            HStack(spacing: 20) {
                if let page { QrImage(text: page).frame(width: 150, height: 150) }
                VStack(alignment: .leading, spacing: 6) {
                    Text(code.map { chunked($0.code) } ?? "…").font(.system(size: 38, weight: .bold, design: .rounded))
                    Text(tr("Scan with your phone, or open")).foregroundStyle(Palette.muted).font(.subheadline)
                    if let page { Text(page.replacingOccurrences(of: "https://", with: "")).font(.subheadline.weight(.semibold)) }
                }
            }
            Text(tr("Or on a device where you're signed in: Settings → This device → Sign in another device, and type this code."))
                .foregroundStyle(Palette.muted).font(.subheadline)
            if let code { Text(tr("Check number: {number}. It must match if the other device shows one.", ["number": code.check])).foregroundStyle(Palette.subtle) }
            Text(tr("Waiting for the other device…")).font(.footnote).foregroundStyle(Palette.subtle)
            ErrorText(text: error)
        }
        .task(id: url) { await run() }
    }

    private func run() async {
        let core = model.core
        while !Task.isCancelled {
            do {
                let c = try await core.transferStart(url: url, device: UIDevice.current.name)
                code = c
                error = nil
                let until = Date().addingTimeInterval(TimeInterval(c.expiresIn))
                while Date() < until {
                    try await Task.sleep(nanoseconds: 2_000_000_000)
                    guard let got = try? await core.transferReceive(url: url, handle: c.handle) else { continue }
                    if let token = got.accountToken {
                        Keychain.put(accountTokenKey, token)
                        _ = try? await core.accountUseToken(url: url, token: token)
                    }
                    let account = got.bundle.flatMap { restoreBundle(core, $0) }
                    AppModel.activate(core)
                    if let account {
                        onDone(account)
                    } else if got.accountToken != nil {
                        _ = try? await core.syncSettings()
                        onDone(nil)
                    } else {
                        error = tr("Nothing to copy: that device has no accounts.")
                    }
                    return
                }
            } catch {
                if error is CancellationError { return }
                self.error = message(error)
                try? await Task.sleep(nanoseconds: 5_000_000_000)
            }
        }
    }
}

/// New device: stores what arrived (accounts, settings, tokens). Returns the account to open.
func restoreBundle(_ core: AlwyWatch, _ json: String) -> String? {
    guard let data = json.data(using: .utf8),
          let o = try? JSONSerialization.jsonObject(with: data) as? [String: Any],
          let coreJson = o["core"] as? String,
          let ids = try? core.importConfig(json: coreJson) else { return nil }
    for (k, v) in (o["tokens"] as? [String: String]) ?? [:] { Keychain.put(k, v) }
    let current = (try? core.getSetting(key: "current_account")) ?? nil
    if let current, ids.contains(current) { return current }
    return ids.first
}

/// A QR code drawn from the core's modules (dark on white, with a quiet zone).
struct QrImage: View {
    let text: String
    var body: some View {
        if let qr = try? qrCode(text: text) {
            Canvas { ctx, size in
                let n = Int(qr.size)
                let cell = min(size.width, size.height) / CGFloat(n)
                for y in 0 ..< n {
                    for x in 0 ..< n where qr.modules[y * n + x] {
                        ctx.fill(Path(CGRect(x: CGFloat(x) * cell, y: CGFloat(y) * cell, width: cell + 0.5, height: cell + 0.5)), with: .color(.black))
                    }
                }
            }
            .padding(10)
            .background(Color.white, in: RoundedRectangle(cornerRadius: 12))
        }
    }
}

/// Settings → Servers → a Plex login → switch to another user of its Plex Home (Android's
/// PlexSwitchScreen): the same server, signed in as that user.
struct PlexSwitchSheet: View {
    @EnvironmentObject var model: AppModel
    @Environment(\.dismiss) private var dismiss
    let account: Account
    @State private var users: [PlexHomeUser]?
    @State private var error: String?
    @State private var busy = false

    /// The Plex Home's own token (kept at sign-in), else this user's: both can list and switch.
    private var homeToken: String? { Keychain.get("\(account.id):plexhome") ?? Keychain.get("\(account.id):plextv") }

    var body: some View {
        NavigationStack {
            ScrollView {
                VStack(alignment: .leading, spacing: 14) {
                    Text(tr("{name} on {server}", ["name": account.userName, "server": account.serverName])).foregroundStyle(Palette.muted)
                    if users == nil, error == nil {
                        Text(tr("Loading…")).foregroundStyle(Palette.subtle)
                    } else if let list = users, list.count < 2 {
                        Text(tr("This Plex account has no other users (it is not a Plex Home).")).foregroundStyle(Palette.muted)
                    } else if let list = users, let token = homeToken {
                        PlexUsers(token: token, users: list) { userToken in
                            guard !busy else { return }
                            Task { await switchTo(userToken, home: token) }
                        }
                    }
                    ErrorText(text: error)
                }
                .padding(20)
                .frame(maxWidth: .infinity, alignment: .leading)
            }
            .background(Palette.background.ignoresSafeArea())
            .navigationTitle(tr("Switch Plex user"))
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) { Button(tr("Cancel")) { dismiss() } }
            }
        }
        .task {
            guard let token = homeToken else {
                error = tr("Sign in to Plex again to switch user.")
                return
            }
            do {
                users = try await model.core.plexHomeUsers(accountToken: token)
            } catch {
                self.error = message(error)
            }
        }
    }

    private func switchTo(_ userToken: String, home: String) async {
        busy = true
        defer { busy = false }
        let core = model.core
        let old = account
        do {
            guard let server = try await core.plexServers(accountToken: userToken).first(where: { $0.machineId == old.serverId }) else {
                throw CoreError.Invalid(message: tr("This user has no access to {server}.", ["server": old.serverName]))
            }
            let s = try await core.plexConnect(accountToken: userToken, server: server, preferredUrl: nil)
            Keychain.put(s.account.id, s.token)
            if let t = s.plexAccountToken { Keychain.put("\(s.account.id):plextv", t) }
            Keychain.put("\(s.account.id):plexhome", home)
            core.setPlexAccountToken(accountId: s.account.id, token: home)
            // The AlwyWatch server followed the old user: follow the new one.
            if let keep = core.keepConnection(), keep.accountId == old.id {
                _ = try? await core.keepConnect(url: keep.baseUrl, accountId: s.account.id, token: s.plexAccountToken ?? s.token)
            }
            if s.account.id != old.id {
                try? core.removeAccount(accountId: old.id)
                for k in [old.id, "\(old.id):plextv", "\(old.id):plexhome"] { Keychain.remove(k) }
            }
            model.use(s.account.id)
            dismiss()
        } catch {
            self.error = message(error)
        }
    }
}
