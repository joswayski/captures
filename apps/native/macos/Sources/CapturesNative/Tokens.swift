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
}
