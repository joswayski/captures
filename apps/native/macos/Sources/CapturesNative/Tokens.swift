import AppKit

struct Tokens: Decodable {
    let colors: [String: [Double]]
    let numbers: [String: Double]

    static let variants: [String: Tokens] = {
        let url = Bundle.module.url(forResource: "tokens", withExtension: "json")!
        return try! JSONDecoder().decode([String: Tokens].self, from: Data(contentsOf: url))
    }()

    func color(_ name: String) -> NSColor {
        guard let c = colors[name], c.count == 4 else { preconditionFailure("Missing color \(name)") }
        return NSColor(srgbRed: c[0], green: c[1], blue: c[2], alpha: c[3])
    }

    func number(_ name: String) -> CGFloat {
        guard let n = numbers[name] else { preconditionFailure("Missing number \(name)") }
        return CGFloat(n)
    }

    func applyingCustomTheme(_ custom: [String: Any], light: Bool,
                             transport: SettingsTransport = SettingsBridge()) -> Tokens {
        guard let response = try? transport.request([
            "operation": "theme", "accent": custom.string("accent", "#32d3ff"),
            "signal": custom.string("signal", "#ff4fc3"), "light": light,
        ]), let derived = response["colors"] as? [String: [Double]] else { return self }
        return Tokens(colors: colors.merging(derived) { _, value in value }, numbers: numbers)
    }
}

extension NSColor {
    convenience init?(hex: String) {
        guard let value = PreferencesController.normalizeHex(hex), let rgb = Int(value.dropFirst(), radix: 16) else { return nil }
        self.init(srgbRed: CGFloat((rgb >> 16) & 255) / 255, green: CGFloat((rgb >> 8) & 255) / 255,
                  blue: CGFloat(rgb & 255) / 255, alpha: 1)
    }

    var rgbHex: String? {
        guard let color = usingColorSpace(.sRGB) else { return nil }
        return String(format: "#%02X%02X%02X", Int((color.redComponent * 255).rounded()),
                      Int((color.greenComponent * 255).rounded()), Int((color.blueComponent * 255).rounded()))
    }
}
