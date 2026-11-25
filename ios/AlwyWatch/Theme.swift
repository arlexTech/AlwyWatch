// SPDX-License-Identifier: AGPL-3.0-or-later
import SwiftUI

/// The Android app's palette (ui/Theme.kt).
enum Palette {
    static let background = Color(red: 0x09 / 255, green: 0x09 / 255, blue: 0x0C / 255)
    static let surface = Color.white.opacity(0x14 / 255)
    static let surfaceHigh = Color.white.opacity(0x24 / 255)
    static let outline = Color.white.opacity(0x1F / 255)
    static let onSurface = Color(red: 0xF4 / 255, green: 0xF4 / 255, blue: 0xF6 / 255)
    static let muted = onSurface.opacity(0x99 / 255)
    static let subtle = onSurface.opacity(0x66 / 255)
    static let accent = Color(red: 1, green: 0xB2 / 255, blue: 0x24 / 255)
    static let danger = Color(red: 1, green: 0x5A / 255, blue: 0x5F / 255)
    static let success = Color(red: 0x3D / 255, green: 0xDC / 255, blue: 0x97 / 255)
}

/// A primary (accent) or secondary button, as on Android.
struct ActionButtonStyle: ButtonStyle {
    var primary = false
    func makeBody(configuration: Configuration) -> some View {
        configuration.label
            .font(.body.weight(.semibold))
            .padding(.horizontal, 20)
            .padding(.vertical, 12)
            .frame(minHeight: 44)
            .foregroundStyle(primary ? Color.black : Palette.onSurface)
            .background(primary ? Palette.accent : Palette.surfaceHigh, in: Capsule())
            .opacity(configuration.isPressed ? 0.7 : 1)
    }
}

/// A big tappable choice (sign-in steps).
struct ChoiceCard: View {
    let title: String
    let subtitle: String
    let icon: String
    let action: () -> Void

    var body: some View {
        Button(action: action) {
            HStack(spacing: 16) {
                Image(systemName: icon)
                    .font(.title3)
                    .foregroundStyle(Palette.accent)
                    .frame(width: 48, height: 48)
                    .background(Palette.surfaceHigh, in: RoundedRectangle(cornerRadius: 14))
                VStack(alignment: .leading, spacing: 4) {
                    Text(title).font(.headline).foregroundStyle(Palette.onSurface)
                    Text(subtitle).font(.subheadline).foregroundStyle(Palette.muted)
                }
                Spacer()
                Image(systemName: "chevron.right").foregroundStyle(Palette.subtle)
            }
            .padding(18)
            .background(Palette.surface, in: RoundedRectangle(cornerRadius: 20))
            .overlay(RoundedRectangle(cornerRadius: 20).stroke(Palette.outline))
        }
        .buttonStyle(.plain)
    }
}

/// A row title ("Continue watching") with an optional "See all".
struct SectionTitle: View {
    let title: String
    var seeAll: Route? = nil

    var body: some View {
        HStack {
            Text(title).font(.title3.weight(.bold))
            Spacer()
            if let seeAll {
                NavigationLink(tr("See all"), value: seeAll).font(.subheadline).foregroundStyle(Palette.accent)
            }
        }
        .padding(.horizontal, 16)
    }
}

/// An error line (danger colour).
struct ErrorText: View {
    let text: String?
    var body: some View {
        if let text { Text(text).foregroundStyle(Palette.danger).font(.subheadline) }
    }
}

/// Lays its views out in rows, wrapping to the next row when one is full (Android's FlowRow).
struct FlowLayout: Layout {
    var spacing: CGFloat = 8

    func sizeThatFits(proposal: ProposedViewSize, subviews: Subviews, cache: inout ()) -> CGSize {
        let rows = arrange(proposal.width ?? .infinity, subviews)
        let width = rows.map(\.width).max() ?? 0
        let height = rows.map(\.height).reduce(0, +) + spacing * CGFloat(max(0, rows.count - 1))
        return CGSize(width: proposal.width ?? width, height: height)
    }

    func placeSubviews(in bounds: CGRect, proposal: ProposedViewSize, subviews: Subviews, cache: inout ()) {
        var y = bounds.minY
        for row in arrange(bounds.width, subviews) {
            var x = bounds.minX
            for i in row.items {
                let size = subviews[i].sizeThatFits(ProposedViewSize(width: bounds.width, height: nil))
                subviews[i].place(at: CGPoint(x: x, y: y + (row.height - size.height) / 2), proposal: ProposedViewSize(size))
                x += size.width + spacing
            }
            y += row.height + spacing
        }
    }

    private struct Row {
        var items: [Int] = []
        var width: CGFloat = 0
        var height: CGFloat = 0
    }

    private func arrange(_ maxWidth: CGFloat, _ subviews: Subviews) -> [Row] {
        var rows: [Row] = []
        var row = Row()
        for i in subviews.indices {
            let size = subviews[i].sizeThatFits(ProposedViewSize(width: maxWidth.isFinite ? maxWidth : nil, height: nil))
            let needed = row.items.isEmpty ? size.width : row.width + spacing + size.width
            if !row.items.isEmpty, needed > maxWidth {
                rows.append(row)
                row = Row()
            }
            row.width = row.items.isEmpty ? size.width : row.width + spacing + size.width
            row.height = max(row.height, size.height)
            row.items.append(i)
        }
        if !row.items.isEmpty { rows.append(row) }
        return rows
    }
}
