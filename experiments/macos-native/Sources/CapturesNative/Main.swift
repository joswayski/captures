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

final class AppDelegate: NSObject, NSApplicationDelegate {
    private var statusItem: NSStatusItem?
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
        AppStore.shared.start()
        installMenus()
        route(arguments)
        Backend.shared.call("recover_list") { result in
            if case .success(let value) = result,
               let drafts = value["drafts"] as? [Any], !drafts.isEmpty { AppStore.shared.showHistory() }
        }
    }

    func applicationShouldTerminateAfterLastWindowClosed(_ sender: NSApplication) -> Bool { false }
    func applicationShouldHandleReopen(_ sender: NSApplication, hasVisibleWindows flag: Bool) -> Bool {
        AppStore.shared.showPreferences(); return true
    }
    func application(_ application: NSApplication, open urls: [URL]) { urls.forEach { AppStore.shared.importFile($0) } }

    func applicationShouldTerminate(_ sender: NSApplication) -> NSApplication.TerminateReply {
        CaptureController.shared.cancel()
        Backend.shared.call("record_status") { result in
            guard case .success(let status) = result else {
                if case .failure(let error) = result { AppStore.shared.report(error) }
                sender.reply(toApplicationShouldTerminate: false); return
            }
            let state = status["state"] as? String ?? "idle"
            guard ["recording", "paused", "failed", "selecting"].contains(state) else {
                sender.reply(toApplicationShouldTerminate: true); return
            }
            let discardAndQuit = {
                Backend.shared.call("record_discard") { result in
                    if case .success = result { sender.reply(toApplicationShouldTerminate: true) }
                    else {
                        if case .failure(let error) = result { AppStore.shared.report(error) }
                        sender.reply(toApplicationShouldTerminate: false)
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
                    onCancel: { sender.reply(toApplicationShouldTerminate: false) }
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
                            sender.reply(toApplicationShouldTerminate: true)
                        } catch {
                            AppStore.shared.report(error)
                            sender.reply(toApplicationShouldTerminate: false)
                        }
                    }
                },
                onAlternate: discardAndQuit,
                onCancel: { sender.reply(toApplicationShouldTerminate: false) }
            )
        }
        return .terminateLater
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
        default: AppStore.shared.showPreferences()
        }
    }

    private func installMenus() {
        let main = NSMenu()
        let appItem = NSMenuItem()
        let appMenu = NSMenu(title: "Captures")
        appMenu.addItem(item("Preferences…", #selector(preferences), key: ","))
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
    @objc private func capture() { CaptureController.shared.show() }
    @objc private func record() { CaptureController.shared.show(kind: "video", target: "region") }
    @objc private func gif() { CaptureController.shared.show(kind: "gif", target: "region") }
    @objc private func restoreRecording() { RecordingHUDController.shared.show() }
    @objc private func quit() { NSApp.terminate(nil) }
}
