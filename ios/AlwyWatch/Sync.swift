// SPDX-License-Identifier: AGPL-3.0-or-later
import BackgroundTasks
import SwiftUI
import UIKit

/// Keeping two servers' watch state in sync (the core's `sync`) and the one-time history copy:
/// Android's SyncWorker and HistoryJob. iOS has no long background jobs: a run started in the
/// app goes on for the time iOS allows after leaving it, the periodic one runs when iOS grants a
/// background refresh (asked for every 6 hours, as on Android), and coming back to the app
/// catches up when the last run is older than that.
@MainActor
final class SyncJobs: ObservableObject {
    static let refreshId = "be.alwyapps.watch.sync"
    static let every: TimeInterval = 6 * 3600

    /// A sync runs.
    @Published var running = false
    /// Bumped when a sync ends: the Settings rows re-read their status.
    @Published var version = 0

    // The history copy (survives leaving its page, like Android's HistoryJob).
    @Published var from: String?
    @Published var to: String?
    @Published var plan: HistoryPlan?
    @Published var result: HistoryCopyResult?
    @Published var copying = false
    @Published var error: String?

    private let core: AlwyWatch

    init(core: AlwyWatch) {
        self.core = core
    }

    /// Syncs every pair now (turning a pair on, "Sync now").
    func syncNow() {
        guard !running, !core.syncPairs().isEmpty else { return }
        running = true
        let finish = keepAlive("sync")
        Task {
            _ = try? await core.syncNow()
            running = false
            version += 1
            finish()
        }
    }

    /// Coming back to the app: syncs when a pair never ran or ran more than 6 hours ago.
    func syncIfDue() {
        let now = Int64(Date().timeIntervalSince1970)
        let due = core.syncPairs().contains { p in
            guard let at = core.syncStatus(a: p.a, b: p.b).lastRunAt else { return true }
            return now - at >= Int64(Self.every)
        }
        if due { syncNow() }
    }

    /// Asks iOS for the next background refresh while a pair is on; cancels it otherwise.
    func schedule() {
        guard !core.syncPairs().isEmpty else {
            BGTaskScheduler.shared.cancel(taskRequestWithIdentifier: Self.refreshId)
            return
        }
        let r = BGAppRefreshTaskRequest(identifier: Self.refreshId)
        r.earliestBeginDate = Date(timeIntervalSinceNow: Self.every)
        try? BGTaskScheduler.shared.submit(r)
    }

    /// The background refresh: settings, then the pairs; then asks for the next one.
    func backgroundRefresh() async {
        schedule()
        _ = try? await core.syncSettings()
        guard !running, !core.syncPairs().isEmpty else { return }
        running = true
        _ = try? await core.syncNow()
        running = false
        version += 1
    }

    // MARK: History copy

    func resetCopy() {
        plan = nil
        result = nil
        error = nil
    }

    /// Reads both histories and plans the copy (writes nothing).
    func check(from: String, to: String) {
        guard from != to, !copying else { return }
        self.from = from
        self.to = to
        resetCopy()
        copying = true
        let finish = keepAlive("history-check")
        Task {
            do {
                plan = try await core.historyCopyPlan(fromAccount: from, toAccount: to)
            } catch {
                self.error = message(error)
            }
            copying = false
            finish()
        }
    }

    /// Writes the planned changes on the target.
    func copy() {
        guard let to, let changes = plan?.changes, !changes.isEmpty, !copying else { return }
        copying = true
        error = nil
        let finish = keepAlive("history-copy")
        Task {
            do {
                result = try await core.historyCopyApply(toAccount: to, changes: changes)
                plan = nil
            } catch {
                self.error = message(error)
            }
            copying = false
            finish()
        }
    }

    /// Asks iOS for time to finish after the app is left; returns the call that ends it.
    private func keepAlive(_ name: String) -> () -> Void {
        var id = UIBackgroundTaskIdentifier.invalid
        id = UIApplication.shared.beginBackgroundTask(withName: name) {
            UIApplication.shared.endBackgroundTask(id)
            id = .invalid
        }
        return {
            if id != .invalid {
                UIApplication.shared.endBackgroundTask(id)
                id = .invalid
            }
        }
    }
}

func kindName(_ k: ServerKind) -> String { k == .plex ? "Plex" : "Jellyfin" }

/// Settings → Keep in sync: one switch per pair of servers, when it last ran and what it changed
/// (Android's SyncGroup).
struct SyncSection: View {
    @EnvironmentObject var model: AppModel
    @ObservedObject var jobs: SyncJobs
    @State private var pairs: [SyncPair] = []

    private var candidates: [(Account, Account)] {
        let a = model.accounts
        var out: [(Account, Account)] = []
        for (i, x) in a.enumerated() {
            for y in a.dropFirst(i + 1) where y.serverId != x.serverId { out.append((x, y)) }
        }
        return out
    }

    var body: some View {
        if !candidates.isEmpty {
            Section {
                ForEach(Array(candidates.enumerated()), id: \.offset) { _, pair in
                    row(pair.0, pair.1)
                }
                if !pairs.isEmpty {
                    Button {
                        jobs.syncNow()
                    } label: {
                        Label(jobs.running ? tr("Syncing…") : tr("Sync now"), systemImage: "arrow.triangle.2.circlepath")
                    }
                    .disabled(jobs.running)
                }
            } header: {
                Text(tr("Keep in sync"))
            } footer: {
                Text(tr("Both ways: what you watch, unwatch or where you stop on one server is copied to the other. Right away for what you watch in AlwyWatch, every 6 hours for other apps. Changed on both since the last sync: the further one wins."))
            }
            .onAppear { pairs = model.core.syncPairs() }
            .onChange(of: jobs.version) { _ in pairs = model.core.syncPairs() }
        }
    }

    private func isOn(_ a: Account, _ b: Account) -> Bool {
        pairs.contains { ($0.a == a.id && $0.b == b.id) || ($0.a == b.id && $0.b == a.id) }
    }

    @ViewBuilder
    private func row(_ a: Account, _ b: Account) -> some View {
        let on = isOn(a, b)
        let status = on ? model.core.syncStatus(a: a.id, b: b.id) : nil
        VStack(alignment: .leading, spacing: 4) {
            Toggle(isOn: Binding(get: { on }, set: { want in
                try? model.core.setSyncPair(a: a.id, b: b.id, on: want)
                pairs = model.core.syncPairs()
                jobs.schedule()
                if want { jobs.syncNow() }
            })) {
                VStack(alignment: .leading, spacing: 2) {
                    Text("\(a.serverName) ⇄ \(b.serverName)").foregroundStyle(Palette.onSurface)
                    Text(subtitle(a, b, on, status)).font(.footnote).foregroundStyle(Palette.muted)
                }
            }
            .tint(Palette.accent)
            if on, let recent = status?.recent, !recent.isEmpty, !jobs.running {
                Text(recent.prefix(5).joined(separator: "\n")).font(.footnote).foregroundStyle(Palette.muted)
            }
        }
        // Re-read after a run (the status is not observable).
        .id("\(a.id)|\(b.id)|\(jobs.version)|\(jobs.running)")
    }

    private func subtitle(_ a: Account, _ b: Account, _ on: Bool, _ s: SyncStatus?) -> String {
        if !on {
            return tr("{server} ({kind}) and {other} ({otherKind})", [
                "server": a.serverName, "kind": kindName(a.kind), "other": b.serverName, "otherKind": kindName(b.kind),
            ])
        }
        if jobs.running { return tr("Syncing now…") }
        if let e = s?.error { return tr("Last sync failed: {reason}", ["reason": e]) }
        if let at = s?.lastRunAt {
            let f = DateFormatter()
            f.locale = Locale(identifier: I18n.language)
            f.dateStyle = .none
            f.timeStyle = .short
            let time = f.string(from: Date(timeIntervalSince1970: TimeInterval(at)))
            let changed = Int(s?.changes ?? 0)
            return changed == 0
                ? tr("Last synced {time} · nothing to change", ["time": time])
                : tr("Last synced {time} · {count} updated", ["time": time, "count": changed])
        }
        return tr("Waiting for the first sync")
    }
}

/// Copy watch history from one server to another (Plex → Jellyfin when moving): checks first
/// and shows what will change, writes only on "Copy". Only adds (Android's HistoryCopyScreen).
struct HistoryCopyView: View {
    @EnvironmentObject var model: AppModel
    @ObservedObject var jobs: SyncJobs
    @State private var progress: HistoryProgress?
    @State private var showMissing = false

    private var accounts: [Account] { model.accounts }
    // Plex to Jellyfin by default: the usual move.
    private var from: String {
        jobs.from ?? accounts.first { $0.kind == .plex }?.id ?? accounts.first?.id ?? ""
    }

    private var to: String {
        jobs.to ?? accounts.first { $0.kind == .jellyfin && $0.id != from }?.id ?? accounts.first { $0.id != from }?.id ?? ""
    }

    private func label(_ a: Account) -> String { "\(a.serverName) · \(a.userName) (\(kindName(a.kind)))" }

    var body: some View {
        List {
            Section {
                Text(tr("Marks what you watched on one server as watched on the other, and copies where you stopped. It only adds: nothing gets unmarked, and a resume point never goes back."))
                    .font(.subheadline).foregroundStyle(Palette.muted)
            }
            if accounts.count < 2 {
                Text(tr("Sign in to both servers first: Settings → Servers → Add a server.")).foregroundStyle(Palette.muted)
            } else {
                Section {
                    Picker(tr("From"), selection: Binding(get: { from }, set: { jobs.from = $0; jobs.resetCopy() })) {
                        ForEach(accounts, id: \.id) { Text(label($0)).tag($0.id) }
                    }
                    Picker(tr("To"), selection: Binding(get: { to }, set: { jobs.to = $0; jobs.resetCopy() })) {
                        ForEach(accounts, id: \.id) { Text(label($0)).tag($0.id) }
                    }
                }
                .disabled(jobs.copying)
                if from == to {
                    Text(tr("Pick two different servers.")).foregroundStyle(Palette.danger)
                }
                Section {
                    content
                }
            }
        }
        .scrollContentBackground(.hidden)
        .background(Palette.background.ignoresSafeArea())
        .navigationTitle(tr("Copy watch history"))
        .task(id: jobs.copying) {
            while jobs.copying {
                progress = model.core.historyCopyProgress()
                try? await Task.sleep(nanoseconds: 500_000_000)
            }
            progress = nil
        }
    }

    @ViewBuilder
    private var content: some View {
        if jobs.copying {
            running
        } else if jobs.plan == nil, jobs.result == nil {
            Button {
                jobs.check(from: from, to: to)
            } label: {
                Label(tr("Check what will change"), systemImage: "magnifyingglass")
            }
            .disabled(from == to)
        }
        if let p = jobs.plan {
            let watched = p.changes.filter(\.played).count
            let started = p.changes.count - watched
            Text(p.changes.isEmpty
                ? tr("Nothing to copy: everything is already there.")
                : [
                    watched > 0 ? tr("{count} to mark as watched", ["count": watched]) : nil,
                    started > 0 ? trn("{n} resume point", "{n} resume points", started) : nil,
                ].compactMap { $0 }.joined(separator: " · "))
                .font(.headline)
            let notes = [
                p.upToDate > 0 ? tr("{count} already up to date", ["count": p.upToDate]) : nil,
                p.notFound.isEmpty ? nil : tr("{count} not on that server", ["count": p.notFound.count]),
            ].compactMap { $0 }
            if !notes.isEmpty { Text(notes.joined(separator: " · ")).foregroundStyle(Palette.muted) }
            if !p.changes.isEmpty, !jobs.copying {
                Button {
                    jobs.copy()
                } label: {
                    Label(trn("Copy {n} title", "Copy {n} titles", p.changes.count), systemImage: "checkmark")
                }
            }
            if !p.notFound.isEmpty {
                Button(showMissing ? tr("Hide missing titles") : tr("Show missing titles")) { showMissing.toggle() }
                if showMissing {
                    Text(p.notFound.joined(separator: "\n")).font(.footnote).foregroundStyle(Palette.muted)
                }
            }
        }
        if let r = jobs.result {
            Text(r.failed > 0
                ? tr("Done: {copied} titles copied, {failed} failed (try again).", ["copied": r.applied, "failed": r.failed])
                : trn("Done: {n} title copied.", "Done: {n} titles copied.", Int(r.applied)))
                .font(.headline)
            Button(tr("Check again")) { jobs.check(from: from, to: to) }
        }
        if let e = jobs.error {
            Text(e).foregroundStyle(Palette.danger)
        }
    }

    /// The step it is on and a bar (exact when the count is known).
    @ViewBuilder
    private var running: some View {
        let total = Int(progress?.total ?? 0)
        let done = Int(progress?.done ?? 0)
        let step = (progress?.step).flatMap { $0.isEmpty ? nil : $0 } ?? tr("Starting")
        VStack(alignment: .leading, spacing: 10) {
            Text(total > 0
                ? tr("{step} · {done} of {total} ({percent} %)", ["step": step, "done": done, "total": total, "percent": done * 100 / total])
                : tr("{step}…", ["step": step]))
                .font(.subheadline.weight(.semibold))
            if total > 0 {
                ProgressView(value: Double(done), total: Double(total)).tint(Palette.accent)
            } else {
                ProgressView().tint(Palette.accent)
            }
            Text(tr("Keep AlwyWatch open until it ends: iOS pauses it shortly after you leave the app."))
                .font(.footnote).foregroundStyle(Palette.muted)
        }
    }
}
