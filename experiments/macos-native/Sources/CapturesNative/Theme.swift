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
    // Match the settings CSS's 52ch explanatory-copy measure in the native font.
    static var settingsCopyWidth: CGFloat {
        ("0" as NSString).size(withAttributes: [.font: NSFont.systemFont(ofSize: metric("text-sm"))]).width * 52
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
                .foregroundColor(NativeTheme.color("text-subtle", scheme))
                .frame(maxWidth: NativeTheme.settingsCopyWidth, alignment: .leading)
                .fixedSize(horizontal: false, vertical: true) }
        }
    }
}

struct CaptureOption<Value: Hashable>: Identifiable {
    let label: String
    let value: Value
    var id: Value { value }
}

struct CaptureSegments<Value: Hashable>: View {
    @Binding var selection: Value
    let options: [CaptureOption<Value>]
    var glass = false

    var body: some View {
        HStack(spacing: 2) {
            ForEach(options) { option in
                Button {
                    selection = option.value
                } label: {
                    Text(option.label)
                        .font(.system(size: NativeTheme.metric("text-sm"), weight: .medium))
                        .frame(maxWidth: .infinity)
                        .frame(height: NativeTheme.metric("h-xs"))
                        .foregroundColor(glass ? NativeTheme.glassText : nil)
                        .background(selection == option.value
                                    ? (glass ? NativeTheme.glassRaised : NativeTheme.accent.opacity(0.18))
                                    : Color.clear)
                        .clipShape(RoundedRectangle(cornerRadius: NativeTheme.metric("r-sm")))
                }
                .buttonStyle(.plain)
                .accessibilityAddTraits(selection == option.value ? .isSelected : [])
            }
        }
        .padding(3)
        .background(glass ? NativeTheme.glass : Color.primary.opacity(0.06))
        .clipShape(RoundedRectangle(cornerRadius: NativeTheme.metric("r-md")))
        .overlay(RoundedRectangle(cornerRadius: NativeTheme.metric("r-md"))
            .stroke(glass ? NativeTheme.color("glass-border") : Color.primary.opacity(0.1)))
    }
}

struct CaptureToggle: View {
    let title: String
    @Binding var isOn: Bool

    var body: some View {
        Button { isOn.toggle() } label: {
            Capsule()
                .fill(isOn ? NativeTheme.accent : Color.primary.opacity(0.16))
                .frame(width: 42, height: 24)
                .overlay(alignment: isOn ? .trailing : .leading) {
                    Circle().fill(isOn ? Color.black.opacity(0.82) : Color.white)
                        .shadow(color: .black.opacity(0.2), radius: 2, y: 1)
                        .padding(3)
                }
        }
        .buttonStyle(.plain)
        .accessibilityLabel(title)
        .accessibilityValue(isOn ? "On" : "Off")
        .accessibilityAddTraits(.isButton)
        .animation(NativeTheme.standard, value: isOn)
    }
}

struct CaptureToggleRow: View {
    let title: String
    @Binding var isOn: Bool

    var body: some View {
        HStack {
            Text(title)
            Spacer(minLength: NativeTheme.metric("s-3"))
            CaptureToggle(title: title, isOn: $isOn)
        }
    }
}

struct CaptureChoice<Value: Hashable>: View {
    let title: String
    @Binding var selection: Value
    let options: [CaptureOption<Value>]
    @State private var open = false
    @Environment(\.colorScheme) private var scheme

    private var selectedLabel: String {
        options.first(where: { $0.value == selection })?.label ?? "Choose"
    }

    var body: some View {
        Button { open.toggle() } label: {
            HStack(spacing: NativeTheme.metric("s-3")) {
                Text(selectedLabel).lineLimit(1)
                Spacer(minLength: 0)
                Image(systemName: "chevron.down")
                    .font(.system(size: 9, weight: .semibold))
                    .foregroundColor(NativeTheme.muted(scheme))
                    .rotationEffect(open ? .degrees(180) : .zero)
            }
        }
        .buttonStyle(CaptureButtonStyle())
        .accessibilityLabel(title)
        .overlay(alignment: .topLeading) {
            if open {
                VStack(spacing: 3) {
                    ForEach(options) { option in
                        Button {
                            selection = option.value
                            open = false
                        } label: {
                            HStack {
                                Text(option.label)
                                Spacer()
                                if option.value == selection { Image(systemName: "checkmark") }
                            }
                            .padding(.horizontal, NativeTheme.metric("s-4"))
                            .frame(minWidth: 190, minHeight: NativeTheme.metric("h-md"))
                            .background(option.value == selection ? NativeTheme.accent.opacity(0.16) : .clear)
                            .clipShape(RoundedRectangle(cornerRadius: NativeTheme.metric("r-md")))
                        }
                        .buttonStyle(.plain)
                    }
                }
                .padding(NativeTheme.metric("s-3"))
                .foregroundColor(NativeTheme.text(scheme))
                .background(NativeTheme.raised(scheme), in: RoundedRectangle(cornerRadius: NativeTheme.metric("r-lg")))
                .overlay(RoundedRectangle(cornerRadius: NativeTheme.metric("r-lg")).stroke(NativeTheme.border(scheme)))
                .shadow(color: .black.opacity(scheme == .dark ? 0.42 : 0.16), radius: 18, y: 8)
                .offset(y: NativeTheme.metric("h-md") + NativeTheme.metric("s-2"))
                .zIndex(100)
            }
        }
        .zIndex(open ? 100 : 0)
        .background(Button("") { open = false }.keyboardShortcut(.cancelAction).hidden())
    }
}

struct CaptureSlider: View {
    @Binding var value: Double
    let range: ClosedRange<Double>
    var tint = NativeTheme.accent

    var body: some View {
        GeometryReader { geometry in
            let progress = CGFloat((value - range.lowerBound) / max(.ulpOfOne, range.upperBound - range.lowerBound))
            ZStack(alignment: .leading) {
                Capsule().fill(Color.primary.opacity(0.14)).frame(height: 5)
                Capsule().fill(tint).frame(width: max(5, geometry.size.width * progress), height: 5)
                Circle().fill(Color.white).shadow(color: .black.opacity(0.24), radius: 2, y: 1)
                    .frame(width: 15, height: 15)
                    .offset(x: max(0, min(geometry.size.width - 15, geometry.size.width * progress - 7.5)))
            }
            .contentShape(Rectangle())
            .gesture(DragGesture(minimumDistance: 0).onChanged { drag in
                let unit = min(1, max(0, drag.location.x / max(1, geometry.size.width)))
                value = range.lowerBound + Double(unit) * (range.upperBound - range.lowerBound)
            })
        }
        .frame(height: 20)
        .accessibilityElement()
        .accessibilityValue("\(Int(value.rounded()))")
        .accessibilityAdjustableAction { direction in
            let step = (range.upperBound - range.lowerBound) / 20
            value = min(range.upperBound, max(range.lowerBound, value + (direction == .increment ? step : -step)))
        }
    }
}
