import AppKit
import SwiftUI

/// Actual AppKit/SwiftUI content renders, not simulated browser "after" images.
/// Runs in its own process/profile, never invokes capture or registers shortcuts.
@main
@MainActor
enum RenderReferences {
    static func main() {
        guard CommandLine.arguments.count == 2 else { exit(2) }
        let output = URL(fileURLWithPath: CommandLine.arguments[1], isDirectory: true)
        do { try FileManager.default.createDirectory(at: output, withIntermediateDirectories: true) }
        catch { fputs("Reference output: \(error)\n", stderr); exit(1) }
        let application = NSApplication.shared
        application.setActivationPolicy(.regular)
        application.finishLaunching()
        let window = NSWindow(contentRect: NSRect(x: 0, y: 0, width: 980, height: 720),
                              styleMask: [.titled, .closable], backing: .buffered, defer: false)
        window.isReleasedWhenClosed = false
        window.title = "Native reference render"
        window.center()

        func render(_ index: Int) {
            guard index < 2 else { window.orderOut(nil); exit(0) }
            let scheme: ColorScheme = index == 0 ? .light : .dark
            let name = index == 0 ? "preferences-light" : "preferences-dark"
            window.appearance = NSAppearance(named: index == 0 ? .aqua : .darkAqua)
            let view = NSHostingView(rootView: PreferencesView()
                .font(.system(size: NativeTheme.metric("text-md")))
                .tint(NativeTheme.accent)
                .environment(\.colorScheme, scheme)
                .frame(width: 980, height: 720))
            window.contentView = view
            window.makeKeyAndOrderFront(nil)
            application.activate(ignoringOtherApps: true)
            DispatchQueue.main.asyncAfter(deadline: .now() + 2) {
                view.layoutSubtreeIfNeeded()
                view.displayIfNeeded()
                guard let bitmap = view.bitmapImageRepForCachingDisplay(in: view.bounds) else {
                    fputs("AppKit could not allocate a reference bitmap.\n", stderr); exit(1)
                }
                view.cacheDisplay(in: view.bounds, to: bitmap)
                guard let data = bitmap.representation(using: .png, properties: [:]) else { exit(1) }
                do { try data.write(to: output.appendingPathComponent("\(name).png"), options: .withoutOverwriting) }
                catch { fputs("Reference write: \(error)\n", stderr); exit(1) }
                print("Rendered native \(name): \(bitmap.pixelsWide) × \(bitmap.pixelsHigh)")
                render(index + 1)
            }
        }
        DispatchQueue.main.async { render(0) }
        application.run()
    }
}
