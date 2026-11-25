// SPDX-License-Identifier: AGPL-3.0-or-later
import SwiftUI
import UIKit

// ----- Running the queue ---------------------------------------------------------------------

/// Runs the core's download queue (Android's DownloadWorker): while the app is open, and for
/// as long as iOS lets it after leaving. Every download resumes where it stopped (the core's
/// `.part` files and Range requests), so being suspended only pauses it: coming back starts
/// the queue again.
@MainActor
final class DownloadRunner: ObservableObject {
    private let core: AlwyWatch
    /// One queue per device (the profiles share it): never two runs at once, even across a
    /// profile switch.
    private static var running = false
    /// The runner of the profile in use (the last made).
    private static weak var current: DownloadRunner?
    private var retry: Task<Void, Never>?
    /// Items (or shows) whose download was just asked for: buttons spin at once.
    @Published private(set) var pending = Set<String>()
    /// Bumped when downloads change here (lists re-read at once).
    @Published private(set) var version = 0

    private var stopped = false

    init(core: AlwyWatch) {
        self.core = core
        Self.current = self
    }

    /// Another profile takes over: a run in progress ends on its own (the new profile's runner
    /// takes the queue over then); nothing starts here again.
    func stop() {
        stopped = true
        retry?.cancel()
    }

    func start() {
        guard !stopped, !Self.running, core.hasPendingDownloads() else { return }
        Self.running = true
        retry?.cancel()
        var task = UIBackgroundTaskIdentifier.invalid
        task = UIApplication.shared.beginBackgroundTask(withName: "downloads") {
            UIApplication.shared.endBackgroundTask(task)
            task = .invalid
        }
        let core = core
        Task {
            _ = try? await Task.detached { try await core.runDownloads() }.value
            Self.running = false
            if task != .invalid { UIApplication.shared.endBackgroundTask(task) }
            if stopped {
                Self.current?.start()
                return
            }
            changed()
            // Left waiting for the network: try again in a while (the core backs off too).
            if core.hasPendingDownloads() {
                retry = Task {
                    try? await Task.sleep(nanoseconds: 30_000_000_000)
                    if !Task.isCancelled { start() }
                }
            }
        }
    }

    /// Rules due (watched, expired), "next episodes" topped up, then the queue.
    func sync() {
        let core = core
        Task {
            _ = try? await Task.detached { try await core.syncDownloads() }.value
            changed()
            start()
        }
    }

    func add(_ id: String) {
        pending.insert(id)
        changed()
    }

    func remove(_ id: String) {
        pending.remove(id)
        changed()
    }

    func changed() {
        version += 1
    }

    /// Starts a download in the background (the page's button spins until it is queued).
    func download(_ marker: String, onError: @escaping (String) -> Void = { _ in }, _ work: @escaping (AlwyWatch) async throws -> Void) {
        add(marker)
        let core = core
        Task {
            do {
                try await work(core)
                start()
            } catch {
                onError(message(error))
            }
            remove(marker)
        }
    }
}

/// The downloads, re-read every second while shown (or when they change here).
struct DownloadPoll: ViewModifier {
    @EnvironmentObject var model: AppModel
    @Binding var list: [Download]

    func body(content: Content) -> some View {
        content.task(id: model.downloads.version) {
            while !Task.isCancelled {
                list = (try? model.core.downloads()) ?? list
                // Every second while something downloads, every few seconds otherwise.
                let busy = list.contains { $0.state != .completed }
                try? await Task.sleep(nanoseconds: busy ? 1_000_000_000 : 3_000_000_000)
            }
        }
    }
}

extension View {
    func pollDownloads(_ list: Binding<[Download]>) -> some View { modifier(DownloadPoll(list: list)) }
}

// ----- Rules and labels ----------------------------------------------------------------------

/// The deletion rules offered, in order.
let rulePresets: [DeleteRule] = [.never, .whenWatched, .daysAfterWatched(days: 1), .daysAfterWatched(days: 7), .afterDays(days: 7), .afterDays(days: 30)]

extension DeleteRule {
    var label: String {
        switch self {
        case .never: return tr("Keep")
        case .whenWatched: return tr("Once watched")
        case let .daysAfterWatched(days): return trn("{n} day after watching", "{n} days after watching", Int(days))
        case let .afterDays(days): return trn("After {n} day", "After {n} days", Int(days))
        }
    }

    /// "deletes once watched"…, after a downloaded file's size (nil: kept).
    var deletesLabel: String? {
        switch self {
        case .never: return nil
        case .whenWatched: return tr("deletes once watched")
        case let .daysAfterWatched(days): return trn("deletes {n} day after watching", "deletes {n} days after watching", Int(days))
        case let .afterDays(days): return trn("deletes after {n} day", "deletes after {n} days", Int(days))
        }
    }

    /// "Deletes them once watched"…, under "Keeps the next N episodes".
    var deletesThemLabel: String {
        switch self {
        case .never: return tr("Keep")
        case .whenWatched: return tr("Deletes them once watched")
        case let .daysAfterWatched(days): return trn("Deletes them {n} day after watching", "Deletes them {n} days after watching", Int(days))
        case let .afterDays(days): return trn("Deletes them after {n} day", "Deletes them after {n} days", Int(days))
        }
    }
}

private func gb(_ b: UInt64) -> String { tr("{size} GB", ["size": String(format: "%.1f", Double(b) / 1e9)]) }

extension Download: Identifiable {
    var done: Bool { state == .completed }

    /// The show of a download: from its metadata, or parsed from older titles ("Show · S1 · E2 · …").
    var showKey: (id: String, title: String)? {
        if let e = episode { return (e.seriesId, e.seriesTitle) }
        let parts = title.components(separatedBy: " · ")
        return parts.count >= 3 && parts[1].hasPrefix("S") ? ("title:" + parts[0], parts[0]) : nil
    }

    var seasonEpisode: (season: Int, episode: Int) {
        if let e = episode { return (Int(e.seasonNumber ?? 0), Int(e.episodeNumber ?? 0)) }
        let parts = title.components(separatedBy: " · ")
        let s = parts.count > 1 ? Int(parts[1].dropFirst()) ?? 0 : 0
        let e = parts.count > 2 ? Int(parts[2].dropFirst()) ?? 0 : 0
        return (s, e)
    }

    var episodeTitle: String { title.components(separatedBy: " · ").last ?? title }

    var fraction: Double {
        guard let t = total, t > 0 else { return 0 }
        return min(1, Double(downloaded) / Double(t))
    }

    /// "42%" while downloading, "0.8 GB" once done.
    var progressLabel: String {
        if done { return gb(total ?? downloaded) }
        guard let t = total, t > 0 else { return "…" }
        return "\(Int(fraction * 100))%"
    }

    var status: String {
        let size = total.flatMap { $0 > 0 ? "\(Int(fraction * 100))% · \(gb($0))" : nil } ?? gb(downloaded)
        switch state {
        case .queued: return tr("Queued · {progress}", ["progress": size])
        case .running: return tr("Downloading {progress}", ["progress": size])
        case .paused: return tr("Paused · {progress}", ["progress": size])
        case .pausedNoSpace: return tr("Paused: storage full. Free some space, then resume.")
        case .waiting: return tr("Waiting for the network · {progress}", ["progress": size])
        case .failed: return tr("Failed: {reason}", ["reason": error ?? tr("unknown error")])
        case .completed:
            return [total.map(gb), watchedAt != nil ? tr("watched") : nil, deleteRule.deletesLabel].compactMap { $0 }.joined(separator: " · ")
        }
    }

    /// Next to a button: "Downloading 42% · 0.3 / 0.8 GB" or "Downloaded · 0.8 GB".
    var summary: String {
        switch state {
        case .completed: return tr("Downloaded · {size}", ["size": gb(total ?? downloaded)])
        case .waiting: return tr("Waiting for the network · {progress}", ["progress": progressLabel])
        case .paused: return tr("Paused · {progress}", ["progress": progressLabel])
        case .failed: return tr("Download failed")
        case .pausedNoSpace: return tr("Paused: storage full")
        default:
            if let t = total {
                return tr("Downloading {progress} · {done} / {total}", ["progress": progressLabel, "done": gb(downloaded), "total": gb(t)])
            }
            return tr("Downloading {progress}", ["progress": progressLabel])
        }
    }
}

// ----- Small pieces ------------------------------------------------------------------------------

struct RuleChips: View {
    let selected: DeleteRule
    let onSelect: (DeleteRule) -> Void

    var body: some View {
        LazyVGrid(columns: [GridItem(.adaptive(minimum: 150), spacing: 8, alignment: .leading)], alignment: .leading, spacing: 8) {
            ForEach(rulePresets, id: \.self) { r in
                Button(r.label) { onSelect(r) }.buttonStyle(ChipStyle(selected: r == selected))
            }
        }
    }
}

/// A ring filling up with the percentage.
struct ProgressRing: View {
    let fraction: Double
    var size: CGFloat = 34

    var body: some View {
        ZStack {
            Circle().stroke(Color.white.opacity(0.15), lineWidth: 3)
            Circle().trim(from: 0, to: max(0.001, fraction)).stroke(Palette.accent, style: StrokeStyle(lineWidth: 3, lineCap: .round)).rotationEffect(.degrees(-90))
            Text("\(Int(fraction * 100))").font(.caption2.weight(.semibold)).foregroundStyle(.white)
        }
        .frame(width: size, height: size)
    }
}

/// A download's icon: arrow; spinning while starting; ring while downloading; filled once done.
struct DownloadStateIcon: View {
    let dl: Download?
    var pending = false
    var size: CGFloat = 34

    var body: some View {
        if let dl {
            if dl.done {
                Image(systemName: "arrow.down.circle.fill").font(.title3).foregroundStyle(Palette.accent)
            } else if dl.state == .paused || dl.state == .pausedNoSpace || dl.state == .failed {
                ZStack {
                    ProgressRing(fraction: dl.fraction, size: size).opacity(0.5)
                    Image(systemName: "pause.fill").font(.caption).foregroundStyle(Palette.muted)
                }
            } else if dl.downloaded == 0 || (dl.total ?? 0) == 0 {
                ProgressView().tint(Palette.accent).frame(width: size, height: size)
            } else {
                ProgressRing(fraction: dl.fraction, size: size)
            }
        } else if pending {
            ProgressView().tint(Palette.accent).frame(width: size, height: size)
        } else {
            Image(systemName: "arrow.down.circle").font(.title3)
        }
    }
}

/// Download, or (after asking) cancel/remove, a movie or an episode.
struct DownloadToggle<Content: View>: View {
    @EnvironmentObject var model: AppModel
    @ObservedObject var runner: DownloadRunner
    let account: String
    let item: Item
    let dl: Download?
    @ViewBuilder let label: (Bool) -> Content
    @State private var confirm = false
    @State private var error: String?

    var body: some View {
        let pending = dl == nil && runner.pending.contains(item.id)
        Button {
            if dl == nil {
                if pending { return }
                let account = account, id = item.id
                runner.download(id, onError: { error = $0 }) { core in
                    _ = try await core.downloadItem(accountId: account, itemId: id, rule: nil)
                }
            } else {
                confirm = true
            }
        } label: {
            label(pending)
        }
        .confirmationDialog(
            dl?.done == true ? tr("Remove the download?") : tr("Cancel the download?"),
            isPresented: $confirm,
            titleVisibility: .visible
        ) {
            Button(tr("Remove"), role: .destructive) {
                if let dl { try? model.core.removeDownload(id: dl.id) }
                runner.changed()
            }
            Button(tr("Keep"), role: .cancel) {}
        } message: {
            Text(tr("{title} ({size}) will be deleted from this device.", ["title": item.title, "size": dl?.progressLabel ?? ""]))
        }
        .alert(error ?? "", isPresented: Binding(get: { error != nil }, set: { if !$0 { error = nil } })) {
            Button(tr("Close"), role: .cancel) {}
        }
    }
}

/// Episode rows: the round download button.
struct EpisodeDownloadButton: View {
    @EnvironmentObject var model: AppModel
    let account: String
    let item: Item
    let dl: Download?

    var body: some View {
        DownloadToggle(runner: model.downloads, account: account, item: item, dl: dl) { pending in
            DownloadStateIcon(dl: dl, pending: pending, size: 30)
                .frame(width: 44, height: 44)
                .background(dl?.done == true ? Palette.accent.opacity(0.18) : Palette.surfaceHigh, in: Circle())
        }
        .buttonStyle(.plain)
        .accessibilityLabel(dl == nil ? tr("Download") : dl!.done ? tr("Downloaded, {progress}", ["progress": dl!.progressLabel]) : tr("Downloading, {progress}", ["progress": dl!.progressLabel]))
    }
}

/// Plays a download (its cached item; the server's when not cached).
@MainActor
func playDownload(_ dl: Download, model: AppModel, onClose: @escaping () -> Void = {}) {
    Task {
        var item = ((try? model.core.cachedItem(accountId: dl.accountId, itemId: dl.itemId)) ?? nil)?.item
        if item == nil { item = try? await model.core.refreshItem(accountId: dl.accountId, itemId: dl.itemId, force: false).item }
        guard let item else { return }
        PlayerPresenter.shared.play(PlayRequest(account: dl.accountId, item: item), core: model.core, onClose: onClose)
    }
}

// ----- Rows and pages ------------------------------------------------------------------------

/// One download with its state and actions (pause/resume, delete); a finished one plays.
struct DownloadRow: View {
    @EnvironmentObject var model: AppModel
    let dl: Download
    var label: String?
    @State private var confirm = false

    var body: some View {
        let item = ((try? model.core.cachedItem(accountId: dl.accountId, itemId: dl.itemId)) ?? nil)?.item
        let text = label ?? dl.title
        let parts = text.components(separatedBy: " · ")
        let mine = dl.owners.contains(model.core.profile())
        HStack(spacing: 12) {
            ServerImage(account: dl.accountId, itemId: dl.itemId, image: item?.thumb, width: 240)
                .frame(width: 112, height: 63)
                .clipShape(RoundedRectangle(cornerRadius: 10))
                .overlay(alignment: .topTrailing) {
                    if dl.watchedAt != nil {
                        Image(systemName: "checkmark.circle.fill").foregroundStyle(Palette.accent, Color.black.opacity(0.6)).padding(4)
                    }
                }
            VStack(alignment: .leading, spacing: 4) {
                if parts.count >= 3 {
                    Text(parts[0]).font(.subheadline.weight(.semibold)).lineLimit(1)
                    Text(parts.dropFirst().joined(separator: " · ")).font(.caption).lineLimit(1)
                } else {
                    Text(text).font(.subheadline.weight(.semibold)).lineLimit(2)
                }
                if !mine {
                    Text(tr("Another profile's download")).font(.caption).foregroundStyle(Palette.subtle)
                }
                Text(dl.status).font(.caption).lineLimit(2)
                    .foregroundStyle(dl.state == .failed || dl.state == .pausedNoSpace ? Palette.danger : Palette.muted)
                if !dl.done, (dl.total ?? 0) > 0 {
                    ProgressView(value: dl.fraction).tint(Palette.accent)
                }
            }
            Spacer(minLength: 0)
            switch dl.state {
            case .completed: EmptyView()
            case .running, .queued, .waiting:
                Button { try? model.core.pauseDownload(id: dl.id); model.downloads.changed() } label: {
                    Image(systemName: "pause.fill").frame(width: 40, height: 40)
                }
                .accessibilityLabel(tr("Pause"))
            default:
                Button {
                    try? model.core.resumeDownload(id: dl.id)
                    model.downloads.start()
                    model.downloads.changed()
                } label: {
                    Image(systemName: "arrow.clockwise").frame(width: 40, height: 40)
                }
                .accessibilityLabel(tr("Resume"))
            }
            // Another profile's download: watchable, not deletable from here.
            if mine {
                Button { confirm = true } label: { Image(systemName: "trash").frame(width: 40, height: 40) }
                    .accessibilityLabel(tr("Delete"))
            }
        }
        .buttonStyle(.plain)
        .padding(10)
        .background(Palette.surface, in: RoundedRectangle(cornerRadius: 14))
        .contentShape(Rectangle())
        .onTapGesture { if dl.done { playDownload(dl, model: model) } }
        .confirmationDialog(dl.done ? tr("Delete the download?") : tr("Cancel the download?"), isPresented: $confirm, titleVisibility: .visible) {
            Button(dl.done ? tr("Delete") : tr("Cancel download"), role: .destructive) {
                try? model.core.removeDownload(id: dl.id)
                model.downloads.changed()
            }
        } message: {
            Text(tr("{title} ({progress}) will be removed from this device.", ["title": dl.title.components(separatedBy: " · ").first ?? dl.title, "progress": dl.progressLabel]))
        }
    }
}

/// Every download, newest first, to pause, resume or delete (all accounts).
struct ManageDownloadsView: View {
    @State private var list: [Download] = []

    var body: some View {
        let bytes = list.filter(\.done).reduce(UInt64(0)) { $0 + ($1.total ?? $1.downloaded) }
        ScrollView {
            LazyVStack(alignment: .leading, spacing: 10) {
                Text(tr("{size} GB on this device", ["size": String(format: "%.1f", Double(bytes) / 1e9)])).foregroundStyle(Palette.muted)
                if list.isEmpty { Text(tr("Nothing downloaded.")).foregroundStyle(Palette.muted) }
                ForEach(list) { DownloadRow(dl: $0) }
            }
            .padding(16)
        }
        .background(Palette.background.ignoresSafeArea())
        .navigationTitle(tr("Manage downloads"))
        .pollDownloads($list)
    }
}

/// What is downloaded, organized like the home: what is downloading, then shows and movies
/// (the home's downloads mode, from the top bar).
struct DownloadsHomeContent: View {
    @EnvironmentObject var model: AppModel
    let accounts: Set<String>
    @State private var list: [Download] = []

    var body: some View {
        let mine = list.filter { accounts.contains($0.accountId) }
        let active = mine.filter { !$0.done }
        let done = mine.filter(\.done)
        let shows = showGroups(done)
        let movies = done.filter { $0.showKey == nil }
        LazyVStack(alignment: .leading, spacing: 22) {
            if mine.isEmpty {
                VStack(spacing: 12) {
                    Image(systemName: "arrow.down.circle").font(.system(size: 52)).foregroundStyle(Palette.subtle)
                    Text(tr("Nothing downloaded yet")).font(.headline)
                    Text(tr("Download a movie, an episode, a season or a whole show from its page."))
                        .foregroundStyle(Palette.muted).multilineTextAlignment(.center)
                }
                .frame(maxWidth: .infinity)
                .padding(.top, 60)
                .padding(.horizontal, 16)
            }
            if !active.isEmpty {
                let total = active.reduce(UInt64(0)) { $0 + ($1.total ?? 0) }
                let got = active.reduce(UInt64(0)) { $0 + $1.downloaded }
                NavigationLink(value: Route.manageDownloads) {
                    Label(
                        total > 0
                            ? tr("Downloading {count} · {percent}% · manage", ["count": active.count, "percent": Int(Double(got) * 100 / Double(total))])
                            : tr("Downloading {count} · manage", ["count": active.count]),
                        systemImage: "arrow.down.circle"
                    )
                }
                .buttonStyle(ActionButtonStyle())
                .padding(.horizontal, 16)
            }
            if !shows.isEmpty {
                VStack(alignment: .leading, spacing: 10) {
                    SectionTitle(title: tr("Shows"))
                    ScrollView(.horizontal, showsIndicators: false) {
                        LazyHStack(alignment: .top, spacing: 12) {
                            ForEach(shows) { s in
                                NavigationLink(value: Route.downloadedShow(id: s.id, title: s.title)) {
                                    OfflineTile(account: s.downloads[0].accountId, itemId: s.id.hasPrefix("title:") ? s.downloads[0].itemId : s.id, title: s.title, subtitle: trn("{n} episode", "{n} episodes", s.downloads.count))
                                }
                                .buttonStyle(.plain)
                            }
                        }
                        .padding(.horizontal, 16)
                    }
                }
            }
            if !movies.isEmpty {
                VStack(alignment: .leading, spacing: 10) {
                    SectionTitle(title: tr("Movies"))
                    ScrollView(.horizontal, showsIndicators: false) {
                        LazyHStack(alignment: .top, spacing: 12) {
                            ForEach(movies) { m in
                                OfflineTile(account: m.accountId, itemId: m.itemId, title: m.title, subtitle: m.status)
                                    .onTapGesture { playDownload(m, model: model) }
                            }
                        }
                        .padding(.horizontal, 16)
                    }
                }
            }
            if !mine.isEmpty {
                NavigationLink(value: Route.manageDownloads) {
                    Label(tr("Manage downloads"), systemImage: "gearshape")
                }
                .buttonStyle(ActionButtonStyle())
                .padding(.horizontal, 16)
            }
        }
        .pollDownloads($list)
    }

    private func showGroups(_ done: [Download]) -> [ShowGroup] {
        var order: [String] = []
        var groups: [String: ShowGroup] = [:]
        for d in done {
            guard let key = d.showKey else { continue }
            if groups[key.id] == nil {
                order.append(key.id)
                groups[key.id] = ShowGroup(id: key.id, title: key.title, downloads: [])
            }
            groups[key.id]?.downloads.append(d)
        }
        return order.compactMap { groups[$0] }
    }
}

/// A show's downloaded episodes.
struct ShowGroup: Identifiable {
    let id: String
    let title: String
    var downloads: [Download]
}

/// A poster with "Offline" on it.
private struct OfflineTile: View {
    @EnvironmentObject var model: AppModel
    let account: String
    let itemId: String
    let title: String
    let subtitle: String

    var body: some View {
        let item = ((try? model.core.cachedItem(accountId: account, itemId: itemId)) ?? nil)?.item
        VStack(alignment: .leading, spacing: 6) {
            ServerImage(account: account, itemId: itemId, image: item?.poster, width: 120)
                .frame(width: 120, height: 180)
                .clipShape(RoundedRectangle(cornerRadius: 10))
                .overlay(alignment: .topLeading) {
                    Text(tr("Offline")).font(.caption2.weight(.bold)).padding(.horizontal, 8).padding(.vertical, 4)
                        .background(Color.black.opacity(0.6), in: Capsule()).padding(8)
                }
            Text(title).font(.subheadline.weight(.semibold)).lineLimit(1)
            Text(subtitle).font(.caption).foregroundStyle(Palette.muted).lineLimit(1)
        }
        .frame(width: 120)
    }
}

/// The downloaded episodes of one show, by season, plus its "keep next episodes" setting.
struct DownloadedShowView: View {
    @EnvironmentObject var model: AppModel
    let seriesId: String
    let title: String
    @State private var list: [Download] = []
    @State private var auto: AutoDownload?

    var body: some View {
        let account = model.account ?? ""
        let eps = list.filter { d in
            d.showKey?.id == seriesId || d.showKey?.title == title
                || (d.episode == nil && ((try? model.core.cachedItem(accountId: account, itemId: d.itemId)) ?? nil)?.item.seriesId == seriesId)
        }
        .sorted { ($0.seasonEpisode.season, $0.seasonEpisode.episode) < ($1.seasonEpisode.season, $1.seasonEpisode.episode) }
        let seasons = Array(Set(eps.map(\.seasonEpisode.season))).sorted()
        ScrollView {
            LazyVStack(alignment: .leading, spacing: 8) {
                Text(tr("{count} downloaded", ["count": eps.count])).foregroundStyle(Palette.muted)
                if let a = auto {
                    HStack(spacing: 12) {
                        VStack(alignment: .leading, spacing: 2) {
                            Text(trn("Keeps the next {n} episode", "Keeps the next {n} episodes", Int(a.count))).font(.subheadline.weight(.semibold))
                            Text(a.rule.deletesThemLabel).font(.caption).foregroundStyle(Palette.muted)
                        }
                        Spacer()
                        Button(tr("Stop")) {
                            Task {
                                _ = try? await model.core.setAutoDownload(accountId: account, seriesId: seriesId, seriesTitle: title, count: 0, rule: .never)
                                auto = nil
                            }
                        }
                        .buttonStyle(ActionButtonStyle())
                    }
                    .padding(14)
                    .background(Palette.surface, in: RoundedRectangle(cornerRadius: 14))
                }
                if !seriesId.hasPrefix("title:") {
                    NavigationLink(value: Route.detail(account: account, itemId: seriesId)) {
                        Label(tr("Open the show"), systemImage: "info.circle")
                    }
                    .buttonStyle(ActionButtonStyle())
                }
                ForEach(seasons, id: \.self) { season in
                    Text(season == 0 ? tr("Episodes") : tr("Season {number}", ["number": season]))
                        .font(.headline).padding(.top, 14).padding(.bottom, 4)
                    ForEach(eps.filter { $0.seasonEpisode.season == season }) { dl in
                        DownloadRow(dl: dl, label: tr("E{number} · {title}", ["number": dl.seasonEpisode.episode, "title": dl.episodeTitle]))
                    }
                }
            }
            .padding(16)
        }
        .background(Palette.background.ignoresSafeArea())
        .navigationTitle(title)
        .pollDownloads($list)
        .onAppear { auto = model.core.autoDownloadOf(accountId: account, seriesId: seriesId) }
    }
}

// ----- Download sheet ------------------------------------------------------------------------

/// What the sheet downloads from: a show, maybe a season and an episode.
struct DownloadTarget: Identifiable {
    let seriesId: String
    let seriesTitle: String
    let season: (id: String, title: String)?
    let episode: Item?
    var id: String { seriesId + (episode?.id ?? "") }
}

/// Download choices for a show, a season or an episode: this one, its season, the whole show,
/// or "keep the next N episodes"; and when they delete themselves.
struct DownloadSheet: View {
    @EnvironmentObject var model: AppModel
    @Environment(\.dismiss) private var dismiss
    let target: DownloadTarget
    @State private var choice = "next"
    @State private var count = 3
    @State private var rule: DeleteRule = .never
    @State private var existing: AutoDownload?
    @State private var all: [Download] = []
    @State private var ask: (question: String, list: [Download])?
    @State private var ready = false

    var body: some View {
        let account = model.account ?? ""
        ScrollView {
            VStack(alignment: .leading, spacing: 14) {
                Text(tr("Download")).font(.title.weight(.bold))
                Text(target.seriesTitle).foregroundStyle(Palette.muted)
                LazyVGrid(columns: [GridItem(.adaptive(minimum: 130), spacing: 8, alignment: .leading)], alignment: .leading, spacing: 8) {
                    if target.episode != nil {
                        Button(tr("This episode")) { choice = "episode" }.buttonStyle(ChipStyle(selected: choice == "episode"))
                    }
                    if let s = target.season {
                        Button(s.title) { choice = "season" }.buttonStyle(ChipStyle(selected: choice == "season"))
                    }
                    Button(tr("Whole show")) { choice = "show" }.buttonStyle(ChipStyle(selected: choice == "show"))
                    Button(tr("Next episodes")) {
                        choice = "next"
                        if rule == .never { rule = .whenWatched }
                    }
                    .buttonStyle(ChipStyle(selected: choice == "next"))
                }
                if choice == "next" {
                    Text(tr("Keeps your next unwatched episodes on this device and replaces them as you watch."))
                        .font(.footnote).foregroundStyle(Palette.muted)
                    HStack(spacing: 8) {
                        ForEach([1, 3, 5, 10], id: \.self) { n in
                            Button("\(n)") { count = n }.buttonStyle(ActionButtonStyle(primary: count == n))
                        }
                    }
                }
                Text(tr("Delete them")).font(.headline).padding(.top, 4)
                RuleChips(selected: rule) { rule = $0 }
                Button {
                    start(account)
                } label: {
                    Label(tr("Download"), systemImage: "arrow.down.circle")
                }
                .buttonStyle(ActionButtonStyle(primary: true))
                removals
            }
            .padding(20)
        }
        .background(Palette.background.ignoresSafeArea())
        .presentationDetents([.medium, .large])
        .onAppear {
            guard !ready else { return }
            ready = true
            existing = model.core.autoDownloadOf(accountId: account, seriesId: target.seriesId)
            choice = target.episode != nil ? "episode" : target.season != nil ? "season" : "next"
            if let e = existing { count = Int(e.count) }
            rule = existing?.rule ?? (choice == "next" ? .whenWatched : model.core.defaultDeleteRule())
            all = ((try? model.core.downloads()) ?? []).filter { $0.accountId == account }
        }
        .confirmationDialog(ask?.question ?? "", isPresented: Binding(get: { ask != nil }, set: { if !$0 { ask = nil } }), titleVisibility: .visible) {
            Button(tr("Delete"), role: .destructive) {
                for d in ask?.list ?? [] { try? model.core.removeDownload(id: d.id) }
                model.downloads.changed()
                dismiss()
            }
        } message: {
            let bytes = (ask?.list ?? []).reduce(UInt64(0)) { $0 + $1.downloaded }
            Text(tr("{size} GB will be freed on this device.", ["size": String(format: "%.1f", Double(bytes) / 1e9)]))
        }
    }

    @ViewBuilder private var removals: some View {
        let account = model.account ?? ""
        let mine = target.episode.flatMap { ep in all.first { $0.itemId == ep.id } }
        let show = all.filter { $0.episode?.seriesId == target.seriesId }
        let running = show.filter { !$0.done }
        VStack(alignment: .leading, spacing: 10) {
            if let mine {
                Button {
                    ask = (mine.done ? tr("Remove this episode's download?") : tr("Cancel this episode's download?"), [mine])
                } label: {
                    Label(mine.done ? tr("Remove this episode") : tr("Cancel this episode"), systemImage: "trash")
                }
                .buttonStyle(ActionButtonStyle())
            }
            // The whole show: cancel what is still downloading, or remove everything.
            if !running.isEmpty {
                Button {
                    ask = (trn("Cancel {n} download in progress?", "Cancel {n} downloads in progress?", running.count), running)
                } label: {
                    Label(tr("Cancel {count} downloading", ["count": running.count]), systemImage: "xmark")
                }
                .buttonStyle(ActionButtonStyle())
            }
            if show.contains(where: \.done) {
                Button {
                    ask = (trn("Remove all {n} download of {title}?", "Remove all {n} downloads of {title}?", show.count, ["title": target.seriesTitle]), show)
                } label: {
                    Label(trn("Remove all {n} download", "Remove all {n} downloads", show.count), systemImage: "trash")
                }
                .buttonStyle(ActionButtonStyle())
            }
            if existing != nil {
                Button(tr("Stop keeping next")) {
                    Task {
                        _ = try? await model.core.setAutoDownload(accountId: account, seriesId: target.seriesId, seriesTitle: target.seriesTitle, count: 0, rule: .never)
                        dismiss()
                    }
                }
                .buttonStyle(ActionButtonStyle())
            }
        }
    }

    private func start(_ account: String) {
        let choice = choice, rule = rule, count = UInt32(count), t = target
        // The page's button spins right away.
        let marker = choice == "episode" ? (t.episode?.id ?? t.seriesId) : t.seriesId
        dismiss()
        model.downloads.download(marker) { core in
            switch choice {
            case "episode":
                if let ep = t.episode { _ = try await core.downloadItem(accountId: account, itemId: ep.id, rule: rule) }
            case "season":
                if let s = t.season { _ = try await core.downloadSeason(accountId: account, seriesId: t.seriesId, seasonId: s.id, rule: rule) }
            case "show":
                _ = try await core.downloadSeries(accountId: account, seriesId: t.seriesId, rule: rule)
            default:
                _ = try await core.setAutoDownload(accountId: account, seriesId: t.seriesId, seriesTitle: t.seriesTitle, count: count, rule: rule)
            }
        }
    }
}

// ----- Settings ------------------------------------------------------------------------------

/// Settings → Downloads: when new downloads delete themselves, and the list.
struct DownloadSettings: View {
    @EnvironmentObject var model: AppModel
    @State private var rule: DeleteRule = .never

    var body: some View {
        Section {
            VStack(alignment: .leading, spacing: 10) {
                Text(tr("When to delete")).font(.subheadline.weight(.semibold))
                RuleChips(selected: rule) {
                    rule = $0
                    try? model.core.setDefaultDeleteRule(rule: $0)
                }
            }
            .padding(.vertical, 6)
            NavigationLink(value: Route.manageDownloads) {
                Label(tr("Manage downloads"), systemImage: "arrow.down.circle")
            }
        } header: {
            Text(tr("Delete downloads"))
        } footer: {
            Text(tr("For new downloads; a show's \"next episodes\" keeps its own choice."))
        }
        .listRowBackground(Palette.surface)
        .onAppear { rule = model.core.defaultDeleteRule() }
    }
}
