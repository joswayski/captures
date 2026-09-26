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
    /// Workbench update notice fixture (stub status source; no updater).
    var updateState: String?
    var updateTray: String?
    var appearanceOverride = false
    var themeOverride = false

    init(_ arguments: [String], bundled: Bool = false) throws {
        live = bundled
        var iterator = arguments.makeIterator()
        while let argument = iterator.next() {
            switch argument {
            case "--":
                while let path = iterator.next() {
                    guard !path.isEmpty else { throw Usage.invalid }
                    openMedia.append(path)
                }
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
            case "--update-state":
                guard let value = iterator.next(), UpdateNoticeModel.fixtures.contains(value) else { throw Usage.invalid }
                updateState = value
            case "--update-tray":
                guard let value = iterator.next(), ["top", "bottom", "none"].contains(value) else { throw Usage.invalid }
                updateTray = value
            case "--quit-after":
                guard let raw = iterator.next(), let seconds = Double(raw), seconds.isFinite, seconds > 0 else { throw Usage.invalid }
                quitAfter = seconds
            default: throw Usage.invalid
            }
        }
        guard ["preferences", "history", "hud", "preview", "region", "window", "update", "idle"].contains(scene),
            (updateState == nil && updateTray == nil) || scene == "update",
            !(scene == "update" && (live || exercise)),
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
                guard let url = NativeResources.bundle.url(forResource: "EDITOR-FONT-LICENSE", withExtension: "txt")
                else { throw Options.Usage.invalid }
                print(try String(contentsOf: url, encoding: .utf8), terminator: "")
                return
            }
            let bundled = Bundle.main.object(forInfoDictionaryKey: "CapturesNativeLive") as? Bool == true
            let options = try Options(Array(CommandLine.arguments.dropFirst()), bundled: bundled)
            if bundled {
                runBundle(options)
                return
            }
            var nativeInstance: NativeInstance?
            if options.live {
                let result = try NativeInstance.start(historyRoot: options.historyRoot, paths: options.openMedia)
                guard result.primary else { return }
                nativeInstance = result.owner
            }
            defer { nativeInstance?.close() }
            let application = NSApplication.shared
            application.setActivationPolicy(options.scene == "idle" ? .accessory : .regular)
            let delegate = Workbench(options: options, nativeInstance: nativeInstance)
            application.delegate = delegate
            withExtendedLifetime(delegate) { application.run() }
        } catch Options.Usage.invalid {
            FileHandle.standardError.write(Data("Usage: CapturesNative [--live [--history-root PATH] [--open-media PATH|--open-image PATH]...] [--scene preferences|history|hud|preview|region|window|update|idle] [--update-state available|single|closing|manual|downloading|restarting|error|checking|up-to-date] [--update-tray top|bottom|none] [--appearance light|dark|system] [--theme mustard|ember|rose|violet|cobalt|aqua|mint|lime|mono] [--history-count 0..10000] [--settings-file PATH] [--screenshot PATH] [--reference-chips] [--exercise] [--quit-after SECONDS] [-- FILE...]\n".utf8))
            exit(1)
        } catch {
            FileHandle.standardError.write(Data("Captures could not start: \(error.localizedDescription)\n".utf8))
            exit(1)
        }
    }

    private static func runBundle(_ options: Options) {
        let application = NSApplication.shared
        application.setActivationPolicy(.regular)
        var workbench: Workbench?
        var nativeInstance: NativeInstance?
        defer { nativeInstance?.close() }
        // LaunchServices delivers cold-open Apple events before didFinishLaunching.
        // Collect those before election so a secondary does not exit and lose them.
        let launch = BundledLaunch(options: options) { options, notification in
            let result = try NativeInstance.start(historyRoot: options.historyRoot, paths: options.openMedia)
            guard result.primary else { return false }
            nativeInstance = result.owner
            let delegate = Workbench(options: options, nativeInstance: nativeInstance)
            workbench = delegate
            application.delegate = delegate
            delegate.applicationDidFinishLaunching(notification)
            return true
        }
        application.delegate = launch
        withExtendedLifetime(launch) { application.run() }
        withExtendedLifetime(workbench) {}
    }
}
