// SPDX-License-Identifier: AGPL-3.0-or-later
import SwiftUI

/// One search for everything (Android's SearchScreen): your libraries first (every server
/// shown together, one card per title), then, with Seerr, what you could request. Searches as
/// you type; All / On my servers / Not on my servers.
struct SearchView: View {
    @EnvironmentObject var model: AppModel
    @State private var query = ""
    @State private var results: [OwnedItem] = []
    @State private var requestable: [SeerrResult] = []
    @State private var error: String?
    @State private var seerrError: String?
    @State private var filter = "all"
    @State private var searched = ""

    var body: some View {
        let seerrOn = model.core.seerrActive()
        let showMine = filter != "other"
        let showOther = filter != "mine"
        let long = query.trimmingCharacters(in: .whitespaces).count >= 2
        ScrollView {
            LazyVStack(alignment: .leading, spacing: 16) {
                if long {
                    ScrollView(.horizontal, showsIndicators: false) {
                        HStack(spacing: 8) {
                            ForEach(["all", "mine", "other"], id: \.self) { key in
                                Button(filterName(key)) { filter = key }
                                    .buttonStyle(ChipStyle(selected: filter == key))
                            }
                        }
                    }
                }
                if let error { Text(error).foregroundStyle(Palette.muted) }
                if showMine, !results.isEmpty {
                    Text(tr("On your servers")).font(.headline)
                    grid {
                        ForEach(results) { e in
                            NavigationLink(value: Route.detail(account: e.account, itemId: e.item.id)) {
                                GridPoster(account: e.account, item: e.item)
                            }
                            .buttonStyle(.plain)
                            .contextMenu { ItemMenu(account: e.account, item: e.item) {} }
                        }
                    }
                }
                if showOther, long, !seerrOn {
                    Text(tr("To also find what is not on your servers (and request it), connect requests in Settings."))
                        .font(.footnote).foregroundStyle(Palette.muted)
                }
                if showOther, let seerrError {
                    Text(tr("Requests unavailable: {reason}", ["reason": seerrError])).font(.footnote).foregroundStyle(Palette.muted)
                }
                if showOther, !requestable.isEmpty {
                    VStack(alignment: .leading, spacing: 2) {
                        Text(tr("Not on your servers")).font(.headline)
                        Text(tr("Open one to see it and request it")).font(.footnote).foregroundStyle(Palette.muted)
                    }
                    .padding(.top, 8)
                    grid {
                        ForEach(requestable) { r in
                            NavigationLink(value: Route.seerrTitle(type: r.mediaType == .tv ? "tv" : "movie", tmdbId: r.tmdbId)) {
                                SeerrPoster(r: r)
                            }
                            .buttonStyle(.plain)
                        }
                    }
                }
                if long, searched == query, (!showMine || results.isEmpty), (!showOther || requestable.isEmpty), error == nil {
                    Text(tr("No results for “{query}”.", ["query": query])).foregroundStyle(Palette.muted)
                }
            }
            .padding(16)
        }
        .background(Palette.background.ignoresSafeArea())
        .navigationTitle(tr("Search"))
        .searchable(text: $query, placement: .navigationBarDrawer(displayMode: .always), prompt: tr("Movies, shows, episodes…"))
        .autocorrectionDisabled()
        .task(id: query) { await search() }
    }

    private func grid<Content: View>(@ViewBuilder _ content: () -> Content) -> some View {
        LazyVGrid(columns: [GridItem(.adaptive(minimum: 104), spacing: 12, alignment: .top)], alignment: .leading, spacing: 18, content: content)
    }

    private func filterName(_ key: String) -> String {
        switch key {
        case "mine": return tr("On my servers")
        case "other": return tr("Not on my servers")
        default: return tr("All")
        }
    }

    private func search() async {
        let q = query.trimmingCharacters(in: .whitespaces)
        guard q.count >= 2 else {
            results = []
            requestable = []
            error = nil
            return
        }
        // Typing: wait for a pause before asking the servers.
        try? await Task.sleep(nanoseconds: 300_000_000)
        if Task.isCancelled { return }
        var found: [OwnedItem] = []
        var failed: String?
        // Preferred server first: its copy wins when a title is on several.
        for a in model.listAccounts() {
            do {
                found += try await model.core.search(accountId: a, query: q, limit: 60).map { OwnedItem(account: a, item: $0) }
            } catch {
                if error is CancellationError { return }
                failed = message(error)
            }
        }
        if Task.isCancelled { return }
        var seen = Set<String>()
        results = found.filter { seen.insert(sameAs(item: $0.item)).inserted }
        error = found.isEmpty ? failed : nil
        if model.core.seerrActive() {
            do {
                // Everything that exists, minus what the library results already show.
                let have = Set(results.compactMap { $0.item.providerIds.tmdb })
                let norm = { (t: String) in t.lowercased().filter { $0.isLetter || $0.isNumber } }
                let names = Set(results.map { norm($0.item.title) })
                let all = try await model.core.seerrSearch(query: q)
                requestable = Array(all.filter { !have.contains(String($0.tmdbId)) && !names.contains(norm($0.title)) }.prefix(30))
                seerrError = nil
            } catch {
                if error is CancellationError { return }
                requestable = []
                seerrError = message(error)
            }
        }
        searched = query
    }
}
