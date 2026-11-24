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
