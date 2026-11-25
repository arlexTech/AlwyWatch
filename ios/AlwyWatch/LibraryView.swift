// SPDX-License-Identifier: AGPL-3.0-or-later
import SwiftUI

private enum LibTab: CaseIterable {
    case recommended, browse, collections, categories

    var label: String {
        switch self {
        case .recommended: return tr("Recommended")
        case .browse: return tr("Browse")
        case .collections: return tr("Collections")
        case .categories: return tr("Categories")
        }
    }
}

/// A library like Plex (Android's LibraryScreen): Recommended (the home rails, only this
/// library's kind), Browse (the whole library, sortable), Collections and Categories (genres).
/// `recent`: opened from "Recently added", on Browse newest first.
struct LibraryView: View {
    let account: String
    let libraryId: String
    let title: String
    @State private var tab: LibTab
    private let recent: Bool

    init(account: String, libraryId: String, title: String, recent: Bool = false) {
        self.account = account
        self.libraryId = libraryId
        self.title = title
        self.recent = recent
        _tab = State(initialValue: recent ? .browse : .recommended)
    }

    var body: some View {
        VStack(spacing: 0) {
            ScrollView(.horizontal, showsIndicators: false) {
                HStack(spacing: 8) {
                    ForEach(LibTab.allCases, id: \.self) { t in
                        Button { tab = t } label: {
                            Text(t.label)
                                .font(.subheadline.weight(.semibold))
                                .padding(.horizontal, 16).padding(.vertical, 9)
                                .foregroundStyle(tab == t ? Color.black : Palette.onSurface)
                                .background(tab == t ? Palette.accent : Palette.surfaceHigh, in: Capsule())
                        }
                        .buttonStyle(.plain)
                    }
                }
                .padding(.horizontal, 16)
                .padding(.vertical, 8)
            }
            switch tab {
            case .recommended: LibraryRecommended(account: account, libraryId: libraryId, title: title)
            case .browse: LibraryGrid(account: account, libraryId: libraryId, initialSort: recent ? .dateAdded : .title)
            case .collections: LibraryCollections(account: account, libraryId: libraryId)
            case .categories: LibraryCategories(account: account, libraryId: libraryId, title: title)
            }
        }
        .background(Palette.background.ignoresSafeArea())
        .navigationTitle(title)
    }
}

/// A library narrowed to a genre or a collection (its own page).
struct FilteredLibraryView: View {
    let account: String
    let libraryId: String
    let title: String
    let genreId: String?
    let collectionId: String?

    var body: some View {
        LibraryGrid(account: account, libraryId: libraryId, genreId: genreId, collectionId: collectionId)
            .background(Palette.background.ignoresSafeArea())
            .navigationTitle(title)
    }
}

/// The home's rails, keeping this library's kind only (movies, or shows and their episodes).
private struct LibraryRecommended: View {
    @EnvironmentObject var model: AppModel
    let account: String
    let libraryId: String
    let title: String
    @State private var home: Home?
    @State private var error: String?

    var body: some View {
        ScrollView {
            LazyVStack(alignment: .leading, spacing: 26) {
                if let home {
                    let kind = home.libraries.first { $0.id == libraryId }?.kind
                    let watching = home.continueWatching.filter { keep($0, kind) }
                    if !watching.isEmpty {
                        VStack(alignment: .leading, spacing: 10) {
                            SectionTitle(title: tr("Continue watching"), seeAll: .continueWatching(account: account))
                            ScrollView(.horizontal, showsIndicators: false) {
                                LazyHStack(alignment: .top, spacing: 12) {
                                    ForEach(watching) { item in
                                        NavigationLink(value: Route.detail(account: account, itemId: item.id)) {
                                            WideCard(account: account, item: item)
                                        }
                                        .buttonStyle(.plain)
                                    }
                                }
                                .padding(.horizontal, 16)
                            }
                        }
                    }
                    if let rail = home.latest.first(where: { $0.library.id == libraryId }), !rail.items.isEmpty {
                        VStack(alignment: .leading, spacing: 10) {
                            // Browse, newest first.
                            SectionTitle(title: tr("Recently added"), seeAll: .library(account: account, id: libraryId, title: title, recent: true))
                            ScrollView(.horizontal, showsIndicators: false) {
                                LazyHStack(alignment: .top, spacing: 12) {
                                    ForEach(rail.items) { item in
                                        let note = rail.notes.first { $0.itemId == item.id }
                                        NavigationLink(value: Route.detail(account: account, itemId: note?.openId ?? item.id)) {
                                            VStack(alignment: .leading, spacing: 6) {
                                                PosterCard(account: account, item: item)
                                                if let label = note?.label {
                                                    Text(label).font(.caption2.weight(.semibold)).foregroundStyle(Palette.accent)
                                                        .lineLimit(1).frame(width: 120, alignment: .leading)
                                                }
                                            }
                                        }
                                        .buttonStyle(.plain)
                                    }
                                }
                                .padding(.horizontal, 16)
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
        .task {
            if home == nil, let cached = (try? model.core.cachedHome(accountId: account)) ?? nil { home = cached }
            await refresh(force: false)
        }
        .refreshable { await refresh(force: true) }
    }

    private func keep(_ item: Item, _ kind: LibraryKind?) -> Bool {
        switch kind {
        case .movies?: return item.kind == .movie
        case .shows?: return item.kind == .episode || item.kind == .series
        default: return true
        }
    }

    private func refresh(force: Bool) async {
        do {
            home = try await model.core.refreshHome(accountId: account, force: force)
            error = nil
        } catch {
            if home == nil { self.error = message(error) }
        }
    }
}

/// Collections of a library, as posters; one opens its titles.
private struct LibraryCollections: View {
    @EnvironmentObject var model: AppModel
    let account: String
    let libraryId: String
    @State private var list: [Item]?
    @State private var error: String?

    var body: some View {
        ScrollView {
            if let list, list.isEmpty {
                Text(tr("No collections in this library.")).foregroundStyle(Palette.muted)
                    .frame(maxWidth: .infinity, alignment: .leading).padding(16)
            }
            LazyVGrid(columns: [GridItem(.adaptive(minimum: 104), spacing: 12, alignment: .top)], spacing: 18) {
                ForEach(list ?? []) { c in
                    NavigationLink(value: Route.filtered(account: account, libraryId: libraryId, title: c.title, genreId: nil, collectionId: c.id)) {
                        GridPoster(account: account, item: c)
                    }
                    .buttonStyle(.plain)
                }
            }
            .padding(16)
            ErrorText(text: error).padding(.horizontal, 16)
        }
        .task {
            if list == nil { list = model.core.cachedCollections(accountId: account, libraryId: libraryId) }
            await refresh(force: false)
        }
        .refreshable { await refresh(force: true) }
    }

    private func refresh(force: Bool) async {
        do {
            list = try await model.core.refreshCollections(accountId: account, libraryId: libraryId, force: force)
            error = nil
        } catch {
            if list == nil { self.error = message(error) }
        }
    }
}

/// Genres of a library; one opens the library narrowed to it.
private struct LibraryCategories: View {
    @EnvironmentObject var model: AppModel
    let account: String
    let libraryId: String
    let title: String
    @State private var list: [Genre]?
    @State private var error: String?

    var body: some View {
        List {
            if let error { ErrorText(text: error) }
            ForEach(list ?? [], id: \.id) { g in
                NavigationLink(value: Route.filtered(account: account, libraryId: libraryId, title: "\(g.name) · \(title)", genreId: g.id, collectionId: nil)) {
                    Text(g.name)
                }
            }
            .listRowBackground(Palette.surface)
        }
        .scrollContentBackground(.hidden)
        .task {
            if list == nil { list = model.core.cachedGenres(accountId: account, libraryId: libraryId) }
            await refresh(force: false)
        }
        .refreshable { await refresh(force: true) }
    }

    private func refresh(force: Bool) async {
        do {
            list = try await model.core.refreshGenres(accountId: account, libraryId: libraryId, force: force)
            error = nil
        } catch {
            if list == nil { self.error = message(error) }
        }
    }
}

/// A library's titles as a sortable poster grid (Browse), or narrowed to a genre or a
/// collection; the next page fetched before the end is reached.
struct LibraryGrid: View {
    @EnvironmentObject var model: AppModel
    let account: String
    let libraryId: String
    var genreId: String?
    var collectionId: String?
    var initialSort = SortBy.title

    private static let pageSize: UInt32 = 60
    @State private var items: [Item] = []
    @State private var total: UInt32 = 0
    @State private var loading = false
    @State private var sort: SortBy?
    @State private var error: String?

    /// A collection keeps its own order (saga order); libraries and genres sort.
    private var sortable: Bool { collectionId == nil }
    private var current: SortBy { sort ?? initialSort }

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 12) {
                // Always one line, empty until the count is known (nothing jumps).
                Text(total > 0 ? trn("{count} title", "{count} titles", Int(total), ["count": total.formatted()]) : " ")
                    .font(.subheadline).foregroundStyle(Palette.muted)
                if sortable {
                    HStack(spacing: 8) {
                        chip(tr("A–Z"), .title)
                        chip(tr("Recently added"), .dateAdded)
                        chip(tr("Release date"), .releaseDate)
                    }
                }
            }
            .frame(maxWidth: .infinity, alignment: .leading)
            .padding(.horizontal, 16)
            .padding(.top, 4)
            ErrorText(text: error).padding(.horizontal, 16)
            LazyVGrid(columns: [GridItem(.adaptive(minimum: 104), spacing: 12, alignment: .top)], spacing: 18) {
                ForEach(items) { item in
                    NavigationLink(value: Route.detail(account: account, itemId: item.id)) {
                        GridPoster(account: account, item: item)
                    }
                    .buttonStyle(.plain)
                    .onAppear {
                        if let i = items.firstIndex(where: { $0.id == item.id }), i >= items.count - 12 { Task { await more() } }
                    }
                }
            }
            .padding(16)
            if total == 0, items.isEmpty, error == nil, !loading {
                Text(tr("Nothing here.")).foregroundStyle(Palette.muted).frame(maxWidth: .infinity, alignment: .leading).padding(.horizontal, 16)
            }
        }
        .task(id: current) { await first() }
        .refreshable { await first(force: true) }
    }

    private func chip(_ label: String, _ s: SortBy) -> some View {
        Button { sort = s } label: {
            Text(label)
                .font(.subheadline.weight(.semibold))
                .padding(.horizontal, 14).padding(.vertical, 8)
                .foregroundStyle(current == s ? Color.black : Palette.onSurface)
                .background(current == s ? Palette.accent : Palette.surfaceHigh, in: Capsule())
        }
        .buttonStyle(.plain)
    }

    private func query(start: UInt32) -> ItemQuery {
        ItemQuery(
            libraryId: libraryId, start: start, limit: Self.pageSize, sortBy: current,
            descending: current != .title, unplayedOnly: false, search: nil, genreId: genreId, collectionId: collectionId
        )
    }

    private func first(force: Bool = false) async {
        let q = query(start: 0)
        loading = true
        defer { loading = false }
        if !force, let cached = (try? model.core.cachedItems(accountId: account, query: q)) ?? nil {
            items = cached.page.items
            total = cached.page.total
        }
        do {
            let r = try await model.core.refreshItems(accountId: account, query: q, force: force)
            items = r.page.items
            total = r.page.total
            error = nil
        } catch {
            if items.isEmpty { self.error = message(error) }
        }
    }

    private func more() async {
        guard !loading, items.count < Int(total) else { return }
        loading = true
        defer { loading = false }
        let q = query(start: UInt32(items.count))
        if let r = try? await model.core.refreshItems(accountId: account, query: q, force: false) {
            let known = Set(items.map(\.id))
            items += r.page.items.filter { !known.contains($0.id) }
            total = r.page.total
        }
    }
}

/// A poster that fills its grid cell.
struct GridPoster: View {
    let account: String
    let item: Item

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            Color.clear
                .aspectRatio(2 / 3, contentMode: .fit)
                .overlay { ServerImage(account: account, itemId: item.id, image: item.poster, width: 160) }
                .clipShape(RoundedRectangle(cornerRadius: 10))
                .overlay(alignment: .bottom) { ProgressLine(progress: item.progress) }
                .overlay(alignment: .topTrailing) {
                    if item.user.played {
                        Image(systemName: "checkmark.circle.fill")
                            .foregroundStyle(Palette.accent, Color.black.opacity(0.6))
                            .padding(6)
                    } else if let n = item.user.unplayedCount, n > 0, item.kind == .series {
                        Text("\(n)")
                            .font(.caption2.weight(.bold))
                            .foregroundStyle(.black)
                            .padding(.horizontal, 6).padding(.vertical, 2)
                            .background(Palette.accent, in: Capsule())
                            .padding(6)
                    }
                }
            Text(item.cardTitle).font(.footnote.weight(.semibold)).lineLimit(1)
            Text(item.subtitle).font(.caption2).foregroundStyle(Palette.muted).lineLimit(1)
        }
    }
}
