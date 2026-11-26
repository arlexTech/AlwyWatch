// SPDX-License-Identifier: AGPL-3.0-or-later
import Combine
import SwiftUI

// ----- Words ---------------------------------------------------------------------------------

extension TogetherMedia {
    /// A room opened before choosing what to watch ("New room"): no title yet.
    var untitled: Bool { title.trimmingCharacters(in: .whitespaces).isEmpty }

    /// "Show · S1 · E2", or the movie's title.
    var label: String {
        if untitled { return tr("Nothing chosen yet") }
        if let show = seriesTitle {
            return tr("{show} · S{season} · E{episode}", ["show": show, "season": season ?? 1, "episode": episode ?? 1])
        }
        return title
    }
}

/// One person in a room, with how many devices they sit on.
struct RoomPerson: Identifiable {
    let member: TogetherMember
    let devices: Int
    var id: String { member.id }
}

/// One entry per person (a user on two devices has two seats), with how many devices.
func people(_ members: [TogetherMember]) -> [RoomPerson] {
    var order: [String] = []
    var groups: [String: [TogetherMember]] = [:]
    for m in members {
        let k = m.userKey.isEmpty ? m.id : m.userKey
        if groups[k] == nil { order.append(k) }
        groups[k, default: []].append(m)
    }
    return order.compactMap { k in groups[k].map { RoomPerson(member: $0[0], devices: $0.count) } }
}

/// Plain words for what went wrong with a watch together call.
func friendly(_ error: Error) -> String {
    guard let e = error as? CoreError else { return message(error) }
    switch e {
    case .Network: return tr("Can't reach your AlwyWatch server right now.")
    case .NotFound: return tr("This room has closed.")
    default: break
    }
    let text = message(e)
    if text.contains("HTTP 429") { return tr("Too many changes. Try again in a few minutes.") }
    if text.contains("HTTP 409") { return tr("This room is full.") }
    if text.contains("HTTP 413") { return tr("This picture is too big.") }
    if text.contains("HTTP 403") { return tr("Only the host can do that.") }
    return text.isEmpty ? tr("Something went wrong.") : text
}

func formatTime(_ ms: UInt64) -> String { formatClock(Double(ms) / 1000) }

/// The accounts to look a room's title up in: the preferred one first.
@MainActor
func preferredFirst(_ model: AppModel) -> [String] {
    let all = model.accounts.map(\.id)
    let pref = (try? model.core.getSetting(key: preferredKey)) ?? nil
    return all.filter { $0 == pref } + all.filter { $0 != pref }
}

// ----- The room this device sits in ----------------------------------------------------------

/// The watch together room this device sits in (one at a time), shared by the Room screen,
/// the player and the rest of the app (Android's TogetherSession). Its `wait` loop is the
/// seat's heartbeat (a seat silent for a minute is dropped): it runs while something shows
/// the room.
@MainActor
final class TogetherSession: ObservableObject {
    static let shared = TogetherSession()

    struct Seat: Equatable {
        let code: String
        let memberId: String
    }

    @Published private(set) var seat: Seat?
    /// The room as last heard (nil before the first answer).
    @Published private(set) var state: TogetherState?
    /// Why the room went away under us ("This room has closed."), until someone reads it.
    @Published var ended: String?
    /// Why the last start didn't open the player here (the title isn't on my servers).
    @Published var problem: String?
    /// A room to show on the Together tab (opened from a title, or the player closed in it).
    @Published var show: String?
    /// Where the host starts playback (their resume point when they opened the room).
    var startAt: UInt64 = 0
    /// The last start (`round`) this device opened the player for, or chose not to.
    var handledRound: UInt64 = 0
    /// Every answer of the room (also the unchanged ones every ~25 s: they correct drift).
    let updates = PassthroughSubject<TogetherState, Never>()

    private var core: AlwyWatch?
    private var users = 0
    private var loop: Task<Void, Never>?
    private var stopping: Task<Void, Never>?

    /// A room just joined or created: this device's seat.
    func begin(_ core: AlwyWatch, _ joined: TogetherJoined, startAt: UInt64 = 0) {
        self.core = core
        if seat?.code != joined.state.code {
            loop?.cancel()
            loop = nil
            state = nil
            handledRound = 0
        }
        seat = Seat(code: joined.state.code, memberId: joined.memberId)
        self.startAt = startAt
        ended = nil
        accept(joined.state, force: true)
        if users > 0 { ensureLoop() }
    }

    /// A newer state of this room (from `wait`, `update`, `invite`…).
    func accept(_ s: TogetherState, force: Bool = false) {
        guard seat?.code == s.code else { return }
        if force || state == nil || s.seq >= state!.seq { state = s }
    }

    var isHost: Bool {
        guard let s = state, let me = seat?.memberId else { return false }
        return togetherIsHost(state: s, memberId: me)
    }

    /// Something shows the room: keep the seat alive.
    func attach(_ core: AlwyWatch) {
        self.core = core
        users += 1
        stopping?.cancel()
        stopping = nil
        ensureLoop()
    }

    /// It no longer does. A short grace bridges the Room screen handing over to the player.
    func detach() {
        users = max(0, users - 1)
        guard users == 0 else { return }
        stopping?.cancel()
        stopping = Task {
            try? await Task.sleep(nanoseconds: 3_000_000_000)
            if !Task.isCancelled, users == 0 {
                loop?.cancel()
                loop = nil
            }
        }
    }

    /// Another profile takes over: this one's seat is forgotten here (the room drops it).
    func reset() {
        clear()
        core = nil
        ended = nil
        problem = nil
        show = nil
        startAt = 0
        handledRound = 0
    }

    /// Leaves for good (a new invitation is needed to come back).
    func leave() {
        guard let s = seat else { return }
        TogetherInbox.shared.forget(s.code)
        let c = core
        clear()
        if let c { Task.detached { try? await c.togetherLeave(code: s.code, memberId: s.memberId) } }
    }

    private func clear() {
        loop?.cancel()
        loop = nil
        seat = nil
        state = nil
    }

    private func ensureLoop() {
        if loop != nil || seat == nil { return }
        loop = Task {
            var failures = 0
            var rejoined = false
            while !Task.isCancelled {
                guard let c = core, let s = seat else { break }
                let answer: TogetherState
                do {
                    answer = try await c.togetherWait(code: s.code, memberId: s.memberId, after: state?.seq ?? 0)
                } catch {
                    if Task.isCancelled { break }
                    if case .NotFound = error as? CoreError {
                        // A dropped seat may come back once; else the room is gone.
                        if !rejoined {
                            rejoined = true
                            do {
                                let j = try await c.togetherJoin(code: s.code)
                                if seat?.code == s.code {
                                    seat = Seat(code: s.code, memberId: j.memberId)
                                    accept(j.state, force: true)
                                }
                                continue
                            } catch {
                                if case .NotFound = error as? CoreError {} else {
                                    try? await Task.sleep(nanoseconds: 3_000_000_000)
                                    continue
                                }
                            }
                        }
                        if seat?.code == s.code {
                            TogetherInbox.shared.forget(s.code)
                            clear()
                            ended = tr("This room has closed.")
                        }
                        break
                    }
                    failures += 1
                    try? await Task.sleep(nanoseconds: UInt64(min(failures * 2000, 15000)) * 1_000_000)
                    continue
                }
                failures = 0
                rejoined = false
                if seat?.code != s.code { break }
                accept(answer, force: true)
                updates.send(answer)
            }
            loop = nil
        }
    }
}

/// My rooms and invitations, kept current by a long poll while the app is in front and an
/// AlwyWatch server is connected (Android's TogetherInbox). A new invitation becomes `prompt`.
@MainActor
final class TogetherInbox: ObservableObject {
    static let shared = TogetherInbox()

    @Published private(set) var rooms: TogetherRooms?
    /// The last try failed (the list shown is the last known one).
    @Published private(set) var unreachable = false
    /// An invitation to offer with Join / Not now.
    @Published var prompt: TogetherRoom?
    /// The player is in front (no prompt over it: a note instead).
    var playerVisible = false

    private var known = Set<String>()
    private var gone = Set<String>()
    private var job: Task<Void, Never>?
    private var core: AlwyWatch?

    func start(_ core: AlwyWatch) {
        self.core = core
        if job != nil { return }
        job = Task {
            var version: UInt64 = 0
            var failures = 0
            while !Task.isCancelled {
                guard let c = self.core else { break }
                if !c.keepActive() {
                    rooms = nil
                    prompt = nil
                    version = 0
                    try? await Task.sleep(nanoseconds: 10_000_000_000)
                    continue
                }
                do {
                    let r = try await c.togetherRoomsWait(after: version)
                    failures = 0
                    unreachable = false
                    version = r.version
                    take(r)
                } catch {
                    if Task.isCancelled { break }
                    unreachable = true
                    failures += 1
                    try? await Task.sleep(nanoseconds: UInt64(min(failures * 3000, 30000)) * 1_000_000)
                }
            }
        }
    }

    func stop() {
        job?.cancel()
        job = nil
    }

    /// Another profile takes over: its rooms and invitations aren't this one's.
    func reset() {
        stop()
        core = nil
        rooms = nil
        prompt = nil
        unreachable = false
        known = []
        gone = []
    }

    /// Left or declined: no prompt for it again.
    func forget(_ code: String) {
        gone.insert(code)
        if prompt?.code == code { prompt = nil }
        if var r = rooms {
            r.rooms = r.rooms.filter { $0.code != code || $0.mine }
            rooms = r
        }
    }

    /// A fresh list (Together page): shown, no prompt for what it lists.
    func refresh(_ core: AlwyWatch) async {
        do {
            let r = try await core.togetherRooms()
            prune(r)
            for room in r.rooms where room.invited { known.insert(room.code) }
            rooms = r
            unreachable = false
        } catch {
            unreachable = true
        }
    }

    /// A room gone from the list may invite me again later (the host left it to someone who
    /// invites me back): that invitation is a new one, prompted like any other.
    private func prune(_ r: TogetherRooms) {
        known = known.filter { c in r.rooms.contains { $0.code == c && $0.invited } }
        gone = gone.filter { c in r.rooms.contains { $0.code == c } }
    }

    private func take(_ r: TogetherRooms) {
        rooms = r
        prune(r)
        let invited = r.rooms.filter(\.invited)
        let fresh = invited.filter { !known.contains($0.code) && !gone.contains($0.code) }
        for room in invited { known.insert(room.code) }
        // Withdrawn, or the room closed: no prompt for it any more.
        if let p = prompt, !invited.contains(where: { $0.code == p.code }) { prompt = nil }
        if let newest = fresh.first, prompt == nil, !playerVisible { prompt = newest }
    }
}

// ----- Opening rooms -------------------------------------------------------------------------

/// "Watch together" on a title: opens a private waiting room with it (never the player: the
/// host starts when everyone is there). Hosting a room already: this title becomes the room's.
@MainActor
func createRoom(model: AppModel, account: String, itemId: String, resumeMs: UInt64) async throws {
    let session = TogetherSession.shared
    if let mine = session.seat, session.isHost {
        let s = try await model.core.togetherUpdate(code: mine.code, memberId: mine.memberId, playing: false, positionMs: resumeMs, accountId: account, itemId: itemId)
        session.accept(s)
        session.startAt = resumeMs
        session.show = mine.code
        return
    }
    let joined = try await model.core.togetherCreate(accountId: account, itemId: itemId)
    // One room at a time on a device.
    if let seat = session.seat, seat.code != joined.state.code { session.leave() }
    session.begin(model.core, joined, startAt: resumeMs)
    session.show = joined.state.code
}

/// "New room": a waiting room with no title yet.
@MainActor
func openEmptyRoom(model: AppModel) async throws -> String {
    let session = TogetherSession.shared
    let joined = try await model.core.togetherOpen()
    if let seat = session.seat, seat.code != joined.state.code { session.leave() }
    session.begin(model.core, joined)
    return joined.state.code
}

/// An invitation accepted: my seat in the room.
@MainActor
func joinRoom(model: AppModel, code: String) async throws {
    let session = TogetherSession.shared
    let joined = try await model.core.togetherJoin(code: code)
    if let seat = session.seat, seat.code != code { session.leave() }
    session.begin(model.core, joined)
    if TogetherInbox.shared.prompt?.code == code { TogetherInbox.shared.prompt = nil }
}

/// The player, in sync with the started room this device sits in. A problem to show, or nil.
@MainActor
func openRoomPlayer(model: AppModel) async -> String? {
    let session = TogetherSession.shared
    guard let s = session.state, let seat = session.seat else { return nil }
    session.handledRound = max(session.handledRound, s.round)
    guard let found = (try? await model.core.togetherFind(accountIds: preferredFirst(model), media: s.media)) ?? nil else {
        return tr("{title} isn't on your servers, so you can't watch it with them.", ["title": s.media.label])
    }
    var item = ((try? model.core.cachedItem(accountId: found.accountId, itemId: found.itemId)) ?? nil)?.item
    if item == nil { item = try? await model.core.refreshItem(accountId: found.accountId, itemId: found.itemId, force: false).item }
    guard let item else { return tr("This file can't be played on this device.") }
    let now = session.state ?? s
    let start = togetherPositionAt(state: now, serverNowMs: model.core.togetherNowMs())
    PlayerPresenter.shared.play(
        PlayRequest(account: found.accountId, item: item, startMs: start, room: seat),
        core: model.core
    )
    return nil
}

// ----- Pieces --------------------------------------------------------------------------------

/// The room's title on this user's servers (each member plays their own copy).
private struct RoomPoster: View {
    @EnvironmentObject var model: AppModel
    let media: TogetherMedia
    var width: CGFloat = 64
    @State private var found: (account: String, item: Item)?

    var body: some View {
        ZStack {
            Palette.surface
            if let found {
                ServerImage(account: found.account, itemId: found.item.id, image: found.item.poster, width: width)
            } else {
                Image(systemName: "person.2").foregroundStyle(Palette.muted)
            }
        }
        .frame(width: width, height: width * 1.5)
        .clipShape(RoundedRectangle(cornerRadius: 8))
        .task(id: media.label) {
            guard let m = (try? await model.core.togetherFind(accountIds: preferredFirst(model), media: media)) ?? nil else { return }
            var item = ((try? model.core.cachedItem(accountId: m.accountId, itemId: m.itemId)) ?? nil)?.item
            if item == nil { item = try? await model.core.refreshItem(accountId: m.accountId, itemId: m.itemId, force: false).item }
            if let item { found = (m.accountId, item) }
        }
    }
}

private func roomStatus(_ room: TogetherRoom) -> String {
    let others = room.members.count
    var who: String?
    if others == 1 {
        who = room.members[0]
    } else if others > 1 {
        who = trn("{name} and {n} other", "{name} and {n} others", others - 1, ["name": room.members[0]])
    }
    let moved = room.playing ? UInt64(max(0, room.nowMs - room.atMs)) : 0
    let status = room.started ? tr("Watching now · {time}", ["time": formatTime(room.positionMs + moved)]) : tr("Waiting room")
    return [status, who].compactMap { $0 }.joined(separator: " · ")
}

private func listHeader(_ text: String) -> some View {
    Text(text.uppercased()).font(.caption.weight(.semibold)).foregroundStyle(Palette.subtle).padding(.top, 8).padding(.leading, 4)
}

// ----- Together page -------------------------------------------------------------------------

/// Watch together (a tab): the rooms I'm in and the ones I'm invited to (nobody sees others).
struct TogetherView: View {
    @EnvironmentObject var model: AppModel
    @ObservedObject private var inbox = TogetherInbox.shared
    @State private var loaded = false
    @State private var opening = false
    @State private var problem: String?

    var body: some View {
        let list = inbox.rooms?.rooms ?? []
        let invites = list.filter(\.invited)
        let mine = list.filter(\.mine)
        ScrollView {
            LazyVStack(alignment: .leading, spacing: 12) {
                Text(tr("Rooms are private: only the people you invite can see them and join."))
                    .foregroundStyle(Palette.muted)
                // A room first, the title afterwards: people (and my other devices) join meanwhile.
                Button {
                    guard !opening else { return }
                    opening = true
                    problem = nil
                    Task {
                        do {
                            TogetherSession.shared.show = try await openEmptyRoom(model: model)
                        } catch {
                            problem = friendly(error)
                        }
                        opening = false
                    }
                } label: {
                    Label(opening ? tr("Opening a room…") : tr("New room"), systemImage: "plus")
                }
                .buttonStyle(ActionButtonStyle(primary: invites.isEmpty && mine.isEmpty))
                Text(tr("Invite people first, choose what to watch afterwards.")).font(.footnote).foregroundStyle(Palette.muted)
                ErrorText(text: problem)
                if loaded, invites.isEmpty, mine.isEmpty {
                    Text(inbox.unreachable && inbox.rooms == nil
                        ? tr("Can't reach your AlwyWatch server right now. Trying again…")
                        : tr("No rooms yet. Start one here, or open a movie or an episode and choose Watch together."))
                        .padding(.top, 12)
                }
                if !invites.isEmpty {
                    listHeader(tr("Invitations"))
                    ForEach(invites, id: \.code) { InvitationRow(room: $0) }
                }
                if !mine.isEmpty {
                    listHeader(tr("Your rooms"))
                    ForEach(mine, id: \.code) { room in
                        NavigationLink(value: Route.room(code: room.code)) { MyRoomRow(room: room) }
                            .buttonStyle(.plain)
                    }
                }
            }
            .padding(16)
        }
        .background(Palette.background.ignoresSafeArea())
        .navigationTitle(tr("Watch together"))
        .refreshable { await inbox.refresh(model.core) }
        .task {
            await inbox.refresh(model.core)
            loaded = true
        }
    }
}

/// "<host> invites you": Join / Not now.
private struct InvitationRow: View {
    @EnvironmentObject var model: AppModel
    let room: TogetherRoom
    @State private var busy = false
    @State private var error: String?

    var body: some View {
        HStack(alignment: .top, spacing: 14) {
            RoomPoster(media: room.media)
            VStack(alignment: .leading, spacing: 6) {
                HStack(spacing: 8) {
                    PersonAvatar(avatar: room.hostAvatar, name: room.host, userKey: room.hostUserKey, size: 28)
                    Text(tr("{name} invites you", ["name": room.host])).font(.subheadline).lineLimit(1)
                }
                Text(room.media.label).font(.headline).lineLimit(2)
                Text(roomStatus(room)).font(.caption).foregroundStyle(Palette.muted).lineLimit(2)
                HStack(spacing: 10) {
                    Button {
                        guard !busy else { return }
                        busy = true
                        error = nil
                        Task {
                            do {
                                try await joinRoom(model: model, code: room.code)
                                TogetherSession.shared.show = room.code
                            } catch {
                                self.error = friendly(error)
                            }
                            busy = false
                        }
                    } label: {
                        Label(busy ? tr("Joining…") : tr("Join"), systemImage: "play.fill")
                    }
                    .buttonStyle(ActionButtonStyle(primary: true))
                    Button(tr("Not now")) {
                        let code = room.code
                        TogetherInbox.shared.forget(code)
                        Task.detached { [core = model.core] in try? await core.togetherDecline(code: code) }
                    }
                    .buttonStyle(ActionButtonStyle())
                }
                .padding(.top, 4)
                ErrorText(text: error)
            }
            Spacer(minLength: 0)
        }
        .padding(14)
        .background(Palette.surface, in: RoundedRectangle(cornerRadius: 16))
    }
}

/// A room I'm in: opens the Room screen.
private struct MyRoomRow: View {
    let room: TogetherRoom

    var body: some View {
        HStack(spacing: 14) {
            RoomPoster(media: room.media)
            VStack(alignment: .leading, spacing: 3) {
                Text(room.media.label).font(.headline).lineLimit(2)
                Text(room.hosting ? tr("You're the host") : tr("Hosted by {name}", ["name": room.host])).font(.subheadline).lineLimit(1)
                Text(roomStatus(room)).font(.caption).foregroundStyle(Palette.muted).lineLimit(2)
            }
            Spacer(minLength: 0)
            Text(tr("Open")).font(.subheadline.weight(.semibold)).foregroundStyle(.black)
                .padding(.horizontal, 14).padding(.vertical, 8).background(Palette.accent, in: Capsule())
        }
        .padding(10)
        .background(Palette.surface, in: RoundedRectangle(cornerRadius: 16))
        .contentShape(Rectangle())
    }
}

/// Home: invitations waiting for an answer, as a row (nothing when there are none).
struct TogetherRail: View {
    @ObservedObject private var inbox = TogetherInbox.shared

    var body: some View {
        let invites = (inbox.rooms?.rooms ?? []).filter(\.invited)
        if !invites.isEmpty {
            VStack(alignment: .leading, spacing: 10) {
                VStack(alignment: .leading, spacing: 2) {
                    Text(tr("Invitations to watch together")).font(.title3.weight(.bold))
                    Text(tr("Join or answer later")).font(.footnote).foregroundStyle(Palette.muted)
                }
                .padding(.horizontal, 16)
                ScrollView(.horizontal, showsIndicators: false) {
                    LazyHStack(spacing: 12) {
                        ForEach(invites, id: \.code) { room in
                            Button { inbox.prompt = room } label: {
                                HStack(spacing: 12) {
                                    RoomPoster(media: room.media, width: 56)
                                    VStack(alignment: .leading, spacing: 4) {
                                        HStack(spacing: 8) {
                                            PersonAvatar(avatar: room.hostAvatar, name: room.host, userKey: room.hostUserKey, size: 24)
                                            Text(tr("{name} invites you", ["name": room.host])).font(.subheadline).lineLimit(1)
                                        }
                                        Text(room.media.label).font(.subheadline.weight(.semibold)).lineLimit(2)
                                        Text(room.started ? tr("Watching now") : tr("Waiting room")).font(.caption).foregroundStyle(Palette.muted)
                                    }
                                    Spacer(minLength: 0)
                                }
                                .padding(10)
                                .frame(width: 320)
                                .background(Palette.surface, in: RoundedRectangle(cornerRadius: 16))
                            }
                            .buttonStyle(.plain)
                        }
                    }
                    .padding(.horizontal, 16)
                }
            }
        }
    }
}

/// A new invitation, wherever the user is in the app: Join, or Not now (declines). Closing it
/// without answering leaves it on the Together page.
struct InvitationPrompt: View {
    @EnvironmentObject var model: AppModel
    let room: TogetherRoom
    @State private var busy = false
    @State private var error: String?

    var body: some View {
        VStack(spacing: 14) {
            PersonAvatar(avatar: room.hostAvatar, name: room.host, userKey: room.hostUserKey, size: 56)
            Text(tr("{name} invites you to watch together", ["name": room.host])).font(.title3.weight(.bold)).multilineTextAlignment(.center)
            Text(room.media.label).font(.headline)
            Text(room.started ? tr("They're already watching: you'll join where they are.") : tr("They're in the waiting room."))
                .foregroundStyle(Palette.muted).multilineTextAlignment(.center)
            ErrorText(text: error)
            HStack(spacing: 10) {
                Button(tr("Not now")) {
                    let code = room.code
                    TogetherInbox.shared.forget(code)
                    Task.detached { [core = model.core] in try? await core.togetherDecline(code: code) }
                }
                .buttonStyle(ActionButtonStyle())
                Button {
                    guard !busy else { return }
                    busy = true
                    error = nil
                    Task {
                        do {
                            try await joinRoom(model: model, code: room.code)
                            TogetherInbox.shared.prompt = nil
                            TogetherSession.shared.show = room.code
                        } catch {
                            self.error = friendly(error)
                        }
                        busy = false
                    }
                } label: {
                    Label(busy ? tr("Joining…") : tr("Join"), systemImage: "play.fill")
                }
                .buttonStyle(ActionButtonStyle(primary: true))
            }
        }
        .padding(24)
        .frame(maxWidth: .infinity)
        .background(Palette.background.ignoresSafeArea())
        .presentationDetents([.medium])
    }
}

// ----- Room screen ---------------------------------------------------------------------------

/// A watch together room: the title, who is in it, who is invited. The host invites people and
/// starts; everyone else waits. When it starts, the player opens in sync for everyone here.
struct RoomView: View {
    @EnvironmentObject var model: AppModel
    @Environment(\.dismiss) private var dismiss
    @ObservedObject private var session = TogetherSession.shared
    let code: String
    @State private var problem: String?
    @State private var joining = false
    @State private var hadSeat = false
    @State private var inviting = false
    @State private var confirmLeave = false
    @State private var busy = false

    var body: some View {
        let seat = session.seat?.code == code ? session.seat : nil
        let state = seat != nil && session.state?.code == code ? session.state : nil
        ScrollView {
            LazyVStack(alignment: .leading, spacing: 20) {
                if let state, let seat {
                    content(state, seat)
                } else {
                    Text(problem ?? tr("Opening the room…"))
                    if problem != nil {
                        Button(tr("Back")) { dismiss() }.buttonStyle(ActionButtonStyle(primary: true))
                    }
                }
            }
            .padding(16)
        }
        .background(Palette.background.ignoresSafeArea())
        .navigationTitle(tr("Watch together"))
        .navigationBarTitleDisplayMode(.inline)
        .task(id: code) {
            if session.seat?.code != code {
                joining = true
                do {
                    let j = try await model.core.togetherJoin(code: code)
                    if session.seat != nil { session.leave() }
                    session.begin(model.core, j)
                } catch {
                    problem = friendly(error)
                }
                joining = false
            }
        }
        // The room's heartbeat while this screen is shown (the player takes over when it opens).
        .onAppear { session.attach(model.core) }
        .onDisappear { session.detach() }
        // The room went away (or we left it): say why, or just go back.
        .onChange(of: seat != nil) { has in
            if has {
                hadSeat = true
            } else if hadSeat, !joining {
                let why = session.ended
                session.ended = nil
                if let why { problem = why } else { dismiss() }
            }
        }
        .onAppear { if seat != nil { hadSeat = true } }
        .sheet(isPresented: $inviting) {
            if let state, let seat { InvitePeopleSheet(state: state, memberId: seat.memberId).environmentObject(model) }
        }
        .confirmationDialog(tr("Leave this room?"), isPresented: $confirmLeave, titleVisibility: .visible) {
            Button(tr("Leave"), role: .destructive) { session.leave() }
        } message: {
            Text(leaveText(state, seat))
        }
    }

    private func leaveText(_ st: TogetherState?, _ seat: TogetherSession.Seat?) -> String {
        guard let st, let seat else { return tr("To come back, you'll need a new invitation.") }
        let hosting = togetherIsHost(state: st, memberId: seat.memberId)
        let myKey = st.members.first { $0.id == seat.memberId }?.userKey
        let othersHere = st.members.contains { $0.userKey != myKey }
        if hosting && othersHere { return tr("You're the host: the room stays open for the others and one of them becomes the host.") }
        if hosting { return tr("You're alone in it: the room closes, and the invitations you sent are withdrawn.") }
        return tr("To come back, you'll need a new invitation.")
    }

    @ViewBuilder private func content(_ state: TogetherState, _ seat: TogetherSession.Seat) -> some View {
        let host = state.members.first { $0.id == state.host }
        let hostName = host?.name ?? tr("the host")
        let isHost = togetherIsHost(state: state, memberId: seat.memberId)
        let myKey = state.members.first { $0.id == seat.memberId }?.userKey
        let untitled = state.media.untitled
        let started = state.started
        // Back in the waiting room after watching something together.
        let again = state.round > 0
        // Host, nobody else in the room or invited yet: Invite first.
        let alone = state.members.allSatisfy { $0.userKey == myKey } && state.invited.isEmpty
        Text(started ? tr("Watching now") : tr("Waiting room")).foregroundStyle(Palette.muted)
        HStack(spacing: 16) {
            RoomPoster(media: state.media, width: 88)
            VStack(alignment: .leading, spacing: 6) {
                Text(state.media.label).font(.title2.weight(.bold)).lineLimit(3)
                Text(isHost ? tr("You're the host") : tr("Hosted by {name}", ["name": hostName])).foregroundStyle(Palette.muted)
                if !started {
                    Text(waitingText(untitled: untitled, isHost: isHost, again: again, hostName: hostName))
                        .font(.subheadline)
                        .foregroundStyle(isHost ? Palette.muted : Palette.onSurface)
                }
            }
        }
        LazyVGrid(columns: [GridItem(.adaptive(minimum: 150), spacing: 10, alignment: .leading)], alignment: .leading, spacing: 10) {
            if started {
                Button {
                    Task { problem = await openRoomPlayer(model: model) }
                } label: {
                    Label(tr("Watch with them"), systemImage: "play.fill")
                }
                .buttonStyle(ActionButtonStyle(primary: true))
            } else if isHost && untitled {
                Button {
                    session.show = nil
                    model.chooseForRoom()
                } label: {
                    Label(tr("Choose what to watch"), systemImage: "square.stack")
                }
                .buttonStyle(ActionButtonStyle(primary: !alone))
            } else if isHost {
                let resume = session.startAt
                Button {
                    start(seat, resume)
                } label: {
                    Label(busy ? tr("Starting…") : resume > 0 ? tr("Start at {time}", ["time": formatTime(resume)]) : tr("Start watching"), systemImage: "play.fill")
                }
                .buttonStyle(ActionButtonStyle(primary: !alone))
                if resume > 0 {
                    Button(tr("From the beginning")) { start(seat, 0) }.buttonStyle(ActionButtonStyle())
                }
            }
            if isHost {
                Button { inviting = true } label: { Label(tr("Invite people"), systemImage: "plus") }
                    .buttonStyle(ActionButtonStyle(primary: alone && !started))
            }
            Button { confirmLeave = true } label: { Label(tr("Leave"), systemImage: "xmark") }
                .buttonStyle(ActionButtonStyle())
        }
        ErrorText(text: problem ?? (started ? session.problem : nil))
        let all = people(state.members)
        VStack(alignment: .leading, spacing: 0) {
            listHeader(all.count == 1 ? tr("In the room") : tr("In the room · {count}", ["count": all.count])).padding(.bottom, 6)
            ForEach(all) { entry in
                let m = entry.member
                let notes = [
                    host != nil && m.userKey == host!.userKey ? tr("Host") : nil,
                    entry.devices > 1 ? trn("on {n} device", "on {n} devices", entry.devices) : nil,
                ].compactMap { $0 }.joined(separator: " · ")
                personRow(m.avatar, m.name, m.userKey, title: m.userKey == myKey ? tr("{name} (you)", ["name": m.name]) : m.name, subtitle: notes.isEmpty ? nil : notes)
            }
        }
        if !state.invited.isEmpty {
            VStack(alignment: .leading, spacing: 0) {
                listHeader(tr("Invited")).padding(.bottom, 6)
                ForEach(state.invited, id: \.userKey) { p in
                    HStack {
                        personRow(p.avatar, p.name, p.userKey, title: p.name, subtitle: tr("Hasn't joined yet"))
                        if isHost {
                            Button {
                                Task {
                                    do {
                                        session.accept(try await model.core.togetherUninvite(code: code, memberId: seat.memberId, userKey: p.userKey))
                                    } catch {
                                        problem = friendly(error)
                                    }
                                }
                            } label: {
                                Image(systemName: "xmark").frame(width: 40, height: 40)
                            }
                            .accessibilityLabel(tr("Remove {name}", ["name": p.name]))
                        }
                    }
                }
                Text(tr("They can join until they say no.")).font(.footnote).foregroundStyle(Palette.muted).padding(.top, 6)
            }
        }
    }

    private func personRow(_ avatar: Avatar, _ name: String, _ key: String, title: String, subtitle: String?) -> some View {
        HStack(spacing: 12) {
            PersonAvatar(avatar: avatar, name: name, userKey: key, size: 40)
            VStack(alignment: .leading, spacing: 2) {
                Text(title)
                if let subtitle { Text(subtitle).font(.caption).foregroundStyle(Palette.muted) }
            }
            Spacer(minLength: 0)
        }
        .padding(.vertical, 6)
    }

    private func waitingText(untitled: Bool, isHost: Bool, again: Bool, hostName: String) -> String {
        if untitled && isHost {
            return tr("Open a movie or an episode and choose Watch in your room. The people you invite can join in the meantime.")
        }
        if untitled { return tr("{name} is choosing what to watch", ["name": hostName]) }
        if isHost && again {
            return tr("Start this again, or pick another title: open a movie or an episode and choose Watch in your room. Everyone stays here.")
        }
        if isHost { return tr("Invite people, then start when everyone is here. Play, pause and seeking follow everyone.") }
        if again { return tr("{name} is picking what to watch next", ["name": hostName]) }
        return tr("Waiting for {name} to start", ["name": hostName])
    }

    private func start(_ seat: TogetherSession.Seat, _ from: UInt64) {
        guard !busy else { return }
        busy = true
        problem = nil
        Task {
            do {
                session.accept(try await model.core.togetherStart(code: code, memberId: seat.memberId, positionMs: from))
            } catch {
                problem = friendly(error)
            }
            busy = false
        }
    }
}

/// The host picks people to invite: the ones they last watched with first, then everyone by
/// name. Several at once, then one "Send".
struct InvitePeopleSheet: View {
    @EnvironmentObject var model: AppModel
    @Environment(\.dismiss) private var dismiss
    let state: TogetherState
    let memberId: String
    @State private var people: [TogetherPerson]?
    @State private var error: String?
    @State private var selected: [String] = []
    @State private var query = ""
    @State private var busy = false

    var body: some View {
        let present = Set(state.members.map(\.userKey) + state.invited.map(\.userKey))
        // Recent partners first (most recent first), then everyone by name.
        let all = (people ?? []).filter { !present.contains($0.userKey) }.sorted { a, b in
            if (a.lastTogetherAt == nil) != (b.lastTogetherAt == nil) { return a.lastTogetherAt != nil }
            if let x = a.lastTogetherAt, let y = b.lastTogetherAt, x != y { return x > y }
            return a.name.lowercased() < b.name.lowercased()
        }
        let q = query.trimmingCharacters(in: .whitespaces)
        let shown = q.isEmpty ? all : all.filter { $0.name.localizedCaseInsensitiveContains(q) || $0.mediaName.localizedCaseInsensitiveContains(q) }
        let counts = Dictionary(grouping: all) { $0.name.lowercased() }.mapValues(\.count)
        let n = selected.count
        NavigationStack {
            List {
                if people == nil {
                    Text(tr("Loading…")).foregroundStyle(Palette.muted)
                } else if all.isEmpty {
                    Text(error ?? ((people ?? []).isEmpty ? tr("Nobody else uses this AlwyWatch server yet.") : tr("Everyone is already here or invited.")))
                        .foregroundStyle(Palette.muted)
                } else if shown.isEmpty {
                    Text(tr("Nobody by that name.")).foregroundStyle(Palette.muted)
                }
                ForEach(shown, id: \.userKey) { p in
                    let i = shown.firstIndex { $0.userKey == p.userKey } ?? 0
                    let recent = p.lastTogetherAt != nil
                    if q.isEmpty, recent, i == 0 { listHeader(tr("Recent")) }
                    if q.isEmpty, !recent, i == 0 || shown[i - 1].lastTogetherAt != nil, shown.first?.lastTogetherAt != nil { listHeader(tr("Everyone")) }
                    let on = selected.contains(p.userKey)
                    let sub = [
                        !p.mediaName.isEmpty && p.mediaName.lowercased() != p.name.lowercased() ? p.mediaName : nil,
                        !p.serverName.isEmpty && (counts[p.name.lowercased()] ?? 0) > 1 ? p.serverName : nil,
                    ].compactMap { $0 }.joined(separator: " · ")
                    Button {
                        if on { selected.removeAll { $0 == p.userKey } } else { selected.append(p.userKey) }
                    } label: {
                        HStack(spacing: 12) {
                            PersonAvatar(avatar: p.avatar, name: p.name, userKey: p.userKey, size: 40)
                            VStack(alignment: .leading, spacing: 2) {
                                Text(p.name).foregroundStyle(Palette.onSurface)
                                if !sub.isEmpty { Text(sub).font(.caption).foregroundStyle(Palette.muted) }
                            }
                            Spacer()
                            Image(systemName: on ? "checkmark.circle.fill" : "circle").font(.title2)
                                .foregroundStyle(on ? Palette.accent : Palette.subtle)
                                .accessibilityLabel(on ? tr("Selected") : "")
                        }
                    }
                }
                if let error, !all.isEmpty { ErrorText(text: error) }
            }
            .scrollContentBackground(.hidden)
            .background(Palette.background.ignoresSafeArea())
            .navigationTitle(tr("Invite people"))
            .navigationBarTitleDisplayMode(.inline)
            .searchable(text: $query, prompt: tr("Search by name"))
            .toolbar {
                ToolbarItem(placement: .cancellationAction) { Button(tr("Cancel")) { dismiss() } }
                ToolbarItem(placement: .confirmationAction) {
                    Button(busy ? tr("Sending…") : n == 0 ? tr("Send invitations") : trn("Send {n} invitation", "Send {n} invitations", n)) { send() }
                        .disabled(n == 0 || busy)
                }
            }
            .task {
                do {
                    people = try await model.core.togetherPeople()
                } catch {
                    self.error = friendly(error)
                    people = []
                }
            }
        }
    }

    private func send() {
        guard !busy, !selected.isEmpty else { return }
        busy = true
        error = nil
        Task {
            do {
                var rest = selected[...]
                while !rest.isEmpty {
                    let chunk = Array(rest.prefix(20))
                    rest = rest.dropFirst(20)
                    TogetherSession.shared.accept(try await model.core.togetherInvite(code: state.code, memberId: memberId, userKeys: chunk))
                }
                dismiss()
            } catch {
                self.error = friendly(error)
            }
            busy = false
        }
    }
}
