import AppKit

/// The shipping document windows. Mirrors `captures_app::app_windows`: History,
/// Preferences and first-run setup are separate, resizable top-level windows.
enum AppWindowKind: CaseIterable {
    case history, preferences, setup

    var title: String {
        switch self {
        case .history: return "Capture History"
        case .preferences: return "Captures Preferences"
        case .setup: return "Captures"
        }
    }

    var size: NSSize {
        switch self {
        case .history: return NSSize(width: 1020, height: 720)
        case .preferences: return NSSize(width: 880, height: 660)
        case .setup: return NSSize(width: 620, height: 560)
        }
    }

    var minimumSize: NSSize {
        switch self {
        case .history: return NSSize(width: 640, height: 440)
        case .preferences: return NSSize(width: 560, height: 440)
        case .setup: return NSSize(width: 480, height: 440)
        }
    }

    /// Shipping `primary_app_window_priority`: lower focuses first on reopen.
    /// Editors (priority 1) sit between setup and History.
    var reactivationPriority: Int {
        switch self {
        case .setup: return 0
        case .history: return 2
        case .preferences: return 3
        }
    }

    static let styleMask: NSWindow.StyleMask = [.titled, .closable, .miniaturizable, .resizable]
}

/// Shipping editor window titles.
enum EditorWindowTitle {
    static let screenshot = "Captures Screenshot Editor"
    static let recording = "Captures Editor"
}

/// Shipping `@media (max-width: 720px)` and `(max-height: 600px)` breakpoints
/// (`app_windows::COMPACT_MAX_WIDTH` / `SHORT_MAX_HEIGHT`).
enum AppWindowLayout {
    static let compactMaximumWidth: CGFloat = 720
    static let shortMaximumHeight: CGFloat = 600
    static func compact(width: CGFloat) -> Bool { width <= compactMaximumWidth }
    static func short(height: CGFloat) -> Bool { height <= shortMaximumHeight }
}

enum AppReactivation: Equatable {
    case showSetup
    case restoreRecordingControls
    case focus(AppWindowKind)
    case showPreferences
}

/// Shipping `app_reactivation`: setup first, then hidden recording controls,
/// then the highest-priority visible window, else Preferences.
func appReactivation(onboardingComplete: Bool, restoreRecordingControls: Bool,
                     visible: [AppWindowKind]) -> AppReactivation {
    guard onboardingComplete else { return .showSetup }
    if restoreRecordingControls { return .restoreRecordingControls }
    guard let first = visible.min(by: { $0.reactivationPriority < $1.reactivationPriority }) else {
        return .showPreferences
    }
    return .focus(first)
}

/// Creates, shows, focuses and forgets the separate document windows. The
/// History window is created by the host and adopted with its own close
/// handler, so it hides into the menu bar instead of closing.
final class AppWindows: NSObject, NSWindowDelegate {
    private var openWindows: [AppWindowKind: NSWindow] = [:]
    private var adopted: Set<AppWindowKind> = []
    /// Return false to keep a window the user asked to close.
    var shouldClose: ((AppWindowKind) -> Bool)?
    /// A window closed; it is forgotten and the next `show` creates it anew.
    var didClose: ((AppWindowKind) -> Void)?

    func window(_ kind: AppWindowKind) -> NSWindow? { openWindows[kind] }

    /// The document window `window` is, or is a sheet of.
    func kind(of window: NSWindow) -> AppWindowKind? {
        openWindows.first { $0.value === window || window.sheetParent === $0.value }?.key
    }

    /// The kinds whose windows are on screen.
    var visibleKinds: [AppWindowKind] {
        AppWindowKind.allCases.filter { openWindows[$0]?.isVisible == true }
    }

    static func makeWindow(_ kind: AppWindowKind) -> NSWindow {
        let made = NSWindow(contentRect: NSRect(origin: .zero, size: kind.size),
                            styleMask: AppWindowKind.styleMask, backing: .buffered, defer: false)
        made.title = kind.title
        made.contentMinSize = kind.minimumSize
        made.isReleasedWhenClosed = false
        made.center()
        return made
    }

    /// Track a window the host created and whose delegate it owns.
    func adopt(_ adoptedWindow: NSWindow, as kind: AppWindowKind) {
        openWindows[kind] = adoptedWindow
        adopted.insert(kind)
    }

    /// Create the window without showing it; `build` fills it once.
    @discardableResult func prepare(_ kind: AppWindowKind, build: (NSWindow) -> Void) -> NSWindow {
        if let existing = openWindows[kind] { return existing }
        let made = Self.makeWindow(kind)
        made.delegate = self
        openWindows[kind] = made
        build(made)
        return made
    }

    /// Shipping `show_*`: create the window, or show, restore and focus the
    /// one that is already open.
    @discardableResult func show(_ kind: AppWindowKind, activate: Bool = true,
                                 build: (NSWindow) -> Void = { _ in }) -> NSWindow {
        let shown = prepare(kind, build: build)
        if shown.isMiniaturized { shown.deminiaturize(nil) }
        shown.makeKeyAndOrderFront(nil)
        if activate { NSApp.activate(ignoringOtherApps: true) }
        return shown
    }

    /// Close a window programmatically (no `shouldClose` check).
    func close(_ kind: AppWindowKind) {
        openWindows[kind]?.close()
    }

    func windowShouldClose(_ sender: NSWindow) -> Bool {
        guard let kind = openWindows.first(where: { $0.value === sender })?.key else { return true }
        return shouldClose?(kind) ?? true
    }

    func windowWillClose(_ notification: Notification) {
        guard let closing = notification.object as? NSWindow,
              let kind = openWindows.first(where: { $0.value === closing })?.key,
              !adopted.contains(kind) else { return }
        openWindows[kind] = nil
        didClose?(kind)
    }
}
