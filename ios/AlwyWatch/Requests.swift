// SPDX-License-Identifier: AGPL-3.0-or-later
import SwiftUI

// Seerr: requests to follow, titles that are not on the servers, sagas, the request panel and
// the season picker (Android's RequestsScreen, SeerrTitleScreen, RequestSeasons).

extension Availability {
    var label: String? {
        switch self {
        case .available: return tr("Available")
        case .partiallyAvailable: return tr("Partly available")
        // Seerr's "processing" = approved, not on the server yet.
        case .processing: return tr("Requested")
        case .pending: return tr("Waiting for approval")
        case .blocklisted: return tr("Blocked")
        case .deleted, .unknown: return nil
        }
    }
}

extension SeerrResult: Identifiable {
    public var id: String { "\(mediaType == .tv ? "tv" : "movie")-\(tmdbId)" }

    /// "Downloading 42%" only when Radarr/Sonarr are on it.
    var stateLabel: String? {
        if let p = downloadProgress { return tr("Downloading {percent}%", ["percent": Int(p * 100)]) }
        return availability.label
    }
}

extension SeerrDetails {
    var stateLabel: String? {
        if let p = downloadProgress { return tr("Downloading {percent}%", ["percent": Int(p * 100)]) }
        return availability.label
    }

    var asResult: SeerrResult {
        SeerrResult(tmdbId: tmdbId, mediaType: mediaType, title: title, year: year, overview: overview,
                    posterUrl: posterUrl, availability: availability, downloadProgress: downloadProgress)
    }

    /// Whether it can be requested now; shows: their seasons decide once known.
    func canRequest(_ plan: SeasonPlan?) -> Bool {
        if mediaType == .tv, let plan { return !plan.missing.isEmpty }
        return availability == .unknown || availability == .deleted || (mediaType == .tv && availability == .partiallyAvailable)
    }

    /// Why there is no Request button, in one line.
    func whyNoRequest(_ plan: SeasonPlan?) -> String {
        if plan != nil, mediaType == .tv, availability != .available { return tr("Every season is on your servers or already requested.") }
        if downloadProgress != nil { return tr("Already requested: it is downloading.") }
        switch availability {
        case .available: return tr("Already on your servers: search for it to watch it.")
        case .partiallyAvailable: return tr("Already partly on your servers.")
        case .processing: return tr("Already requested and approved: it will be added soon.")
        case .pending: return tr("Already requested: waiting for approval.")
        case .blocklisted: return tr("Blocked on the request server: it can't be requested.")
        default: return tr("It can't be requested.")
        }
    }
}

extension RequestStatus {
    var label: String {
        switch self {
        case .pending: return tr("Waiting for approval")
        case .approved: return tr("Approved")
        case .declined: return tr("Declined")
        case .failed: return tr("Failed")
        case .completed: return tr("Available")
        }
    }

    var colors: (Color, Color) {
        switch self {
        case .completed: return (Palette.success, .black)
        case .approved: return (Color(red: 0x5B / 255, green: 0x8C / 255, blue: 1), .white)
        case .declined, .failed: return (Palette.danger, .white)
        case .pending: return (Palette.accent, .black)
        }
    }
}

extension SeerrRequest: Identifiable {}

/// A Seerr request error in plain words (Android's requestErrorText).
func requestErrorText(_ error: Error) -> String {
    guard let e = error as? CoreError else { return message(error) }
    switch e {
    case .Unauthorized:
        return tr("The request was refused: you may have no requests left, or not be allowed to request. If it keeps happening, sign in again in Settings.")
    case .Network: return tr("Can't reach the request site right now.")
    case .NotFound: return tr("The request site doesn't know this title.")
    case let .Server(m): return m.contains("409") ? tr("It has already been requested.") : tr("The request site had a problem. Try again in a moment.")
    default: return message(e)
    }
}

/// "Season N" in the app's language when the server's name is only a word and that number.
func seasonLabel(_ number: UInt32, _ name: String) -> String {
    if number == 0 { return name.trimmingCharacters(in: .whitespaces).isEmpty ? tr("Specials") : name }
    let t = name.trimmingCharacters(in: .whitespaces)
    let generic = t.isEmpty || t.range(of: "^\\p{L}+\\.?\\s*0*\(number)$", options: .regularExpression) != nil
    return generic ? tr("Season {number}", ["number": number]) : name
}

private func formatRuntimeMin(_ min: Int) -> String {
    min >= 60 ? tr("{hours} h {minutes} min", ["hours": min / 60, "minutes": min % 60]) : tr("{minutes} min", ["minutes": min])
}

/// A small coloured label.
struct Pill: View {
    let text: String
    var background: Color = Palette.surfaceHigh
    var foreground: Color = Palette.onSurface

    var body: some View {
        Text(text)
            .font(.caption.weight(.bold))
            .padding(.horizontal, 8).padding(.vertical, 4)
            .foregroundStyle(foreground)
            .background(background, in: Capsule())
    }
}

/// A Seerr title as a poster with its state (search, sagas).
struct SeerrPoster: View {
    let r: SeerrResult
    var width: CGFloat = 104

    var body: some View {
        VStack(alignment: .leading, spacing: 4) {
            UrlImage(url: r.posterUrl)
                .frame(width: width, height: width * 1.5)
                .overlay {
                    if r.posterUrl == nil {
                        Text(r.title).font(.subheadline.weight(.semibold)).foregroundStyle(Palette.muted)
                            .multilineTextAlignment(.center).lineLimit(4).padding(10)
                    }
                }
                .clipShape(RoundedRectangle(cornerRadius: 10))
                .overlay(alignment: .topLeading) {
                    if let s = r.stateLabel {
                        let ok = r.availability == .available
                        Pill(text: s, background: ok ? Palette.success : Color.black.opacity(0.6), foreground: ok ? .black : .white).padding(8)
                    }
                }
            Text(r.title).font(.subheadline.weight(.semibold)).lineLimit(1)
            Text(r.year ?? "").font(.caption).foregroundStyle(Palette.muted)
        }
        .frame(width: width, alignment: .leading)
    }
}

/// Seerr requests to follow them: mine, or everyone's (for request managers). New requests
/// are made from the search.
struct RequestsView: View {
    @EnvironmentObject var model: AppModel
    @State private var all = false
    @State private var list: [SeerrRequest]?
    @State private var error: String?
    @State private var unauthorized = false
    @State private var places: [String: LibraryTitle?] = [:]
    @State private var opened: Route?

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 14) {
                Text(tr("To ask for something new, search for it.")).font(.subheadline).foregroundStyle(Palette.muted)
                HStack(spacing: 8) {
                    Button(tr("Mine")) { all = false }.buttonStyle(ActionButtonStyle(primary: !all))
                    Button(tr("Everyone")) { all = true }.buttonStyle(ActionButtonStyle(primary: all))
                }
                if let error {
                    Text(all && unauthorized ? tr("Only request managers can see everyone's requests.") : error).foregroundStyle(Palette.muted)
                }
                let items = list ?? []
                if items.isEmpty, error == nil {
                    Text(list == nil ? tr("Loading requests…") : (all ? tr("No requests.") : tr("You haven't requested anything yet. Search for a title that isn't on your servers to request it.")))
                        .foregroundStyle(Palette.muted)
                }
                LazyVGrid(columns: [GridItem(.adaptive(minimum: 104), spacing: 12, alignment: .top)], alignment: .leading, spacing: 16) {
                    ForEach(items) { r in
                        Button { open(r) } label: { card(r) }.buttonStyle(.plain)
                    }
                }
            }
            .padding(16)
        }
        .background(Palette.background.ignoresSafeArea())
        .navigationTitle(tr("Requests"))
        .navigationDestination(isPresented: Binding(get: { opened != nil }, set: { if !$0 { opened = nil } })) {
            switch opened {
            case let .detail(account, itemId): DetailView(account: account, itemId: itemId)
            case let .seerrTitle(type, tmdb): SeerrTitleView(movie: type == "movie", tmdbId: tmdb)
            default: EmptyView()
            }
        }
        .refreshable { await load(force: true) }
        .task(id: all) { await load(force: false) }
    }

    private func key(_ r: SeerrRequest) -> String { "\(r.mediaType == .tv ? "tv" : "movie")/\(r.tmdbId)" }

    private func card(_ r: SeerrRequest) -> some View {
        VStack(alignment: .leading, spacing: 4) {
            UrlImage(url: r.posterUrl)
                .frame(width: 104, height: 156)
                .clipShape(RoundedRectangle(cornerRadius: 10))
                .overlay(alignment: .topLeading) {
                    Pill(text: r.status.label, background: r.status.colors.0, foreground: r.status.colors.1).padding(8)
                }
            Text(r.title ?? "TMDB \(r.tmdbId)").font(.subheadline.weight(.semibold)).lineLimit(1)
            Text([all && !r.requestedBy.isEmpty ? r.requestedBy : nil, String(r.createdAt.prefix(10))].compactMap { $0 }.joined(separator: " · "))
                .font(.caption).foregroundStyle(Palette.muted).lineLimit(1)
        }
        .frame(width: 104, alignment: .leading)
    }

    /// The title's page on the server when it is there (to play it), else Seerr's page.
    private func open(_ r: SeerrRequest) {
        Task {
            var at: LibraryTitle?
            if let known = places[key(r)] {
                at = known
            } else {
                at = (try? await model.core.libraryTitle(mediaType: r.mediaType, tmdbId: r.tmdbId, prefer: model.account)) ?? nil
            }
            if let at {
                opened = .detail(account: at.accountId, itemId: at.itemId)
            } else {
                opened = .seerrTitle(type: r.mediaType == .tv ? "tv" : "movie", tmdbId: r.tmdbId)
            }
        }
    }

    private func load(force: Bool) async {
        if !force { list = (try? model.core.cachedSeerrRequests(all: all)) ?? nil }
        do {
            list = try await model.core.refreshSeerrRequests(force: force, all: all)
            error = nil
        } catch {
            if case .Unauthorized = error as? CoreError { unauthorized = true } else { unauthorized = false }
            self.error = message(error)
        }
        // Where each title is on the servers, looked up ahead so a tap opens at once.
        for r in list ?? [] where places[key(r)] == nil {
            places[key(r)] = .some((try? await model.core.libraryTitle(mediaType: r.mediaType, tmdbId: r.tmdbId, prefer: model.account)) ?? nil)
        }
    }
}

/// Request a movie, or pick seasons of a show (Android's RequestPanel).
struct RequestPanel: View {
    @EnvironmentObject var model: AppModel
    @Environment(\.dismiss) private var dismiss
    let r: SeerrResult
    let done: (String) -> Void
    @State private var seasons: [SeerrSeasonInfo]?
    @State private var chosen = Set<UInt32>()
    @State private var busy = false

    var body: some View {
        NavigationStack {
            ScrollView {
                VStack(alignment: .leading, spacing: 14) {
                    HStack(alignment: .top, spacing: 16) {
                        UrlImage(url: r.posterUrl).frame(width: 110, height: 165).clipShape(RoundedRectangle(cornerRadius: 12))
                        VStack(alignment: .leading, spacing: 6) {
                            Text(r.title).font(.title2.weight(.bold)).lineLimit(2)
                            Text([r.year, r.mediaType == .tv ? tr("Show") : tr("Movie"), r.stateLabel].compactMap { $0 }.joined(separator: " · "))
                                .font(.subheadline).foregroundStyle(Palette.muted)
                        }
                    }
                    if let o = r.overview { Text(o).font(.subheadline).foregroundStyle(Palette.muted).lineLimit(4) }
                    if r.mediaType == .movie {
                        if r.availability != .available {
                            Button { send([]) } label: {
                                Label(busy ? tr("Requesting…") : tr("Request"), systemImage: "plus").frame(maxWidth: .infinity)
                            }
                            .buttonStyle(ActionButtonStyle(primary: true))
                        }
                    } else if let info = seasons {
                        let plan = seasonPlan(seasons: info, have: [])
                        if plan.missing.isEmpty {
                            Text(tr("Every season is on your servers or already requested.")).foregroundStyle(Palette.muted)
                        } else {
                            Button {
                                send(!chosen.isEmpty ? chosen.sorted() : (plan.hasSome ? plan.missing : []))
                            } label: {
                                Label(busy ? tr("Requesting…")
                                    : !chosen.isEmpty ? trn("Request {n} season", "Request {n} seasons", chosen.count)
                                    : plan.hasSome ? tr("Request the missing seasons") : tr("Request all seasons"),
                                    systemImage: "plus").frame(maxWidth: .infinity)
                            }
                            .buttonStyle(ActionButtonStyle(primary: true))
                        }
                        ScrollView(.horizontal, showsIndicators: false) {
                            HStack(spacing: 8) {
                                ForEach(plan.seasons, id: \.number) { s in
                                    if s.state == .requestable {
                                        let on = chosen.contains(s.number)
                                        Button {
                                            if on { chosen.remove(s.number) } else { chosen.insert(s.number) }
                                        } label: {
                                            Label(s.name, systemImage: on ? "checkmark" : "circle").labelStyle(.titleOnly)
                                        }
                                        .buttonStyle(ActionButtonStyle(primary: on))
                                    } else {
                                        Pill(text: "\(s.name) · \(s.label)")
                                    }
                                }
                            }
                        }
                    } else {
                        Text(tr("Loading seasons…")).foregroundStyle(Palette.muted)
                    }
                }
                .padding(16)
            }
            .background(Palette.background.ignoresSafeArea())
            .disabled(busy)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) { Button(tr("Close")) { done(""); dismiss() } }
            }
            .task {
                if r.mediaType == .tv { seasons = try? await model.core.seerrSeasonInfo(tmdbId: r.tmdbId) }
            }
        }
        .presentationDetents([.medium, .large])
    }

    private func send(_ numbers: [UInt32]) {
        guard !busy else { return }
        busy = true
        Task {
            let msg: String
            do {
                _ = try await model.core.seerrRequest(mediaType: r.mediaType, tmdbId: r.tmdbId, seasons: numbers)
                msg = numbers.isEmpty ? tr("Requested: {title}", ["title": r.title]) : seasonsRequestedMessage(numbers: numbers)
            } catch {
                msg = tr("Could not request {title}: {reason}", ["title": r.title, "reason": requestErrorText(error)])
            }
            busy = false
            done(msg)
            dismiss()
        }
    }
}

/// A title that is not (or not fully) on the servers, like Seerr's own page: artwork,
/// details, state, request, cast, and its saga (Android's SeerrTitleScreen).
struct SeerrTitleView: View {
    @EnvironmentObject var model: AppModel
    let movie: Bool
    let tmdbId: Int64
    @State private var title: SeerrDetails?
    @State private var plan: SeasonPlan?
    @State private var error: String?
    @State private var requesting = false
    @State private var note: String?
    @State private var reload = 0

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 16) {
                Color.clear.frame(height: 260)
                    .overlay { UrlImage(url: title?.backdropUrl) }
                    .clipped()
                    .overlay { LinearGradient(colors: [.clear, .clear, Palette.background], startPoint: .top, endPoint: .bottom) }
                if let error {
                    HStack {
                        ErrorText(text: error)
                        Button(tr("Retry")) { reload += 1 }
                    }
                    .padding(.horizontal, 16)
                }
                if let t = title { content(t) }
            }
            .padding(.bottom, 32)
        }
        .background(Palette.background.ignoresSafeArea())
        .ignoresSafeArea(edges: .top)
        .navigationBarTitleDisplayMode(.inline)
        .sheet(isPresented: $requesting) {
            if let t = title {
                RequestPanel(r: t.asResult) { msg in
                    note = msg.isEmpty ? nil : msg
                    reload += 1
                }
            }
        }
        .task(id: reload) {
            let type: SeerrMediaType = movie ? .movie : .tv
            do {
                title = try await model.core.seerrTitle(mediaType: type, tmdbId: tmdbId)
                error = nil
            } catch {
                self.error = message(error)
            }
            if type == .tv, let info = try? await model.core.seerrSeasonInfo(tmdbId: tmdbId) {
                plan = seasonPlan(seasons: info, have: [])
            }
        }
    }

    @ViewBuilder private func content(_ t: SeerrDetails) -> some View {
        VStack(alignment: .leading, spacing: 14) {
            HStack(alignment: .top, spacing: 16) {
                UrlImage(url: t.posterUrl).frame(width: 110, height: 165).clipShape(RoundedRectangle(cornerRadius: 12))
                VStack(alignment: .leading, spacing: 8) {
                    Text(t.title).font(.title.weight(.bold)).lineLimit(3)
                    Text([
                        t.year,
                        t.mediaType == .tv ? (t.seasons.map { trn("{n} season", "{n} seasons", Int($0)) } ?? tr("Show")) : nil,
                        t.runtimeMin.map { formatRuntimeMin(Int($0)) },
                        t.rating.map { String(format: "★ %.1f", $0) },
                    ].compactMap { $0 }.joined(separator: " · "))
                        .font(.subheadline).foregroundStyle(Palette.muted)
                    if let s = t.stateLabel {
                        let ok = t.availability == .available
                        Pill(text: s, background: ok ? Palette.success : Palette.surfaceHigh, foreground: ok ? .black : .white)
                    }
                }
            }
            if t.canRequest(plan) {
                Button { requesting = true } label: {
                    Label(t.mediaType == .tv && plan?.hasSome == true ? tr("Request the missing seasons") : tr("Request"), systemImage: "plus")
                        .frame(maxWidth: .infinity)
                }
                .buttonStyle(ActionButtonStyle(primary: true))
            } else {
                Text(t.whyNoRequest(plan)).foregroundStyle(Palette.muted)
            }
            if let note { Text(note).foregroundStyle(Palette.accent) }
            if let tagline = t.tagline { Text(tagline).font(.headline) }
            if let o = t.overview { Text(o).foregroundStyle(Palette.onSurface.opacity(0.85)) }
            if !t.genres.isEmpty {
                ScrollView(.horizontal, showsIndicators: false) {
                    HStack(spacing: 8) { ForEach(t.genres.prefix(5), id: \.self) { Pill(text: $0) } }
                }
            }
            if let c = t.collection {
                NavigationLink(value: Route.seerrSaga(id: c.id, name: c.name)) {
                    HStack {
                        VStack(alignment: .leading, spacing: 2) {
                            Text(tr("PART OF THE SAGA")).font(.caption.weight(.bold)).foregroundStyle(Palette.accent)
                            Text(c.name).font(.headline).foregroundStyle(Palette.onSurface)
                        }
                        Spacer()
                        Image(systemName: "chevron.right").foregroundStyle(Palette.subtle)
                    }
                    .padding(16)
                    .background(Palette.surface, in: RoundedRectangle(cornerRadius: 14))
                }
                .buttonStyle(.plain)
            }
        }
        .padding(.horizontal, 16)
        if !t.cast.isEmpty {
            Text(tr("Cast")).font(.headline).padding(.horizontal, 16)
            ScrollView(.horizontal, showsIndicators: false) {
                HStack(alignment: .top, spacing: 14) {
                    ForEach(Array(t.cast.enumerated()), id: \.offset) { _, p in
                        VStack(spacing: 4) {
                            UrlImage(url: p.photoUrl).frame(width: 76, height: 76).clipShape(Circle())
                            Text(p.name).font(.caption).lineLimit(2).multilineTextAlignment(.center)
                            if let c = p.character { Text(c).font(.caption).foregroundStyle(Palette.subtle).lineLimit(1) }
                        }
                        .frame(width: 88)
                    }
                }
                .padding(.horizontal, 16)
            }
        }
    }
}

/// A saga: its films with their state; each opens its page.
struct SeerrSagaView: View {
    @EnvironmentObject var model: AppModel
    let id: Int64
    let name: String
    @State private var saga: SeerrCollection?
    @State private var error: String?

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 12) {
                if let parts = saga?.parts { Text(trn("{n} film", "{n} films", parts.count)).foregroundStyle(Palette.muted) }
                if let o = saga?.overview { Text(o).font(.subheadline).foregroundStyle(Palette.muted) }
                ErrorText(text: error)
                LazyVGrid(columns: [GridItem(.adaptive(minimum: 104), spacing: 12, alignment: .top)], alignment: .leading, spacing: 20) {
                    ForEach(saga?.parts ?? []) { r in
                        NavigationLink(value: Route.seerrTitle(type: "movie", tmdbId: r.tmdbId)) { SeerrPoster(r: r) }
                            .buttonStyle(.plain)
                    }
                }
            }
            .padding(16)
        }
        .background(Palette.background.ignoresSafeArea())
        .navigationTitle(saga?.name ?? name)
        .task {
            do {
                saga = try await model.core.seerrCollection(id: id)
            } catch {
                self.error = message(error)
            }
        }
    }
}

/// "Request more seasons" for a show of the library: its seasons as Seerr knows them, the ones
/// that can be requested are check boxes (Android's RequestSeasonsDialog).
struct RequestSeasonsSheet: View {
    @EnvironmentObject var model: AppModel
    @Environment(\.dismiss) private var dismiss
    let tmdbId: Int64
    let showTitle: String
    let have: [UInt32]
    @State var info: [SeerrSeasonInfo]
    @State private var chosen = Set<UInt32>()
    @State private var busy = false
    @State private var done: String?
    @State private var error: String?

    var body: some View {
        let plan = seasonPlan(seasons: info, have: have)
        NavigationStack {
            List {
                Section {
                    ForEach(plan.seasons, id: \.number) { s in
                        if s.state == .requestable {
                            let on = chosen.contains(s.number)
                            Button {
                                if on { chosen.remove(s.number) } else { chosen.insert(s.number) }
                            } label: {
                                row(s, mark: on ? "checkmark.square.fill" : "square", tint: on ? Palette.accent : Palette.muted)
                            }
                        } else {
                            row(s, mark: mark(s.state), tint: s.state == .onServer ? Palette.accent : Palette.subtle)
                        }
                    }
                } header: {
                    Text(showTitle)
                }
                if let done { Text(done).foregroundStyle(Palette.accent) }
                if let error { ErrorText(text: error) }
                if plan.missing.count > 1 {
                    Button(tr("Select all missing")) { chosen = Set(plan.missing) }
                }
                if !plan.missing.isEmpty {
                    Button {
                        send()
                    } label: {
                        Label(busy ? tr("Requesting…") : trn("Request {n} season", "Request {n} seasons", chosen.count), systemImage: "plus")
                    }
                    .disabled(chosen.isEmpty || busy)
                }
            }
            .scrollContentBackground(.hidden)
            .background(Palette.background.ignoresSafeArea())
            .navigationTitle(tr("Request more seasons"))
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .confirmationAction) { Button(tr("Close")) { dismiss() } }
            }
        }
    }

    private func mark(_ s: SeasonState) -> String {
        switch s {
        case .onServer, .partlyOnServer: return "checkmark"
        case .downloading: return "arrow.down.circle"
        case .requested: return "hourglass"
        default: return "xmark"
        }
    }

    private func row(_ s: SeasonChoice, mark: String, tint: Color) -> some View {
        HStack(spacing: 14) {
            Image(systemName: mark).foregroundStyle(tint).frame(width: 24)
            VStack(alignment: .leading, spacing: 2) {
                Text(seasonLabel(s.number, s.name)).foregroundStyle(Palette.onSurface)
                Text(s.label).font(.footnote).foregroundStyle(Palette.muted)
            }
        }
    }

    private func send() {
        guard !busy, !chosen.isEmpty else { return }
        busy = true
        error = nil
        let numbers = chosen.sorted()
        Task {
            do {
                _ = try await model.core.seerrRequest(mediaType: .tv, tmdbId: tmdbId, seasons: numbers)
                done = seasonsRequestedMessage(numbers: numbers)
                chosen = []
                if let fresh = try? await model.core.seerrSeasonInfo(tmdbId: tmdbId) { info = fresh }
            } catch {
                self.error = tr("Couldn't request them: {reason}", ["reason": requestErrorText(error)])
            }
            busy = false
        }
    }
}

// ----- Settings → Requests ------------------------------------------------------------------

/// Setting: requests turned off on purpose (the AlwyWatch server's Seerr is not connected again).
let seerrOffKey = "seerr.off"

/// Settings → Requests: through the AlwyWatch server (a switch when it takes requests), or a
/// request site (Seerr) directly by its address.
struct RequestsSettings: View {
    @EnvironmentObject var model: AppModel
    @State private var version = 0
    @State private var error: String?
    @State private var busy = false

    var body: some View {
        let _ = version
        let seerr = model.core.seerrConnection()
        if model.core.keepConnection() != nil {
            Section(tr("Through your AlwyWatch server")) {
                if let seerr, seerr.viaKeep {
                    Toggle(isOn: Binding(get: { true }, set: { _ in turnOff() })) {
                        label(tr("Requests"), tr("Through your AlwyWatch server, as {name}", ["name": seerr.userName]))
                    }
                    .tint(Palette.accent)
                } else if model.services.requests {
                    Toggle(isOn: Binding(get: { false }, set: { _ in turnOn() })) {
                        label(tr("Requests"), seerr != nil ? tr("Off: you use a request site directly") : busy ? tr("Connecting…") : tr("Off"))
                    }
                    .tint(Palette.accent)
                } else {
                    Text(tr("Your AlwyWatch server doesn't take requests. Ask its owner, or use a request site address below."))
                        .font(.subheadline).foregroundStyle(Palette.muted)
                }
                ErrorText(text: error)
            }
            .listRowBackground(Palette.surface)
        }
        Section {
            if let seerr, !seerr.viaKeep {
                TitledRow(title: host(seerr.baseUrl), subtitle: tr("Signed in as {name}", ["name": seerr.userName]), icon: "plus")
                Button { turnOff() } label: {
                    TitledRow(title: tr("Sign out"), icon: "xmark", tint: Palette.danger)
                }
            } else {
                NavigationLink(value: Route.seerrConnect) {
                    TitledRow(title: tr("Use a request site address"), subtitle: tr("It only accepts the logins it's set up for (Plex or Jellyfin)"), icon: "plus")
                }
            }
        } header: {
            Text(tr("Request site"))
        } footer: {
            Text(tr("Ask for movies and shows that aren't on the server yet, on a request site (Seerr, Overseerr or Jellyseerr) run by your server's owner. It only accepts the logins it's set up for (Plex or Jellyfin)."))
        }
        .listRowBackground(Palette.surface)
    }

    private func label(_ title: String, _ subtitle: String) -> some View {
        VStack(alignment: .leading, spacing: 2) {
            Text(title).foregroundStyle(Palette.onSurface)
            Text(subtitle).font(.footnote).foregroundStyle(Palette.muted)
        }
    }

    private func turnOff() {
        try? model.core.seerrSignOut()
        try? model.core.setSetting(key: seerrOffKey, value: "1")
        Keychain.remove("seerr")
        changed()
    }

    private func turnOn() {
        guard !busy else { return }
        busy = true
        error = nil
        Task {
            try? model.core.setSetting(key: seerrOffKey, value: "0")
            await model.connectSeerrThroughKeep()
            if model.core.seerrConnection() == nil { error = tr("Can't reach the request site right now.") }
            busy = false
            changed()
        }
    }

    /// The tabs (Requests) follow at once.
    private func changed() {
        version += 1
        model.reload()
    }
}

/// Connects a request site (Seerr) with whatever sign-in it accepts (Android's SeerrConnectScreen).
struct SeerrConnectView: View {
    @EnvironmentObject var model: AppModel
    @Environment(\.dismiss) private var dismiss
    @Environment(\.openURL) private var openURL
    @State private var url = ""
    @State private var info: SeerrInfo?
    @State private var method: SeerrLogin?
    @State private var error: String?
    @State private var code: String?
    @State private var user = ""
    @State private var pass = ""

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 16) {
                Text(tr("Ask for movies and shows that aren't on the server yet")).foregroundStyle(Palette.muted)
                if let info {
                    Text("\(info.title) · \(info.baseUrl)").foregroundStyle(Palette.muted)
                    if let method {
                        signIn(info, method)
                    } else {
                        FlowLayout(spacing: 10) {
                            ForEach(info.logins, id: \.self) { m in
                                switch m {
                                case .plex: Button(tr("Sign in with Plex")) { choose(m, info) }.buttonStyle(ActionButtonStyle(primary: true))
                                case .jellyfin: Button(tr("Sign in with Jellyfin")) { choose(m, info) }.buttonStyle(ActionButtonStyle(primary: true))
                                case .emby: Button(tr("Sign in with Emby")) { choose(m, info) }.buttonStyle(ActionButtonStyle(primary: true))
                                case .local: Button(tr("An account made on the request site")) { choose(m, info) }.buttonStyle(ActionButtonStyle())
                                case .notConfigured:
                                    Text(tr("This request site has no sign-in method enabled. Tell your server's owner.")).foregroundStyle(Palette.danger)
                                }
                            }
                        }
                    }
                } else {
                    if model.core.keepActive() {
                        // Works whatever media server the user signed in with.
                        ChoiceCard(title: tr("Use my AlwyWatch server"), subtitle: tr("Recommended"), icon: "hourglass") {
                            attempt {
                                _ = try await model.core.seerrConnectBridge()
                                done()
                            }
                        }
                        Text(tr("No extra sign-in: your AlwyWatch server handles it.")).font(.footnote).foregroundStyle(Palette.muted)
                        Text(tr("Or type the address of the request site (Seerr, Overseerr or Jellyseerr)")).font(.headline).foregroundStyle(Palette.muted)
                    }
                    TextField("requests.example.org", text: $url)
                        .textInputAutocapitalization(.never)
                        .autocorrectionDisabled()
                        .keyboardType(.URL)
                        .submitLabel(.go)
                        .onSubmit(lookUp)
                        .padding(.horizontal, 20)
                        .padding(.vertical, 14)
                        .background(Palette.surface, in: Capsule())
                        .overlay(Capsule().stroke(Palette.outline))
                    Button(tr("Continue"), action: lookUp).buttonStyle(ActionButtonStyle(primary: true))
                }
                ErrorText(text: error)
            }
            .padding(20)
            .frame(maxWidth: 640, alignment: .leading)
            .frame(maxWidth: .infinity, alignment: .leading)
        }
        .background(Palette.background.ignoresSafeArea())
        .navigationTitle(tr("Requests"))
    }

    @ViewBuilder
    private func signIn(_ info: SeerrInfo, _ method: SeerrLogin) -> some View {
        switch method {
        case .plex:
            Text(tr("Finish signing in to Plex in the browser.")).foregroundStyle(Palette.muted)
        case .jellyfin, .emby, .local:
            let local = method == .local
            if method == .jellyfin, let code {
                Text(tr("Quick Connect: enter this code in a signed-in Jellyfin app")).foregroundStyle(Palette.muted)
                Text(code).font(.system(size: 40, weight: .bold, design: .rounded))
                Text(tr("or sign in with your password:")).foregroundStyle(Palette.muted)
            }
            TextField(local ? tr("Email") : tr("Username"), text: $user)
                .textInputAutocapitalization(.never)
                .autocorrectionDisabled()
                .keyboardType(local ? .emailAddress : .default)
                .textContentType(.username)
                .padding(14)
                .background(Palette.surface, in: RoundedRectangle(cornerRadius: 14))
            SecureField(tr("Password"), text: $pass)
                .textContentType(.password)
                .submitLabel(.done)
                .onSubmit { passwordSignIn(info, local: local) }
                .padding(14)
                .background(Palette.surface, in: RoundedRectangle(cornerRadius: 14))
            Button(tr("Sign in")) { passwordSignIn(info, local: local) }.buttonStyle(ActionButtonStyle(primary: true))
        case .notConfigured:
            EmptyView()
        }
    }

    private func attempt(_ block: @escaping () async throws -> Void) {
        error = nil
        Task {
            do { try await block() } catch { self.error = message(error) }
        }
    }

    private func lookUp() {
        let typed = url
        attempt { info = try await model.core.seerrInfo(url: normalizeServerUrl(input: typed)) }
    }

    private func finish(_ s: SeerrSignedIn) {
        Keychain.put("seerr", s.cookie)
        done()
    }

    private func done() {
        try? model.core.setSetting(key: seerrOffKey, value: "0")
        model.reload()
        dismiss()
    }

    private func passwordSignIn(_ info: SeerrInfo, local: Bool) {
        let (u, p, base) = (user, pass, info.baseUrl)
        attempt {
            if local {
                finish(try await model.core.seerrSignInLocal(url: base, email: u, password: p))
            } else {
                finish(try await model.core.seerrSignInJellyfin(url: base, username: u, password: p))
            }
        }
    }

    private func choose(_ m: SeerrLogin, _ info: SeerrInfo) {
        method = m
        let core = model.core
        let base = info.baseUrl
        switch m {
        case .plex:
            Task {
                do {
                    let pin = try await core.plexCreatePin(strong: true)
                    if !pin.authUrl.isEmpty, let u = URL(string: pin.authUrl) { openURL(u) }
                    while !Task.isCancelled, method == .plex {
                        try await Task.sleep(nanoseconds: 2_000_000_000)
                        do {
                            let token = try await core.plexCheckPin(pinId: pin.id)
                            finish(try await core.seerrSignInPlex(url: base, plexAccountToken: token))
                            return
                        } catch {
                            if !isPending(error) { throw error }
                        }
                    }
                } catch {
                    self.error = message(error)
                }
            }
        case .jellyfin:
            Task {
                guard let qc = try? await core.seerrQuickConnectStart(url: base) else { return }
                code = qc.code
                while !Task.isCancelled, method == .jellyfin {
                    try? await Task.sleep(nanoseconds: 3_000_000_000)
                    do {
                        finish(try await core.seerrQuickConnectPoll(url: base, secret: qc.secret))
                        return
                    } catch {
                        if !isPending(error) { return }
                    }
                }
            }
        default: break
        }
    }
}
