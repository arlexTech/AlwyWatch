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
    @State private var listed = false
    @State private var downloads: [Download] = []
    @State private var sheet: DownloadTarget?
    @State private var creatingRoom = false
    /// The show's seasons in Seerr ("More seasons"), looked up once the page is shown.
    @State private var seerrSeasons: [SeerrSeasonInfo]?
    @State private var requestingSeasons = false

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
        .pollDownloads($downloads)
        .sheet(item: $sheet) { DownloadSheet(target: $0).environmentObject(model) }
        .task(id: showTmdb) { await lookUpSeerrSeasons() }
        .sheet(isPresented: $requestingSeasons, onDismiss: { Task { await lookUpSeerrSeasons() } }) {
            if let showTmdb, let info = seerrSeasons {
                RequestSeasonsSheet(tmdbId: showTmdb, showTitle: item?.title ?? "", have: haveSeasons, info: info)
                    .environmentObject(model)
            }
        }
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
        // Wraps on narrow phones: Play first, the icon actions after it (Android's row).
        FlowLayout(spacing: 10) {
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
                // Opens a private room with it (never the player): the host invites, then starts.
                if model.core.keepActive() {
                    Button {
                        guard !creatingRoom else { return }
                        creatingRoom = true
                        Task {
                            do {
                                try await createRoom(model: model, account: account, itemId: target.id, resumeMs: target.user.positionMs)
                            } catch {
                                self.error = friendly(error)
                            }
                            creatingRoom = false
                        }
                    } label: {
                        Image(systemName: "person.2")
                    }
                    .buttonStyle(ActionButtonStyle())
                    .accessibilityLabel(TogetherSession.shared.seat != nil && TogetherSession.shared.isHost ? tr("Watch in your room") : tr("Watch together"))
                }
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
            // My list (plex.tv watchlist, Jellyfin favorites): movies and shows.
            if item.kind == .movie || item.kind == .series {
                Button {
                    let want = !listed
                    listed = want
                    Task {
                        do {
                            try await model.core.setWatchlisted(accountId: account, itemId: item.id, on: want)
                        } catch {
                            listed = !want
                            self.error = message(error)
                        }
                    }
                } label: {
                    Image(systemName: listed ? "bookmark.fill" : "bookmark")
                }
                .buttonStyle(ActionButtonStyle())
                .accessibilityLabel(listed ? tr("Remove from my list") : tr("Add to my list"))
                .onAppear { listed = model.core.isWatchlisted(accountId: account, itemId: item.id) }
            }
            downloadButton(item)
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

    private func download(_ id: String) -> Download? {
        downloads.first { $0.accountId == account && $0.itemId == id }
    }

    /// Movies: download, or cancel/remove. Shows and episodes: the download choices.
    @ViewBuilder private func downloadButton(_ item: Item) -> some View {
        switch item.kind {
        case .movie:
            let dl = download(item.id)
            DownloadToggle(runner: model.downloads, account: account, item: item, dl: dl) { pending in
                DownloadStateIcon(dl: dl, pending: pending, size: 22)
            }
            .buttonStyle(ActionButtonStyle())
            .accessibilityLabel(dl?.summary ?? tr("Download"))
        case .episode, .series:
            let mine = item.kind == .episode
                ? downloads.filter { $0.accountId == account && $0.itemId == item.id }
                : downloads.filter { $0.accountId == account && $0.episode?.seriesId == item.id }
            let active = mine.filter { !$0.done }
            let pending = mine.isEmpty && model.downloads.pending.contains(item.id)
            Button {
                openSheet(item)
            } label: {
                if pending || (!active.isEmpty && active.allSatisfy { $0.downloaded == 0 }) {
                    DownloadStateIcon(dl: nil, pending: true, size: 22)
                } else if !active.isEmpty {
                    let total = active.reduce(UInt64(0)) { $0 + ($1.total ?? 0) }
                    let got = active.reduce(UInt64(0)) { $0 + $1.downloaded }
                    ProgressRing(fraction: total > 0 ? Double(got) / Double(total) : 0, size: 26)
                } else {
                    Image(systemName: mine.isEmpty ? "arrow.down.circle" : "arrow.down.circle.fill")
                        .foregroundStyle(mine.isEmpty ? Palette.onSurface : Palette.accent)
                }
            }
            .buttonStyle(ActionButtonStyle())
            .accessibilityLabel(
                !active.isEmpty ? tr("Downloading {count}", ["count": active.count])
                    : !mine.isEmpty ? tr("{count} downloaded", ["count": mine.count]) : tr("Download")
            )
        default:
            EmptyView()
        }
    }

    private func openSheet(_ item: Item) {
        if item.kind == .episode, let sid = item.seriesId {
            let season = item.seasonId.map { (id: $0, title: item.seasonNumber.map { tr("Season {season}", ["season": $0]) } ?? tr("Season")) }
            sheet = DownloadTarget(seriesId: sid, seriesTitle: item.seriesTitle ?? item.title, season: season, episode: item)
        } else if item.kind == .series {
            let s = seasons.first { $0.id == season }
            sheet = DownloadTarget(seriesId: item.id, seriesTitle: item.title, season: s.map { (id: $0.id, title: seasonLabel($0)) }, episode: nil)
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
                    // "More seasons" (Seerr): only when some season can be requested.
                    if canRequestMore {
                        Button { requestingSeasons = true } label: {
                            Label(tr("More seasons"), systemImage: "plus")
                                .font(.subheadline.weight(.semibold))
                                .padding(.horizontal, 16).padding(.vertical, 9)
                                .foregroundStyle(Palette.onSurface)
                                .overlay(Capsule().stroke(Palette.accent, lineWidth: 1.5))
                        }
                        .buttonStyle(.plain)
                    }
                }
                .padding(.horizontal, 16)
            }
            LazyVStack(alignment: .leading, spacing: 14) {
                ForEach(episodes) { ep in
                    EpisodeRow(account: account, episode: ep, dl: download(ep.id))
                        .onTapGesture { player = PlayRequest(account: account, item: ep) }
                        .contextMenu { ItemMenu(account: account, item: ep) { changed += 1 } }
                }
            }
            .padding(.horizontal, 16)
        }
    }

    private var showTmdb: Int64? {
        guard let item, item.kind == .series else { return nil }
        return item.providerIds.tmdb.flatMap { Int64($0) }
    }

    private var haveSeasons: [UInt32] { seasons.compactMap(\.seasonNumber) }

    private var canRequestMore: Bool {
        guard let info = seerrSeasons else { return false }
        return !seasonPlan(seasons: info, have: haveSeasons).missing.isEmpty
    }

    /// Never delays the page; nothing shows while it loads or if Seerr can't be reached (it may
    /// still be connecting at the start: waits up to 10 s).
    private func lookUpSeerrSeasons() async {
        guard let tmdb = showTmdb else { return }
        for _ in 0 ..< 10 {
            if model.core.seerrActive() { break }
            try? await Task.sleep(nanoseconds: 1_000_000_000)
        }
        guard model.core.seerrActive() else { return }
        if let info = try? await model.core.seerrSeasonInfo(tmdbId: tmdb) { seerrSeasons = info }
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
    let dl: Download?

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
            EpisodeDownloadButton(account: account, item: episode, dl: dl)
        }
        .contentShape(Rectangle())
    }
}
