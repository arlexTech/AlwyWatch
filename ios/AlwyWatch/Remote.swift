// SPDX-License-Identifier: AGPL-3.0-or-later
import SwiftUI

/// Play on the TV, the phone side (docs/keep-protocol.md "Play on the TV"), as on Android: my
/// TVs with AlwyWatch open (asked every 20 s while the app is in front), Play asking "This
/// phone" or a TV, and a mini remote.
final class RemotePhone: ObservableObject {
    static let shared = RemotePhone()

    /// My TVs there now (empty: Play starts here, as before).
    @Published private(set) var devices: [RemoteDevice] = []
    /// A title waiting for "This phone" or a TV.
    @Published var asking: Asking?
    /// The mini remote shown (a sheet over everything).
    @Published var remote: RemoteTarget?

    struct Asking: Identifiable {
        let request: PlayRequest
        let onClose: () -> Void
        var id: String { request.id }
    }

    private var job: Task<Void, Never>?

    func start(_ core: AlwyWatch) {
        job?.cancel()
        job = Task {
            while !Task.isCancelled {
                await refresh(core)
                try? await Task.sleep(nanoseconds: 20_000_000_000)
            }
        }
    }

    func stop() {
        job?.cancel()
        job = nil
    }

    @MainActor
    func refresh(_ core: AlwyWatch) async {
        guard core.keepActive() else {
            devices = []
            return
        }
        if let d = try? await core.remoteDevices() { devices = d }
    }

    /// Play: here at once when no TV is there, else ask where.
    @MainActor
    func play(_ request: PlayRequest, core: AlwyWatch, onClose: @escaping () -> Void = {}) {
        if devices.isEmpty {
            PlayerPresenter.shared.play(request, core: core, onClose: onClose)
        } else {
            asking = Asking(request: request, onClose: onClose)
        }
    }
}

/// The mini remote of TV `deviceId` (nil: the list of my TVs).
struct RemoteTarget: Identifiable {
    let deviceId: String?
    var id: String { deviceId ?? "" }
}

/// "Play on": this phone or one of my TVs.
struct RemoteChooser: ViewModifier {
    @EnvironmentObject var model: AppModel
    @ObservedObject private var remote = RemotePhone.shared
    @State private var error: String?

    func body(content: Content) -> some View {
        content
            .confirmationDialog(
                tr("Play on"),
                isPresented: Binding(get: { remote.asking != nil }, set: { if !$0 { remote.asking = nil } }),
                titleVisibility: .visible,
                presenting: remote.asking
            ) { pending in
                Button(tr("This phone")) {
                    remote.asking = nil
                    PlayerPresenter.shared.play(pending.request, core: model.core, onClose: pending.onClose)
                }
                ForEach(remote.devices, id: \.id) { tv in
                    Button(tv.name) {
                        remote.asking = nil
                        Task { await playOn(tv.id, pending.request) }
                    }
                }
                Button(tr("Cancel"), role: .cancel) { remote.asking = nil }
            }
            .alert(error ?? "", isPresented: Binding(get: { error != nil }, set: { if !$0 { error = nil } })) {
                Button(tr("Close"), role: .cancel) { error = nil }
            }
            .sheet(item: $remote.remote) { target in
                NavigationStack { RemoteView(deviceId: target.deviceId) }
                    .environmentObject(model)
                    .preferredColorScheme(.dark)
            }
    }

    @MainActor
    private func playOn(_ device: String, _ r: PlayRequest) async {
        // What the button said: from the start, a given point, or the resume point.
        let position = r.fromStart ? 0 : Int64(r.startMs ?? r.item.user.positionMs)
        do {
            try await model.core.remotePlay(deviceId: device, accountId: r.account, itemId: r.item.id, positionMs: position)
            remote.remote = RemoteTarget(deviceId: device)
        } catch {
            self.error = message(error)
        }
    }
}

extension View {
    func remoteChooser() -> some View { modifier(RemoteChooser()) }
}

/// The mini remote: what the TV plays, where it is, pause, jumps, tracks and stop. Follows the
/// TV's reports (a long poll).
struct RemoteView: View {
    @EnvironmentObject var model: AppModel
    @ObservedObject private var remote = RemotePhone.shared
    @Environment(\.dismiss) private var dismiss
    let deviceId: String?
    @State private var picked: String?

    var body: some View {
        Group {
            if let id = deviceId ?? picked ?? (remote.devices.count == 1 ? remote.devices.first?.id : nil) {
                RemoteControls(deviceId: id, name: remote.devices.first { $0.id == id }?.name ?? tr("TV"))
            } else {
                List {
                    if remote.devices.isEmpty {
                        Text(tr("No TV here. Open AlwyWatch on the TV, with this profile, and it shows here."))
                            .foregroundStyle(Palette.muted)
                    }
                    ForEach(remote.devices, id: \.id) { tv in
                        Button { picked = tv.id } label: {
                            Label {
                                VStack(alignment: .leading) {
                                    Text(tv.name)
                                    Text(tv.title.isEmpty ? tr("Nothing playing") : tv.title)
                                        .font(.caption).foregroundStyle(Palette.muted)
                                }
                            } icon: { Image(systemName: "tv") }
                        }
                    }
                }
                .navigationTitle(tr("Play on the TV"))
            }
        }
        .toolbar {
            ToolbarItem(placement: .confirmationAction) { Button(tr("Close")) { dismiss() } }
        }
        .task { await remote.refresh(model.core) }
    }
}

private struct RemoteControls: View {
    @EnvironmentObject var model: AppModel
    let deviceId: String
    let name: String
    @State private var state: RemoteState?
    @State private var gone = false
    @State private var now: Int64 = 0
    @State private var dragging: Double?
    @State private var clock = RemoteClock()
    @State private var playing = false

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 18) {
                if gone {
                    Text(tr("This TV isn't there anymore: AlwyWatch was closed on it.")).foregroundStyle(Palette.muted)
                } else if let r = state?.report {
                    if let e = r.error { Text(e).foregroundStyle(Palette.accent) }
                    if r.title.isEmpty {
                        Text(tr("Nothing playing. Pick a title, then Play and this TV.")).foregroundStyle(Palette.muted)
                    } else {
                        playing(r)
                    }
                } else {
                    Text(tr("Connecting…")).foregroundStyle(Palette.muted)
                }
            }
            .padding(16)
        }
        .navigationTitle(name)
        .navigationBarTitleDisplayMode(.inline)
        .task(id: deviceId) { await follow() }
        .task(id: deviceId) {
            while !Task.isCancelled {
                now = clock.now()
                playing = clock.playing
                try? await Task.sleep(nanoseconds: 200_000_000)
            }
        }
    }

    @ViewBuilder private func playing(_ r: RemoteReport) -> some View {
        let duration = Double(max(r.durationMs, 1))
        VStack(alignment: .leading, spacing: 4) {
            Text(r.title).font(.title2.weight(.bold))
            if let s = r.subtitle { Text(s).foregroundStyle(Palette.muted) }
            if r.loading { Text(tr("Loading…")).font(.caption).foregroundStyle(Palette.muted) }
        }
        VStack(spacing: 2) {
            Slider(
                value: Binding(get: { dragging ?? min(max(Double(now) / duration, 0), 1) }, set: { dragging = $0 }),
                onEditingChanged: { editing in
                    if !editing, let d = dragging {
                        send(.seek, position: Int64(d * duration))
                        dragging = nil
                    }
                }
            )
            HStack {
                Text(formatTime(UInt64(max(dragging.map { Int64($0 * duration) } ?? now, 0))))
                Spacer()
                Text(formatTime(UInt64(max(r.durationMs, 0))))
            }
            .font(.caption).foregroundStyle(Palette.muted)
        }
        HStack(spacing: 28) {
            Spacer()
            Button { send(.seek, position: max(now - 10000, 0)) } label: { Image(systemName: "gobackward.10").font(.title) }
                .accessibilityLabel(tr("Back 10 seconds"))
            Button { send(playing ? .pause : .resume) } label: {
                Image(systemName: playing ? "pause.circle.fill" : "play.circle.fill").font(.system(size: 64))
            }
            .accessibilityLabel(playing ? tr("Pause") : tr("Play"))
            Button { send(.seek, position: min(now + 30000, r.durationMs)) } label: { Image(systemName: "goforward.30").font(.title) }
                .accessibilityLabel(tr("Forward 30 seconds"))
            Spacer()
        }
        .foregroundStyle(Palette.onSurface)
        if r.audio.count > 1 {
            Menu {
                ForEach(r.audio, id: \.index) { t in
                    Button { send(.audio, index: t.index) } label: {
                        if t.index == r.audioIndex { Label(t.label, systemImage: "checkmark") } else { Text(t.label) }
                    }
                }
            } label: {
                row(tr("Audio"), "speaker.wave.2", r.audio.first { $0.index == r.audioIndex }?.label ?? "")
            }
        }
        Menu {
            Button { send(.subtitles, index: nil) } label: {
                if r.subtitleIndex == nil { Label(tr("Off"), systemImage: "checkmark") } else { Text(tr("Off")) }
            }
            ForEach(r.subtitles, id: \.index) { t in
                Button { send(.subtitles, index: t.index) } label: {
                    if t.index == r.subtitleIndex { Label(t.label, systemImage: "checkmark") } else { Text(t.label) }
                }
            }
        } label: {
            row(tr("Subtitles"), "captions.bubble", r.subtitles.first { $0.index == r.subtitleIndex }?.label ?? tr("Off"))
        }
        Button { send(.stop) } label: { row(tr("Stop"), "stop.fill", "") }
    }

    private func row(_ title: String, _ icon: String, _ value: String) -> some View {
        HStack {
            Label(title, systemImage: icon)
            Spacer()
            Text(value).foregroundStyle(Palette.muted).lineLimit(1)
        }
        .padding(14)
        .background(Palette.surfaceHigh, in: RoundedRectangle(cornerRadius: 14))
        .foregroundStyle(Palette.onSurface)
    }

    private func send(_ action: RemoteAction, position: Int64? = nil, index: Int32? = nil) {
        switch action {
        case .pause: clock.acted(playing: false)
        case .resume: clock.acted(playing: true)
        case .seek: clock.acted(position: position)
        default: break
        }
        now = clock.now()
        playing = clock.playing
        Task { try? await model.core.remoteControl(deviceId: deviceId, action: action, positionMs: position, index: index) }
    }

    @MainActor
    private func follow() async {
        var after: UInt64 = 0
        var failures: UInt64 = 0
        while !Task.isCancelled {
            do {
                let s = try await model.core.remoteState(deviceId: deviceId, after: after)
                clock.take(s)
                state = s
                after = s.version
                gone = false
                failures = 0
            } catch CoreError.NotFound {
                gone = true
                try? await Task.sleep(nanoseconds: 5_000_000_000)
            } catch {
                if Task.isCancelled { break }
                failures += 1
                try? await Task.sleep(nanoseconds: min(failures * 2_000_000_000, 15_000_000_000))
            }
        }
    }
}

/// Where the TV is, here: a local clock set by each report and moved on while the TV plays
/// (smooth between reports), and moved at once by this phone's buttons. Reports the TV sent
/// before the last button press are stale for the position and play state.
final class RemoteClock {
    private var base: Int64 = 0
    private var baseAt: TimeInterval = 0
    private var duration: Int64 = 0
    private var receivedAt: TimeInterval = 0
    private var serverNow: Int64 = 0
    private var actedAt = Int64.min
    private(set) var playing = false

    private func mono() -> TimeInterval { ProcessInfo.processInfo.systemUptime }

    func take(_ s: RemoteState) {
        let t = mono()
        serverNow = s.nowMs
        receivedAt = t
        duration = s.report.durationMs
        guard s.atMs >= actedAt else { return }
        base = remotePositionNow(state: s)
        baseAt = t
        playing = s.report.playing && !s.report.loading
    }

    func now() -> Int64 {
        let p = playing ? base + Int64((mono() - baseAt) * 1000) : base
        return duration > 0 ? min(max(p, 0), duration) : max(p, 0)
    }

    func acted(playing: Bool? = nil, position: Int64? = nil) {
        base = position ?? now()
        baseAt = mono()
        if let playing { self.playing = playing }
        actedAt = serverNow + Int64((mono() - receivedAt) * 1000) + 250
    }
}
