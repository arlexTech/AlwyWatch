// SPDX-License-Identifier: AGPL-3.0-or-later
import SwiftUI
import UIKit

private let urlMemory: NSCache<NSString, UIImage> = {
    let c = NSCache<NSString, UIImage>()
    c.totalCostLimit = 32 << 20
    return c
}()

/// An image from a plain URL (Leaving soon posters, Seerr) through the core's disk cache.
struct UrlImage: View {
    @EnvironmentObject var model: AppModel
    let url: String?
    @State private var loaded: UIImage?

    var body: some View {
        ZStack {
            Palette.surface
            if let loaded { Image(uiImage: loaded).resizable().scaledToFill() }
        }
        .clipped()
        .task(id: url) {
            guard let url else { loaded = nil; return }
            if let hit = urlMemory.object(forKey: url as NSString) { loaded = hit; return }
            let core = model.core
            let decoded: UIImage? = await Task.detached(priority: .utility) {
                var path = core.cachedRemoteImage(url: url)
                if path == nil { path = try? await core.remoteImage(url: url) }
                guard let path, let img = UIImage(contentsOfFile: path) else { return nil }
                return img.preparingForDisplay() ?? img
            }.value
            if let decoded {
                urlMemory.setObject(decoded, forKey: url as NSString, cost: Int(decoded.size.width * decoded.size.height * 4))
                loaded = decoded
            }
        }
    }
}

private let day: Int64 = 86400
private func nowSeconds() -> Int64 { Int64(Date().timeIntervalSince1970) }

extension LeavingItem: Identifiable {
    public var id: String { key }

    /// The date has passed but deletion is off for its collection: nothing happens yet.
    func paused(_ now: Int64 = nowSeconds()) -> Bool {
        !collectionActive && protection == .none && (effectiveDeletionDate ?? .max) < now
    }

    /// "Leaves in 4 days", "Not scheduled for deletion", "Kept forever"…
    var leavesLabel: String {
        let now = nowSeconds()
        switch protection {
        case .forever: return tr("Kept forever")
        case .untilWatched: return tr("Kept until watched")
        default:
            guard let d = effectiveDeletionDate else { return tr("No date") }
            if paused(now) { return tr("Not scheduled for deletion") }
            let days = (d - now) / day
            let base: String
            switch days {
            case ..<0: base = tr("Any day now")
            case 0: base = tr("Leaves today")
            case 1: base = tr("Leaves tomorrow")
            default: base = trn("Leaves in {n} day", "Leaves in {n} days", Int(days))
            }
            return protection == .extended ? tr("{status} (extended)", ["status": base]) : base
        }
    }

    /// Short countdown for badges, and whether it is urgent (a week or less).
    var shortLabel: (String, Bool) {
        let now = nowSeconds()
        if protection == .forever || protection == .untilWatched { return (tr("Kept"), false) }
        if paused(now) { return (tr("No date"), false) }
        guard let d = effectiveDeletionDate else { return (tr("Soon"), false) }
        let days = (d - now) / day
        switch days {
        case ..<0: return (tr("Any day now"), true)
        case 0: return (tr("Today"), true)
        default: return (trn("{n} day", "{n} days", Int(days)), days <= 7)
        }
    }

    var voteLabel: String? {
        guard let v = myVote else { return nil }
        switch v.kind {
        case .dontCare: return tr("You're fine with it going")
        case .keepDays: return tr("You keep it a while")
        case .keepUntilWatched: return tr("You keep it until you watch it")
        case .keepForever:
            if let r = v.reason { return tr("You keep it forever ({reason})", ["reason": r]) }
            return tr("You keep it forever")
        }
    }

    /// Other people keeping it (my own keep left out).
    var othersKeeping: Int {
        let c = counts
        let mine = (myVote != nil && myVote?.kind != .dontCare) ? 1 : 0
        return Int(c.forever + c.untilWatched + c.days) - mine
    }

    var kindYear: String {
        [year.map(String.init), kind.prefix(1).uppercased() + kind.dropFirst()].compactMap { $0 }.joined(separator: " · ")
    }
}

/// A countdown badge.
struct KeepBadge: View {
    let text: String
    var urgent = false
    var dark = false

    var body: some View {
        Text(text)
            .font(.caption.weight(.bold))
            .padding(.horizontal, 8)
            .padding(.vertical, 4)
            .foregroundStyle(Palette.onSurface)
            .background(urgent ? Palette.danger : (dark ? Color.black.opacity(0.6) : Palette.surfaceHigh), in: Capsule())
    }
}

/// A Leaving soon title's choices (Android's KeepSheet): what it is, when it goes, one row per
/// choice with the current one ticked.
struct KeepSheet: View {
    @EnvironmentObject var model: AppModel
    @Environment(\.dismiss) private var dismiss
    let item: LeavingItem
    let voted: (LeavingItem) -> Void
    @State private var error: String?
    @State private var busy = false

    var body: some View {
        let mine = item.myVote
        let days = mine?.until.map { Int(($0 - nowSeconds()) / day) }
        NavigationStack {
            List {
                HStack(spacing: 14) {
                    UrlImage(url: item.posterUrl)
                        .frame(width: 64, height: 96)
                        .clipShape(RoundedRectangle(cornerRadius: 10))
                    VStack(alignment: .leading, spacing: 6) {
                        Text(item.title).font(.headline).lineLimit(2)
                        Text(item.kindYear).font(.footnote).foregroundStyle(Palette.muted)
                        KeepBadge(text: item.shortLabel.0, urgent: item.shortLabel.1)
                    }
                }
                .listRowBackground(Color.clear)
                Section {
                    choice(tr("Until I watch it"), tr("Kept until you've seen it"), "clock", mine?.kind == .keepUntilWatched,
                           VoteRequest(kind: .keepUntilWatched, days: nil, reminderDays: 7, reason: nil))
                    choice(tr("Keep forever"), tr("It's a keeper"), "heart.fill", mine?.kind == .keepForever,
                           VoteRequest(kind: .keepForever, days: nil, reminderDays: nil, reason: nil))
                    ForEach([UInt32(30), 7], id: \.self) { n in
                        let ticked = mine?.kind == .keepDays && days.map { (Int(n) - 2) ... Int(n) ~= $0 } == true
                        choice(trn("Keep {n} more day", "Keep {n} more days", Int(n)), nil, "hourglass", ticked,
                               VoteRequest(kind: .keepDays, days: n, reminderDays: nil, reason: nil))
                    }
                    choice(tr("Fine to delete"), tr("It only goes if nobody else keeps it"), "trash", mine?.kind == .dontCare,
                           VoteRequest(kind: .dontCare, days: nil, reminderDays: nil, reason: nil))
                    if mine != nil {
                        Button { send(nil) } label: {
                            Label(tr("Remove my choice"), systemImage: "xmark").foregroundStyle(Palette.onSurface)
                        }
                    }
                } footer: {
                    let others = item.othersKeeping
                    Text([
                        others > 0 ? trn("{n} other person keeps it.", "{n} other people keep it.", others) : nil,
                        tr("Everyone's choices count and the longest one wins."),
                    ].compactMap { $0 }.joined(separator: " "))
                }
                if let error { ErrorText(text: error) }
            }
            .scrollContentBackground(.hidden)
            .background(Palette.background.ignoresSafeArea())
            .disabled(busy)
            .toolbar {
                ToolbarItem(placement: .confirmationAction) { Button(tr("Close")) { dismiss() } }
            }
        }
        .presentationDetents([.medium, .large])
    }

    private func choice(_ title: String, _ subtitle: String?, _ icon: String, _ ticked: Bool, _ r: VoteRequest) -> some View {
        Button { send(r) } label: {
            HStack(spacing: 14) {
                Image(systemName: icon).foregroundStyle(Palette.accent).frame(width: 24)
                VStack(alignment: .leading, spacing: 2) {
                    Text(title).foregroundStyle(Palette.onSurface)
                    if let subtitle { Text(subtitle).font(.footnote).foregroundStyle(Palette.muted) }
                }
                Spacer()
                if ticked { Image(systemName: "checkmark").foregroundStyle(Palette.accent) }
            }
        }
    }

    private func send(_ r: VoteRequest?) {
        guard !busy else { return }
        busy = true
        Task {
            do {
                let updated: LeavingItem
                if let r {
                    updated = try await model.core.keepVote(key: item.key, vote: r)
                } else {
                    updated = try await model.core.keepUnvote(key: item.key)
                }
                voted(updated)
                NotificationCenter.default.post(name: .keepVoted, object: nil)
                dismiss()
            } catch {
                self.error = message(error)
            }
            busy = false
        }
    }
}

/// The home's Leaving soon row (Android's LeavingSoonRail): titles still at risk, a tap opens
/// the choices; "Swipe them!" and "See all".
struct LeavingSoonRail: View {
    @EnvironmentObject var model: AppModel
    @State private var items: [LeavingItem] = []
    @State private var open: LeavingItem?

    var body: some View {
        Group {
            if !items.isEmpty {
                VStack(alignment: .leading, spacing: 10) {
                    HStack(alignment: .center, spacing: 10) {
                        VStack(alignment: .leading, spacing: 2) {
                            Text(tr("Leaving soon")).font(.title3.weight(.bold))
                            Text(tr("Keep what you still want to watch")).font(.footnote).foregroundStyle(Palette.muted)
                        }
                        Spacer()
                        NavigationLink(value: Route.triage) {
                            Label(tr("Swipe them!"), systemImage: "heart").font(.subheadline.weight(.semibold))
                                .padding(.horizontal, 12).padding(.vertical, 8)
                                .foregroundStyle(Color.black)
                                .background(Palette.accent, in: Capsule())
                        }
                        NavigationLink(value: Route.leavingSoon) {
                            Image(systemName: "chevron.right").font(.headline).foregroundStyle(Palette.onSurface)
                                .frame(width: 36, height: 36).background(Palette.surfaceHigh, in: Circle())
                        }
                        .accessibilityLabel(tr("See all"))
                    }
                    .padding(.horizontal, 16)
                    ScrollView(.horizontal, showsIndicators: false) {
                        LazyHStack(alignment: .top, spacing: 12) {
                            ForEach(items) { item in
                                Button { open = item } label: {
                                    VStack(alignment: .leading, spacing: 4) {
                                        UrlImage(url: item.posterUrl)
                                            .frame(width: 120, height: 180)
                                            .clipShape(RoundedRectangle(cornerRadius: 10))
                                            .overlay(alignment: .topLeading) {
                                                KeepBadge(text: item.shortLabel.0, urgent: item.shortLabel.1, dark: true).padding(8)
                                            }
                                        Text(item.title).font(.subheadline.weight(.semibold)).lineLimit(1)
                                        Text(item.kindYear).font(.caption).foregroundStyle(Palette.muted).lineLimit(1)
                                    }
                                    .frame(width: 120, alignment: .leading)
                                }
                                .buttonStyle(.plain)
                            }
                        }
                        .padding(.horizontal, 16)
                    }
                }
            }
        }
        .sheet(item: $open) { o in
            KeepSheet(item: o) { _ in Task { await load(force: true) } }
        }
        .task { await load(force: false) }
        // A choice made anywhere (the list, swiping) takes the title out at once.
        .onReceive(NotificationCenter.default.publisher(for: .keepVoted)) { _ in
            if let cached = (try? model.core.cachedLeavingSoon()) ?? nil { items = toReview(cached) }
        }
    }

    private func load(force: Bool) async {
        // Not when Maintainerr doesn't answer the AlwyWatch server (Android's keepServices).
        guard model.core.keepActive() else { items = []; return }
        if !model.core.keepServices().leavingSoon {
            guard let fresh = try? await model.core.refreshKeepServices(), fresh.leavingSoon else { items = []; return }
        }
        if !force, let cached = (try? model.core.cachedLeavingSoon()) ?? nil { items = toReview(cached) }
        if let fresh = try? await model.core.refreshLeavingSoon(force: force) { items = toReview(fresh) }
    }

    /// Only what is still mine to review (the core's rule); nothing left: no row.
    private func toReview(_ l: [LeavingItem]) -> [LeavingItem] { keepToReview(items: l, limit: 15) }
}

/// Everything leaving, most urgent first, with the keep choices (Android's LeavingSoonScreen).
struct LeavingSoonView: View {
    @EnvironmentObject var model: AppModel
    @State private var all: [LeavingItem] = []
    @State private var loaded = false
    @State private var error: String?
    @State private var filter = "all"
    @State private var open: LeavingItem?

    private var shown: [LeavingItem] {
        switch filter {
        case "urgent": return all.filter { $0.shortLabel.1 }
        case "mine": return all.filter { $0.myVote != nil }
        case "kept": return all.filter { $0.protection != .none }
        default: return all
        }
    }

    var body: some View {
        let paused = all.filter { $0.paused() }.count
        List {
            Section {
                Text([
                    trn("{count} title", "{count} titles", all.count, ["count": all.count.formatted()]),
                    tr("{count} with your choice", ["count": all.filter { $0.myVote != nil }.count]),
                ].joined(separator: " · "))
                    .font(.subheadline).foregroundStyle(Palette.muted)
                NavigationLink(value: Route.triage) {
                    Label(tr("Swipe them!"), systemImage: "heart.fill").font(.headline).foregroundStyle(Palette.accent)
                }
                ScrollView(.horizontal, showsIndicators: false) {
                    HStack(spacing: 8) {
                        ForEach(["all", "urgent", "mine", "kept"], id: \.self) { key in
                            Button(filterName(key)) { filter = key }
                                .buttonStyle(ChipStyle(selected: filter == key))
                        }
                    }
                }
                .listRowInsets(EdgeInsets(top: 8, leading: 16, bottom: 8, trailing: 16))
                if paused > 0 {
                    Text(tr("{count} aren't scheduled yet: deletion is off for now. Your choices count when it's on.", ["count": paused.formatted()]))
                        .font(.footnote).foregroundStyle(Palette.muted)
                }
                if let error { ErrorText(text: error) }
            }
            Section {
                ForEach(shown) { item in
                    Button { open = item } label: { row(item) }
                }
                if loaded, shown.isEmpty {
                    Text(tr("Nothing here.")).foregroundStyle(Palette.muted)
                }
            }
        }
        .scrollContentBackground(.hidden)
        .background(Palette.background.ignoresSafeArea())
        .navigationTitle(tr("Leaving soon"))
        .toolbar {
            // Admins: the countdown and people's rights.
            if model.core.keepConnection()?.isAdmin == true {
                ToolbarItem(placement: .primaryAction) {
                    NavigationLink(value: Route.keepAdmin) { Image(systemName: "gearshape") }
                        .accessibilityLabel(tr("Admin"))
                }
            }
        }
        .refreshable { await load(force: true) }
        .task { await load(force: false) }
        .sheet(item: $open) { o in
            KeepSheet(item: o) { _ in Task { await load(force: true) } }
        }
    }

    private func filterName(_ key: String) -> String {
        switch key {
        case "urgent": return tr("Within a week")
        case "mine": return tr("My choices")
        case "kept": return tr("Kept")
        default: return tr("All")
        }
    }

    private func row(_ item: LeavingItem) -> some View {
        HStack(spacing: 14) {
            UrlImage(url: item.posterUrl)
                .frame(width: 58, height: 87)
                .clipShape(RoundedRectangle(cornerRadius: 10))
            VStack(alignment: .leading, spacing: 4) {
                Text([item.title, item.year.map(String.init)].compactMap { $0 }.joined(separator: " · "))
                    .font(.subheadline.weight(.semibold)).foregroundStyle(Palette.onSurface).lineLimit(1)
                KeepBadge(text: item.shortLabel.0, urgent: item.shortLabel.1)
                let others = item.othersKeeping
                let line = [item.voteLabel, others > 0 ? trn("{n} other keeps it", "{n} others keep it", others) : nil]
                    .compactMap { $0 }.joined(separator: " · ")
                if !line.isEmpty {
                    Text(line).font(.footnote).foregroundStyle(item.myVote != nil ? Palette.accent : Palette.muted).lineLimit(1)
                }
            }
            Spacer()
            Image(systemName: "chevron.right").foregroundStyle(Palette.subtle)
        }
    }

    private func load(force: Bool) async {
        if !force, !loaded, let cached = (try? model.core.cachedLeavingSoon()) ?? nil { all = cached }
        do {
            all = try await model.core.refreshLeavingSoon(force: force)
            error = nil
        } catch let e as CoreError {
            if case .Network = e {
                let at = model.core.keepConnection()?.baseUrl
                switch (at, all.isEmpty) {
                case let (at?, false): error = tr("Can't reach your AlwyWatch server ({address}). This is the last list it sent.", ["address": at])
                case let (at?, true): error = tr("Can't reach your AlwyWatch server ({address}).", ["address": at])
                case (nil, false): error = tr("Can't reach your AlwyWatch server. This is the last list it sent.")
                case (nil, true): error = tr("Can't reach your AlwyWatch server.")
                }
            } else {
                error = message(e)
            }
        } catch {
            self.error = message(error)
        }
        loaded = true
    }
}

/// A filter chip (selected: accent outline), as on Android.
struct ChipStyle: ButtonStyle {
    var selected = false
    func makeBody(configuration: Configuration) -> some View {
        configuration.label
            .font(.subheadline.weight(.semibold))
            .padding(.horizontal, 16)
            .padding(.vertical, 9)
            .foregroundStyle(selected ? Palette.accent : Palette.onSurface)
            .background(Palette.surfaceHigh, in: Capsule())
            .overlay(Capsule().stroke(selected ? Palette.accent : .clear, lineWidth: 2))
            .opacity(configuration.isPressed ? 0.7 : 1)
    }
}

extension Notification.Name {
    /// A keep choice of mine was sent (the home's Leaving soon row re-reads the list).
    static let keepVoted = Notification.Name("alwywatch.keepVoted")
}
