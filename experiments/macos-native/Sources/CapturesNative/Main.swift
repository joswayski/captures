import AppKit
import SwiftUI

@main
enum CapturesNative {
    static func main() {
        // Finder does not inherit a developer shell's PATH. Prepared sidecars
        // take precedence; otherwise allow the standard Homebrew locations.
        let environment = ProcessInfo.processInfo.environment
        setenv("PATH", (environment["PATH"] ?? "/usr/bin:/bin") + ":/opt/homebrew/bin:/usr/local/bin", 1)
        for tool in ["ffmpeg", "ffprobe"] {
            let key = "CAPTURES_NATIVE_" + tool.uppercased()
            if environment[key] == nil,
               let url = Bundle.main.resourceURL?.appendingPathComponent("media/\(tool)"),
               FileManager.default.isExecutableFile(atPath: url.path) { setenv(key, url.path, 1) }
        }
        let application = NSApplication.shared
        let delegate = AppDelegate()
        application.delegate = delegate
        application.setActivationPolicy(.regular)
        withExtendedLifetime(delegate) { application.run() }
    }
}

@MainActor
final class AppDelegate: NSObject, NSApplicationDelegate, ApplicationRestarting {
    private var statusItem: NSStatusItem?
    private var deferredLaunch: DispatchWorkItem?
    private var restartBundleURL: URL?
    private var restartWaiter: Process?
    private var crashDiagnosticsStarted = false
    private var terminationInProgress = false
    private let routeNotification = Notification.Name("es.captur.native-experiment.route")

    func applicationDidFinishLaunching(_ notification: Notification) {
        let arguments = Array(CommandLine.arguments.dropFirst())
        if let bundleID = Bundle.main.bundleIdentifier,
           let other = NSRunningApplication.runningApplications(withBundleIdentifier: bundleID)
            .first(where: { $0.processIdentifier != ProcessInfo.processInfo.processIdentifier }) {
            DistributedNotificationCenter.default().postNotificationName(routeNotification, object: bundleID,
                                                                         userInfo: ["arguments": arguments], deliverImmediately: true)
            other.activate(options: .activateIgnoringOtherApps)
            // No engine/settings have been opened in the duplicate process.
            exit(0)
        }
        DistributedNotificationCenter.default().addObserver(self, selector: #selector(receiveRoute(_:)),
                                                            name: routeNotification, object: Bundle.main.bundleIdentifier)
        Backend.shared.call("crash_start", ["profile_root": AppStore.dataDirectory.path]) { result in
            self.crashDiagnosticsStarted = (try? result.get()) != nil
            if case .failure(let error) = result { AppStore.shared.report(error) }
            self.finishStartup(arguments: arguments)
        }
    }

    private func finishStartup(arguments: [String]) {
        AppStore.shared.start()
        installMenus()
        if arguments.isEmpty {
            // Finder can deliver Open With URLs just after launch. Avoid
            // flashing setup or Preferences before the editor opens.
            let work = DispatchWorkItem { [weak self] in self?.route([]) }
            deferredLaunch = work
            DispatchQueue.main.asyncAfter(deadline: .now() + 0.5, execute: work)
        } else {
            route(arguments)
        }
        Backend.shared.call("recover_list") { result in
            if case .success(let value) = result,
               let drafts = value["drafts"] as? [Any], !drafts.isEmpty { AppStore.shared.showHistory() }
        }
        guard crashDiagnosticsStarted,
              let executableURL = Bundle.main.executableURL else { return }
        Backend.shared.call("crash_preview", [
            "reports": ownCrashReportCandidates(executableName: executableURL.lastPathComponent),
            "executable_name": executableURL.lastPathComponent,
            "bundle_id": (Bundle.main.bundleIdentifier as Any?) ?? NSNull(),
            "executable_path": executableURL.path,
        ]) { result in
            if case let .success(preview) = result,
               preview["unclean_exit"] as? Bool == true {
                AppStore.shared.showCrashDiagnostics(preview)
            }
        }
    }

    func applicationShouldTerminateAfterLastWindowClosed(_ sender: NSApplication) -> Bool { false }
    func applicationShouldHandleReopen(_ sender: NSApplication, hasVisibleWindows flag: Bool) -> Bool {
        if !flag {
            if AppStore.shared.onboardingCompleted { AppStore.shared.showPreferences() }
            else { AppStore.shared.showOnboarding() }
        }
        return true
    }
    func application(_ application: NSApplication, open urls: [URL]) {
        deferredLaunch?.cancel()
        urls.forEach { AppStore.shared.importFile($0) }
    }

    func applicationShouldTerminate(_ sender: NSApplication) -> NSApplication.TerminateReply {
        guard !terminationInProgress else { return .terminateLater }
        terminationInProgress = true
        CaptureController.shared.cancel()
        Backend.shared.call("record_status") { result in
            guard case .success(let status) = result else {
                if case .failure(let error) = result { AppStore.shared.report(error) }
                self.finishTermination(sender, allowed: false); return
            }
            let state = status["state"] as? String ?? "idle"
            guard ["recording", "paused", "failed", "selecting"].contains(state) else {
                self.finishTermination(sender, allowed: true); return
            }
            let discardAndQuit = {
                Backend.shared.call("record_discard") { result in
                    if case .success = result { self.finishTermination(sender, allowed: true) }
                    else {
                        if case .failure(let error) = result { AppStore.shared.report(error) }
                        self.finishTermination(sender, allowed: false)
                    }
                }
            }
            if state == "selecting" {
                CaptureDialogController.shared.present(
                    title: "Discard the restarting recording?",
                    message: "The previous take is already gone. Quit will discard the empty restart draft.",
                    action: "Discard and Quit",
                    destructive: true,
                    onConfirm: discardAndQuit,
                    onCancel: { self.finishTermination(sender, allowed: false) }
                )
                return
            }
            CaptureDialogController.shared.present(
                title: "Finish the recording before quitting?",
                message: "Save the recording, cancel quitting, or discard the current take from the recording controls.",
                action: "Save and Quit",
                alternate: "Discard and Quit",
                onConfirm: {
                    Backend.shared.call("record_stop") { result in
                        do {
                            AppStore.shared.addArtifact(try Artifact(response: result.get()))
                            self.finishTermination(sender, allowed: true)
                        } catch {
                            AppStore.shared.report(error)
                            self.finishTermination(sender, allowed: false)
                        }
                    }
                },
                onAlternate: discardAndQuit,
                onCancel: { self.finishTermination(sender, allowed: false) }
            )
        }
        return .terminateLater
    }

    func requestRestart() {
        restartBundleURL = Bundle.main.bundleURL
        NSApp.terminate(nil)
    }

    private func finishTermination(_ sender: NSApplication, allowed: Bool) {
        guard allowed else {
            cancelTermination(sender)
            return
        }
        do {
            try prepareRestartWaiter()
        } catch {
            AppStore.shared.report(error)
            cancelTermination(sender)
            return
        }
        guard crashDiagnosticsStarted else {
            completeTermination(sender)
            return
        }
        Backend.shared.call("crash_mark_clean") { result in
            switch result {
            case .success:
                self.crashDiagnosticsStarted = false
                self.completeTermination(sender)
            case .failure(let error):
                AppStore.shared.report(error)
                // mark_clean may have removed the current marker before a
                // later filesystem operation failed. Restore this same live
                // session before cancelling; crash_start would incorrectly
                // rotate it into prior-session evidence and reinstall hooks.
                Backend.shared.call("crash_resume") { resumeResult in
                    if case .failure(let resumeError) = resumeResult {
                        AppStore.shared.report(resumeError)
                    }
                    self.cancelTermination(sender)
                }
            }
        }
    }

    private func completeTermination(_ sender: NSApplication) {
        restartBundleURL = nil
        restartWaiter = nil
        sender.reply(toApplicationShouldTerminate: true)
    }

    private func prepareRestartWaiter() throws {
        guard let bundleURL = restartBundleURL, restartWaiter == nil else { return }
        let waiter = Process()
        waiter.executableURL = URL(fileURLWithPath: "/bin/sh")
        waiter.arguments = [
            "-c",
            "while kill -0 \"$1\" 2>/dev/null; do sleep 0.05; done; exec /usr/bin/open -n \"$2\"",
            "captures-restart", String(ProcessInfo.processInfo.processIdentifier), bundleURL.path,
        ]
        try waiter.run()
        restartWaiter = waiter
    }

    private func cancelTermination(_ sender: NSApplication) {
        if restartWaiter?.isRunning == true { restartWaiter?.terminate() }
        restartWaiter = nil
        restartBundleURL = nil
        terminationInProgress = false
        sender.reply(toApplicationShouldTerminate: false)
    }

    private func ownCrashReportCandidates(executableName: String) -> [[String: Any]] {
        let directory = FileManager.default.homeDirectoryForCurrentUser
            .appendingPathComponent("Library/Logs/DiagnosticReports", isDirectory: true)
        let keys: Set<URLResourceKey> = [.contentModificationDateKey, .isRegularFileKey, .isSymbolicLinkKey]
        let urls = (try? FileManager.default.contentsOfDirectory(
            at: directory, includingPropertiesForKeys: Array(keys),
            options: [.skipsHiddenFiles, .skipsSubdirectoryDescendants]
        )) ?? []
        return urls.compactMap { url -> (URL, Date)? in
            guard ["ips", "crash"].contains(url.pathExtension.lowercased()),
                  url.deletingPathExtension().lastPathComponent.hasPrefix("\(executableName)-"),
                  let values = try? url.resourceValues(forKeys: keys),
                  values.isRegularFile == true, values.isSymbolicLink != true,
                  let modified = values.contentModificationDate else { return nil }
            return (url, modified)
        }
        .sorted { $0.1 > $1.1 }
        .prefix(8)
        .map { url, modified in
            return ["path": url.path, "modified_ms": Int64(modified.timeIntervalSince1970 * 1_000)]
        }
    }

    @objc private func receiveRoute(_ notification: Notification) {
        guard let arguments = notification.userInfo?["arguments"] as? [String] else { return }
        route(arguments)
    }

    private func route(_ arguments: [String]) {
        switch arguments.first {
        case "--capture": CaptureController.shared.show()
        case "--record": CaptureController.shared.show(kind: "video", target: "region")
        case "--gif": CaptureController.shared.show(kind: "gif", target: "region")
        case "--history": AppStore.shared.showHistory()
        case "--open": arguments.dropFirst().forEach { AppStore.shared.importFile(URL(fileURLWithPath: $0)) }
        case "--previews":
            AppStore.shared.previews = arguments.dropFirst().map {
                let url = URL(fileURLWithPath: $0)
                return Artifact(path: url.path, kind: url.pathExtension.lowercased() == "gif" ? "gif" :
                                    ["mp4", "webm", "mov"].contains(url.pathExtension.lowercased()) ? "video" : "image")
            }
            PreviewController.shared.refresh()
        default:
            if AppStore.shared.onboardingCompleted { AppStore.shared.showPreferences() }
            else { AppStore.shared.showOnboarding() }
        }
    }

    private func installMenus() {
        let main = NSMenu()
        let appItem = NSMenuItem()
        let appMenu = NSMenu(title: "Captures")
        appMenu.addItem(item("Preferences…", #selector(preferences), key: ","))
        appMenu.addItem(item("Send Feedback…", #selector(feedback)))
        appMenu.addItem(.separator())
        appMenu.addItem(item("Quit Captures Native", #selector(quit), key: "q"))
        appItem.submenu = appMenu; main.addItem(appItem)
        let fileItem = NSMenuItem(); let file = NSMenu(title: "File")
        file.addItem(item("New Capture", #selector(capture)))
        file.addItem(item("Open…", #selector(openFile), key: "o"))
        file.addItem(item("Capture History", #selector(history)))
        fileItem.submenu = file; main.addItem(fileItem)
        let editItem = NSMenuItem(); let edit = NSMenu(title: "Edit")
        for (title, action, key) in [("Undo", "undo:", "z"), ("Cut", "cut:", "x"),
                                     ("Copy", "copy:", "c"), ("Paste", "paste:", "v"), ("Select All", "selectAll:", "a")] {
            edit.addItem(NSMenuItem(title: title, action: Selector(action), keyEquivalent: key))
        }
        let redo = NSMenuItem(title: "Redo", action: Selector(("redo:")), keyEquivalent: "z")
        redo.keyEquivalentModifierMask = [.command, .shift]; edit.insertItem(redo, at: 1)
        editItem.submenu = edit; main.addItem(editItem)
        NSApp.mainMenu = main

        statusItem = NSStatusBar.system.statusItem(withLength: NSStatusItem.squareLength)
        statusItem?.button?.image = NSImage(systemSymbolName: "viewfinder", accessibilityDescription: "Captures Native")
        let tray = NSMenu()
        tray.addItem(item("New Capture", #selector(capture)))
        tray.addItem(item("Record", #selector(record)))
        tray.addItem(item("Record GIF", #selector(gif)))
        tray.addItem(item("Restore Recording Controls", #selector(restoreRecording)))
        tray.addItem(.separator())
        tray.addItem(item("Preferences…", #selector(preferences)))
        tray.addItem(item("Capture History…", #selector(history)))
        tray.addItem(item("Open File…", #selector(openFile)))
        tray.addItem(item("Send Feedback…", #selector(feedback)))
        tray.addItem(.separator())
        tray.addItem(item("Quit Captures Native", #selector(quit)))
        statusItem?.menu = tray
    }

    private func item(_ title: String, _ action: Selector, key: String = "") -> NSMenuItem {
        let item = NSMenuItem(title: title, action: action, keyEquivalent: key); item.target = self; return item
    }
    @objc private func preferences() { AppStore.shared.showPreferences() }
    @objc private func history() { AppStore.shared.showHistory() }
    @objc private func openFile() { AppStore.shared.chooseFile() }
    @objc private func feedback() { AppStore.shared.showFeedback() }
    @objc private func capture() { CaptureController.shared.show() }
    @objc private func record() { CaptureController.shared.show(kind: "video", target: "region") }
    @objc private func gif() { CaptureController.shared.show(kind: "gif", target: "region") }
    @objc private func restoreRecording() { RecordingHUDController.shared.show() }
    @objc private func quit() { NSApp.terminate(nil) }
}
