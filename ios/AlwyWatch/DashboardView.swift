// SPDX-License-Identifier: AGPL-3.0-or-later
import SwiftUI
import UIKit

/// Admins: what plays now on Plex and Jellyfin, and for each track whether it plays as is or
/// what the server converts it to; then bandwidth, CPU and RAM over the last minutes and who
/// watched what over the last days. Android's DashboardScreen, the web app's /dashboard.
struct DashboardView: View {
    @EnvironmentObject var model: AppModel
    @AppStorage("dashboard.server") private var server = "all"
    @State private var days = 30
    @State private var live: Live?
    @State private var liveError: String?
    @State private var stats: Stats?
    @State private var statsError: String?

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 18) {
                ScrollView(.horizontal, showsIndicators: false) {
                    HStack(spacing: 8) {
                        chip(tr("All servers"), kind: nil, selected: server == "all") { server = "all" }
                        ForEach(live?.servers ?? [], id: \.id) { s in
                            chip(s.name, kind: s.kind, selected: server == s.id) { server = s.id }
                        }
                    }
                    .padding(.horizontal, 16)
                }
                Group {
                    ForEach((live?.servers ?? []).filter { $0.problem != nil && (server == "all" || server == $0.id) }, id: \.id) { s in
                        Text("\(s.name): \(problemText(s.problem ?? ""))").foregroundStyle(Palette.accent).font(.subheadline)
                    }
                    if let e = liveError { Text(e).foregroundStyle(Palette.accent) }
                    if let l = live {
                        nowPlaying(l)
                        TimeChart(title: tr("Bandwidth"), samples: l.samples, series: [
                            .init(name: tr("Remote"), color: DashColors.remote) { Double($0.remoteKbps) },
                            .init(name: tr("Local"), color: DashColors.local) { Double($0.localKbps) },
                        ], top: nil, format: { mbps(UInt64($0)) })
                        TimeChart(title: tr("CPU"), samples: l.samples, series: [
                            .init(name: tr("Server"), color: DashColors.local) { $0.cpu.map(Double.init) },
                        ], top: 100, format: pct)
                        TimeChart(title: l.ramTotalMb > 0 ? "\(tr("RAM")) · \(Int((Double(l.ramTotalMb) / 1024).rounded())) GB" : tr("RAM"), samples: l.samples, series: [
                            .init(name: tr("Server"), color: DashColors.remote) { $0.ram.map(Double.init) },
                        ], top: 100, format: pct)
                    } else if liveError == nil {
                        ProgressView().frame(maxWidth: .infinity)
                    }
                    Text(tr("Over the last days")).font(.title3.weight(.bold)).padding(.top, 8)
                    HStack(spacing: 8) {
                        ForEach([7, 30, 90], id: \.self) { n in
                            Button(trn("{n} day", "{n} days", n)) { days = n }
                                .buttonStyle(ActionButtonStyle(primary: days == n))
                        }
                    }
                    if let e = statsError { Text(e).foregroundStyle(Palette.accent) }
                    if let st = stats {
                        statsViews(st)
                    } else if statsError == nil {
                        ProgressView().frame(maxWidth: .infinity)
                    }
                }
                .padding(.horizontal, 16)
            }
            .padding(.vertical, 12)
        }
        .background(Palette.background.ignoresSafeArea())
        .navigationTitle(tr("Dashboard"))
        .task(id: server) {
            live = nil
            while !Task.isCancelled {
                do {
                    let l = try await model.core.keepAdminDashboardLive(server: server)
                    StreamClocks.shared.take(l)
                    live = l
                    liveError = nil
                } catch {
                    liveError = error.localizedDescription
                }
                try? await Task.sleep(nanoseconds: 5_000_000_000)
            }
        }
        .task(id: "\(server)|\(days)") {
            stats = nil
            statsError = nil
            let tz = Int32(TimeZone.current.secondsFromGMT() / 60)
            do {
                stats = try await model.core.keepAdminDashboardStats(server: server, days: UInt32(days), tz: tz)
            } catch {
                statsError = error.localizedDescription
            }
        }
    }

    private func chip(_ label: String, kind: String?, selected: Bool, action: @escaping () -> Void) -> some View {
        Button(action: action) {
            HStack(spacing: 8) {
                if let kind { ServerMark(kind: kind, size: 20) }
                Text(label)
            }
        }
        .buttonStyle(ActionButtonStyle(primary: selected))
    }

    @ViewBuilder private func nowPlaying(_ l: Live) -> some View {
        let n = l.streams.count
        let conv = l.streams.filter(isConverted).count
        let total = l.streams.reduce(UInt64(0)) { $0 + UInt64($1.kbps) }
        Panel(title: tr("Now playing"), note: n == 0 ? tr("Nothing is playing.") : [
            trn("{n} stream", "{n} streams", n),
            conv == 0 ? tr("all direct") : trn("{n} converted", "{n} converted", conv),
            mbps(total),
        ].joined(separator: " · ")) {
            LazyVGrid(columns: [GridItem(.adaptive(minimum: 300), spacing: 12)], spacing: 12) {
                ForEach(Array(l.streams.enumerated()), id: \.offset) { _, s in
                    let server = l.servers.first { $0.id == s.serverId }
                    // The position moves every second between answers.
                    TimelineView(.periodic(from: .now, by: 1)) { _ in
                        StreamCard(s: s, position: StreamClocks.shared.position(s), kind: server?.kind ?? "", serverName: self.server == "all" ? server?.name : nil)
                    }
                }
            }
        }
    }

    @ViewBuilder private func statsViews(_ st: Stats) -> some View {
        if !st.since.isEmpty {
            let names = Dictionary((live?.servers ?? []).map { ($0.id, $0.name) }, uniquingKeysWith: { a, _ in a })
            Text(st.since.map {
                tr("{server}: history since {date}", ["server": names[$0.serverId] ?? "", "date": Date(timeIntervalSince1970: TimeInterval($0.since)).formatted(date: .abbreviated, time: .omitted)])
            }.joined(separator: " · ")).font(.footnote).foregroundStyle(Palette.muted)
        }
        Panel(title: tr("Top users"), note: nil) {
            if st.topUsers.isEmpty {
                Text(tr("Nothing yet.")).foregroundStyle(Palette.muted)
            } else {
                ScrollView(.horizontal, showsIndicators: false) {
                    HStack(spacing: 12) {
                        ForEach(Array(st.topUsers.enumerated()), id: \.offset) { _, u in UserCard(u: u) }
                    }
                }
            }
        }
        let decided = st.directPlays + st.transcodes
        Panel(title: tr("Play history"), note: decided > 0 ? tr("{direct} direct play · {transcodes} converted (Plex)", ["direct": st.directPlays, "transcodes": st.transcodes]) : nil) {
            HistoryChart(days: st.history)
        }
        Panel(title: tr("Top played"), note: nil) {
            TitleRow(title: tr("Movies"), list: st.topMovies)
            TitleRow(title: tr("TV"), list: st.topShows)
        }
    }
}

enum DashColors {
    static let local = Color(red: 1, green: 0xB2 / 255, blue: 0x24 / 255)
    static let remote = Color(red: 0x5B / 255, green: 0x9C / 255, blue: 1)
    static let movies = Color(red: 0x3E / 255, green: 0xC7 / 255, blue: 0xB0 / 255)
    static let shows = Color(red: 0xE8 / 255, green: 0x68 / 255, blue: 0x9F / 255)
    static let directStream = Color(red: 0x7C / 255, green: 0xC4 / 255, blue: 1)
}

private func problemText(_ p: String) -> String {
    switch p {
    case "no_tautulli": return tr("Plex is read through Tautulli: set it up in Server settings.")
    case "no_key": return tr("Needs this server's API key in Server settings.")
    case "unreachable": return tr("Unreachable right now.")
    default: return p
    }
}

private func mbps(_ kbps: UInt64) -> String {
    kbps >= 1000 ? String(format: "%.1f Mbps", Double(kbps) / 1000) : "\(kbps) kbps"
}

private func pct(_ v: Double) -> String { String(format: "%.0f %%", v) }

private func clock(_ ms: Int64) -> String {
    let s = max(0, ms / 1000)
    let (h, m, sec) = (s / 3600, s / 60 % 60, s % 60)
    return h > 0 ? String(format: "%d:%02d:%02d", h, m, sec) : String(format: "%d:%02d", m, sec)
}

private func hours(_ seconds: UInt64) -> String {
    let (h, m) = (seconds / 3600, seconds / 60 % 60)
    if h == 0 { return "\(m) min" }
    if m == 0 { return "\(h) h" }
    return String(format: "%d h %02d", h, m)
}

private func isConverted(_ s: Stream) -> Bool {
    [s.video, s.audio, s.subtitleTrack].contains { $0?.decision == "transcode" }
}

/// The mark of a server type, as on Android: Plex's yellow chevron, Jellyfin's triangle.
struct ServerMark: View {
    let kind: String
    var size: CGFloat = 22

    var body: some View {
        Canvas { ctx, sz in
            let inset = sz.width * 0.23
            let w = sz.width - 2 * inset
            let h = sz.height - 2 * inset
            func p(_ x: CGFloat, _ y: CGFloat) -> CGPoint { CGPoint(x: inset + w * x, y: inset + h * y) }
            if kind == "plex" {
                var path = Path()
                path.addLines([p(0.22, 0), p(0.52, 0), p(0.86, 0.5), p(0.52, 1), p(0.22, 1), p(0.56, 0.5)])
                path.closeSubpath()
                ctx.fill(path, with: .color(Color(red: 0xE5 / 255, green: 0xA0 / 255, blue: 0x0D / 255)))
            } else {
                var outer = Path()
                outer.addLines([p(0.5, 0), p(1, 1), p(0, 1)])
                outer.closeSubpath()
                var inner = Path()
                inner.addLines([p(0.5, 0.42), p(0.72, 0.86), p(0.28, 0.86)])
                inner.closeSubpath()
                ctx.fill(outer, with: .linearGradient(
                    Gradient(colors: [Color(red: 0xAA / 255, green: 0x5C / 255, blue: 0xC3 / 255), Color(red: 0, green: 0xA4 / 255, blue: 0xDC / 255)]),
                    startPoint: p(0, 0), endPoint: p(1, 1)
                ))
                ctx.fill(inner, with: .color(.black.opacity(0.72)))
            }
        }
        .frame(width: size, height: size)
        .background(Circle().fill(Color.black.opacity(0.72)))
        .accessibilityLabel(kind == "plex" ? "Plex" : "Jellyfin")
    }
}

/// A poster or avatar through the AlwyWatch server (signed-in), kept in memory.
@MainActor
private final class DashImageCache {
    static var images: [String: UIImage] = [:]
}

private struct DashImage: View {
    @EnvironmentObject var model: AppModel
    let server: String
    let img: String?
    @State private var image: UIImage?

    var body: some View {
        ZStack {
            Palette.surfaceHigh
            if let image { Image(uiImage: image).resizable().scaledToFill() }
        }
        .task(id: img) {
            guard let img else { return }
            let key = "\(server)|\(img)"
            if let cached = DashImageCache.images[key] { image = cached; return }
            guard let data = try? await model.core.keepAdminDashboardImage(server: server, img: img),
                  let ui = UIImage(data: data) else { return }
            DashImageCache.images[key] = ui
            image = ui
        }
    }
}

private struct Panel<Content: View>: View {
    let title: String
    let note: String?
    @ViewBuilder let content: () -> Content

    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            Text(title.uppercased()).font(.footnote.weight(.heavy)).foregroundStyle(Palette.onSurface)
            if let note { Text(note).font(.footnote).foregroundStyle(Palette.muted) }
            content()
        }
        .padding(16)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(Palette.surface, in: RoundedRectangle(cornerRadius: 16))
        .overlay(RoundedRectangle(cornerRadius: 16).stroke(Palette.outline))
    }
}

/// Each stream's clock: set by each answer (`dashStreamPosition`), moved on while it plays.
final class StreamClocks {
    static let shared = StreamClocks()
    private var clocks: [String: (Int64, TimeInterval)] = [:]

    private func key(_ s: Stream) -> String { "\(s.serverId)|\(s.user)|\(s.device)|\(s.title)|\(s.subtitle)" }

    private func shown(_ s: Stream, _ c: (Int64, TimeInterval)) -> Int64 {
        let p = s.state == "playing" ? c.0 + Int64((ProcessInfo.processInfo.systemUptime - c.1) * 1000) : c.0
        return s.durationMs > 0 ? min(max(p, 0), s.durationMs) : max(p, 0)
    }

    func take(_ l: Live) {
        for s in l.streams {
            let k = key(s)
            let was = clocks[k].map { shown(s, $0) }
            clocks[k] = (dashStreamPosition(stream: s, nowMs: l.nowMs, shown: was), ProcessInfo.processInfo.systemUptime)
        }
    }

    func position(_ s: Stream) -> Int64 { clocks[key(s)].map { shown(s, $0) } ?? s.positionMs }
}

private struct StreamCard: View {
    let s: Stream
    /// Where it is now (moved on from the answer).
    let position: Int64
    let kind: String
    let serverName: String?

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            Rectangle().fill(isConverted(s) ? Palette.accent : Palette.success).frame(height: 3)
            HStack(alignment: .top, spacing: 12) {
                ZStack(alignment: .topLeading) {
                    DashImage(server: s.serverId, img: s.image)
                        .frame(width: 64, height: 96)
                        .clipShape(RoundedRectangle(cornerRadius: 6))
                    ServerMark(kind: kind).padding(4)
                }
                VStack(alignment: .leading, spacing: 2) {
                    Text(s.title).font(.headline).lineLimit(2)
                    if !s.subtitle.isEmpty { Text(s.subtitle).font(.subheadline).foregroundStyle(Palette.muted).lineLimit(2) }
                    Text(clock(s.durationMs)).font(.footnote).foregroundStyle(Palette.muted)
                    if let serverName { Text(serverName).font(.footnote).foregroundStyle(Palette.subtle) }
                }
                Spacer(minLength: 0)
            }
            .padding(10)
            GeometryReader { g in
                ZStack(alignment: .leading) {
                    Palette.surfaceHigh
                    Palette.accent.frame(width: s.durationMs > 0 ? g.size.width * CGFloat(min(1, max(0, Double(position) / Double(s.durationMs)))) : 0)
                }
            }
            .frame(height: 3)
            VStack(alignment: .leading, spacing: 2) {
                Text("\(s.product) — \(s.device)").font(.subheadline.weight(.semibold))
                let state = s.state == "paused" ? tr("Paused") : s.state == "buffering" ? tr("Buffering") : tr("Playing")
                Text("\(state) — \(clock(position)) / \(clock(s.durationMs))").font(.footnote).foregroundStyle(Palette.muted)
                Text((s.local ? tr("Local") : tr("Remote")) + (s.ip.isEmpty ? "" : " (\(s.ip))") + (s.kbps > 0 ? " — \(mbps(UInt64(s.kbps)))" : ""))
                    .font(.footnote).foregroundStyle(Palette.muted)
                TrackLine(kind: tr("Video"), t: s.video)
                TrackLine(kind: tr("Audio"), t: s.audio)
                TrackLine(kind: tr("Subtitles"), t: s.subtitleTrack)
            }
            .padding(10)
            HStack(spacing: 10) {
                DashAvatar(server: s.serverId, img: s.userAvatar, name: s.user, size: 32)
                Text(s.user)
                Spacer()
            }
            .padding(10)
            .background(Color.black.opacity(0.15))
        }
        .background(Palette.surface, in: RoundedRectangle(cornerRadius: 12))
        .clipShape(RoundedRectangle(cornerRadius: 12))
    }
}

/// What the file has, then whether it plays as is or what it becomes.
private struct TrackLine: View {
    let kind: String
    let t: StreamTrack?

    var body: some View {
        if let t {
            let color = t.decision == "transcode" ? Palette.accent : t.decision == "direct_stream" ? DashColors.directStream : Palette.success
            let sign = t.decision == "transcode" ? "⚙ " : t.decision == "direct_stream" ? "⇄ " : "✓ "
            let name = t.decision == "transcode" ? tr("Transcode") : t.decision == "direct_stream" ? tr("Direct stream") : tr("Direct play")
            VStack(alignment: .leading, spacing: 4) {
                Divider().overlay(Palette.outline)
                HStack(alignment: .top, spacing: 8) {
                    Text(kind).font(.footnote).foregroundStyle(Palette.muted).frame(width: 70, alignment: .leading)
                    VStack(alignment: .leading, spacing: 2) {
                        Text(t.label).font(.subheadline)
                        Text(sign + name + (t.to.map { " → \($0)" } ?? ""))
                            .font(.footnote.weight(t.decision == "transcode" ? .semibold : .regular))
                            .foregroundStyle(color)
                    }
                }
            }
            .padding(.top, 4)
        }
    }
}

private struct DashAvatar: View {
    let server: String
    let img: String?
    let name: String
    let size: CGFloat

    var body: some View {
        Group {
            if img != nil {
                DashImage(server: server, img: img)
            } else {
                ZStack {
                    Palette.surfaceHigh
                    Text(String(name.prefix(1)).uppercased()).font(.subheadline.weight(.semibold))
                }
            }
        }
        .frame(width: size, height: size)
        .clipShape(Circle())
    }
}

private struct ChartSeries {
    let name: String
    let color: Color
    let value: (Sample) -> Double?
}

private struct TimeChart: View {
    let title: String
    let samples: [Sample]
    let series: [ChartSeries]
    let top: Double?
    let format: (Double) -> String

    var body: some View {
        let maxValue = top ?? {
            let m = samples.flatMap { s in series.compactMap { $0.value(s) } }.max() ?? 0
            return m <= 0 ? 1 : m * 1.15
        }()
        let span: String? = samples.count > 1
            ? tr("last {n} min", ["n": Int((Double(samples.last!.t - samples.first!.t) / 60).rounded())])
            : nil
        Panel(title: title, note: span) {
            HStack(spacing: 16) {
                ForEach(Array(series.enumerated()), id: \.offset) { _, s in
                    HStack(spacing: 6) {
                        Circle().fill(s.color).frame(width: 10, height: 10)
                        Text("\(s.name) \(samples.last.flatMap(s.value).map(format) ?? "—")").font(.footnote).foregroundStyle(Palette.muted)
                    }
                }
            }
            ZStack(alignment: .topLeading) {
                GeometryReader { g in
                    let w = g.size.width, h = g.size.height
                    Path { p in
                        p.move(to: CGPoint(x: 0, y: h / 2))
                        p.addLine(to: CGPoint(x: w, y: h / 2))
                    }
                    .stroke(Palette.outline, style: StrokeStyle(lineWidth: 1, dash: [3, 4]))
                    ForEach(Array(series.enumerated()), id: \.offset) { _, s in
                        Path { p in
                            var first = true
                            for (i, sample) in samples.enumerated() {
                                guard let v = s.value(sample) else { first = true; continue }
                                let x = samples.count <= 1 ? w : CGFloat(i) * w / CGFloat(samples.count - 1)
                                let y = h - CGFloat(min(1, v / maxValue)) * h
                                if first { p.move(to: CGPoint(x: x, y: y)) } else { p.addLine(to: CGPoint(x: x, y: y)) }
                                first = false
                            }
                        }
                        .stroke(s.color, style: StrokeStyle(lineWidth: 2, lineJoin: .round))
                    }
                }
                Text(format(maxValue)).font(.caption2).foregroundStyle(Palette.subtle)
            }
            .frame(height: 120)
        }
    }
}

private struct HistoryChart: View {
    let days: [DayPlays]

    var body: some View {
        let top = Double(max(1, days.map { Int($0.movies + $0.episodes) }.max() ?? 0)) * 1.1
        VStack(alignment: .leading, spacing: 8) {
            HStack(spacing: 16) {
                HStack(spacing: 6) {
                    Circle().fill(DashColors.shows).frame(width: 10, height: 10)
                    Text("\(tr("TV")) \(days.reduce(0) { $0 + Int($1.episodes) })").font(.footnote).foregroundStyle(Palette.muted)
                }
                HStack(spacing: 6) {
                    Circle().fill(DashColors.movies).frame(width: 10, height: 10)
                    Text("\(tr("Movies")) \(days.reduce(0) { $0 + Int($1.movies) })").font(.footnote).foregroundStyle(Palette.muted)
                }
            }
            GeometryReader { g in
                let slot = g.size.width / CGFloat(max(1, days.count))
                let gap: CGFloat = slot > 6 ? 2 : 0
                ForEach(Array(days.enumerated()), id: \.offset) { i, d in
                    let eh = CGFloat(Double(d.episodes) / top) * g.size.height
                    let mh = CGFloat(Double(d.movies) / top) * g.size.height
                    Rectangle().fill(DashColors.shows)
                        .frame(width: slot - gap, height: eh)
                        .position(x: CGFloat(i) * slot + slot / 2, y: g.size.height - eh / 2)
                    Rectangle().fill(DashColors.movies)
                        .frame(width: slot - gap, height: mh)
                        .position(x: CGFloat(i) * slot + slot / 2, y: g.size.height - eh - mh / 2)
                }
            }
            .frame(height: 140)
            HStack {
                Text(days.first?.date ?? "")
                Spacer()
                Text(days.last?.date ?? "")
            }
            .font(.caption2).foregroundStyle(Palette.subtle)
        }
    }
}

private struct UserCard: View {
    let u: TopUser

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            HStack(spacing: 10) {
                DashAvatar(server: u.serverId, img: u.avatar, name: u.name, size: 44)
                VStack(alignment: .leading, spacing: 2) {
                    Text(u.name).font(.subheadline.weight(.semibold)).lineLimit(1)
                    Text(trn("{n} play", "{n} plays", Int(u.plays))).font(.caption).foregroundStyle(Palette.muted)
                    Text(hours(u.seconds)).font(.caption).foregroundStyle(Palette.muted)
                }
            }
            HStack { Text(tr("TV")); Spacer(); Text(hours(u.episodeSeconds)) }.font(.footnote)
            HStack { Text(tr("Movies")); Spacer(); Text(hours(u.movieSeconds)) }.font(.footnote)
        }
        .padding(12)
        .frame(width: 200)
        .background(Palette.surface, in: RoundedRectangle(cornerRadius: 12))
    }
}

private struct TitleRow: View {
    let title: String
    let list: [TopTitle]

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            Text(title).font(.subheadline).foregroundStyle(Palette.muted)
            if list.isEmpty {
                Text(tr("Nothing yet.")).font(.footnote).foregroundStyle(Palette.muted)
            } else {
                ScrollView(.horizontal, showsIndicators: false) {
                    HStack(alignment: .top, spacing: 12) {
                        ForEach(Array(list.enumerated()), id: \.offset) { _, t in
                            VStack(alignment: .leading, spacing: 4) {
                                DashImage(server: t.serverId, img: t.image)
                                    .frame(width: 110, height: 165)
                                    .clipShape(RoundedRectangle(cornerRadius: 8))
                                Text(t.title).font(.caption.weight(.semibold)).lineLimit(2)
                                Text(trn("{n} play", "{n} plays", Int(t.plays))).font(.caption2).foregroundStyle(Palette.muted)
                                Text(trn("{n} person", "{n} people", Int(t.users))).font(.caption2).foregroundStyle(Palette.muted)
                            }
                            .frame(width: 110)
                        }
                    }
                }
            }
        }
    }
}
