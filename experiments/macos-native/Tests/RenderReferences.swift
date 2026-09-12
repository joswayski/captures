import AppKit
import AVFoundation
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

        guard let imagePath = ProcessInfo.processInfo.environment["CAPTURES_NATIVE_REFERENCE_IMAGE"],
              let recordingPath = ProcessInfo.processInfo.environment["CAPTURES_NATIVE_REFERENCE_VIDEO"] else {
            fputs("Reference fixture paths were not supplied by render.sh.\n", stderr); exit(1)
        }
        let imageURL = URL(fileURLWithPath: imagePath)
        let recordingURL = URL(fileURLWithPath: recordingPath)
        guard let fixture = NSImage(contentsOf: imageURL), hasVisibleContent(fixture) else {
            fputs("The shared image fixture could not be decoded.\n", stderr); exit(1)
        }
        let generator = AVAssetImageGenerator(asset: AVURLAsset(url: recordingURL))
        generator.appliesPreferredTrackTransform = true
        guard let frame = try? generator.copyCGImage(
            at: CMTime(seconds: 1, preferredTimescale: 600), actualTime: nil
        ), frame.width == 960, frame.height == 540,
           hasVisibleContent(NSImage(cgImage: frame, size: .zero)) else {
            fputs("The 960 × 540 recording fixture could not produce a visible decoded frame.\n", stderr); exit(1)
        }
        let fixtures = nativeReferenceFixtures(imageURL: imageURL, recordingURL: recordingURL)

        func render(_ index: Int) {
            guard index < fixtures.count else {
                window.orderOut(nil)
                exit(0)
            }
            let fixture = fixtures[index]
            window.setContentSize(fixture.size)
            window.appearance = NSAppearance(named: fixture.scheme == .light ? .aqua : .darkAqua)
            let view = NSHostingView(rootView: fixture.makeView()
                .font(.system(size: NativeTheme.metric("text-md")))
                .tint(NativeTheme.accent)
                .environment(\.colorScheme, fixture.scheme)
                .frame(width: fixture.size.width, height: fixture.size.height))
            window.contentView = view
            window.makeKeyAndOrderFront(nil)
            application.activate(ignoringOtherApps: true)
            DispatchQueue.main.asyncAfter(deadline: .now() + (fixture.name == "preview-dust-layout" ? 0.18 : 0.7)) {
                view.layoutSubtreeIfNeeded()
                view.displayIfNeeded()
                if ["preview-dust-layout", "recording-editor-layout"].contains(fixture.name),
                   ProcessInfo.processInfo.environment["CAPTURES_NATIVE_CAPTURE_ANIMATION"] == "1" {
                    let name = fixture.name == "preview-dust-layout"
                        ? "preview-dust-compositor.png" : "recording-editor-compositor.png"
                    let destination = output.appendingPathComponent(name)
                    let capture = Process()
                    capture.executableURL = URL(fileURLWithPath: "/usr/sbin/screencapture")
                    capture.arguments = ["-x", "-o", "-l", "\(window.windowNumber)", destination.path]
                    do {
                        try capture.run(); capture.waitUntilExit()
                        guard capture.terminationStatus == 0,
                              let image = NSImage(contentsOf: destination), hasVisibleContent(image) else {
                            fputs("Compositor capture for \(fixture.name) was blank or unavailable. Grant Screen Recording permission and rerun.\n", stderr)
                            exit(1)
                        }
                    } catch {
                        fputs("Compositor dust capture: \(error)\n", stderr); exit(1)
                    }
                }
                guard let bitmap = view.bitmapImageRepForCachingDisplay(in: view.bounds) else {
                    fputs("AppKit could not allocate a reference bitmap.\n", stderr); exit(1)
                }
                view.cacheDisplay(in: view.bounds, to: bitmap)
                guard hasVisibleContent(bitmap) else {
                    fputs("Native \(fixture.name) reference contained no visible surface content.\n", stderr); exit(1)
                }
                guard let data = bitmap.representation(using: .png, properties: [:]) else { exit(1) }
                do { try data.write(to: output.appendingPathComponent("\(fixture.name).png"), options: .withoutOverwriting) }
                catch { fputs("Reference write: \(error)\n", stderr); exit(1) }
                print("Rendered native \(fixture.name): \(bitmap.pixelsWide) × \(bitmap.pixelsHigh)")
                render(index + 1)
            }
        }
        DispatchQueue.main.async { render(0) }
        application.run()
    }

    private static func hasVisibleContent(_ image: NSImage) -> Bool {
        guard let bitmap = NSBitmapImageRep(data: image.tiffRepresentation ?? Data()) else { return false }
        return hasVisibleContent(bitmap)
    }

    private static func hasVisibleContent(_ bitmap: NSBitmapImageRep) -> Bool {
        var colors = Set<UInt32>()
        let stepX = max(1, bitmap.pixelsWide / 40)
        let stepY = max(1, bitmap.pixelsHigh / 30)
        for y in stride(from: 0, to: bitmap.pixelsHigh, by: stepY) {
            for x in stride(from: 0, to: bitmap.pixelsWide, by: stepX) {
                guard let color = bitmap.colorAt(x: x, y: y)?.usingColorSpace(.sRGB), color.alphaComponent > 0.02 else { continue }
                let red = UInt32((color.redComponent * 255).rounded())
                let green = UInt32((color.greenComponent * 255).rounded())
                let blue = UInt32((color.blueComponent * 255).rounded())
                colors.insert(red << 16 | green << 8 | blue)
                if colors.count >= 4 { return true }
            }
        }
        return false
    }
}
