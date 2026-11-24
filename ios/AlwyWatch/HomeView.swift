// SPDX-License-Identifier: AGPL-3.0-or-later
import SwiftUI

/// The home (Android's HomeScreen, phone layout): continue watching, the libraries, then what
/// was added recently in each. Rendered from the core's cache, then refreshed.
struct HomeView: View {
    @EnvironmentObject var model: AppModel
    @State private var home: Home?
    @State private var error: String?
    @State private var player: PlayRequest?

    var body: some View {
        let account = model.account ?? ""
        ScrollView {
            LazyVStack(alignment: .leading, spacing: 26) {
                if let home {
                    if !home.continueWatching.isEmpty {
                        VStack(alignment: .leading, spacing: 10) {
                            SectionTitle(title: tr("Continue watching"))
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
                                    seeAll: .library(account: account, id: rail.library.id, title: rail.library.title)
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
        .background(Palette.background.ignoresSafeArea())
        .navigationTitle(model.current?.serverName ?? "AlwyWatch")
        .refreshable { await refresh(force: true) }
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
