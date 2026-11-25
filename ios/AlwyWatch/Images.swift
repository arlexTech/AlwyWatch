// SPDX-License-Identifier: AGPL-3.0-or-later
import SwiftUI
import UIKit

/// Decoded images kept in memory (the core keeps the files on disk).
private let memory: NSCache<NSString, UIImage> = {
    let c = NSCache<NSString, UIImage>()
    c.totalCostLimit = 64 << 20
    return c
}()

/// A server image through the core's disk cache: instant when cached, else fetched in the
/// background. A plain surface while it loads (no spinner).
struct ServerImage: View {
    @EnvironmentObject var model: AppModel
    let account: String
    let itemId: String
    let image: ImageRef?
    /// Width in points; the core rounds it up to a cached size.
    let width: CGFloat
    @State private var loaded: UIImage?

    var body: some View {
        ZStack {
            Palette.surface
            if let loaded {
                Image(uiImage: loaded).resizable().scaledToFill()
            }
        }
        .clipped()
        .task(id: image?.path) { await load() }
    }

    private func load() async {
        guard let image else { loaded = nil; return }
        let px = UInt32(width * UIScreen.main.scale)
        let key = "\(account)|\(image.path)|\(px)" as NSString
        if let hit = memory.object(forKey: key) { loaded = hit; return }
        let core = model.core
        let account = account, itemId = itemId
        let decoded: UIImage? = await Task.detached(priority: .utility) {
            var path = core.cachedImage(accountId: account, itemId: itemId, image: image, width: px)
            if path == nil { path = try? await core.image(accountId: account, itemId: itemId, image: image, width: px) }
            guard let path, let img = UIImage(contentsOfFile: path) else { return nil }
            return img.preparingForDisplay() ?? img
        }.value
        if let decoded {
            memory.setObject(decoded, forKey: key, cost: Int(decoded.size.width * decoded.size.height * decoded.scale * decoded.scale * 4))
            loaded = decoded
        }
    }
}

/// A 2:3 poster card with its title (rows and grids).
struct PosterCard: View {
    let account: String
    let item: Item
    var width: CGFloat = 120

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            ServerImage(account: account, itemId: item.id, image: item.poster, width: width)
                .frame(width: width, height: width * 1.5)
                .clipShape(RoundedRectangle(cornerRadius: 10))
                .overlay(alignment: .bottom) { ProgressLine(progress: item.progress).padding(8) }
                .overlay(alignment: .topTrailing) {
                    if item.user.played {
                        Image(systemName: "checkmark.circle.fill")
                            .foregroundStyle(Palette.accent, Color.black.opacity(0.6))
                            .padding(6)
                    }
                }
            Text(item.cardTitle).font(.subheadline.weight(.semibold)).lineLimit(1)
            Text(item.kind == .episode ? (item.episodeCode ?? item.subtitle) : item.subtitle).font(.caption).foregroundStyle(Palette.muted).lineLimit(1)
        }
        .frame(width: width)
    }
}

/// The watched part, inset along the bottom of a poster (as on Android).
struct ProgressLine: View {
    let progress: Double?
    var body: some View {
        if let progress {
            GeometryReader { g in
                ZStack(alignment: .leading) {
                    Capsule().fill(.white.opacity(0.25))
                    Capsule().fill(Palette.accent).frame(width: g.size.width * progress)
                }
            }
            .frame(height: 4)
        }
    }
}
