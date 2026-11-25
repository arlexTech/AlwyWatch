// SPDX-License-Identifier: AGPL-3.0-or-later
import AVFoundation
import SwiftUI

@main
struct AlwyWatchApp: App {
    @StateObject private var model = AppModel()
    @Environment(\.scenePhase) private var phase

    init() {
        // Films keep their sound with the silent switch on, and in picture in picture.
        try? AVAudioSession.sharedInstance().setCategory(.playback, mode: .moviePlayback)
    }

    var body: some Scene {
        WindowGroup {
            RootView()
                .environmentObject(model)
                .preferredColorScheme(.dark)
                .tint(Palette.accent)
                .onChange(of: phase) { p in
                    if p == .active { model.foreground() }
                    if p == .background { TogetherInbox.shared.stop() }
                }
        }
        // Every 6 hours when iOS allows it: the watch-state sync (Android's periodic SyncWorker).
        .backgroundTask(.appRefresh(SyncJobs.refreshId)) {
            await model.sync.backgroundRefresh()
        }
    }
}

/// Places to go from anywhere (navigation values).
enum Route: Hashable {
    case detail(account: String, itemId: String)
    case library(account: String, id: String, title: String)
    case addServer
    case dashboard
    case historyCopy
    case leavingSoon
    case triage
    case search
    case settings
    case watchlist
    case continueWatching(account: String)
    case requests
    case seerrTitle(type: String, tmdbId: Int64)
    case seerrSaga(id: Int64, name: String)
    case settingsSection(String)
    case manageDownloads
    case downloadedShow(id: String, title: String)
    case room(code: String)
}

struct RootView: View {
    @EnvironmentObject var model: AppModel
    @ObservedObject private var inbox = TogetherInbox.shared
    @ObservedObject private var session = TogetherSession.shared
    @State private var tab = 0
    @State private var togetherPath = NavigationPath()

    /// Android's phone tabs: Home, Leaving soon when the server has Maintainerr, Requests when
    /// Seerr answers (Search, My list and Settings are in the home's top bar). One tab: no bar.
    private var leaving: Bool { model.core.keepActive() && model.services.leavingSoon }
    private var requests: Bool {
        model.core.seerrActive() && (model.core.seerrConnection()?.viaKeep != true || model.services.requests)
    }

    /// Watch together goes through the AlwyWatch server.
    private var together: Bool { model.core.keepActive() }

    var body: some View {
        Group {
            if model.account == nil {
                NavigationStack { SignInView() }
            } else if !leaving && !requests && !together {
                NavigationStack { HomeView().withRoutes() }
            } else {
                TabView(selection: $tab) {
                    NavigationStack { HomeView().withRoutes() }
                        .tabItem { Label(tr("Home"), systemImage: "house") }
                        .tag(0)
                    if leaving {
                        NavigationStack { LeavingSoonView().withRoutes() }
                            .tabItem { Label(tr("Leaving"), systemImage: "hourglass") }
                            .tag(1)
                    }
                    if together {
                        NavigationStack(path: $togetherPath) { TogetherView().withRoutes() }
                            .tabItem { Label(tr("Together"), systemImage: "person.2") }
                            .tag(2)
                    }
                    if requests {
                        NavigationStack { RequestsView().withRoutes() }
                            .tabItem { Label(tr("Requests"), systemImage: "plus") }
                            .tag(3)
                    }
                }
            }
        }
        .background(Palette.background.ignoresSafeArea())
        // A new invitation, wherever the user is.
        .sheet(item: $inbox.prompt) { InvitationPrompt(room: $0).environmentObject(model) }
        // A room to show: the Together tab, on that room.
        .onReceive(session.$show) { code in
            guard let code else { return }
            session.show = nil
            tab = 2
            togetherPath = NavigationPath()
            togetherPath.append(Route.room(code: code))
        }
        .onReceive(model.$tabRequest) { t in
            guard let t else { return }
            model.tabRequest = nil
            tab = t
        }
        // Each start opens the player in sync, wherever the app is.
        .onReceive(session.$state) { s in
            guard let s, s.started, s.round > session.handledRound, session.seat != nil, !PlayerPresenter.shared.isOpen else { return }
            session.handledRound = s.round
            Task { session.problem = await openRoomPlayer(model: model) }
        }
    }
}

extension TogetherRoom: Identifiable {
    public var id: String { code }
}

extension View {
    /// Every screen a stack can open.
    func withRoutes() -> some View {
        navigationDestination(for: Route.self) { route in
            switch route {
            case let .detail(account, itemId): DetailView(account: account, itemId: itemId)
            case let .library(account, id, title): LibraryView(account: account, libraryId: id, title: title)
            case .addServer: SignInView()
            case .dashboard: DashboardView()
            case .historyCopy: HistoryCopyScreen()
            case .leavingSoon: LeavingSoonView()
            case .triage: TriageView()
            case .search: SearchView()
            case .settings: SettingsScreen()
            case .watchlist: WatchlistView()
            case let .continueWatching(account): ContinueWatchingView(account: account)
            case .requests: RequestsView()
            case let .seerrTitle(type, tmdbId): SeerrTitleView(movie: type == "movie", tmdbId: tmdbId)
            case let .seerrSaga(id, name): SeerrSagaView(id: id, name: name)
            case let .settingsSection(id): SettingsSectionView(section: SettingsSection(rawValue: id) ?? .device)
            case .manageDownloads: ManageDownloadsView()
            case let .downloadedShow(id, title): DownloadedShowView(seriesId: id, title: title)
            case let .room(code): RoomView(code: code)
            }
        }
    }
}

/// The history copy page with the app's job (its state outlives the page).
struct HistoryCopyScreen: View {
    @EnvironmentObject var model: AppModel

    var body: some View { HistoryCopyView(jobs: model.sync) }
}
