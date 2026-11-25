// SPDX-License-Identifier: AGPL-3.0-or-later
import SwiftUI

/// Setting: the home mixes every server (Android's COMBINED_KEY); "0" is off.
let combinedKey = "ui.combined"
/// Setting: the account whose copy wins when a title is on several servers.
let preferredKey = "ui.preferred_account"

extension AppModel {
    /// The accounts a list draws from: every one, preferred first, when they are shown
    /// together; else the current one (Android's watchlist rule).
    func listAccounts() -> [String] {
        let all = accounts.map(\.id)
        let together = all.count > 1 && ((try? core.getSetting(key: combinedKey)) ?? nil) != "0"
        guard together else { return account.map { [$0] } ?? [] }
        let pref = (try? core.getSetting(key: preferredKey)) ?? nil
        return all.filter { $0 == pref } + all.filter { $0 != pref }
    }
}

/// One card of a grid: the item and the account it comes from.
struct OwnedItem: Identifiable {
    let account: String
    let item: Item
    var id: String { "\(account)/\(item.id)" }
}

/// A grid of posters that open their title (My list, Continue watching).
struct PosterGrid: View {
    let title: String
    let entries: [OwnedItem]
    var note: String?

    var body: some View {
        ScrollView {
            LazyVGrid(columns: [GridItem(.adaptive(minimum: 104), spacing: 12, alignment: .top)], alignment: .leading, spacing: 20) {
                ForEach(entries) { e in
                    NavigationLink(value: Route.detail(account: e.account, itemId: e.item.id)) {
                        PosterCard(account: e.account, item: e.item, width: 104)
                    }
                    .buttonStyle(.plain)
                    .contextMenu { ItemMenu(account: e.account, item: e.item) {} }
                }
            }
            .padding(16)
            if let note {
                Text(note).font(.subheadline).foregroundStyle(Palette.muted).padding(.horizontal, 16)
            }
        }
        .background(Palette.background.ignoresSafeArea())
        .navigationTitle(title)
    }
}

/// "My list" (Android's WatchlistScreen): what was saved to watch later, from every server
/// shown (plex.tv watchlist, Jellyfin favorites). Cached first, then refreshed.
struct WatchlistView: View {
    @EnvironmentObject var model: AppModel
    @State private var entries: [OwnedItem] = []
    @State private var loaded = false
    @State private var error: String?

    var body: some View {
        PosterGrid(
            title: tr("My list"),
            entries: entries,
            note: error ?? (loaded && entries.isEmpty ? tr("Nothing saved yet. On a movie or show, press the bookmark to keep it here.") : nil)
        )
        .refreshable { await load(force: true) }
        .task { await load(force: false) }
    }

    private func combine(_ lists: [(String, [Item])]) -> [OwnedItem] {
        var seen = Set<String>()
        var out: [OwnedItem] = []
        for (a, items) in lists {
            for i in items where seen.insert(sameAs(item: i)).inserted { out.append(OwnedItem(account: a, item: i)) }
        }
        return out
    }

    private func load(force: Bool) async {
        let accounts = model.listAccounts()
        if !loaded {
            entries = combine(accounts.map { ($0, model.core.cachedWatchlist(accountId: $0)?.items ?? []) })
        }
        var lists: [(String, [Item])] = []
        var failed: String?
        for a in accounts {
            do {
                lists.append((a, try await model.core.refreshWatchlist(accountId: a, force: force).items))
            } catch {
                failed = message(error)
                lists.append((a, model.core.cachedWatchlist(accountId: a)?.items ?? []))
            }
        }
        entries = combine(lists)
        error = entries.isEmpty ? failed : nil
        loaded = true
    }
}

/// Everything in Continue watching, as a grid (the row's See all).
struct ContinueWatchingView: View {
    @EnvironmentObject var model: AppModel
    let account: String

    var body: some View {
        let items = ((try? model.core.cachedHome(accountId: account)) ?? nil)?.continueWatching ?? []
        PosterGrid(title: tr("Continue watching"), entries: items.map { OwnedItem(account: account, item: $0) })
    }
}
