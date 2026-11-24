// SPDX-License-Identifier: AGPL-3.0-or-later
import AVFoundation
import AVKit
import SwiftUI

/// What to play (a full-screen cover).
struct PlayRequest: Identifiable {
    let account: String
    let item: Item
    var fromStart = false
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
        self.vc = vc
        self.playback = playback
        self.onClose = onClose
        Self.top()?.present(vc, animated: true)
        Task { await playback.start(core: core, account: request.account, item: request.item, fromStart: request.fromStart) }
    }

    private func finish() {
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

/// One playback: picks the original file or the server's conversion, reports progress to the
/// server (start, every 10 s, pause, stop), falls back to the conversion when the original
/// fails, and goes on with the next episode at the end.
@MainActor
final class Playback {
    let player = AVPlayer()
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

    func start(core: AlwyWatch, account: String, item: Item, fromStart: Bool) async {
        self.core = core
        self.account = account
        await open(item, at: fromStart || item.user.played ? 0 : item.user.positionMs)
    }

    private func open(_ item: Item, at startMs: UInt64, forceConversion: Bool = false) async {
        guard let core else { return }
        self.item = item
        self.startMs = startMs
        error = nil
        let profile = DeviceProfiles.ios
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
            watch(playerItem, startMs: startMs)
            player.replaceCurrentItem(with: playerItem)
            if startMs > 0 {
                _ = await player.seek(to: CMTime(value: CMTimeValue(startMs), timescale: 1000), toleranceBefore: .zero, toleranceAfter: .positiveInfinity)
            }
            player.play()
            report(.start)
        } catch {
            self.error = message(error)
        }
    }

    private func watch(_ playerItem: AVPlayerItem, startMs: UInt64) {
        observers.removeAll()
        if let endObserver { NotificationCenter.default.removeObserver(endObserver) }
        observers.append(playerItem.observe(\.status) { [weak self] it, _ in
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
        if tick == nil {
            tick = player.addPeriodicTimeObserver(forInterval: CMTime(seconds: 10, preferredTimescale: 1), queue: .main) { [weak self] _ in
                Task { @MainActor in
                    guard let self, self.player.rate > 0 else { return }
                    self.report(.progress)
                }
            }
        }
    }

    /// The original didn't play here: the server's conversion, from the same point.
    private func failed(_ reason: String?) {
        guard let item, !transcoding, source?.transcode != nil else {
            error = reason ?? tr("This file can't be played on this device.")
            return
        }
        let at = positionMs > 0 ? positionMs : startMs
        Task { await open(item, at: at, forceConversion: true) }
    }

    private func rateChanged(_ rate: Float) {
        defer { lastRate = rate }
        if rate == 0, lastRate > 0, !stopped { report(.pause) }
        if rate > 0, lastRate == 0, info != nil { report(.progress) }
    }

    private func ended() async {
        report(.stop)
        guard let core, let item, item.kind == .episode,
              core.getSettingOr("player.auto_next", "1") != "0",
              let next = try? await core.adjacentEpisodes(accountId: account, itemId: item.id).next
        else {
            onFinished?()
            return
        }
        await open(next, at: next.user.played ? 0 : next.user.positionMs)
    }

    func stop() {
        guard !stopped else { return }
        report(.stop)
        stopped = true
        player.pause()
        if let tick { player.removeTimeObserver(tick) }
        tick = nil
        observers.removeAll()
        if let endObserver { NotificationCenter.default.removeObserver(endObserver) }
        player.replaceCurrentItem(with: nil)
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
            audioIndex: nil,
            subtitleIndex: nil,
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
