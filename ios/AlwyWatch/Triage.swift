// SPDX-License-Identifier: AGPL-3.0-or-later
import SwiftUI

/// Swipe mode (Android's TriageScreen): one card at a time, most urgent first, only titles I
/// have not chosen for. Left: fine to delete, right: keep until I watch it, up: keep forever;
/// the buttons do the same, a tap opens the title.
struct TriageView: View {
    @EnvironmentObject var model: AppModel
    @State private var queue: [LeavingItem]?
    @State private var done = 0
    @State private var error: String?
    @State private var drag: CGSize = .zero
    @State private var opening = false
    // The title opened from a card (the queue stays here underneath: back is the same card).
    @State private var opened: LibraryTitle?

    var body: some View {
        let total = (queue?.count ?? 0) + done
        ScrollView {
            VStack(spacing: 14) {
                Text(tr("Swipe them!")).font(.largeTitle.weight(.bold))
                if total > 0 {
                    ZStack(alignment: .leading) {
                        Capsule().fill(Palette.surfaceHigh)
                        Capsule().fill(Palette.accent).frame(width: 220 * CGFloat(done) / CGFloat(total))
                    }
                    .frame(width: 220, height: 4)
                }
                ErrorText(text: error)
                if let queue {
                    if let item = queue.first {
                        Text(trn(
                            "{n} left  ·  Swipe left: fine to delete · right: keep until I watch it · up: keep forever",
                            "{n} left  ·  Swipe left: fine to delete · right: keep until I watch it · up: keep forever",
                            queue.count
                        ))
                        .font(.footnote).foregroundStyle(Palette.subtle).multilineTextAlignment(.center)
                        card(item)
                        buttons
                    } else {
                        Text(done > 0 ? tr("All done, thanks! {count} sorted.", ["count": done]) : tr("Nothing to sort right now."))
                            .foregroundStyle(Palette.muted)
                    }
                } else if error == nil {
                    Text(tr("Loading…")).foregroundStyle(Palette.muted)
                }
            }
            .padding(16)
            .frame(maxWidth: .infinity)
        }
        .background(
            RadialGradient(colors: [Palette.accent.opacity(0.25), Palette.background], center: .top, startRadius: 0, endRadius: 500)
                .ignoresSafeArea()
        )
        .navigationBarTitleDisplayMode(.inline)
        .navigationDestination(isPresented: Binding(get: { opened != nil }, set: { if !$0 { opened = nil } })) {
            if let o = opened { DetailView(account: o.accountId, itemId: o.itemId) }
        }
        .task {
            if queue != nil { return }
            do {
                queue = try await model.core.keepTriage(limit: 50)
            } catch {
                self.error = message(error)
            }
        }
    }

    private func card(_ item: LeavingItem) -> some View {
        VStack(alignment: .leading, spacing: 0) {
            UrlImage(url: item.posterUrl)
                .aspectRatio(2 / 3, contentMode: .fit)
            VStack(alignment: .leading, spacing: 6) {
                Text(item.title).font(.title2.weight(.bold)).lineLimit(2)
                HStack(spacing: 8) {
                    if let y = item.year { Text(String(y)).foregroundStyle(Palette.muted) }
                    KeepBadge(text: item.shortLabel.0, urgent: item.shortLabel.1)
                }
                let c = item.counts
                let keepers = Int(c.forever + c.untilWatched + c.days)
                if keepers > 0 {
                    Text(trn("{n} person keeps it", "{n} people keep it", keepers)).font(.footnote).foregroundStyle(Palette.muted)
                }
            }
            .padding(12)
        }
        .frame(width: 260)
        .background(Palette.surface)
        .clipShape(RoundedRectangle(cornerRadius: 12))
        .offset(x: drag.width, y: min(drag.height, 0))
        .rotationEffect(.degrees(Double(drag.width) / 40))
        .id(item.key)
        .onTapGesture { open(item) }
        .gesture(
            DragGesture()
                .onChanged { drag = $0.translation }
                .onEnded { g in
                    // Distance or a quick flick decides (people swipe fast).
                    let dx = g.translation.width, dy = g.translation.height
                    let vx = g.predictedEndTranslation.width, vy = g.predictedEndTranslation.height
                    if (dy < -120 || vy < -400) && abs(dy) > abs(dx) {
                        vote(.keepForever)
                    } else if dx > 120 || vx > 400 {
                        vote(.keepUntilWatched)
                    } else if dx < -120 || vx < -400 {
                        vote(.dontCare)
                    }
                    drag = .zero
                }
        )
    }

    private var buttons: some View {
        VStack(spacing: 10) {
            HStack(spacing: 10) {
                Button { vote(.keepUntilWatched) } label: { Label(tr("Until I watch it"), systemImage: "clock") }
                    .buttonStyle(ActionButtonStyle(primary: true))
                Button { vote(.keepForever) } label: { Label(tr("Forever"), systemImage: "heart.fill") }
                    .buttonStyle(ActionButtonStyle())
            }
            HStack(spacing: 10) {
                Button(tr("Keep 30 days")) { vote(.keepDays) }
                    .buttonStyle(ActionButtonStyle())
                Button { vote(.dontCare) } label: { Label(tr("Fine to delete"), systemImage: "xmark") }
                    .buttonStyle(ActionButtonStyle())
            }
        }
    }

    private func vote(_ kind: VoteKind) {
        guard let item = queue?.first else { return }
        let r = VoteRequest(
            kind: kind,
            days: kind == .keepDays ? 30 : nil,
            reminderDays: kind == .keepUntilWatched ? 7 : nil,
            reason: nil
        )
        queue = Array(queue?.dropFirst() ?? [])
        done += 1
        Task {
            do {
                _ = try await model.core.keepVote(key: item.key, vote: r)
            } catch {
                self.error = message(error)
            }
        }
    }

    /// The title's page; back comes to the same card.
    private func open(_ item: LeavingItem) {
        guard !opening, let account = model.account else { return }
        opening = true
        Task {
            var at: LibraryTitle?
            if let tmdb = item.tmdbId.flatMap(Int64.init) {
                at = (try? await model.core.libraryTitle(mediaType: item.kind == "show" ? .tv : .movie, tmdbId: tmdb, prefer: account)) ?? nil
            }
            if at == nil, let id = item.serverItemId { at = LibraryTitle(accountId: account, itemId: id) }
            opening = false
            opened = at
        }
    }
}
