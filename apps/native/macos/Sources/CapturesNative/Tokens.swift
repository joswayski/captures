import AppKit

struct Tokens: Decodable {
    let colors: [String: [Double]]
    let numbers: [String: Double]
    /// `--ease-*` tokens as cubic-bezier control points.
    let easings: [String: [Double]]?
    /// `box-shadow` tokens (`shadow-*`, `glass-shadow`, `thumbnail-card-shadow`),
    /// first layer on top.
    let shadows: [String: [BoxShadowLayer]]?

    static let variants: [String: Tokens] = {
        let url = NativeResources.bundle.url(forResource: "tokens", withExtension: "json")!
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

    func easing(_ name: String) -> [Double] {
        guard let points = easings?[name], points.count == 4 else { preconditionFailure("Missing easing \(name)") }
        return points
    }

    func shadow(_ name: String) -> [BoxShadowLayer] {
        guard let layers = shadows?[name] else { preconditionFailure("Missing shadow \(name)") }
        return layers
    }

    func applyingCustomTheme(_ custom: [String: Any], light: Bool,
                             transport: SettingsTransport = SettingsBridge()) -> Tokens {
        guard let response = try? transport.request([
            "operation": "theme", "accent": custom.string("accent", "#32d3ff"),
            "signal": custom.string("signal", "#ff4fc3"), "light": light,
        ]), let derived = response["colors"] as? [String: [Double]] else { return self }
        return Tokens(colors: colors.merging(derived) { _, value in value }, numbers: numbers, easings: easings,
                      shadows: shadows)
    }
}

/// One CSS `box-shadow` layer: offsets, blur radius and spread in points,
/// unmultiplied sRGB color.
struct BoxShadowLayer: Decodable, Equatable {
    let x: Double
    let y: Double
    let blur: Double
    let spread: Double
    let color: [Double]

    var nsColor: NSColor {
        NSColor(srgbRed: color[0], green: color[1], blue: color[2], alpha: color.count > 3 ? color[3] : 1)
    }
}

/// CSS `box-shadow` layers as Core Animation shadow sublayers. Each layer
/// casts from a rounded-rect `shadowPath` (grown by the spread) with
/// `shadowRadius` = blur / 2, Core Animation's Gaussian standard deviation,
/// matching CSS's blur-radius-to-sigma rule. Offsets follow the repository's
/// layer-backed convention (negative height moves the shadow down).
enum BoxShadowLayers {
    static let layerName = "captures-box-shadow"

    /// Insert (or replace) the layers under `host`'s content for a
    /// `bounds`-sized element with `radius` corners.
    @discardableResult
    static func install(_ shadows: [BoxShadowLayer], on host: CALayer, bounds: CGRect,
                        radius: CGFloat, opacity: Float = 1) -> [CALayer] {
        CATransaction.begin(); CATransaction.setDisableActions(true)
        defer { CATransaction.commit() }
        remove(from: host)
        var installed: [CALayer] = []
        // CSS paints the first layer on top: insert the last one lowest.
        for shadow in shadows.reversed() {
            let layer = CALayer()
            layer.name = layerName
            layer.frame = bounds
            let local = CGRect(origin: .zero, size: bounds.size)
            let spread = CGFloat(shadow.spread)
            let shape = local.insetBy(dx: -spread, dy: -spread)
            guard !shape.isNull, !shape.isEmpty else { continue }
            layer.shadowPath = roundedPath(shape, radius: radius + spread)
            layer.shadowColor = shadow.nsColor.withAlphaComponent(1).cgColor
            layer.shadowOpacity = Float(shadow.color.count > 3 ? shadow.color[3] : 1)
            layer.shadowRadius = CGFloat(shadow.blur) / 2
            layer.shadowOffset = CGSize(width: shadow.x, height: -shadow.y)
            layer.opacity = opacity
            // CSS never paints an outer shadow under its element.
            let outside = CAShapeLayer()
            let reach = CGFloat(shadow.blur) * 2 + abs(spread) + CGFloat(max(abs(shadow.x), abs(shadow.y))) + 4
            let cutout = CGMutablePath()
            cutout.addRect(local.insetBy(dx: -reach, dy: -reach))
            cutout.addPath(roundedPath(local, radius: radius))
            outside.frame = local
            outside.path = cutout
            outside.fillRule = .evenOdd
            layer.mask = outside
            host.insertSublayer(layer, at: UInt32(installed.count))
            installed.append(layer)
        }
        return installed
    }

    static func remove(from host: CALayer?) {
        host?.sublayers?.filter { $0.name == layerName }.forEach { $0.removeFromSuperlayer() }
    }

    /// A rounded rect path; CGPath requires corners within half each side.
    static func roundedPath(_ rect: CGRect, radius: CGFloat) -> CGPath {
        let corner = max(0, min(radius, rect.width / 2, rect.height / 2))
        return CGPath(roundedRect: rect, cornerWidth: corner, cornerHeight: corner, transform: nil)
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
