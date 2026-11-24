// SPDX-License-Identifier: AGPL-3.0-or-later
import SwiftUI

/// A library as a poster grid (Android's LibraryScreen, Browse tab): sorted, unwatched only on
/// demand, next page fetched before the end is reached.
struct LibraryView: View {
    @EnvironmentObject var model: AppModel
    let account: String
    let libraryId: String
    let title: String

    private static let pageSize: UInt32 = 60
    @State private var items: [Item] = []
    @State private var total: UInt32 = 0
    @State private var loading = false
    @State private var sort = SortBy.title
    @State private var unplayed = false
    @State private var error: String?

    var body: some View {
        ScrollView {
            LazyVGrid(columns: [GridItem(.adaptive(minimum: 104), spacing: 12, alignment: .top)], spacing: 18) {
                ForEach(Array(items.enumerated()), id: \.element.id) { i, item in
                    NavigationLink(value: Route.detail(account: account, itemId: item.id)) {
                        GridPoster(account: account, item: item)
                    }
                    .buttonStyle(.plain)
                    .onAppear { if i >= items.count - 12 { Task { await more() } } }
                }
            }
            .padding(16)
            ErrorText(text: error).padding(.horizontal, 16)
        }
        .background(Palette.background.ignoresSafeArea())
        .navigationTitle(title)
        .toolbar {
            ToolbarItem(placement: .navigationBarTrailing) {
                Menu {
                    Picker(tr("Sort"), selection: $sort) {
                        Text(tr("Title")).tag(SortBy.title)
                        Text(tr("Date added")).tag(SortBy.dateAdded)
                        Text(tr("Release date")).tag(SortBy.releaseDate)
                        Text(tr("Rating")).tag(SortBy.rating)
                    }
                    Toggle(tr("Unwatched only"), isOn: $unplayed)
                } label: {
                    Image(systemName: "line.3.horizontal.decrease.circle")
                }
            }
        }
        .task(id: "\(sort)\(unplayed)") { await first() }
        .refreshable { await first(force: true) }
    }

    private func query(start: UInt32) -> ItemQuery {
        ItemQuery(
            libraryId: libraryId, start: start, limit: Self.pageSize, sortBy: sort,
            descending: sort != .title, unplayedOnly: unplayed, search: nil, genreId: nil, collectionId: nil
        )
    }

    private func first(force: Bool = false) async {
        let q = query(start: 0)
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
