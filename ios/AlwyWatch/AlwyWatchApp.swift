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
}

struct RootView: View {
    @EnvironmentObject var model: AppModel
    @State private var tab = 0

    /// Android's phone tabs: Home, then Leaving soon when the server has Maintainerr (Search,
    /// My list, Downloads and Settings are in the home's top bar). One tab: no bar at all.
    private var leaving: Bool { model.core.keepActive() && model.services.leavingSoon }

    var body: some View {
        Group {
            if model.account == nil {
                NavigationStack { SignInView() }
            } else if !leaving {
                NavigationStack { HomeView().withRoutes() }
            } else {
                TabView(selection: $tab) {
                    NavigationStack { HomeView().withRoutes() }
                        .tabItem { Label(tr("Home"), systemImage: "house") }
                        .tag(0)
                    NavigationStack { LeavingSoonView().withRoutes() }
                        .tabItem { Label(tr("Leaving"), systemImage: "hourglass") }
                        .tag(1)
                }
            }
        }
        .background(Palette.background.ignoresSafeArea())
    }
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
            }
        }
    }
}

/// The history copy page with the app's job (its state outlives the page).
struct HistoryCopyScreen: View {
    @EnvironmentObject var model: AppModel

    var body: some View { HistoryCopyView(jobs: model.sync) }
}
