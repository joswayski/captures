import AppKit
import SwiftUI

enum NativeTheme {
    private static let tokens: [String: Any] = {
        guard let url = Bundle.main.url(forResource: "tokens", withExtension: "json"),
              let bytes = try? Data(contentsOf: url),
              let result = try? JSONSerialization.jsonObject(with: bytes) as? [String: Any] else {
            preconditionFailure("Missing design tokens. Build with experiments/macos-native/build.sh.")
        }
        return result
    }()
    static var palettes: [String: [String: String]] { tokens["themes"] as? [String: [String: String]] ?? [:] }
    static func value(_ name: String, _ scheme: ColorScheme = .dark) -> String {
        guard let result = (tokens[scheme == .dark ? "dark" : "light"] as? [String: String])?[name] else {
            preconditionFailure("Missing design token: \(name)")
        }
        return result
    }
    static func metric(_ name: String) -> CGFloat {
        CGFloat(Double(value(name).replacingOccurrences(of: "px", with: "")) ?? 0)
    }
    static func color(_ name: String, _ scheme: ColorScheme = .dark) -> Color { Color(css: value(name, scheme)) }
    static func canvas(_ scheme: ColorScheme) -> Color { color("surface-canvas", scheme) }
    static func raised(_ scheme: ColorScheme) -> Color { color("surface-raised", scheme) }
    static func field(_ scheme: ColorScheme) -> Color { color("surface-field", scheme) }
    static func text(_ scheme: ColorScheme) -> Color { color("text", scheme) }
    static func muted(_ scheme: ColorScheme) -> Color { color("text-muted", scheme) }
    static func border(_ scheme: ColorScheme) -> Color { color("border", scheme) }
    static var glass: Color { color("glass") }
    static var glassRaised: Color { color("glass-raised") }
    static var glassText: Color { color("glass-text") }
    static var glassMuted: Color { color("glass-text-muted") }
    static var accent: Color { Color(css: AppStore.shared.settings.accentHex) }
    static var signal: Color { Color(css: AppStore.shared.settings.signalHex) }
    static var motion: Animation { .timingCurve(0.16, 1, 0.3, 1, duration: duration("dur-4")) }
    static var standard: Animation { .timingCurve(0.2, 0.8, 0.2, 1, duration: duration("dur-2")) }
    static func duration(_ name: String) -> Double {
        (Double(value(name).replacingOccurrences(of: "ms", with: "")) ?? 0) / 1000
    }
}

extension Color {
    init(css: String) {
        self.init(nsColor: NSColor(css: css))
    }
}

extension NSColor {
    convenience init(css: String) {
        let input = css.trimmingCharacters(in: .whitespacesAndNewlines)
        if input.hasPrefix("#"), let hex = UInt64(input.dropFirst(), radix: 16), input.count == 7 {
            self.init(srgbRed: Double((hex >> 16) & 255) / 255,
                      green: Double((hex >> 8) & 255) / 255, blue: Double(hex & 255) / 255, alpha: 1)
        } else {
            let components = input.components(separatedBy: CharacterSet(charactersIn: "0123456789.").inverted)
                .compactMap(Double.init)
            precondition(components.count == 3 || components.count == 4, "Unsupported color token: \(input)")
            self.init(srgbRed: components[0] / 255, green: components[1] / 255,
                      blue: components[2] / 255, alpha: components.count == 4 ? components[3] : 1)
        }
    }
}

struct CaptureButtonStyle: ButtonStyle {
    var primary = false
    var glass = false
    var destructive = false
    func makeBody(configuration: Configuration) -> some View {
        StyledButton(configuration: configuration, primary: primary, glass: glass, destructive: destructive)
    }
    private struct StyledButton: View {
        let configuration: ButtonStyle.Configuration
        let primary: Bool
        let glass: Bool
        let destructive: Bool
        @Environment(\.colorScheme) private var scheme
        @Environment(\.isEnabled) private var enabled
        @Environment(\.accessibilityReduceMotion) private var reduceMotion
        @State private var hover = false
        var body: some View {
            configuration.label
                .font(.system(size: NativeTheme.metric("text-md"), weight: .medium))
                .padding(.horizontal, NativeTheme.metric("s-5"))
                .frame(minHeight: NativeTheme.metric("h-md"))
                .foregroundColor(primary ? .black : destructive && hover ? NativeTheme.signal :
                                    glass ? NativeTheme.glassText : NativeTheme.text(scheme))
                .background(background)
                .clipShape(RoundedRectangle(cornerRadius: NativeTheme.metric("r-md")))
                .overlay(RoundedRectangle(cornerRadius: NativeTheme.metric("r-md"))
                    .strokeBorder(glass ? NativeTheme.color("glass-border") : NativeTheme.border(scheme)))
                .opacity(enabled ? (configuration.isPressed ? 0.75 : 1) : 0.4)
                .onHover { hover = $0 }
                .animation(reduceMotion ? nil : NativeTheme.standard, value: hover)
        }
        private var background: Color {
            if primary { return NativeTheme.accent }
            if destructive && hover { return NativeTheme.signal.opacity(0.14) }
            if glass { return hover ? NativeTheme.glassRaised : NativeTheme.glass }
            return NativeTheme.color(hover ? "surface-hover" : "surface-raised", scheme)
        }
    }
}

struct SectionTitle: View {
    let title: String
    let subtitle: String?
    @Environment(\.colorScheme) private var scheme
    init(_ title: String, subtitle: String? = nil) { self.title = title; self.subtitle = subtitle }
    var body: some View {
        VStack(alignment: .leading, spacing: NativeTheme.metric("s-2")) {
            Text(title).font(.system(size: NativeTheme.metric("text-lg"), weight: .semibold))
            if let subtitle { Text(subtitle).font(.system(size: NativeTheme.metric("text-sm")))
                .foregroundColor(NativeTheme.muted(scheme)) }
        }
    }
}
