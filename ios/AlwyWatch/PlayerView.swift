// SPDX-License-Identifier: AGPL-3.0-or-later
import AVFoundation
import AVKit
import Combine
import SwiftUI

/// What to play (a full-screen cover).
struct PlayRequest: Identifiable {
    let account: String
    let item: Item
    var fromStart = false
    /// Where to start (a watch together room's position), instead of the resume point.
    var startMs: UInt64?
    /// The watch together seat this playback follows.
    var room: TogetherSession.Seat?
    var id: String { item.id }
}

/// What this device plays without the server touching the file: AVPlayer's containers and
/// codecs (HEVC and Dolby Vision in hardware on every supported iPhone/iPad). The rest is
/// converted by the server (HLS, H.264).
enum DeviceProfiles {
    static let ios: DeviceProfile = {
        var hdr: [HdrKind] = [.sdr]
        if AVPlayer.eligibleForHDRPlayback { hdr += [.hdr10, .hlg, .dolbyVision] }
        return DeviceProfile(
            maxBitrate: 120_000_000,
            containers: ["mp4", "m4v", "mov"],
            videoCodecs: ["h264", "hevc"],
            video10bitCodecs: ["hevc"],
            audioCodecs: ["aac", "mp3", "ac3", "eac3", "alac", "flac"],
            // Text subtitles stay text (no conversion only to burn them in).
            subtitleCodecs: ["mov_text", "tx3g", "srt", "subrip", "vtt", "webvtt"],
            maxWidth: 3840,
            maxHeight: 2160,
            hdr: hdr
        )
    }()

    /// True when the original file plays as is here (Android's canDirectPlay).
    static func canDirectPlay(_ source: MediaSource, _ p: DeviceProfile) -> Bool {
        guard let video = source.streams.first(where: { $0.kind == .video }) else { return true }
        let container = source.container?.lowercased().split(separator: ",").first.map(String.init)
        let containerOk = container == nil || p.containers.contains(container!)
        let audio = source.streams.filter { $0.kind == .audio }
        let audioOk = audio.isEmpty || audio.contains { p.audioCodecs.contains($0.codec.lowercased()) }
        let depthOk = (video.bitDepth ?? 8) <= 8 || p.video10bitCodecs.contains(video.codec.lowercased())
        let sizeOk = (p.maxWidth.map { (video.width ?? 0) <= $0 } ?? true) && (p.maxHeight.map { (video.height ?? 0) <= $0 } ?? true)
        return p.videoCodecs.contains(video.codec.lowercased()) && depthOk && sizeOk && containerOk && audioOk
    }
}

/// Shows Apple's player full screen (its controls, AirPlay, audio and subtitle tracks of the
/// file) and keeps the playback alive in picture in picture after it is dismissed.
@MainActor
final class PlayerPresenter: NSObject, AVPlayerViewControllerDelegate {
    static let shared = PlayerPresenter()
    private var vc: PlayerViewController?
    private var playback: Playback?
    private var onClose: (() -> Void)?
    private var inPip = false

    func play(_ request: PlayRequest, core: AlwyWatch, onClose: @escaping () -> Void = {}) {
        finish()
        let playback = Playback()
        let vc = PlayerViewController()
        vc.player = playback.player
        vc.delegate = self
        vc.allowsPictureInPicturePlayback = true
        vc.canStartPictureInPictureAutomaticallyFromInline = true
        vc.updatesNowPlayingInfoCenter = true
        vc.modalPresentationStyle = .fullScreen
        vc.onGone = { [weak self, weak vc] in
            guard let self, vc === self.vc, !self.inPip else { return }
            self.finish()
        }
        playback.onError = { [weak vc] text in
            let alert = UIAlertController(title: nil, message: text, preferredStyle: .alert)
            alert.addAction(UIAlertAction(title: tr("Close"), style: .default) { _ in vc?.dismiss(animated: true) })
            vc?.present(alert, animated: true)
        }
        playback.onFinished = { [weak vc] in vc?.dismiss(animated: true) }
        // "Skip intro" / "Next episode": above the controls bar, bottom right (where Android
        // puts them). Sized to its content, so it never covers the video's gestures.
        let prompt = UIHostingController(rootView: PlayerPrompt(playback: playback))
        prompt.view.backgroundColor = .clear
        prompt.sizingOptions = .intrinsicContentSize
        vc.addChild(prompt)
        vc.view.addSubview(prompt.view)
        prompt.didMove(toParent: vc)
        prompt.view.translatesAutoresizingMaskIntoConstraints = false
        NSLayoutConstraint.activate([
            prompt.view.trailingAnchor.constraint(equalTo: vc.view.safeAreaLayoutGuide.trailingAnchor, constant: -16),
            prompt.view.bottomAnchor.constraint(equalTo: vc.view.safeAreaLayoutGuide.bottomAnchor, constant: -84),
        ])
        self.vc = vc
        self.playback = playback
        self.onClose = onClose
        Self.top()?.present(vc, animated: true)
        TogetherInbox.shared.playerVisible = true
        Task { await playback.start(core: core, request: request) }
    }

    /// A playback is showing (or in picture in picture).
    var isOpen: Bool { playback != nil }

    private func finish() {
        TogetherInbox.shared.playerVisible = false
        if playback != nil { NotificationCenter.default.post(name: .playbackEnded, object: nil) }
        playback?.stop()
        playback = nil
        vc = nil
        inPip = false
        let done = onClose
        onClose = nil
        done?()
    }

    private static func top() -> UIViewController? {
        let scenes = UIApplication.shared.connectedScenes.compactMap { $0 as? UIWindowScene }
        var top = scenes.flatMap(\.windows).first { $0.isKeyWindow }?.rootViewController
        while let presented = top?.presentedViewController { top = presented }
        return top
    }

    // AVKit calls its delegate on the main thread; synchronously, so the dismissal for picture
    // in picture already knows it isn't a close.
    func playerViewControllerWillStartPictureInPicture(_ playerViewController: AVPlayerViewController) {
        inPip = true
    }

    func playerViewControllerDidStopPictureInPicture(_ playerViewController: AVPlayerViewController) {
        inPip = false
        // Closed from the picture in picture window (not brought back): the end.
        if let vc, vc.presentingViewController == nil { finish() }
    }

    func playerViewController(
        _ playerViewController: AVPlayerViewController,
        restoreUserInterfaceForPictureInPictureStopWithCompletionHandler completionHandler: @escaping (Bool) -> Void
    ) {
        guard let vc, vc.presentingViewController == nil, let top = Self.top() else {
            completionHandler(true)
            return
        }
        top.present(vc, animated: true) { completionHandler(true) }
    }
}

/// Apple's player view, telling when it's gone (closed with its X, or for picture in picture).
final class PlayerViewController: AVPlayerViewController {
    var onGone: (() -> Void)?

    override func viewDidDisappear(_ animated: Bool) {
        super.viewDidDisappear(animated)
        if isBeingDismissed || presentingViewController == nil { onGone?() }
    }
}

/// "Skip intro" or "Next episode" (with its countdown during the credits), like Android.
struct PlayerPrompt: View {
    @ObservedObject var playback: Playback

    var body: some View {
        VStack(alignment: .trailing, spacing: 8) {
            // Watch together: who did what, and what the start waits for.
            if let note = playback.togetherWaiting ?? playback.togetherNote {
                Text(note)
                    .font(.subheadline.weight(.semibold))
                    .padding(.horizontal, 14).padding(.vertical, 8)
                    .background(Color.black.opacity(0.6), in: Capsule())
            }
            prompts
        }
        .foregroundStyle(.white)
        .padding(2)
    }

    private var prompts: some View {
        HStack(spacing: 8) {
            if playback.nearEnd {
                if playback.upNextIn != nil {
                    Button { playback.cancelCountdown() } label: {
                        Image(systemName: "xmark").font(.body.weight(.semibold)).frame(width: 40, height: 40)
                    }
                    .background(Color.black.opacity(0.6), in: Circle())
                    .accessibilityLabel(tr("Cancel"))
                }
                pill(playback.upNextIn.map { tr("Next episode in {seconds} s", ["seconds": $0]) } ?? tr("Next episode")) {
                    playback.playNext()
                }
            } else if let label = playback.skipLabel {
                pill(label) { playback.skip() }
            }
        }
    }

    private func pill(_ text: String, _ action: @escaping () -> Void) -> some View {
        Button(action: action) {
            Label(text, systemImage: "forward.end.fill")
                .font(.subheadline.weight(.semibold))
                .padding(.horizontal, 16)
                .frame(height: 40)
        }
        .background(Color.black.opacity(0.6), in: Capsule())
        .overlay(Capsule().stroke(Color.white.opacity(0.35), lineWidth: 1))
    }
}

/// One playback: picks the original file or the server's conversion, reports progress to the
/// server (start, every 10 s, pause, stop), falls back to the conversion when the original
/// fails, skips intros and offers the next episode (Android's segments and countdown), picks
/// audio and subtitles from the title's choice or the settings, and goes on with the next
/// episode at the end.
@MainActor
final class Playback: ObservableObject {
    let player = AVPlayer()
    @Published private(set) var skipLabel: String?
    @Published private(set) var nearEnd = false
    @Published private(set) var upNextIn: Int?
    @Published private(set) var togetherNote: String?
    @Published private(set) var togetherWaiting: String?
    var onError: ((String) -> Void)?
    var onFinished: (() -> Void)?
    private var error: String? {
        didSet { if let error { onError?(error) } }
    }

    private var core: AlwyWatch?
    private var account = ""
    private var item: Item?
    private var info: PlaybackInfo?
    private var source: MediaSource?
    private var transcoding = false
    private var tick: Any?
    private var observers: [NSKeyValueObservation] = []
    private var endObserver: NSObjectProtocol?
    private var lastRate: Float = 0
    private var stopped = false
    /// Where the current file was asked to start (a fallback before anything played).
    private var startMs: UInt64 = 0
    private var segments: [Segment] = []
    private var next: Item?
    private var autoSkipped = Set<UInt64>()
    private var countdownFrom: Date?
    private var countdownCancelled = false
    private var segmentTick: Any?
    /// The tracks this file offers, numbered for the core (audio first, then subtitles).
    private var tracks: [(info: TrackInfo, option: AVMediaSelectionOption, group: AVMediaSelectionGroup)] = []
    /// The selection we made (or last saw): a different one was picked in Apple's menu.
    private var shownAudio: AVMediaSelectionOption?
    private var shownSubtitle: AVMediaSelectionOption?
    private var tracksReady = false

    func start(core: AlwyWatch, request: PlayRequest) async {
        self.core = core
        account = request.account
        let item = request.item
        var at = request.fromStart || item.user.played ? 0 : item.user.positionMs
        // Resuming: play a few seconds again, to remember where you were.
        let rewind = UInt64(max(0, Int(core.getSettingOr("player.resume_rewind", "0")) ?? 0)) * 1000
        if at > 0 { at = at > rewind ? at - rewind : 0 }
        if let s = request.startMs { at = s }
        if let seat = request.room { joinRoom(seat) }
        await open(item, at: at)
    }

    private func open(_ item: Item, at startMs: UInt64, forceConversion: Bool = false, local: Bool = true) async {
        guard let core else { return }
        self.item = item
        self.startMs = startMs
        error = nil
        let profile = DeviceProfiles.ios
        // Downloaded: the local original file (works offline; progress is queued).
        if local, !forceConversion, let path = core.downloadedFile(accountId: account, itemId: item.id) {
            info = nil
            source = nil
            transcoding = false
            let playerItem = AVPlayerItem(url: URL(fileURLWithPath: path))
            playerItem.externalMetadata = metadata(item)
            playerItem.textStyleRules = subtitleStyle()
            resetPrompts()
            watch(playerItem, startMs: startMs)
            player.replaceCurrentItem(with: playerItem)
            seekHoldUntil = Date().addingTimeInterval(2)
            if startMs > 0 {
                _ = await player.seek(to: CMTime(value: CMTimeValue(startMs), timescale: 1000), toleranceBefore: .zero, toleranceAfter: .positiveInfinity)
            }
            if room == nil { player.play() }
            report(.start)
            loadExtras(item)
            return
        }
        do {
            let info = try await core.playbackInfo(accountId: account, itemId: item.id, profile: profile)
            guard let src = info.sources.first else { throw CoreError.NotFound(message: tr("Nothing to play.")) }
            let direct = !forceConversion && DeviceProfiles.canDirectPlay(src, profile)
            let chosen: HttpRequest? = direct ? src.direct : src.transcode
            guard let request = chosen, let url = URL(string: request.url) else {
                error = tr("This file can't be played on this device.")
                return
            }
            self.info = info
            source = src
            transcoding = !direct
            var headers: [String: String] = [:]
            for h in request.headers { headers[h.name] = h.value }
            let asset = AVURLAsset(url: url, options: ["AVURLAssetHTTPHeaderFieldsKey": headers])
            let playerItem = AVPlayerItem(asset: asset)
            playerItem.externalMetadata = metadata(item)
            playerItem.textStyleRules = subtitleStyle()
            resetPrompts()
            watch(playerItem, startMs: startMs)
            player.replaceCurrentItem(with: playerItem)
            seekHoldUntil = Date().addingTimeInterval(2)
            if startMs > 0 {
                _ = await player.seek(to: CMTime(value: CMTimeValue(startMs), timescale: 1000), toleranceBefore: .zero, toleranceAfter: .positiveInfinity)
            }
            if room == nil { player.play() }
            report(.start)
            loadExtras(item)
        } catch {
            self.error = message(error)
        }
    }

    /// Skip markers and the next episode, once playing.
    private func loadExtras(_ item: Item) {
        guard let core else { return }
        let id = item.id, account = account
        Task {
            let found = (try? await core.segments(accountId: account, itemId: id)) ?? []
            if self.item?.id == id { segments = found }
        }
        if item.kind == .episode {
            Task {
                let n = (try? await core.adjacentEpisodes(accountId: account, itemId: id))?.next
                if self.item?.id == id { next = n }
            }
        }
    }

    private func resetPrompts() {
        segments = []
        next = nil
        autoSkipped = []
        countdownFrom = nil
        countdownCancelled = false
        skipLabel = nil
        nearEnd = false
        upNextIn = nil
        tracks = []
        tracksReady = false
        shownAudio = nil
        shownSubtitle = nil
    }

    // ----- segments and the next episode ---------------------------------------------------

    private var durationMs: UInt64? {
        let d = player.currentItem?.duration.seconds ?? 0
        return d.isFinite && d > 0 ? UInt64(d * 1000) : nil
    }

    /// Skip intro/credits, and "Next episode" during the credits (or the last 40 s).
    private func watchSegments() {
        guard let core, !stopped else { return }
        let prompt = playbackPrompt(segments: segments, positionMs: positionMs, durationMs: durationMs, hasNext: next != nil)
        nearEnd = prompt.nextEpisode
        let seg = prompt.skip
        if let seg, seg.kind == .intro || seg.kind == .recap,
           core.getSettingOr("player.auto_skip_intro", "0") == "1",
           autoSkipped.insert(seg.startMs).inserted {
            seekTo(seg.endMs)
            skipLabel = nil
            return
        }
        switch seg?.kind {
        case .none: skipLabel = nil
        case .credits: skipLabel = tr("Skip credits")
        case .intro: skipLabel = tr("Skip intro")
        case .recap: skipLabel = tr("Skip recap")
        case .preview: skipLabel = tr("Skip preview")
        case .commercial: skipLabel = tr("Skip")
        }
        // Credits with a next episode: count down, then play it (unless cancelled).
        if prompt.credits, core.getSettingOr("player.auto_next", "1") != "0", !countdownCancelled, player.rate > 0 {
            let from = countdownFrom ?? Date()
            countdownFrom = from
            let total = Int(core.getSettingOr("player.countdown", "10")) ?? 10
            let left = total - Int(Date().timeIntervalSince(from))
            if left <= 0 {
                upNextIn = nil
                playNext()
                return
            }
            upNextIn = left
        } else {
            countdownFrom = nil
            upNextIn = nil
        }
    }

    func skip() {
        let prompt = playbackPrompt(segments: segments, positionMs: positionMs, durationMs: durationMs, hasNext: next != nil)
        guard let seg = prompt.skip else { return }
        seekTo(seg.endMs)
        skipLabel = nil
    }

    func cancelCountdown() {
        countdownCancelled = true
        countdownFrom = nil
        upNextIn = nil
    }

    func playNext() {
        guard let next else { return }
        report(.stop)
        self.next = nil
        nearEnd = false
        upNextIn = nil
        Task {
            await open(next, at: next.user.played ? 0 : next.user.positionMs)
            // In a room: the next episode for everyone.
            if room != nil { sendRoom(newTitle: true) }
        }
    }

    private func seekTo(_ ms: UInt64) {
        player.seek(to: CMTime(value: CMTimeValue(ms), timescale: 1000), toleranceBefore: .zero, toleranceAfter: .zero)
    }

    // ----- audio and subtitles -------------------------------------------------------------

    /// The file's audio and subtitle choices; then the title's choice, else the settings.
    private func applyTrackPrefs(_ playerItem: AVPlayerItem) async {
        guard let core, let item, playerItem === player.currentItem else { return }
        let asset = playerItem.asset
        var found: [(info: TrackInfo, option: AVMediaSelectionOption, group: AVMediaSelectionGroup)] = []
        for (kind, characteristic) in [(TrackKind.audio, AVMediaCharacteristic.audible), (TrackKind.subtitle, AVMediaCharacteristic.legible)] {
            guard let group = try? await asset.loadMediaSelectionGroup(for: characteristic) else { continue }
            for option in group.options where option.isPlayable {
                let title = AVMetadataItem.metadataItems(from: option.commonMetadata, filteredByIdentifier: .commonIdentifierTitle).first?.stringValue
                let info = TrackInfo(
                    id: UInt32(found.count),
                    kind: kind,
                    language: option.extendedLanguageTag ?? option.locale?.identifier,
                    title: title,
                    forced: option.hasMediaCharacteristic(.containsOnlyForcedSubtitles)
                )
                found.append((info: info, option: option, group: group))
            }
        }
        guard playerItem === player.currentItem else { return }
        tracks = found
        let selection = playerItem.currentMediaSelection
        let defaultAudio = found.first { $0.info.kind == .audio && selection.selectedMediaOption(in: $0.group) == $0.option }?.info.id
        let subLang = core.getSettingOr("player.sub_lang", "")
        let rules = TrackRules(
            audioLanguage: core.getSettingOr("player.audio_lang", ""),
            subtitleMode: core.getSettingOr("player.sub_mode", "auto"),
            subtitleLanguage: subLang.isEmpty ? (Locale.current.languageCode ?? "en") : subLang,
            defaultAudio: defaultAudio
        )
        let pref = (try? core.trackPref(accountId: account, titleKey: trackTitleKey(item: item))) ?? nil
        let choice = chooseTracks(pref: pref, rules: rules, tracks: found.map(\.info))
        if let a = choice.audio, Int(a) < found.count {
            playerItem.select(found[Int(a)].option, in: found[Int(a)].group)
        }
        if let legible = found.first(where: { $0.info.kind == .subtitle })?.group {
            if let s = choice.subtitle, Int(s) < found.count {
                playerItem.select(found[Int(s)].option, in: legible)
            } else {
                playerItem.select(nil, in: legible)
            }
        }
        shownAudio = selected(.audio)
        shownSubtitle = selected(.subtitle)
        tracksReady = true
    }

    private func selected(_ kind: TrackKind) -> AVMediaSelectionOption? {
        guard let playerItem = player.currentItem, let group = tracks.first(where: { $0.info.kind == kind })?.group else { return nil }
        return playerItem.currentMediaSelection.selectedMediaOption(in: group)
    }

    /// A track picked in Apple's menu: remembered for the movie or the whole show (Android's
    /// pickTrack), and the server is told.
    private func watchTrackChoice() {
        guard tracksReady, let core, let item else { return }
        let audio = selected(.audio), subtitle = selected(.subtitle)
        var changed: [(TrackKind, AVMediaSelectionOption?)] = []
        if audio != shownAudio { changed.append((.audio, audio)) }
        if subtitle != shownSubtitle { changed.append((.subtitle, subtitle)) }
        guard !changed.isEmpty else { return }
        shownAudio = audio
        shownSubtitle = subtitle
        let key = trackTitleKey(item: item)
        for (kind, option) in changed {
            let picked = option.flatMap { o in tracks.first { $0.option == o }?.info.id }
            if option != nil, picked == nil { continue }
            let old = (try? core.trackPref(accountId: account, titleKey: key)) ?? nil
            let pref = pickTrack(pref: old, kind: kind, tracks: tracks.map(\.info), picked: picked)
            try? core.setTrackPref(accountId: account, titleKey: key, pref: pref)
        }
        report(.progress)
    }

    /// The server's stream index of the track playing (the file's own tracks, in order), so
    /// its dashboard shows the real one. Nothing while converting.
    private func streamIndex(_ kind: TrackKind) -> Int64? {
        guard !transcoding, let source, tracksReady else { return nil }
        let streamKind: StreamKind = kind == .audio ? .audio : .subtitle
        let server = source.streams.filter { $0.kind == streamKind && !$0.isExternal }.sorted { $0.index < $1.index }
        guard let option = selected(kind) else { return kind == .subtitle ? -1 : nil }
        let ofKind = tracks.filter { $0.info.kind == kind }
        guard let n = ofKind.firstIndex(where: { $0.option == option }), n < server.count else { return nil }
        return server[n].index
    }

    /// Subtitle size, colour, background and position from the settings (Android's look).
    private func subtitleStyle() -> [AVTextStyleRule] {
        guard let core else { return [] }
        var attrs: [String: Any] = [:]
        let yellow = core.getSettingOr("player.sub_color", "white") == "yellow"
        attrs[kCMTextMarkupAttribute_ForegroundColorARGB as String] = yellow ? [1, 1, 0.9, 0.31] : [1, 1, 1, 1]
        if core.getSettingOr("player.sub_bg", "0") == "1" {
            attrs[kCMTextMarkupAttribute_BackgroundColorARGB as String] = [0.67, 0, 0, 0]
        } else {
            // No box: a dark edge keeps them readable on bright scenes.
            attrs[kCMTextMarkupAttribute_CharacterEdgeStyle as String] = kCMTextMarkupCharacterEdgeStyle_Uniform
        }
        let size: Double
        switch core.getSettingOr("player.sub_size", "normal") {
        case "small": size = 80
        case "large": size = 125
        case "huge": size = 150
        default: size = 100
        }
        attrs[kCMTextMarkupAttribute_RelativeFontSize as String] = size
        if core.getSettingOr("player.sub_raised", "0") == "1" {
            attrs[kCMTextMarkupAttribute_OrthogonalLinePositionPercentageRelativeToWritingDirection as String] = 80
        }
        return AVTextStyleRule(textMarkupAttributes: attrs).map { [$0] } ?? []
    }

    private func watch(_ playerItem: AVPlayerItem, startMs: UInt64) {
        observers.removeAll()
        if let endObserver { NotificationCenter.default.removeObserver(endObserver) }
        observers.append(playerItem.observe(\.status) { [weak self] it, _ in
            if it.status == .readyToPlay {
                Task { @MainActor in
                    guard let self, !self.tracksReady else { return }
                    await self.applyTrackPrefs(it)
                }
            }
            guard it.status == .failed else { return }
            let reason = it.error?.localizedDescription
            Task { @MainActor in self?.failed(reason) }
        })
        observers.append(player.observe(\.rate) { [weak self] p, _ in
            let rate = p.rate
            Task { @MainActor in self?.rateChanged(rate) }
        })
        endObserver = NotificationCenter.default.addObserver(forName: .AVPlayerItemDidPlayToEndTime, object: playerItem, queue: .main) { [weak self] _ in
            Task { @MainActor in await self?.ended() }
        }
        if let jumpObserver { NotificationCenter.default.removeObserver(jumpObserver) }
        jumpObserver = NotificationCenter.default.addObserver(forName: AVPlayerItem.timeJumpedNotification, object: playerItem, queue: .main) { [weak self] _ in
            Task { @MainActor in self?.timeJumped() }
        }
        if tick == nil {
            tick = player.addPeriodicTimeObserver(forInterval: CMTime(seconds: 10, preferredTimescale: 1), queue: .main) { [weak self] _ in
                Task { @MainActor in
                    guard let self, self.player.rate > 0 else { return }
                    self.report(.progress)
                }
            }
        }
        if segmentTick == nil {
            segmentTick = player.addPeriodicTimeObserver(forInterval: CMTime(value: 1, timescale: 2), queue: .main) { [weak self] _ in
                Task { @MainActor in
                    self?.watchSegments()
                    self?.watchTrackChoice()
                }
            }
        }
    }

    /// The original didn't play here: the server's conversion, from the same point.
    private func failed(_ reason: String?) {
        if source == nil, let item {
            // The downloaded file didn't play: the server's copy.
            let at = positionMs > 0 ? positionMs : startMs
            Task { await open(item, at: at, local: false) }
            return
        }
        guard let item, !transcoding, source?.transcode != nil else {
            error = reason ?? tr("This file can't be played on this device.")
            return
        }
        let at = positionMs > 0 ? positionMs : startMs
        Task { await open(item, at: at, forceConversion: true) }
    }

    private func rateChanged(_ rate: Float) {
        defer { lastRate = rate }
        // In a room: played or paused here (not by the room) goes to everyone.
        if room != nil, let applied = appliedPlaying, (rate > 0) != applied, roomSending == 0, !stopped {
            sendRoom()
        }
        if rate == 0, lastRate > 0, !stopped { report(.pause) }
        if rate > 0, lastRate == 0, info != nil { report(.progress) }
    }

    private func ended() async {
        report(.stop)
        // In a room, the host's next title comes to everyone; the others wait for it.
        if room != nil, !TogetherSession.shared.isHost { return }
        guard let core, let item, item.kind == .episode,
              core.getSettingOr("player.auto_next", "1") != "0"
        else {
            onFinished?()
            return
        }
        var upNext = next
        if upNext == nil { upNext = (try? await core.adjacentEpisodes(accountId: account, itemId: item.id))?.next }
        guard let upNext else {
            onFinished?()
            return
        }
        await open(upNext, at: upNext.user.played ? 0 : upNext.user.positionMs)
        if room != nil { sendRoom(newTitle: true) }
    }

    func stop() {
        guard !stopped else { return }
        exitRoomPlayer()
        report(.stop)
        stopped = true
        player.pause()
        if let tick { player.removeTimeObserver(tick) }
        tick = nil
        if let segmentTick { player.removeTimeObserver(segmentTick) }
        segmentTick = nil
        observers.removeAll()
        if let endObserver { NotificationCenter.default.removeObserver(endObserver) }
        if let jumpObserver { NotificationCenter.default.removeObserver(jumpObserver) }
        player.replaceCurrentItem(with: nil)
    }

    // ----- watch together ------------------------------------------------------------------

    private var room: TogetherSession.Seat?
    private var jumpObserver: NSObjectProtocol?
    private var roomSubs: Set<AnyCancellable> = []
    private var roomLoop: Task<Void, Never>?
    private var roomRound: UInt64 = 0
    private var roomMedia: TogetherMedia?
    private var lastSeen: TogetherState?
    /// Speed used to catch up with the room (1: normal).
    private var roomRate: Float = 1
    /// False once a speed change didn't close the gap: seeks only.
    private var rateWorks = true
    private var rateCheckAt = Date.distantPast
    private var rateCheckDrift: Int64 = 0
    /// No following before this: our own change, not echoed by the room yet.
    private var ownHoldUntil = Date.distantPast
    /// No corrections (and no "I moved" sent) before this: a seek landing.
    private var seekHoldUntil = Date.distantPast
    private var roomSending = 0
    /// Playing or paused as the room last said (a different rate is the viewer's doing).
    private var appliedPlaying: Bool?
    private var scheduledStart: Int64?
    private var startTask: Task<Void, Never>?
    private var readySent: UInt64 = 0
    private var readySince: Date?
    private var noteTask: Task<Void, Never>?

    private func note(_ text: String) {
        togetherNote = text
        noteTask?.cancel()
        noteTask = Task {
            try? await Task.sleep(nanoseconds: 4_000_000_000)
            if !Task.isCancelled { togetherNote = nil }
        }
    }

    /// Follows the room (its long poll runs in TogetherSession: kept alive while playing).
    private func joinRoom(_ seat: TogetherSession.Seat) {
        guard let core else { return }
        let session = TogetherSession.shared
        room = seat
        session.attach(core)
        if let s = session.state {
            lastSeen = s
            roomMedia = s.media
            roomRound = s.round
            session.handledRound = max(session.handledRound, s.round)
        }
        session.updates
            .filter { $0.code == seat.code }
            .sink { [weak self] s in self?.onRoomState(s) }
            .store(in: &roomSubs)
        // The room closed, or this device moved to another one.
        session.$seat
            .dropFirst()
            .sink { [weak self] now in
                guard now?.code != seat.code else { return }
                let why = session.ended
                session.ended = nil
                self?.leaveTogether(why ?? tr("You're no longer in this room"))
            }
            .store(in: &roomSubs)
        // In step every half second (also while paused, waiting for a start).
        roomLoop = Task { [weak self] in
            while !Task.isCancelled {
                if let self, let s = TogetherSession.shared.state, s.code == seat.code { self.syncToRoom(s) }
                try? await Task.sleep(nanoseconds: 500_000_000)
            }
        }
    }

    private func onRoomState(_ s: TogetherState) {
        guard let room else { return }
        let session = TogetherSession.shared
        let before = lastSeen
        lastSeen = s
        // The host went back to the waiting room (to pick something else): so do we, also on
        // the host's other devices (only the seat that sent it is already leaving).
        if !s.started {
            if !session.isHost || (before?.started == true && s.by != room.memberId) {
                session.problem = nil
                onFinished?()
            }
            return
        }
        let newRound = roomRound != 0 && s.round != roomRound
        roomRound = s.round
        session.handledRound = max(session.handledRound, s.round)
        if s.by == room.memberId || roomSending > 0 {
            roomMedia = s.media
            if !s.loading.isEmpty { syncToRoom(s) }
            return
        }
        let known = roomMedia
        roomMedia = s.media
        if newRound || (known != nil && known != s.media) {
            followTitle(s)
            return
        }
        // Someone else played, paused or moved: say who, then follow.
        if let before, s.seq != before.seq {
            if let who = s.members.first(where: { $0.id == s.by })?.name {
                if before.playing && !s.playing {
                    note(tr("{name} paused", ["name": who]))
                } else if !before.playing && s.playing {
                    note(tr("{name} resumed", ["name": who]))
                } else {
                    note(tr("{name} moved to {time}", ["name": who, "time": formatTime(s.positionMs)]))
                }
            } else if s.members.count > before.members.count {
                let joined = s.members.filter { m in !before.members.contains { $0.id == m.id } }.map(\.name).joined(separator: ", ")
                note(tr("{names} joined", ["names": joined]))
            } else if s.members.count < before.members.count {
                let left = before.members.filter { m in !s.members.contains { $0.id == m.id } }.map(\.name).joined(separator: ", ")
                note(tr("{names} left", ["names": left]))
            }
        }
        // Someone else's change wins over our own pending hold.
        ownHoldUntil = .distantPast
        syncToRoom(s)
    }

    /// Plays or pauses like the room and stays in step with it: small drifts by playing a
    /// little faster or slower, a seek past 2 s (the core's `togetherCorrection`).
    private func syncToRoom(_ s: TogetherState) {
        guard let core, room != nil, roomSending == 0, !stopped, player.currentItem != nil else { return }
        let now = Date()
        let serverNow = core.togetherNowMs()
        if !s.loading.isEmpty || togetherStartsIn(state: s, serverNowMs: serverNow) != nil {
            waitForStart(s, serverNow)
            return
        }
        togetherWaiting = nil
        let expected = togetherPositionAt(state: s, serverNowMs: serverNow)
        if now < ownHoldUntil { return }
        if appliedPlaying != s.playing || (player.rate > 0) != s.playing {
            appliedPlaying = s.playing
            if s.playing { player.rate = roomRate } else { player.pause() }
        }
        guard now >= seekHoldUntil, player.currentItem?.status == .readyToPlay else { return }
        let actual = positionMs
        let drift = abs(Int64(actual) - Int64(expected))
        let c = togetherCorrection(expectedMs: expected, actualMs: actual, playing: s.playing, currentRate: Double(roomRate), canChangeRate: rateWorks)
        if let to = c.seekToMs {
            seekHoldUntil = now.addingTimeInterval(2)
            seekTo(to)
        }
        if Float(c.rate) != roomRate {
            if roomRate == 1 {
                rateCheckAt = now.addingTimeInterval(6)
                rateCheckDrift = drift
            }
            roomRate = Float(c.rate)
            if s.playing { player.rate = roomRate }
        } else if roomRate != 1, now >= rateCheckAt {
            // The speed doesn't close the gap: seek instead.
            if drift > rateCheckDrift * 8 / 10 {
                rateWorks = false
                roomRate = 1
                if s.playing { player.rate = 1 }
            }
            rateCheckAt = now.addingTimeInterval(6)
            rateCheckDrift = drift
        }
    }

    /// A start: every seat loads the title paused at the room's position and says when it has
    /// enough; once all have, the room plays from a moment ahead and a timer presses play then.
    private func waitForStart(_ s: TogetherState, _ serverNow: Int64) {
        guard let core, let room else { return }
        let now = Date()
        let at = s.positionMs
        appliedPlaying = false
        if player.rate != 0 { player.pause() }
        roomRate = 1
        let ready = player.currentItem?.status == .readyToPlay
        if now >= seekHoldUntil, ready, abs(Int64(positionMs) - Int64(at)) > 150 {
            seekTo(at)
            seekHoldUntil = now.addingTimeInterval(0.5)
        }
        if let startsIn = togetherStartsIn(state: s, serverNowMs: serverNow) {
            togetherWaiting = tr("Starting…")
            if scheduledStart != s.atMs {
                scheduledStart = s.atMs
                startTask?.cancel()
                startTask = Task {
                    try? await Task.sleep(nanoseconds: UInt64(max(0, startsIn)) * 1_000_000)
                    guard !Task.isCancelled, let st = TogetherSession.shared.state, st.atMs == s.atMs, st.playing else { return }
                    appliedPlaying = true
                    player.rate = 1
                    togetherWaiting = nil
                }
            }
            return
        }
        let mine = togetherIsLoading(state: s, memberId: room.memberId)
        if mine, readySent != s.round {
            let here = ready && abs(Int64(positionMs) - Int64(at)) <= 500
            if !here {
                readySince = nil
            } else if readySince == nil {
                readySince = now
            }
            let enough = here && (bufferedAheadMs >= togetherReadyBufferMs() || bufferedToEnd || now.timeIntervalSince(readySince ?? now) > 8)
            if enough {
                let round = s.round
                readySent = round
                Task {
                    do {
                        TogetherSession.shared.accept(try await core.togetherReady(code: room.code, memberId: room.memberId, round: round))
                    } catch {
                        if readySent == round { readySent = 0 }
                    }
                }
            }
        }
        let others = Array(Set(s.loading.filter { $0 != room.memberId }.compactMap { id in s.members.first { $0.id == id }?.name })).sorted()
        if mine && readySent != s.round {
            togetherWaiting = tr("Loading for everyone…")
        } else if others.isEmpty {
            togetherWaiting = tr("Starting…")
        } else {
            togetherWaiting = tr("Waiting for {names}…", ["names": others.joined(separator: ", ")])
        }
    }

    /// How much is loaded past the current position.
    private var bufferedAheadMs: UInt64 {
        guard let item = player.currentItem else { return 0 }
        let now = item.currentTime()
        for r in item.loadedTimeRanges.map(\.timeRangeValue) where r.containsTime(now) {
            let ahead = (r.end - now).seconds
            return ahead.isFinite && ahead > 0 ? UInt64(ahead * 1000) : 0
        }
        return 0
    }

    private var bufferedToEnd: Bool {
        guard let item = player.currentItem, let d = durationMs else { return false }
        let end = item.loadedTimeRanges.map { $0.timeRangeValue.end.seconds }.max() ?? 0
        return end * 1000 >= Double(d) - 1000
    }

    /// A seek here (not one of ours): everyone moves too.
    private func timeJumped() {
        guard room != nil, !stopped, Date() >= seekHoldUntil, roomSending == 0, appliedPlaying != nil else { return }
        sendRoom()
    }

    /// The room moved to another title: find it on our servers and play it.
    private func followTitle(_ s: TogetherState) {
        guard let core else { return }
        let m = s.media
        Task {
            let ids = ((try? core.accounts()) ?? []).map(\.id)
            let pref = (try? core.getSetting(key: preferredKey)) ?? nil
            let accounts = ids.filter { $0 == pref } + ids.filter { $0 != pref }
            guard let found = (try? await core.togetherFind(accountIds: accounts, media: m)) ?? nil else {
                note(tr("{title} isn't on your servers", ["title": m.label]))
                appliedPlaying = false
                player.pause()
                return
            }
            var it = ((try? core.cachedItem(accountId: found.accountId, itemId: found.itemId)) ?? nil)?.item
            if it == nil { it = try? await core.refreshItem(accountId: found.accountId, itemId: found.itemId, force: false).item }
            guard let it else { return }
            report(.stop)
            account = found.accountId
            appliedPlaying = nil
            await open(it, at: togetherPositionAt(state: s, serverNowMs: core.togetherNowMs()))
        }
    }

    /// Tells the room what we just did (play, pause, seek; with `newTitle`, the title too).
    private func sendRoom(newTitle: Bool = false) {
        guard let core, let room else { return }
        let playing = player.rate > 0
        let position = positionMs
        let acct: String? = newTitle ? account : nil
        let itemId: String? = newTitle ? item?.id : nil
        appliedPlaying = playing
        ownHoldUntil = Date().addingTimeInterval(2)
        roomSending += 1
        Task {
            defer { roomSending -= 1 }
            do {
                let s = try await core.togetherUpdate(code: room.code, memberId: room.memberId, playing: playing, positionMs: position, accountId: acct, itemId: itemId)
                TogetherSession.shared.accept(s)
                lastSeen = s
                roomMedia = s.media
            } catch {
                // The host doesn't allow it (HTTP 423): say so and go back to what the room plays.
                guard message(error).contains("HTTP 423"), let s = TogetherSession.shared.state else { return }
                if newTitle {
                    note(tr("Only the host can change what's playing in this room"))
                } else if s.playing != playing, playing {
                    note(tr("Only the host can play in this room"))
                } else if s.playing != playing {
                    note(tr("Only the host can pause in this room"))
                } else {
                    note(tr("Only the host can move forward or back in this room"))
                }
                ownHoldUntil = .distantPast
                roomSending -= 1
                if newTitle { followTitle(s) } else { syncToRoom(s) }
                roomSending += 1
            }
        }
    }

    private func leaveRoomLoop() {
        roomSubs.removeAll()
        roomLoop?.cancel()
        roomLoop = nil
        startTask?.cancel()
        togetherWaiting = nil
        TogetherSession.shared.detach()
    }

    private func leaveTogether(_ text: String) {
        guard room != nil else { return }
        room = nil
        leaveRoomLoop()
        roomMedia = nil
        lastSeen = nil
        note(text)
    }

    /// The player closes while in a room: we stay in it (the app shows the Room screen). The
    /// host closing it takes everyone back to the waiting room, to pick something else.
    private func exitRoomPlayer() {
        guard let seat = room else { return }
        let session = TogetherSession.shared
        let s = session.state
        let hosting = session.isHost
        let pos = positionMs
        room = nil
        leaveRoomLoop()
        guard let s, session.seat != nil else { return }
        session.handledRound = max(session.handledRound, s.round)
        if hosting && s.started, let core {
            session.startAt = pos
            Task {
                if let st = try? await core.togetherLobby(code: seat.code, memberId: seat.memberId, positionMs: pos) { session.accept(st) }
            }
        }
        session.show = seat.code
    }

    private var positionMs: UInt64 {
        let s = player.currentTime().seconds
        return s.isFinite && s > 0 ? UInt64(s * 1000) : 0
    }

    private func report(_ event: PlaybackEvent) {
        guard let core, let item, !stopped else { return }
        let duration = player.currentItem?.duration.seconds
        let report = ProgressReport(
            itemId: item.id,
            mediaSourceId: source?.id,
            playSessionId: info?.playSessionId,
            positionMs: positionMs,
            durationMs: duration.flatMap { $0.isFinite && $0 > 0 ? UInt64($0 * 1000) : nil },
            event: event,
            audioIndex: streamIndex(.audio),
            subtitleIndex: streamIndex(.subtitle),
            playMethod: transcoding ? .transcode : .direct
        )
        let account = account
        Task.detached { try? await core.reportProgress(accountId: account, report: report) }
    }

    private func metadata(_ item: Item) -> [AVMetadataItem] {
        func entry(_ id: AVMetadataIdentifier, _ value: String) -> AVMetadataItem {
            let m = AVMutableMetadataItem()
            m.identifier = id
            m.value = value as NSString
            m.extendedLanguageTag = "und"
            return m
        }
        var out = [entry(.commonIdentifierTitle, item.kind == .episode ? "\(item.seriesTitle ?? "") · \(item.title)" : item.title)]
        if let o = item.overview { out.append(entry(.commonIdentifierDescription, o)) }
        return out
    }
}

extension AlwyWatch {
    /// A setting, or `fallback` when unset.
    func getSettingOr(_ key: String, _ fallback: String) -> String {
        ((try? getSetting(key: key)) ?? nil) ?? fallback
    }
}

extension Notification.Name {
    /// A playback was closed: watched downloads may go, "next episodes" may come.
    static let playbackEnded = Notification.Name("alwywatch.playbackEnded")
}
