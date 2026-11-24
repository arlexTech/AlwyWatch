// SPDX-License-Identifier: AGPL-3.0-or-later
import SwiftUI
import UIKit

/// Adding a server (Android's AddServerScreen): a code approved on a signed-in device (copies
/// its accounts), Jellyfin (Quick Connect or password) or Plex (plex.tv PIN).
struct SignInView: View {
    @EnvironmentObject var model: AppModel
    @Environment(\.dismiss) private var dismiss
    @Environment(\.openURL) private var openURL

    private enum Step {
        case start
        case code
        case jellyfinUrl
        case jellyfin(url: String, quick: QuickConnectCode?)
        case plex
        case plexUsers(token: String, users: [PlexHomeUser])
        case plexServers(token: String, servers: [PlexServerResource])
    }

    @State private var step = Step.start
    @State private var history: [Step] = []
    @State private var busy = false
    @State private var error: String?
    @State private var url = ""
    @State private var keepUrl = ""
    @State private var username = ""
    @State private var password = ""
    @State private var plexCode: String?
    @State private var plexHomeToken: String?

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 18) {
                header
                content
                ErrorText(text: error)
            }
            .padding(24)
            .frame(maxWidth: 600)
            .frame(maxWidth: .infinity)
        }
        .background(Palette.background.ignoresSafeArea())
        .scrollDismissesKeyboard(.interactively)
        .toolbar {
            if !history.isEmpty {
                ToolbarItem(placement: .navigationBarLeading) {
                    Button { back() } label: { Image(systemName: "chevron.left") }
                        .accessibilityLabel(tr("Back"))
                }
            }
        }
        .navigationBarBackButtonHidden(!history.isEmpty)
    }

    @ViewBuilder private var header: some View {
        switch step {
        case .start:
            Text(tr("Welcome to AlwyWatch")).font(.largeTitle.weight(.bold))
            Text(tr("Your Jellyfin and Plex, fast and beautiful.")).foregroundStyle(Palette.muted)
        case .code:
            Text(tr("Sign in with a code")).font(.title.weight(.bold))
        case .jellyfinUrl, .jellyfin:
            Text("Jellyfin").font(.title.weight(.bold))
        case .plex, .plexUsers, .plexServers:
            Text("Plex").font(.title.weight(.bold))
        }
    }

    @ViewBuilder private var content: some View {
        switch step {
        case .start:
            ChoiceCard(title: tr("Sign in with a code"), subtitle: tr("Approve on a device where you're signed in"), icon: "qrcode") { go(.code) }
            ChoiceCard(title: "Jellyfin", subtitle: tr("Quick Connect or password"), icon: "play.rectangle.on.rectangle") { go(.jellyfinUrl) }
            ChoiceCard(title: "Plex", subtitle: tr("Sign in on plex.tv"), icon: "play.circle") { go(.plex) }

        case .code:
            if keepUrlChosen == nil {
                field(tr("AlwyWatch address"), text: $keepUrl, url: true)
                Text(tr("Type the AlwyWatch address the person who runs your server gave you.")).foregroundStyle(Palette.muted).font(.subheadline)
                Button(tr("Continue")) { keepUrlChosen = normalized(keepUrl) }
                    .buttonStyle(ActionButtonStyle(primary: true))
            } else if let chosen = keepUrlChosen {
                ReceiveCode(url: chosen) { account in
                    if let account { model.use(account) } else { model.reload() }
                    dismiss()
                }
            }

        case .jellyfinUrl:
            field(tr("Server address"), text: $url, url: true)
            Text(tr("Your Jellyfin server's address, as you use it in a browser.")).foregroundStyle(Palette.muted).font(.subheadline)
            Button(busy ? tr("Checking…") : tr("Continue")) {
                attempt {
                    let u = normalized(url)
                    _ = try await model.core.jellyfinServerInfo(url: u)
                    let quick = try? await model.core.jellyfinQuickConnectStart(url: u)
                    go(.jellyfin(url: u, quick: quick))
                }
            }
            .buttonStyle(ActionButtonStyle(primary: true))

        case let .jellyfin(u, quick):
            if let quick {
                VStack(alignment: .leading, spacing: 10) {
                    Text(tr("QUICK CONNECT")).font(.caption.weight(.bold)).foregroundStyle(Palette.accent)
                    Text(quick.code).font(.system(size: 44, weight: .bold, design: .rounded)).textSelection(.enabled)
                    Text(tr("In a Jellyfin app where you're signed in, open your profile → Quick Connect and enter this code."))
                        .foregroundStyle(Palette.muted)
                    Text(tr("Waiting for approval…")).font(.footnote).foregroundStyle(Palette.subtle)
                }
                .padding(20)
                .background(Palette.surface, in: RoundedRectangle(cornerRadius: 24))
                .task(id: quick.secret) { await pollQuickConnect(url: u, secret: quick.secret) }
                Text(tr("Or with your password")).font(.headline).padding(.top, 8)
            }
            field(tr("Username"), text: $username)
            SecureField(tr("Password"), text: $password)
                .textContentType(.password)
                .padding(14)
                .background(Palette.surface, in: RoundedRectangle(cornerRadius: 14))
                .onSubmit { signInJellyfin(u) }
            Button(busy ? tr("Signing in…") : tr("Sign in")) { signInJellyfin(u) }
                .buttonStyle(ActionButtonStyle(primary: true))

        case .plex:
            VStack(alignment: .leading, spacing: 12) {
                Text(tr("Sign in on plex.tv in your browser, then come back here.")).font(.headline)
                Button(tr("Open plex.tv")) { if let u = plexAuthUrl { openURL(u) } }
                    .buttonStyle(ActionButtonStyle(primary: true))
                    .disabled(plexAuthUrl == nil)
                Text(tr("Or, on any device")).font(.footnote).foregroundStyle(Palette.subtle).padding(.top, 8)
                Text(tr("PLEX.TV/LINK")).font(.caption.weight(.bold)).foregroundStyle(Palette.accent)
                Text(plexCode ?? "…").font(.system(size: 44, weight: .bold, design: .rounded)).textSelection(.enabled)
                Text(tr("Go to plex.tv/link and enter this code.")).foregroundStyle(Palette.muted)
                Text(tr("Waiting for Plex…")).font(.footnote).foregroundStyle(Palette.subtle)
            }
            .padding(20)
            .background(Palette.surface, in: RoundedRectangle(cornerRadius: 24))
            .task { await plexSignIn() }

        case let .plexUsers(token, users):
            PlexUsers(token: token, users: users) { userToken in
                plexHomeToken = token
                attempt { try await plexServers(userToken) }
            }

        case let .plexServers(token, servers):
            Text(tr("Choose a server")).font(.headline)
            if servers.isEmpty { Text(tr("This Plex account has no servers.")).foregroundStyle(Palette.muted) }
            ForEach(servers, id: \.machineId) { srv in
                ChoiceCard(title: srv.name, subtitle: srv.owned ? tr("Your server") : tr("Shared with you"), icon: "server.rack") {
                    attempt { finish(try await model.core.plexConnect(accountToken: token, server: srv, preferredUrl: nil)) }
                }
            }
        }
    }

    @State private var keepUrlChosen: String?
    @State private var plexAuthUrl: URL?

    private func field(_ label: String, text: Binding<String>, url: Bool = false) -> some View {
        TextField(label, text: text)
            .textInputAutocapitalization(.never)
            .autocorrectionDisabled()
            .keyboardType(url ? .URL : .default)
            .textContentType(url ? .URL : .username)
            .padding(14)
            .background(Palette.surface, in: RoundedRectangle(cornerRadius: 14))
    }

    private func normalized(_ s: String) -> String {
        var u = s.trimmingCharacters(in: .whitespacesAndNewlines)
        if !u.contains("://") { u = "https://" + u }
        while u.hasSuffix("/") { u.removeLast() }
        return u
    }

    private func go(_ next: Step) {
        history.append(step)
        error = nil
        step = next
    }

    private func back() {
        error = nil
        if case .code = step, keepUrlChosen != nil { keepUrlChosen = nil; return }
        if let last = history.popLast() { step = last }
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

    private func signInJellyfin(_ u: String) {
        attempt { finish(try await model.core.jellyfinSignIn(url: u, username: username, password: password)) }
    }

    private func pollQuickConnect(url u: String, secret: String) async {
        while !Task.isCancelled {
            try? await Task.sleep(nanoseconds: 2_000_000_000)
            do {
                finish(try await model.core.jellyfinQuickConnectPoll(url: u, secret: secret))
                return
            } catch {
                if !isPending(error) { self.error = message(error) }
            }
        }
    }

    private func plexSignIn() async {
        do {
            // The short code for plex.tv/link, and a strong PIN for the browser sign-in.
            let short = try await model.core.plexCreatePin(strong: false)
            plexCode = short.code
            let long = try? await model.core.plexCreatePin(strong: true)
            plexAuthUrl = URL(string: long?.authUrl ?? short.authUrl)
            let pins = [short.id] + (long.map { [$0.id] } ?? [])
            while !Task.isCancelled {
                try await Task.sleep(nanoseconds: 2_000_000_000)
                var token: String?
                for id in pins {
                    do {
                        token = try await model.core.plexCheckPin(pinId: id)
                        error = nil
                        break
                    } catch {
                        if !isPending(error) { self.error = message(error) }
                    }
                }
                guard let token else { continue }
                let users = (try? await model.core.plexHomeUsers(accountToken: token)) ?? []
                if users.count > 1 {
                    go(.plexUsers(token: token, users: users))
                } else {
                    try await plexServers(token)
                }
                return
            }
        } catch {
            if !(error is CancellationError) { self.error = message(error) }
        }
    }

    private func plexServers(_ token: String) async throws {
        let servers = try await model.core.plexServers(accountToken: token)
        go(.plexServers(token: token, servers: servers))
    }
}

/// "Which Plex user?" for a Plex Home, with the PIN of protected users.
private struct PlexUsers: View {
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

    private func chunked(_ s: String) -> String {
        stride(from: 0, to: s.count, by: 3).map { i in
            let start = s.index(s.startIndex, offsetBy: i)
            return String(s[start ..< s.index(start, offsetBy: min(3, s.count - i))])
        }.joined(separator: " ")
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
