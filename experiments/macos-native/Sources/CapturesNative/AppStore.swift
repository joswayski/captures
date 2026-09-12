import AppKit
import Combine
import SwiftUI
import UniformTypeIdentifiers

final class AppStore: NSObject, ObservableObject, NSWindowDelegate {
    static let shared = AppStore()
    static var dataDirectory: URL { NativeStorage.directory }
    @Published var settings = NativeSettings()
    @Published var artifacts: [Artifact] = []
    @Published var previews: [Artifact] = []
    @Published var saveStatus = "Changes save automatically"
    @Published var shortcutWarning = ""
    private var subscriptions = Set<AnyCancellable>()
    private var windows: [String: NSWindow] = [:]
    private var loadingError: Error?

    private override init() {
        super.init()
        do {
            let settingsURL = Self.dataDirectory.appendingPathComponent("settings.json")
            let loaded = try NativeStorage.read(NativeSettings.self, from: settingsURL) ?? NativeSettings()
            settings = loaded
            settings.migrateShortcuts()
            if settings != loaded { try NativeStorage.write(settings, to: settingsURL) }
            artifacts = NativeStorage.recent(try NativeStorage.read([Artifact].self, from: Self.dataDirectory.appendingPathComponent("history.json")) ?? [])
        } catch { loadingError = error }
        $settings.dropFirst().removeDuplicates().sink { [weak self] _ in
            self?.saveStatus = "Saving…"
        }.store(in: &subscriptions)
        $settings.dropFirst().removeDuplicates().debounce(for: .milliseconds(250), scheduler: RunLoop.main)
            .sink { [weak self] settings in
                guard let self else { return }
                do {
                    try NativeStorage.write(settings, to: Self.dataDirectory.appendingPathComponent("settings.json"))
                    self.saveStatus = "All changes saved"
                    self.applyAppearance()
                    ShortcutManager.shared.register(settings.shortcuts)
                    PreviewController.shared.refresh()
                } catch { self.saveStatus = "Could not save: \(error.localizedDescription)" }
            }.store(in: &subscriptions)
    }

    func start() {
        applyAppearance()
        ShortcutManager.shared.register(settings.shortcuts)
        if let loadingError { report(loadingError) }
    }

    func applyAppearance() {
        NSApp.appearance = settings.appearance == "system" ? nil :
            NSAppearance(named: settings.appearance == "dark" ? .darkAqua : .aqua)
    }

    func report(_ error: Error) {
        let message = error.localizedDescription
        DispatchQueue.main.async {
            CaptureDialogController.shared.report(NativeFailure(message))
        }
    }

    func addArtifact(_ artifact: Artifact) {
        artifacts.removeAll { $0.path == artifact.path }
        artifacts.insert(artifact, at: 0)
        artifacts = NativeStorage.recent(artifacts)
        persistHistory()
        if settings.autoCopy, artifact.kind == "image", let image = NSImage(contentsOf: artifact.url) {
            NSPasteboard.general.clearContents()
            NSPasteboard.general.writeObjects([image])
        }
        if settings.showPreviews {
            previews.removeAll { $0.path == artifact.path }
            previews.insert(artifact, at: 0)
            PreviewController.shared.refresh()
        }
    }

    func dismissPreview(_ artifact: Artifact) {
        previews.removeAll { $0.id == artifact.id }
        PreviewController.shared.refresh()
    }

    func removeFromHistory(_ artifact: Artifact) {
        artifacts.removeAll { $0.id == artifact.id }
        persistHistory() // The source file and preview remain intact.
    }

    func clearHistory() { artifacts.removeAll(); persistHistory() }

    func deleteArtifact(_ artifact: Artifact) {
        // Only called after the preview's explicit destructive confirmation.
        // Finder Trash remains recoverable, unlike unlinking a user's source.
        do {
            try FileManager.default.trashItem(at: artifact.url, resultingItemURL: nil)
            removeFromHistory(artifact)
            dismissPreview(artifact)
        } catch { report(error) }
    }

    private func persistHistory() {
        do { try NativeStorage.write(artifacts, to: Self.dataDirectory.appendingPathComponent("history.json")) }
        catch { report(error) }
    }

    func showPreferences() { show("preferences", title: "Preferences", size: NSSize(width: 980, height: 720), view: PreferencesView()) }
    func showHistory() { show("history", title: "Capture History", size: NSSize(width: 980, height: 720), view: HistoryView()) }

    func open(_ artifact: Artifact) {
        guard FileManager.default.fileExists(atPath: artifact.path) else {
            report(NativeFailure("This file has moved or was deleted: \(artifact.url.lastPathComponent)")); return
        }
        if artifact.kind == "image" {
            show(artifact.path, title: "Screenshot Editor", size: NSSize(width: 1280, height: 760), view: ImageEditorView(artifact: artifact))
        } else {
            show(artifact.path, title: "Recording Editor", size: NSSize(width: 1100, height: 800), view: RecordingEditorView(artifact: artifact))
        }
    }

    func importFile(_ url: URL) {
        let ext = url.pathExtension.lowercased()
        if ["png", "jpg", "jpeg", "webp"].contains(ext), let image = NSImage(contentsOf: url) {
            let representation = image.representations.first
            open(Artifact(path: url.path, kind: "image", width: representation?.pixelsWide ?? 0,
                          height: representation?.pixelsHigh ?? 0))
        } else if ["mp4", "mov", "webm", "gif"].contains(ext) {
            open(Artifact(path: url.path, kind: ext == "gif" ? "gif" : "video"))
        } else { report(NativeFailure("Open a PNG, JPEG, WebP, GIF, MP4, MOV, or WebM file.")) }
    }

    func chooseFile() {
        let panel = NSOpenPanel()
        panel.canChooseDirectories = false
        panel.allowsMultipleSelection = true
        panel.allowedContentTypes = [UTType.png, .jpeg, .gif, .movie] + [UTType(filenameExtension: "webp")].compactMap { $0 }
        if panel.runModal() == .OK { panel.urls.forEach(importFile) }
    }

    private func show<V: View>(_ key: String, title: String, size: NSSize, view: V) {
        if let window = windows[key] {
            window.makeKeyAndOrderFront(nil); NSApp.activate(ignoringOtherApps: true); return
        }
        let window = NSWindow(contentRect: NSRect(origin: .zero, size: size),
                              styleMask: [.titled, .closable, .miniaturizable, .resizable],
                              backing: .buffered, defer: false)
        window.title = title
        window.identifier = NSUserInterfaceItemIdentifier(key)
        window.isReleasedWhenClosed = false
        window.delegate = self
        window.minSize = NSSize(width: 760, height: 560)
        window.contentView = NSHostingView(rootView: NativeRoot(content: view))
        windows[key] = window
        window.center()
        window.makeKeyAndOrderFront(nil)
        NSApp.activate(ignoringOtherApps: true)
    }

    func windowWillClose(_ notification: Notification) {
        guard let window = notification.object as? NSWindow, let key = window.identifier?.rawValue else { return }
        windows.removeValue(forKey: key)
        // Destroy closed editors/players instead of retaining every full-size
        // image and undo history for the lifetime of the menu-bar process.
        DispatchQueue.main.async { window.contentView = nil }
    }
}

private struct NativeRoot<Content: View>: View {
    @ObservedObject private var store = AppStore.shared
    let content: Content
    var body: some View {
        content
            .font(.system(size: NativeTheme.metric("text-md")))
            .tint(NativeTheme.accent)
            .preferredColorScheme(store.settings.appearance == "system" ? nil : store.settings.appearance == "dark" ? .dark : .light)
            .frame(maxWidth: .infinity, maxHeight: .infinity)
    }
}
