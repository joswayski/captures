import AppKit
import CCapturesSettings

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
    var nativeUpdateReadyFile: String?
    var nativeUpdateReadyToken: String?
    var nativeUpdateRestorePreferences = false
    var nativeUpdateChecks: UpdateCheckModel?
    var nativeUpdateShutdownIntent: [String: Any]?
    var nativeUpdateInstallSession: String?
    var screenshot: String?
    /// Workbench update notice fixture (stub status source; no updater).
    var updateState: String?
    var updateTray: String?
    var appearanceOverride = false
    var themeOverride = false

    init(_ arguments: [String], bundled: Bool = false) throws {
        live = bundled
        var explicitLive = false
        var updateEndpoint: String?
        var updateKeyFile: String?
        var updateCurrentVersion: String?
        var updateStagingDirectory: String?
        var updateBaseArchive: String?
        var updateShutdownIntentFile: String?
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
            case "--live": live = true; explicitLive = true
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
            case "--native-update-ready-file":
                guard nativeUpdateReadyFile == nil, let value = iterator.next(), !value.isEmpty else { throw Usage.invalid }
                nativeUpdateReadyFile = value
            case "--native-update-ready-token":
                guard nativeUpdateReadyToken == nil, let value = iterator.next(), !value.isEmpty else { throw Usage.invalid }
                nativeUpdateReadyToken = value
            case "--native-update-manifest-url":
                guard updateEndpoint == nil, let value = iterator.next(), !value.isEmpty else { throw Usage.invalid }
                updateEndpoint = value
            case "--native-update-public-key-file":
                guard updateKeyFile == nil, let value = iterator.next(), !value.isEmpty else { throw Usage.invalid }
                updateKeyFile = value
            case "--native-update-current-version":
                guard updateCurrentVersion == nil, let value = iterator.next(), !value.isEmpty else { throw Usage.invalid }
                updateCurrentVersion = value
            case "--native-update-staging-directory":
                guard updateStagingDirectory == nil, let value = iterator.next(), !value.isEmpty else { throw Usage.invalid }
                updateStagingDirectory = value
            case "--native-update-base-archive":
                guard updateBaseArchive == nil, let value = iterator.next(), !value.isEmpty else { throw Usage.invalid }
                updateBaseArchive = value
            case "--native-update-shutdown-intent-file":
                guard updateShutdownIntentFile == nil, let value = iterator.next(), !value.isEmpty else { throw Usage.invalid }
                updateShutdownIntentFile = value
            case "--native-update-install-session":
                guard nativeUpdateInstallSession == nil, let value = iterator.next(),
                      value.range(of: #"^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$"#,
                          options: .regularExpression) == (value.startIndex..<value.endIndex)
                else { throw Usage.invalid }
                nativeUpdateInstallSession = value
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
        guard ["preferences", "history", "hud", "preview", "sharing", "region", "window", "update", "idle"].contains(scene),
            (updateState == nil && updateTray == nil) || scene == "update",
            !(["update", "sharing"].contains(scene) && (live || exercise)),
            ["light", "dark", "system"].contains(appearance), Self.themes.contains(theme),
            !(live && (exercise || referenceChips)), historyRoot == nil || live,
            openMedia.isEmpty || live
        else { throw Usage.invalid }
        let hasHealth = nativeUpdateReadyFile != nil || nativeUpdateReadyToken != nil
        if hasHealth {
            let tokenPattern = #"^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$"#
            guard explicitLive, let file = nativeUpdateReadyFile, file.hasPrefix("/"),
                  let token = nativeUpdateReadyToken,
                  token.range(of: tokenPattern, options: .regularExpression) == (token.startIndex..<token.endIndex),
                  let historyRoot, historyRoot.hasPrefix("/"),
                  let settingsFile, settingsFile.hasPrefix("/"),
                  !exercise, !referenceChips, openMedia.isEmpty,
                  updateState == nil, updateTray == nil, scene == "preferences", screenshot == nil
            else { throw Usage.invalid }
        }
        if updateEndpoint != nil || updateKeyFile != nil || updateCurrentVersion != nil || updateStagingDirectory != nil || updateBaseArchive != nil || updateShutdownIntentFile != nil || nativeUpdateInstallSession != nil {
            guard explicitLive, let endpoint = updateEndpoint, let keyFile = updateKeyFile,
                  let version = updateCurrentVersion, !hasHealth, scene == "preferences",
                  let historyRoot, historyRoot.hasPrefix("/"),
                  let settingsFile, settingsFile.hasPrefix("/"), openMedia.isEmpty
            else { throw Usage.invalid }
            var configuration: [String: Any] = [
                "endpoint": endpoint, "key_file": keyFile, "renderer": "appkit", "current_version": version,
            ]
            if let updateStagingDirectory { configuration["staging_directory"] = updateStagingDirectory }
            if let updateBaseArchive {
                guard updateStagingDirectory != nil else { throw Usage.invalid }
                configuration["base_archive"] = updateBaseArchive
            }
            if let updateShutdownIntentFile {
                guard updateStagingDirectory != nil else { throw Usage.invalid }
                let intent: [String: Any] = ["path": updateShutdownIntentFile,
                    "history_root": historyRoot, "settings_file": settingsFile]
                try updateShutdownIntent(intent)
                nativeUpdateShutdownIntent = intent
            }
            if nativeUpdateInstallSession != nil {
                guard nativeUpdateShutdownIntent != nil else { throw Usage.invalid }
                configuration["install_request"] = true
            }
            nativeUpdateChecks = try UpdateCheckModel(transport: NativeUpdateCheckTransport(configuration: configuration))
        }
    }
    /// Only the elected primary calls this, before window/renderer startup.
    mutating func takeUpdateRestartIntent(transport: SettingsTransport = SettingsBridge()) throws {
        guard let file = nativeUpdateReadyFile else { return }
        let reply = try transport.request(["operation": "update_restart_take", "ready_file": file])
        if let visible = reply["restore_preferences"] as? Bool {
            scene = "idle"
            nativeUpdateRestorePreferences = visible
        }
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
            // Hold across bundled/unbundled loops, secondaries and fixtures,
            // independently of profile election and asynchronous termination.
            guard let packageUse = captures_package_use_current_v1() else {
                throw NSError(domain: "CapturesNativePackage", code: 1, userInfo: [
                    NSLocalizedDescriptionKey: "The native development package is busy or its use guard could not be opened.",
                ])
            }
            defer { captures_package_use_free_v1(packageUse) }
            let bundled = Bundle.main.object(forInfoDictionaryKey: "CapturesNativeLive") as? Bool == true
            var options = try Options(Array(CommandLine.arguments.dropFirst()), bundled: bundled)
            if bundled {
                runBundle(options)
                return
            }
            var nativeInstance: NativeInstance?
            var crashDiagnostics: CrashDiagnostics?
            if options.live {
                let result = try NativeInstance.start(historyRoot: options.historyRoot, paths: options.openMedia)
                guard result.primary else { return }
                nativeInstance = result.owner
                do { crashDiagnostics = try CrashDiagnostics.start(historyRoot: options.historyRoot) }
                catch { reportDiagnosticsFailure(error) }
            }
            defer { nativeInstance?.close() }
            try options.takeUpdateRestartIntent()
            let application = NSApplication.shared
            application.setActivationPolicy(options.scene == "idle" ? .accessory : .regular)
            let delegate = Workbench(options: options, nativeInstance: nativeInstance,
                                     crashDiagnostics: crashDiagnostics)
            application.delegate = delegate
            withExtendedLifetime(delegate) { application.run() }
        } catch Options.Usage.invalid {
            FileHandle.standardError.write(Data("Development metadata checks: explicit --live --history-root ABSOLUTE_PATH --settings-file ABSOLUTE_PATH --native-update-manifest-url URL --native-update-public-key-file PATH --native-update-current-version VERSION (check only; no health launch or media open).\n".utf8))
            FileHandle.standardError.write(Data("Optional --native-update-staging-directory ABSOLUTE_PATH enables explicit temporary download/verification in an existing scratch directory, never installation.\n".utf8))
            FileHandle.standardError.write(Data("Optional --native-update-base-archive ABSOLUTE_PATH enables authenticated delta acquisition with full fallback; requires staging.\n".utf8))
            FileHandle.standardError.write(Data("Usage: CapturesNative [--live [--history-root PATH] [--settings-file PATH] [--native-update-ready-file ABSOLUTE_PATH --native-update-ready-token UUID] [--open-media PATH|--open-image PATH]...] [--scene preferences|history|hud|preview|sharing|region|window|update|idle] [--update-state available|single|closing|manual|downloading|restarting|error|checking|up-to-date] [--update-tray top|bottom|none] [--appearance light|dark|system] [--theme mustard|ember|rose|violet|cobalt|aqua|mint|lime|mono] [--history-count 0..10000] [--screenshot PATH] [--reference-chips] [--exercise] [--quit-after SECONDS] [-- FILE...]\n".utf8))
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
        var crashDiagnostics: CrashDiagnostics?
        defer { nativeInstance?.close() }
        // LaunchServices delivers cold-open Apple events before didFinishLaunching.
        // Collect those before election so a secondary does not exit and lose them.
        let launch = BundledLaunch(options: options) { options, notification in
            let result = try NativeInstance.start(historyRoot: options.historyRoot, paths: options.openMedia)
            guard result.primary else { return false }
            nativeInstance = result.owner
            var primaryOptions = options
            try primaryOptions.takeUpdateRestartIntent()
            do { crashDiagnostics = try CrashDiagnostics.start(historyRoot: options.historyRoot) }
            catch { reportDiagnosticsFailure(error) }
            let delegate = Workbench(options: primaryOptions, nativeInstance: nativeInstance,
                                     crashDiagnostics: crashDiagnostics)
            workbench = delegate
            application.delegate = delegate
            delegate.applicationDidFinishLaunching(notification)
            return true
        }
        application.delegate = launch
        withExtendedLifetime(launch) { application.run() }
        withExtendedLifetime(workbench) {}
    }

    private static func reportDiagnosticsFailure(_ error: Error) {
        // Backend errors are intentionally not printed: they can contain profile paths.
        FileHandle.standardError.write(Data("Captures diagnostics could not start; capture remains available.\n".utf8))
    }
}
