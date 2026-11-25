// SPDX-License-Identifier: AGPL-3.0-or-later
import AVFoundation
import SwiftUI

@main
struct AlwyWatchApp: App {
    @StateObject private var model = AppModel()

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
        }
    }
}

/// Places to go from anywhere (navigation values).
enum Route: Hashable {
    case detail(account: String, itemId: String)
    case library(account: String, id: String, title: String)
    case addServer
    case dashboard
}

struct RootView: View {
    @EnvironmentObject var model: AppModel
    @State private var tab = 0

    var body: some View {
        Group {
            if model.account == nil {
                NavigationStack { SignInView() }
            } else {
                TabView(selection: $tab) {
                    NavigationStack { HomeView().withRoutes() }
                        .tabItem { Label(tr("Home"), systemImage: "house") }
                        .tag(0)
                    NavigationStack { SearchView().withRoutes() }
                        .tabItem { Label(tr("Search"), systemImage: "magnifyingglass") }
                        .tag(1)
                    NavigationStack { SettingsScreen().withRoutes() }
                        .tabItem { Label(tr("Settings"), systemImage: "gearshape") }
                        .tag(2)
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
            }
        }
    }
}
