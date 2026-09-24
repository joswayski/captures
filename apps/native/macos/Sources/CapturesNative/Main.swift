import AppKit

struct Options {
    static let themes = ["mustard", "ember", "rose", "violet", "cobalt", "aqua", "mint", "lime", "mono"]
    var scene = "preferences"
    var appearance = "dark"
    var theme = "mustard"
    var historyCount = 1000
    var referenceChips = false
    var exercise = false
    var live = false
    var historyRoot: String?
    var openMedia: [String] = []
    var quitAfter: Double?
    var settingsFile: String?
    var screenshot: String?
    var appearanceOverride = false
    var themeOverride = false

    init(_ arguments: [String]) throws {
        var iterator = arguments.makeIterator()
        while let argument = iterator.next() {
            switch argument {
            case "--scene": scene = iterator.next() ?? ""
            case "--appearance": appearance = iterator.next() ?? ""; appearanceOverride = true
            case "--theme": theme = iterator.next() ?? ""; themeOverride = true
            case "--history-count":
                guard let raw = iterator.next(), let count = Int(raw), (0...10000).contains(count) else { throw Usage.invalid }
                historyCount = count
            case "--reference-chips": referenceChips = true
            case "--exercise": exercise = true
            case "--live": live = true
            case "--history-root":
                guard let value = iterator.next(), !value.isEmpty else { throw Usage.invalid }
                historyRoot = value
            case "--open-image", "--open-media":
                guard let value = iterator.next(), !value.isEmpty,
                      !value.hasPrefix("--") else { throw Usage.invalid }
                openMedia.append(value)
            case "--settings-file":
                guard let value = iterator.next(), !value.isEmpty else { throw Usage.invalid }
                settingsFile = value
            case "--screenshot":
                guard let value = iterator.next(), !value.isEmpty else { throw Usage.invalid }
                screenshot = value
            case "--quit-after":
                guard let raw = iterator.next(), let seconds = Double(raw), seconds.isFinite, seconds > 0 else { throw Usage.invalid }
                quitAfter = seconds
            default: throw Usage.invalid
            }
        }
        guard ["preferences", "history", "hud", "preview", "region", "window", "idle"].contains(scene),
            ["light", "dark", "system"].contains(appearance), Self.themes.contains(theme),
            !(live && (exercise || referenceChips)), historyRoot == nil || live,
            openMedia.isEmpty || live
        else { throw Usage.invalid }
    }
    enum Usage: Error { case invalid }
}

@main enum Main {
    static func main() {
        do {
            if CommandLine.arguments.dropFirst().elementsEqual(["--font-license"]) {
                guard let url = Bundle.module.url(forResource: "EDITOR-FONT-LICENSE", withExtension: "txt")
                else { throw Options.Usage.invalid }
                print(try String(contentsOf: url, encoding: .utf8), terminator: "")
                return
            }
            let options = try Options(Array(CommandLine.arguments.dropFirst()))
            let application = NSApplication.shared
            application.setActivationPolicy(options.scene == "idle" ? .accessory : .regular)
            let delegate = Workbench(options: options)
            application.delegate = delegate
            withExtendedLifetime(delegate) { application.run() }
        } catch {
            FileHandle.standardError.write(Data("Usage: CapturesNative [--live [--history-root PATH] [--open-media PATH|--open-image PATH]...] [--scene preferences|history|hud|preview|region|window|idle] [--appearance light|dark|system] [--theme mustard|ember|rose|violet|cobalt|aqua|mint|lime|mono] [--history-count 0..10000] [--settings-file PATH] [--screenshot PATH] [--reference-chips] [--exercise] [--quit-after SECONDS]\n".utf8))
            exit(1)
        }
    }
}
