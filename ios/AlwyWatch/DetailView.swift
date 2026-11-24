// SPDX-License-Identifier: AGPL-3.0-or-later
import SwiftUI

/// A movie, show or episode (Android's DetailScreen): backdrop, Play/Resume (a show's episode
/// to continue), overview, watched toggle, then a show's seasons and episodes.
struct DetailView: View {
    @EnvironmentObject var model: AppModel
    let account: String
    let itemId: String

    @State private var item: Item?
    @State private var nextEp: Item?
    @State private var seasons: [Item] = []
    @State private var season: String?
    @State private var episodes: [Item] = []
    @State private var player: PlayRequest?
    @State private var error: String?
    @State private var changed = 0

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 16) {
                hero
                if let item { info(item) }
                if !seasons.isEmpty { seasonList }
                ErrorText(text: error).padding(.horizontal, 16)
            }
            .padding(.bottom, 24)
        }
        .background(Palette.background.ignoresSafeArea())
        .ignoresSafeArea(edges: .top)
        .navigationBarTitleDisplayMode(.inline)
        .task(id: "\(itemId)\(changed)") { await load() }
        .onChange(of: player?.id) { _ in
            guard let request = player else { return }
            player = nil
            PlayerPresenter.shared.play(request, core: model.core) { changed += 1 }
        }
    }

    private var hero: some View {
        Color.clear
            .aspectRatio(16 / 10, contentMode: .fit)
            .overlay {
                if let item {
                    ServerImage(account: account, itemId: item.id, image: item.kind == .episode ? item.thumb : (item.backdrop ?? item.thumb), width: 800)
                }
            }
            .clipped()
            .overlay {
                LinearGradient(colors: [.clear, .clear, Palette.background], startPoint: .top, endPoint: .bottom)
            }
    }

    @ViewBuilder private func info(_ item: Item) -> some View {
        VStack(alignment: .leading, spacing: 12) {
            if item.kind == .episode, let show = item.seriesTitle, let sid = item.seriesId {
                NavigationLink(value: Route.detail(account: account, itemId: sid)) {
                    HStack(spacing: 4) {
                        Text(show).font(.title2.weight(.bold))
                        Image(systemName: "chevron.right").foregroundStyle(Palette.muted)
                    }
                }
                .buttonStyle(.plain)
                Text(item.title).font(.title3.weight(.semibold))
            } else {
                Text(item.title).font(.largeTitle.weight(.bold))
            }
            Text(meta(item)).font(.subheadline).foregroundStyle(Palette.muted)
            playButtons(item)
            if let overview = item.overview, !overview.isEmpty {
                Text(overview).font(.body).foregroundStyle(Palette.onSurface.opacity(0.9))
            }
            if !item.genres.isEmpty {
                Text(item.genres.joined(separator: " · ")).font(.footnote).foregroundStyle(Palette.subtle)
            }
        }
        .padding(.horizontal, 16)
    }

    private func meta(_ item: Item) -> String {
        var parts: [String] = []
        if item.kind == .episode, let s = item.seasonNumber, let e = item.episodeNumber {
            parts.append(tr("S{season} · E{episode}", ["season": s, "episode": e]))
        }
        if let y = item.year { parts.append(String(y)) }
        if let r = item.officialRating { parts.append(r) }
        if let r = item.runtimeMs, r > 0, item.kind != .series { parts.append(formatRuntime(Int64(r))) }
        if item.kind == .series, let c = item.childCount { parts.append(trn("{n} season", "{n} seasons", Int(c))) }
        if let c = item.communityRating { parts.append(String(format: "★ %.1f", c)) }
        return parts.joined(separator: " · ")
    }

    @ViewBuilder private func playButtons(_ item: Item) -> some View {
        let target: Item? = item.playable ? item : nextEp
        HStack(spacing: 10) {
            if let target {
                let resumable = target.user.positionMs > 0 && !target.user.played
                let at = formatClock(Double(target.user.positionMs) / 1000)
                let ep: String? = item.playable ? nil : tr("S{season} · E{episode}", ["season": target.seasonNumber ?? 1, "episode": target.episodeNumber ?? 1])
                let label: String = {
                    switch (resumable, ep) {
                    case let (true, ep?): return tr("Resume {episode} from {time}", ["episode": ep, "time": at])
                    case (true, nil): return tr("Resume from {time}", ["time": at])
                    case let (false, ep?): return tr("Play {episode}", ["episode": ep])
                    default: return tr("Play")
                    }
                }()
                Button {
                    player = PlayRequest(account: account, item: target)
                } label: {
                    Label(label, systemImage: "play.fill").lineLimit(1)
                }
                .buttonStyle(ActionButtonStyle(primary: true))
                if resumable {
                    Button {
                        player = PlayRequest(account: account, item: target, fromStart: true)
                    } label: {
                        Image(systemName: "arrow.counterclockwise")
                    }
                    .buttonStyle(ActionButtonStyle())
                    .accessibilityLabel(tr("Play from the start"))
                }
            }
            Button {
                Task {
                    do {
                        try await model.core.setPlayed(accountId: account, itemId: item.id, played: !item.user.played)
                        changed += 1
                    } catch {
                        self.error = message(error)
                    }
                }
            } label: {
                Image(systemName: item.user.played ? "checkmark.circle.fill" : "checkmark.circle")
            }
            .buttonStyle(ActionButtonStyle())
            .accessibilityLabel(item.user.played ? tr("Mark unwatched") : tr("Mark watched"))
        }
    }

    private var seasonList: some View {
        VStack(alignment: .leading, spacing: 12) {
            ScrollView(.horizontal, showsIndicators: false) {
                HStack(spacing: 8) {
                    ForEach(seasons) { s in
                        Button {
                            season = s.id
                            Task { await loadEpisodes(force: false) }
                        } label: {
                            Text(seasonLabel(s))
                                .font(.subheadline.weight(.semibold))
                                .padding(.horizontal, 16).padding(.vertical, 9)
                                .foregroundStyle(season == s.id ? Color.black : Palette.onSurface)
                                .background(season == s.id ? Palette.accent : Palette.surfaceHigh, in: Capsule())
                        }
                        .buttonStyle(.plain)
                    }
                }
                .padding(.horizontal, 16)
            }
            LazyVStack(alignment: .leading, spacing: 14) {
                ForEach(episodes) { ep in
                    EpisodeRow(account: account, episode: ep)
                        .onTapGesture { player = PlayRequest(account: account, item: ep) }
                        .contextMenu { ItemMenu(account: account, item: ep) { changed += 1 } }
                }
            }
            .padding(.horizontal, 16)
        }
    }

    private func load() async {
        let fresh = changed > 0
        if !fresh, let cached = (try? model.core.cachedItem(accountId: account, itemId: itemId)) ?? nil { item = cached.item }
        do {
            item = try await model.core.refreshItem(accountId: account, itemId: itemId, force: fresh).item
        } catch {
            if item == nil { self.error = message(error) }
        }
        guard let item, item.kind == .series else { return }
        if !fresh, let cached = (try? model.core.cachedSeasons(accountId: account, seriesId: itemId)) ?? nil { seasons = cached.items }
        if let s = try? await model.core.refreshSeasons(accountId: account, seriesId: itemId, force: fresh) { seasons = s.items }
        // The episode to continue with: the server's continue watching, else the first unwatched.
        var home: Home?
        if fresh {
            home = try? await model.core.refreshHome(accountId: account, force: false)
        } else {
            home = (try? model.core.cachedHome(accountId: account)) ?? nil
        }
        if let ep = home?.continueWatching.first(where: { $0.seriesId == itemId }) {
            nextEp = ep
        } else {
            nextEp = nil
            for s in seasons.filter({ !$0.user.played }) {
                let eps = (try? await model.core.refreshEpisodes(accountId: account, seriesId: itemId, seasonId: s.id, force: false))?.items ?? []
                if let ep = eps.first(where: { $0.user.positionMs > 0 && !$0.user.played }) ?? eps.first(where: { !$0.user.played }) {
                    nextEp = ep
                    break
                }
            }
        }
        if season == nil || !seasons.contains(where: { $0.id == season }) {
            season = seasons.first(where: { $0.id == nextEp?.seasonId })?.id ?? seasons.first(where: { !$0.user.played })?.id ?? seasons.first?.id
        }
        await loadEpisodes(force: fresh)
    }

    private func loadEpisodes(force: Bool) async {
        guard let season else { return }
        if !force, let cached = (try? model.core.cachedEpisodes(accountId: account, seriesId: itemId, seasonId: season)) ?? nil {
            episodes = cached.items
        }
        if let r = try? await model.core.refreshEpisodes(accountId: account, seriesId: itemId, seasonId: season, force: force) {
            if self.season == season { episodes = r.items }
        }
    }
}

/// "Season 2", "Specials", or the season's own name when it has a real one.
func seasonLabel(_ s: Item) -> String {
    guard let number = s.seasonNumber else { return s.title }
    let name = s.title.trimmingCharacters(in: .whitespaces)
    if number == 0 { return name.isEmpty ? tr("Specials") : name }
    let generic = name.isEmpty || name.range(of: "^\\p{L}+\\.?\\s*0*\(number)$", options: .regularExpression) != nil
    return generic ? tr("Season {number}", ["number": number]) : name
}

private struct EpisodeRow: View {
    let account: String
    let episode: Item

    var body: some View {
        HStack(alignment: .top, spacing: 12) {
            ServerImage(account: account, itemId: episode.id, image: episode.thumb, width: 160)
                .frame(width: 150, height: 150 * 9 / 16)
                .clipShape(RoundedRectangle(cornerRadius: 8))
                .overlay(alignment: .bottom) { ProgressLine(progress: episode.progress) }
                .overlay(alignment: .topTrailing) {
                    if episode.user.played {
                        Image(systemName: "checkmark.circle.fill")
                            .foregroundStyle(Palette.accent, Color.black.opacity(0.6))
                            .padding(4)
                    }
                }
            VStack(alignment: .leading, spacing: 4) {
                Text("\(episode.episodeNumber.map { "\($0). " } ?? "")\(episode.title)").font(.subheadline.weight(.semibold)).lineLimit(2)
                Text(episode.remaining ?? episode.runtimeMs.map { formatRuntime(Int64($0)) } ?? "").font(.caption).foregroundStyle(Palette.muted)
                if let o = episode.overview { Text(o).font(.caption).foregroundStyle(Palette.subtle).lineLimit(2) }
            }
            Spacer(minLength: 0)
        }
        .contentShape(Rectangle())
    }
}
