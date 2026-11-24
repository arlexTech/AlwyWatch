// SPDX-License-Identifier: AGPL-3.0-or-later
import SwiftUI

/// Search on the current server (Android's SearchScreen, library results).
struct SearchView: View {
    @EnvironmentObject var model: AppModel
    @State private var query = ""
    @State private var results: [Item] = []
    @State private var error: String?

    var body: some View {
        let account = model.account ?? ""
        ScrollView {
            LazyVGrid(columns: [GridItem(.adaptive(minimum: 104), spacing: 12, alignment: .top)], spacing: 18) {
                ForEach(results) { item in
                    NavigationLink(value: Route.detail(account: account, itemId: item.id)) {
                        GridPoster(account: account, item: item)
                    }
                    .buttonStyle(.plain)
                }
            }
            .padding(16)
            if results.isEmpty, query.count >= 2, error == nil {
                Text(tr("Nothing found.")).foregroundStyle(Palette.muted).padding()
            }
            ErrorText(text: error).padding(.horizontal, 16)
        }
        .background(Palette.background.ignoresSafeArea())
        .navigationTitle(tr("Search"))
        .searchable(text: $query, placement: .navigationBarDrawer(displayMode: .always), prompt: tr("Movies, shows, episodes"))
        .autocorrectionDisabled()
        .task(id: query) {
            let q = query.trimmingCharacters(in: .whitespaces)
            guard q.count >= 2 else { results = []; return }
            // Typing: wait for a pause before asking the server.
            try? await Task.sleep(nanoseconds: 300_000_000)
            if Task.isCancelled { return }
            do {
                results = try await model.core.search(accountId: account, query: q, limit: 60)
                error = nil
            } catch {
                if !(error is CancellationError) { self.error = message(error) }
            }
        }
    }
}
