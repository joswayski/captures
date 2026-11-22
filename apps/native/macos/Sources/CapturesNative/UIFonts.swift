import AppKit
import CoreText

/// Registers the bundled cross-platform UI typefaces (`Figtree` and
/// `JetBrains Mono`) from `NativeResources.bundle` with Core Text once per
/// process so macOS matches Windows, Linux, and `captur.es`.
enum UIFonts {
    private static let faceNames = [
        "Figtree-Regular",
        "Figtree-Medium",
        "Figtree-SemiBold",
        "Figtree-Bold",
        "Figtree-SemiBoldItalic",
        "JetBrainsMono-Regular",
        "JetBrainsMono-Medium",
        "JetBrainsMono-SemiBold",
    ]

    private static let registered: Bool = {
        var any = false
        for name in faceNames {
            guard let url = NativeResources.bundle.url(forResource: name, withExtension: "ttf") else {
                continue
            }
            if CTFontManagerRegisterFontsForURL(url as CFURL, .process, nil) {
                any = true
            }
        }
        return any
    }()

    @discardableResult
    static func register() -> Bool { registered }
}

extension NSFont {
    /// Bundled Figtree UI face at `size` and `weight`, falling back to the
    /// system font only when resources are absent.
    static func capturesUI(ofSize size: CGFloat, weight: NSFont.Weight = .regular) -> NSFont {
        _ = UIFonts.register()
        let postScript: String
        switch weight {
        case let w where w.rawValue >= NSFont.Weight.bold.rawValue - 0.01:
            postScript = "Figtree-Bold"
        case let w where w.rawValue >= NSFont.Weight.semibold.rawValue - 0.01:
            postScript = "Figtree-SemiBold"
        case let w where w.rawValue >= NSFont.Weight.medium.rawValue - 0.01:
            postScript = "Figtree-Medium"
        default:
            postScript = "Figtree-Regular"
        }
        return NSFont(name: postScript, size: size) ?? .systemFont(ofSize: size, weight: weight)
    }

    /// Bundled JetBrains Mono face for pixel dimensions, shortcuts, and paths.
    static func capturesMono(ofSize size: CGFloat, weight: NSFont.Weight = .regular) -> NSFont {
        _ = UIFonts.register()
        let postScript: String
        switch weight {
        case let w where w.rawValue >= NSFont.Weight.semibold.rawValue - 0.01:
            postScript = "JetBrainsMono-SemiBold"
        case let w where w.rawValue >= NSFont.Weight.medium.rawValue - 0.01:
            postScript = "JetBrainsMono-Medium"
        default:
            postScript = "JetBrainsMono-Regular"
        }
        return NSFont(name: postScript, size: size)
            ?? .monospacedSystemFont(ofSize: size, weight: weight)
    }

    /// Bundled Figtree with tabular lining digits for live counters and timers.
    static func capturesMonoDigits(ofSize size: CGFloat, weight: NSFont.Weight = .regular) -> NSFont {
        let base = capturesUI(ofSize: size, weight: weight)
        let descriptor = base.fontDescriptor.addingAttributes([
            .featureSettings: [[
                NSFontDescriptor.FeatureKey.typeIdentifier: kNumberSpacingType,
                NSFontDescriptor.FeatureKey.selectorIdentifier: kMonospacedNumbersSelector,
            ]],
        ])
        return NSFont(descriptor: descriptor, size: size)
            ?? .monospacedDigitSystemFont(ofSize: size, weight: weight)
    }
}
