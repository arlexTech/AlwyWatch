// SPDX-License-Identifier: AGPL-3.0-or-later
import SwiftUI

/// Settings: the signed-in servers (switch, add, sign out), language, cache, about.
struct SettingsScreen: View {
    @EnvironmentObject var model: AppModel
    @State private var language = I18n.choice
    @State private var cacheBytes: UInt64 = 0
    @State private var confirmSignOut: Account?

    var body: some View {
        List {
            Section(tr("Servers")) {
                ForEach(model.accounts, id: \.id) { a in
                    Button {
                        model.use(a.id)
                    } label: {
                        HStack {
                            VStack(alignment: .leading, spacing: 2) {
                                Text(a.serverName).foregroundStyle(Palette.onSurface)
                                Text("\(a.kind == .plex ? "Plex" : "Jellyfin") · \(a.userName)").font(.footnote).foregroundStyle(Palette.muted)
                            }
                            Spacer()
                            if a.id == model.account { Image(systemName: "checkmark").foregroundStyle(Palette.accent) }
                        }
                    }
                    .swipeActions {
                        Button(tr("Sign out"), role: .destructive) { confirmSignOut = a }
                    }
                }
                NavigationLink(value: Route.addServer) {
                    Label(tr("Add a server"), systemImage: "plus")
                }
            }
            Section(tr("Language")) {
                Picker(tr("Language"), selection: $language) {
                    Text(tr("System")).tag("")
                    ForEach(i18nLanguages(), id: \.code) { l in Text(l.name).tag(l.code) }
                }
                .onChange(of: language) { code in
                    I18n.choose(code)
                    model.reload()
                }
            }
            Section(tr("Storage")) {
                HStack {
                    Text(tr("Image cache"))
                    Spacer()
                    Text(ByteCountFormatter.string(fromByteCount: Int64(cacheBytes), countStyle: .file)).foregroundStyle(Palette.muted)
                }
                Button(tr("Clear the image cache")) {
                    try? model.core.clearImageCache()
                    cacheBytes = model.core.imageCacheBytes()
                }
            }
            Section {
                HStack {
                    Text("AlwyWatch")
                    Spacer()
                    Text(Bundle.main.object(forInfoDictionaryKey: "CFBundleShortVersionString") as? String ?? "").foregroundStyle(Palette.muted)
                }
            }
        }
        .scrollContentBackground(.hidden)
        .background(Palette.background.ignoresSafeArea())
        .navigationTitle(tr("Settings"))
        .onAppear { cacheBytes = model.core.imageCacheBytes() }
        .confirmationDialog(
            tr("Sign out of {server}?", ["server": confirmSignOut?.serverName ?? ""]),
            isPresented: Binding(get: { confirmSignOut != nil }, set: { if !$0 { confirmSignOut = nil } }),
            titleVisibility: .visible
        ) {
            Button(tr("Sign out"), role: .destructive) {
                if let a = confirmSignOut { model.signOut(a.id) }
                confirmSignOut = nil
            }
        }
    }
}
