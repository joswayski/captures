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
        let captureAnimations = ProcessInfo.processInfo.environment["CAPTURES_NATIVE_CAPTURE_ANIMATION"] == "1"
        let fixtures = nativeReferenceFixtures(
            imageURL: imageURL,
            recordingURL: recordingURL,
            includeAnimationCapture: captureAnimations
        )

        func render(_ index: Int) {
            guard index < fixtures.count else {
                window.orderOut(nil)
                exit(0)
            }
            let fixture = fixtures[index]
            window.setContentSize(fixture.size)
            window.appearance = NSAppearance(named: fixture.scheme == .light ? .aqua : .darkAqua)
            let dustCapture = fixture.name == "preview-dust-compositor-source"
            // A transparent content backing makes an absent presentation layer
            // produce zero alpha instead of an opaque NSWindow background.
            window.isOpaque = !dustCapture
            window.backgroundColor = dustCapture ? .clear : .windowBackgroundColor
            let view = NSHostingView(rootView: fixture.makeView()
                .font(.system(size: NativeTheme.metric("text-md")))
                .tint(NativeTheme.accent)
                .environment(\.colorScheme, fixture.scheme)
                .frame(width: fixture.size.width, height: fixture.size.height))
            view.wantsLayer = true
            view.layer?.backgroundColor = NSColor.clear.cgColor
            window.contentView = view
            window.makeKeyAndOrderFront(nil)
            application.activate(ignoringOtherApps: true)
            DispatchQueue.main.asyncAfter(deadline: .now() + (dustCapture ? 0.18 : 0.7)) {
                view.layoutSubtreeIfNeeded()
                view.displayIfNeeded()
                if captureAnimations && (dustCapture || fixture.name == "recording-editor-layout") {
                    let name = dustCapture
                        ? "preview-dust-compositor.png" : "recording-editor-compositor.png"
                    let destination = output.appendingPathComponent(name)
                    let capture = Process()
                    capture.executableURL = URL(fileURLWithPath: "/usr/sbin/screencapture")
                    capture.arguments = ["-x", "-o", "-l", "\(window.windowNumber)", destination.path]
                    do {
                        try capture.run(); capture.waitUntilExit()
                        guard capture.terminationStatus == 0,
                              let image = NSImage(contentsOf: destination),
                              (dustCapture ? hasDustPresentation(image)
                                  : hasSharedFixtureFeature(image)) else {
                            fputs("Compositor capture for \(fixture.name) did not contain its required presented media. Grant Screen Recording permission, verify the window is unobscured, and rerun.\n", stderr)
                            exit(1)
                        }
                    } catch {
                        fputs("Compositor dust capture: \(error)\n", stderr); exit(1)
                    }
                }
                // cacheDisplay is not evidence for Core Animation presentation
                // layers, so dust has only the compositor-backed output above.
                if dustCapture {
                    print("Rendered native preview-dust-compositor")
                    render(index + 1)
                    return
                }
                guard let bitmap = view.bitmapImageRepForCachingDisplay(in: view.bounds) else {
                    fputs("AppKit could not allocate a reference bitmap.\n", stderr); exit(1)
                }
                view.cacheDisplay(in: view.bounds, to: bitmap)
                guard hasVisibleContent(bitmap) else {
                    fputs("Native \(fixture.name) reference contained no visible surface content.\n", stderr); exit(1)
                }
                if [
                    "previews-collapsed", "previews-collapsed-fanned", "previews-expanded",
                    "image-editor", "image-editor-shapes", "image-editor-properties",
                ].contains(fixture.name),
                   !hasSharedFixtureFeature(bitmap) {
                    fputs("Native \(fixture.name) reference omitted the shared source fixture's distinctive media content.\n", stderr)
                    exit(1)
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

    private static func hasDustPresentation(_ image: NSImage) -> Bool {
        guard let bitmap = NSBitmapImageRep(data: image.tiffRepresentation ?? Data()) else { return false }
        // Window captures can use either bitmap orientation and any backing
        // scale. Ignore symmetric edge bands so neither title-bar orientation
        // nor traffic-light controls can count as presented dust. The window
        // and hosting backgrounds are transparent for this fixture; also
        // require the shared source image's blue chroma, not merely alpha.
        let minimumX = bitmap.pixelsWide * 12 / 100
        let maximumX = bitmap.pixelsWide * 88 / 100
        let minimumY = bitmap.pixelsHigh * 18 / 100
        let maximumY = bitmap.pixelsHigh * 82 / 100
        var sourcePixels = 0
        for y in stride(from: minimumY, to: maximumY, by: 2) {
            for x in stride(from: minimumX, to: maximumX, by: 2) {
                guard let color = bitmap.colorAt(x: x, y: y)?.usingColorSpace(.sRGB),
                      color.alphaComponent > 0.08,
                      color.blueComponent > 0.18,
                      color.blueComponent - color.redComponent > 0.05,
                      color.blueComponent - color.greenComponent > 0.015 else { continue }
                sourcePixels += 1
                if sourcePixels >= 120 { return true }
            }
        }
        return false
    }

    private static func hasSharedFixtureFeature(_ image: NSImage) -> Bool {
        guard let bitmap = NSBitmapImageRep(data: image.tiffRepresentation ?? Data()) else { return false }
        return hasSharedFixtureFeature(bitmap)
    }

    private static func hasSharedFixtureFeature(_ bitmap: NSBitmapImageRep) -> Bool {
        // The shared source fixture contains a large pale-warm moon. Requiring
        // that signature prevents editor chrome alone from passing when the
        // intended image or AVPlayer presentation layer is absent.
        var warmPixels = 0
        for y in stride(from: 0, to: bitmap.pixelsHigh, by: 2) {
            for x in stride(from: 0, to: bitmap.pixelsWide, by: 2) {
                guard let color = bitmap.colorAt(x: x, y: y)?.usingColorSpace(.sRGB) else { continue }
                if color.redComponent > 0.72,
                   color.greenComponent > 0.62,
                   color.blueComponent > 0.45,
                   color.redComponent - color.greenComponent < 0.16,
                   color.greenComponent - color.blueComponent > 0.08 {
                    warmPixels += 1
                    if warmPixels >= 80 { return true }
                }
            }
        }
        return false
    }
}
