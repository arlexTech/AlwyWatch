// SPDX-License-Identifier: AGPL-3.0-or-later
import Foundation

// Item helpers, the same as Android's (ui/Components.kt).
extension Item: Identifiable {}

extension Item {
    /// 2:3 artwork; for an episode, its show's poster (its own primary image is a still).
    var poster: ImageRef? {
        if kind == .episode, let s = images.first(where: { $0.kind == .seriesPrimary }) { return s }
        return images.first { $0.kind == .primary }
    }

    var backdrop: ImageRef? { images.first { $0.kind == .backdrop } }
    var logo: ImageRef? { images.first { $0.kind == .logo } }

    /// 16:9 artwork: an episode's own still, otherwise the backdrop.
    var thumb: ImageRef? {
        if kind == .episode {
            return images.first { $0.kind == .primary } ?? images.first { $0.kind == .thumb } ?? poster
        }
        return images.first { $0.kind == .thumb } ?? backdrop ?? poster
    }

    var subtitle: String {
        switch kind {
        case .episode:
            var parts: [String] = []
            if let s = seasonNumber, let e = episodeNumber { parts.append(tr("S{season} · E{episode}", ["season": s, "episode": e])) }
            if seriesTitle != nil { parts.append(title) }
            return parts.joined(separator: "  ")
        case .series:
            var parts: [String] = []
            if let y = year { parts.append(String(y)) }
            if let c = childCount { parts.append(trn("{n} season", "{n} seasons", Int(c))) }
            return parts.joined(separator: " · ")
        default:
            var parts: [String] = []
            if let y = year { parts.append(String(y)) }
            if let r = runtimeMs, r > 0 { parts.append(formatRuntime(Int64(r))) }
            return parts.joined(separator: " · ")
        }
    }

    /// The title line of a card: an episode shows its show.
    var cardTitle: String { kind == .episode ? (seriesTitle ?? title) : title }

    /// 0...1 when partly watched, else nil.
    var progress: Double? {
        guard let runtime = runtimeMs, runtime > 0, user.positionMs > 0 else { return nil }
        return min(1, Double(user.positionMs) / Double(runtime))
    }

    var remaining: String? {
        guard let runtime = runtimeMs, user.positionMs > 0, runtime > user.positionMs else { return nil }
        return tr("{time} left", ["time": formatRuntime(Int64(runtime - user.positionMs))])
    }

    var playable: Bool { kind == .movie || kind == .episode }
}

func formatRuntime(_ ms: Int64) -> String {
    let min = ms / 60000
    return min >= 60 ? tr("{hours} h {minutes} min", ["hours": min / 60, "minutes": min % 60]) : tr("{minutes} min", ["minutes": min])
}

func formatClock(_ seconds: Double) -> String {
    guard seconds.isFinite, seconds >= 0 else { return "0:00" }
    let s = Int(seconds)
    return s >= 3600 ? String(format: "%d:%02d:%02d", s / 3600, (s / 60) % 60, s % 60) : String(format: "%d:%02d", s / 60, s % 60)
}
