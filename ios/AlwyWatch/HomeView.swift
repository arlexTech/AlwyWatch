// SPDX-License-Identifier: AGPL-3.0-or-later
import SwiftUI

/// The home (Android's HomeScreen, phone layout): continue watching, the libraries, then what
/// was added recently in each. Rendered from the core's cache, then refreshed.
struct HomeView: View {
    @EnvironmentObject var model: AppModel
    @State private var home: Home?
    @State private var error: String?
    @State private var player: PlayRequest?
    /// Downloads mode: the home made of what is on this phone (the top bar toggles it).
    @State private var offline = false

    var body: some View {
        let account = model.account ?? ""
        ScrollView {
            if offline {
                DownloadsHomeContent(accounts: Set(model.listAccounts()))
                    .padding(.vertical, 12)
            } else {
                online(account)
            }
        }
        .background(Palette.background.ignoresSafeArea())
        .safeAreaInset(edge: .top, spacing: 0) {
            HomeTopBar(offline: offline) {
                offline.toggle()
                try? model.core.setSetting(key: "ui.downloads_mode", value: offline ? "1" : "0")
            }
        }
        .toolbar(.hidden, for: .navigationBar)
        .refreshable { await refresh(force: true) }
        .onAppear { offline = model.core.getSettingOr("ui.downloads_mode", "0") == "1" }
        .task(id: account) {
            home = (try? model.core.cachedHome(accountId: account)) ?? nil
            await refresh(force: false)
        }
        .onChange(of: player?.id) { _ in
            guard let request = player else { return }
            player = nil
            PlayerPresenter.shared.play(request, core: model.core) { Task { await refresh(force: true) } }
        }
    }

    private func online(_ account: String) -> some View {
            LazyVStack(alignment: .leading, spacing: 26) {
                if let home {
                    if !home.continueWatching.isEmpty {
                        VStack(alignment: .leading, spacing: 10) {
                            SectionTitle(title: tr("Continue watching"), seeAll: .continueWatching(account: account))
                            ScrollView(.horizontal, showsIndicators: false) {
                                LazyHStack(alignment: .top, spacing: 12) {
                                    ForEach(home.continueWatching) { item in
                                        WideCard(account: account, item: item)
                                            .onTapGesture { player = PlayRequest(account: account, item: item) }
                                            .contextMenu { ItemMenu(account: account, item: item) { Task { await refresh(force: true) } } }
                                    }
                                }
                                .padding(.horizontal, 16)
                            }
                        }
                    }
                    // After what the user is watching, before the "new" rows (as on Android):
                    // invitations to watch together waiting for an answer, then Leaving soon.
                    if model.core.keepActive() { TogetherRail() }
                    LeavingSoonRail()
                    if !home.libraries.isEmpty {
                        ScrollView(.horizontal, showsIndicators: false) {
                            HStack(spacing: 8) {
                                ForEach(home.libraries, id: \.id) { lib in
                                    NavigationLink(value: Route.library(account: account, id: lib.id, title: lib.title)) {
                                        Text(lib.title)
                                            .font(.subheadline.weight(.semibold))
                                            .padding(.horizontal, 16)
                                            .padding(.vertical, 9)
                                            .background(Palette.surfaceHigh, in: Capsule())
                                    }
                                    .buttonStyle(.plain)
                                }
                            }
                            .padding(.horizontal, 16)
                        }
                    }
                    ForEach(home.latest, id: \.library.id) { rail in
                        if !rail.items.isEmpty {
                            VStack(alignment: .leading, spacing: 10) {
                                SectionTitle(
                                    title: tr("Recently added in {library}", ["library": rail.library.title]),
                                    seeAll: .library(account: account, id: rail.library.id, title: rail.library.title, recent: true)
                                )
                                ScrollView(.horizontal, showsIndicators: false) {
                                    LazyHStack(alignment: .top, spacing: 12) {
                                        ForEach(rail.items) { item in
                                            let note = rail.notes.first { $0.itemId == item.id }
                                            NavigationLink(value: Route.detail(account: account, itemId: note?.openId ?? item.id)) {
                                                RailCard(account: account, item: item, note: note?.label)
                                            }
                                            .buttonStyle(.plain)
                                        }
                                    }
                                    .padding(.horizontal, 16)
                                }
                            }
                        }
                    }
                } else if error == nil {
                    Color.clear.frame(height: 300)
                }
                ErrorText(text: error).padding(.horizontal, 16)
            }
            .padding(.vertical, 12)
    }

    private func refresh(force: Bool) async {
        guard let account = model.account else { return }
        do {
            home = try await model.core.refreshHome(accountId: account, force: force)
            error = nil
        } catch {
            if home == nil { self.error = message(error) }
        }
    }
}

/// A poster with the rail's note for shows ("S1 · E6 · Edenia", "New show").
private struct RailCard: View {
    let account: String
    let item: Item
    let note: String?

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            PosterCard(account: account, item: item)
            if let note {
                Text(note).font(.caption2.weight(.semibold)).foregroundStyle(Palette.accent).lineLimit(1).frame(width: 120, alignment: .leading)
            }
        }
    }
}

/// Long-press actions on a card: mark watched / unwatched.
struct ItemMenu: View {
    @EnvironmentObject var model: AppModel
    let account: String
    let item: Item
    let changed: () -> Void

    var body: some View {
        Button {
            Task {
                try? await model.core.setPlayed(accountId: account, itemId: item.id, played: !item.user.played)
                changed()
            }
        } label: {
            if item.user.played {
                Label(tr("Mark unwatched"), systemImage: "eye.slash")
            } else {
                Label(tr("Mark watched"), systemImage: "checkmark")
            }
        }
    }
}

/// Android's home bar: the name on the left; search and you (settings) on the right.
private struct HomeTopBar: View {
    @EnvironmentObject var model: AppModel
    let offline: Bool
    let toggleOffline: () -> Void

    var body: some View {
        HStack(spacing: 4) {
            (Text("alwy") + Text("watch").foregroundColor(Palette.accent))
                .font(.system(size: 28, weight: .heavy))
            Spacer()
            NavigationLink(value: Route.watchlist) {
                Image(systemName: "bookmark").font(.title3).frame(width: 44, height: 44)
            }
            .accessibilityLabel(tr("My list"))
            NavigationLink(value: Route.search) {
                Image(systemName: "magnifyingglass").font(.title3).frame(width: 44, height: 44)
            }
            .accessibilityLabel(tr("Search"))
            Button(action: toggleOffline) {
                Image(systemName: offline ? "arrow.down.circle.fill" : "arrow.down.circle")
                    .font(.title3)
                    .foregroundStyle(offline ? Palette.accent : Palette.onSurface)
                    .frame(width: 44, height: 44)
            }
            .accessibilityLabel(offline ? tr("Leave downloads") : tr("Downloads"))
            NavigationLink(value: Route.settings) {
                Text(String(model.current?.userName.prefix(1) ?? "?").uppercased())
                    .font(.headline)
                    .foregroundStyle(Color.black)
                    .frame(width: 38, height: 38)
                    .background(LinearGradient(colors: [Palette.accent, Color(red: 1, green: 0x6A / 255, blue: 0x3D / 255)], startPoint: .topLeading, endPoint: .bottomTrailing), in: Circle())
            }
            .accessibilityLabel(tr("Settings"))
            .padding(.leading, 6)
        }
        .foregroundStyle(Palette.onSurface)
        .padding(.leading, 16)
        .padding(.trailing, 10)
        .padding(.vertical, 6)
        .background(Palette.background)
    }
}
