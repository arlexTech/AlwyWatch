// SPDX-License-Identifier: AGPL-3.0-or-later
import CryptoKit
import SwiftUI
import UIKit

/// `#RRGGBB` → colour (grey when it isn't one).
func hexColor(_ hex: String) -> Color {
    guard let v = UInt32(hex.trimmingCharacters(in: CharacterSet(charactersIn: "#")), radix: 16) else { return .gray }
    return Color(red: Double((v >> 16) & 0xFF) / 255, green: Double((v >> 8) & 0xFF) / 255, blue: Double(v & 0xFF) / 255)
}

/// The built-in avatars (from the core, which holds the table of docs/keep-protocol.md).
enum AvatarPresets {
    static let all: [AvatarPreset] = avatarPresets()
    static func of(_ id: String?) -> AvatarPreset? { id.flatMap { i in all.first { $0.id == i } } }
}

/// A person's round avatar (Android's Avatar): their uploaded picture, a built-in one, or the
/// default (the initial of their name on a colour derived from it).
struct PersonAvatar: View {
    @EnvironmentObject var model: AppModel
    let avatar: Avatar
    let name: String
    let userKey: String
    var size: CGFloat = 40
    @State private var picture: UIImage?

    var body: some View {
        ZStack {
            if let picture {
                Image(uiImage: picture).resizable().scaledToFill()
            } else if let preset = AvatarPresets.of(avatar.preset) {
                PresetAvatar(preset: preset)
            } else {
                hexColor(avatarDefaultColor(name: name))
                Text(avatarInitial(name: name))
                    .font(.system(size: size * 0.45, weight: .bold))
                    .foregroundStyle(.white)
            }
        }
        .frame(width: size, height: size)
        .clipShape(Circle())
        .task(id: "\(userKey)|\(avatar.customVersion ?? 0)") {
            // No user key (a room's host in a list): their picture can't be fetched.
            guard let version = avatar.customVersion, !userKey.isEmpty else { picture = nil; return }
            picture = await AvatarImages.load(core: model.core, userKey: userKey, version: version)
        }
    }
}

/// A built-in avatar: a disc of its colour with its shape (geometry in docs/keep-protocol.md).
struct PresetAvatar: View {
    let preset: AvatarPreset

    var body: some View {
        Canvas { ctx, size in
            let fg = hexColor(preset.foreground)
            ctx.fill(Path(CGRect(origin: .zero, size: size)), with: .color(hexColor(preset.background)))
            let u = min(size.width, size.height) / 4
            let c = CGPoint(x: size.width / 2, y: size.height / 2)
            func p(_ x: CGFloat, _ y: CGFloat) -> CGPoint { CGPoint(x: c.x + x * u, y: c.y + y * u) }
            func rect(_ x0: CGFloat, _ y0: CGFloat, _ x1: CGFloat, _ y1: CGFloat) -> Path {
                Path(CGRect(x: c.x + x0 * u, y: c.y + y0 * u, width: (x1 - x0) * u, height: (y1 - y0) * u))
            }
            func polygon(_ pts: [(CGFloat, CGFloat)]) -> Path {
                var path = Path()
                for (i, pt) in pts.enumerated() {
                    if i == 0 { path.move(to: p(pt.0, pt.1)) } else { path.addLine(to: p(pt.0, pt.1)) }
                }
                path.closeSubpath()
                return path
            }
            func circle(_ x: CGFloat, _ y: CGFloat, _ r: CGFloat) -> Path {
                Path(ellipseIn: CGRect(x: c.x + (x - r) * u, y: c.y + (y - r) * u, width: 2 * r * u, height: 2 * r * u))
            }
            var plus = rect(-1, -0.3, 1, 0.3)
            plus.addPath(rect(-0.3, -1, 0.3, 1))
            switch preset.shape {
            case "circle": ctx.fill(circle(0, 0, 1), with: .color(fg))
            case "ring": ctx.stroke(circle(0, 0, 0.75), with: .color(fg), lineWidth: 0.5 * u)
            case "square": ctx.fill(rect(-0.85, -0.85, 0.85, 0.85), with: .color(fg))
            case "diamond": ctx.fill(polygon([(0, -1), (1, 0), (0, 1), (-1, 0)]), with: .color(fg))
            case "triangle": ctx.fill(polygon([(0, -1), (0.95, 0.7), (-0.95, 0.7)]), with: .color(fg))
            case "plus": ctx.fill(plus, with: .color(fg))
            case "cross":
                let t = CGAffineTransform(translationX: c.x, y: c.y).rotated(by: .pi / 4).translatedBy(x: -c.x, y: -c.y)
                ctx.fill(plus.applying(t), with: .color(fg))
            case "star":
                ctx.fill(polygon((0 ..< 10).map { k in
                    let r: CGFloat = k % 2 == 0 ? 1 : 0.45
                    let a = -CGFloat.pi / 2 + CGFloat(k) * .pi / 5
                    return (r * cos(a), r * sin(a))
                }), with: .color(fg))
            case "hexagon":
                ctx.fill(polygon((0 ..< 6).map { k in
                    let a = -CGFloat.pi / 2 + CGFloat(k) * .pi / 3
                    return (cos(a), sin(a))
                }), with: .color(fg))
            case "bars":
                for y in [CGFloat(-0.8), -0.2, 0.4] { ctx.fill(rect(-1, y, 1, y + 0.4), with: .color(fg)) }
            case "half":
                var half = Path()
                half.addArc(center: p(0, 0.25), radius: u, startAngle: .degrees(180), endAngle: .degrees(360), clockwise: false)
                half.closeSubpath()
                ctx.fill(half, with: .color(fg))
            case "dots":
                for x in [CGFloat(-0.5), 0.5] {
                    for y in [CGFloat(-0.5), 0.5] { ctx.fill(circle(x, y, 0.35), with: .color(fg)) }
                }
            default: break
            }
        }
    }
}

/// Uploaded avatars: fetched with the AlwyWatch sign-in headers (they aren't public), kept on
/// disk by (user, version) and never revalidated (a new picture is a new version).
@MainActor
enum AvatarImages {
    private static var memory: [String: UIImage] = [:]
    private static var failed = Set<String>()

    private static func file(_ userKey: String, _ version: UInt32) -> URL {
        let digest = SHA256.hash(data: Data(userKey.utf8)).prefix(12).map { String(format: "%02x", $0) }.joined()
        let dir = FileManager.default.urls(for: .cachesDirectory, in: .userDomainMask)[0].appendingPathComponent("avatars")
        return dir.appendingPathComponent("\(digest)_\(version)")
    }

    /// A picture this device just uploaded: no need to fetch it back.
    static func put(userKey: String, version: UInt32, data: Data) {
        let f = file(userKey, version)
        try? FileManager.default.createDirectory(at: f.deletingLastPathComponent(), withIntermediateDirectories: true)
        try? data.write(to: f)
        if let img = UIImage(data: data) { memory["\(userKey)|\(version)"] = img }
    }

    static func load(core: AlwyWatch, userKey: String, version: UInt32) async -> UIImage? {
        let key = "\(userKey)|\(version)"
        if let hit = memory[key] { return hit }
        if failed.contains(key) { return nil }
        let f = file(userKey, version)
        if let img = UIImage(contentsOfFile: f.path) {
            memory[key] = img
            return img
        }
        guard let req = try? core.keepAvatarRequest(userKey: userKey, version: version), let url = URL(string: req.url) else {
            failed.insert(key)
            return nil
        }
        var r = URLRequest(url: url, timeoutInterval: 20)
        for h in req.headers { r.setValue(h.value, forHTTPHeaderField: h.name) }
        guard let (data, resp) = try? await URLSession.shared.data(for: r),
              (resp as? HTTPURLResponse)?.statusCode == 200, data.count < 1 << 20,
              let img = UIImage(data: data)
        else {
            failed.insert(key)
            return nil
        }
        try? FileManager.default.createDirectory(at: f.deletingLastPathComponent(), withIntermediateDirectories: true)
        try? data.write(to: f)
        memory[key] = img
        return img
    }
}
