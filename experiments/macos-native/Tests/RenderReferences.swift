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
            let filmstripDeadline = Date().addingTimeInterval(10)
            let estimateDeadline = Date().addingTimeInterval(30)
            var dustFrozen = false
            func captureWhenReady() {
                view.layoutSubtreeIfNeeded()
                view.displayIfNeeded()
                if dustCapture && !dustFrozen {
                    // Layout and filter setup can consume most of this short
                    // animation on a busy runner. Sample animation-local time,
                    // not elapsed wall time; still read the real compositor.
                    dustFrozen = holdDustPresentation(in: view)
                    guard dustFrozen || Date() < filmstripDeadline else {
                        fputs("No dust animation layers appeared within 10 seconds.\n", stderr)
                        exit(1)
                    }
                    DispatchQueue.main.asyncAfter(deadline: .now() + 0.18) { captureWhenReady() }
                    return
                }
                if fixture.name == "recording-editor-layout",
                   let readiness = view.bitmapImageRepForCachingDisplay(in: view.bounds) {
                    // These are ordinary SwiftUI thumbnail images. This read
                    // only gates readiness; compositor proof still comes from
                    // the independent WindowServer capture below.
                    view.cacheDisplay(in: view.bounds, to: readiness)
                    if !hasTimelineFilmstrip(readiness), Date() < filmstripDeadline {
                        DispatchQueue.main.asyncAfter(deadline: .now() + 0.2) { captureWhenReady() }
                        return
                    }
                }
                if fixture.name == "recording-editor-quality-estimate",
                   resolvedNativeReferenceEstimateLabel() == nil,
                   Date() < estimateDeadline {
                    DispatchQueue.main.asyncAfter(deadline: .now() + 0.2) { captureWhenReady() }
                    return
                }
                if captureAnimations && (dustCapture || fixture.name == "recording-editor-layout") {
                    let name = dustCapture
                        ? "preview-dust-compositor.png" : "recording-editor-compositor.png"
                    let destination = output.appendingPathComponent(name)
                    do {
                        let image: NSImage
                        if dustCapture {
                            // Read WindowServer after the held animation phase
                            // has had a chance to reach the compositor.
                            guard let captured = CGWindowListCreateImage(
                                .null, .optionIncludingWindow,
                                CGWindowID(window.windowNumber), [.boundsIgnoreFraming]
                            ) else { throw NativeFailure("WindowServer did not return the dust presentation.") }
                            let bitmap = NSBitmapImageRep(cgImage: captured)
                            guard let data = bitmap.representation(using: .png, properties: [:]) else {
                                throw NativeFailure("Could not encode the dust compositor capture.")
                            }
                            try data.write(to: destination, options: .withoutOverwriting)
                            image = NSImage(cgImage: captured, size: .zero)
                        } else {
                            let capture = Process()
                            capture.executableURL = URL(fileURLWithPath: "/usr/sbin/screencapture")
                            capture.arguments = ["-x", "-o", "-l", "\(window.windowNumber)", destination.path]
                            try capture.run(); capture.waitUntilExit()
                            guard capture.terminationStatus == 0,
                                  let captured = NSImage(contentsOf: destination) else {
                                throw NativeFailure("screencapture did not return the recording presentation.")
                            }
                            image = captured
                        }
                        guard
                              (dustCapture ? hasDustPresentation(image)
                                  : hasSharedFixtureFeature(image) && hasTimelineFilmstrip(image)) else {
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
                guard let data = bitmap.representation(using: .png, properties: [:]) else { exit(1) }
                do { try data.write(to: output.appendingPathComponent("\(fixture.name).png"), options: .withoutOverwriting) }
                catch { fputs("Reference write: \(error)\n", stderr); exit(1) }
                if [
                    "previews-collapsed", "previews-collapsed-fanned", "previews-expanded",
                    "image-editor", "image-editor-shapes", "image-editor-properties",
                    "image-editor-overflow", "image-editor-snap-guides",
                    "image-editor-viewport-pan-zoom",
                    "image-editor-layer-compositing", "image-editor-layer-combine-controls",
                    "image-editor-text-styles", "image-editor-text-defaults",
                    "image-editor-erase", "image-editor-wand",
                ].contains(fixture.name),
                   !hasSharedFixtureFeature(bitmap) {
                    fputs("Native \(fixture.name) reference omitted the shared source fixture's distinctive media content; diagnostic PNG saved.\n", stderr)
                    exit(1)
                }
                if fixture.name == "image-editor-layer-compositing",
                   resolvedNativeReferenceLayerCanvasOffscreen() != false {
                    fputs("Native image-editor-layer-compositing incorrectly offered Recenter for a fitted visible canvas; diagnostic PNG saved.\n", stderr)
                    exit(1)
                }
                if fixture.name == "image-editor-layer-combine-controls",
                   resolvedNativeReferenceCombineControlsVisible() != true {
                    fputs("Native image-editor-layer-combine-controls did not fully expose its combine controls; diagnostic PNG saved.\n", stderr)
                    exit(1)
                }
                if fixture.name == "recording-editor-layout", !hasTimelineFilmstrip(bitmap) {
                    fputs("Native recording-editor-layout reference did not contain decoded filmstrip frames within 10 seconds; diagnostic PNG saved.\n", stderr)
                    exit(1)
                }
                if fixture.name == "recording-editor-quality-estimate" {
                    guard let label = resolvedNativeReferenceEstimateLabel(),
                          label != "—", label != "Estimating…" else {
                        let state = resolvedNativeReferenceEstimateState() ?? "no lifecycle notification"
                        fputs("Native recording quality reference did not complete a successful media estimate within 30 seconds (\(state)); diagnostic PNG saved.\n", stderr)
                        exit(1)
                    }
                    print("Completed native recording estimate: \(label)")
                }
                print("Rendered native \(fixture.name): \(bitmap.pixelsWide) × \(bitmap.pixelsHigh)")
                render(index + 1)
            }
            DispatchQueue.main.asyncAfter(deadline: .now() + (dustCapture ? 0.18 : 0.7)) {
                captureWhenReady()
            }
        }
        DispatchQueue.main.async { render(0) }
        application.run()
    }

    private static func holdDustPresentation(in view: NSView) -> Bool {
        var chips: [(CALayer, CAAnimation)] = []
        var visited = Set<ObjectIdentifier>()
        func visit(_ layer: CALayer) {
            guard visited.insert(ObjectIdentifier(layer)).inserted else { return }
            if let animation = layer.animation(forKey: "captures-dust") {
                chips.append((layer, animation))
            }
            for child in layer.sublayers ?? [] { visit(child) }
        }
        func visitView(_ view: NSView) {
            if let layer = view.layer { visit(layer) }
            for child in view.subviews { visitView(child) }
        }
        visitView(view)
        guard let firstStart = chips.map({ $0.1.beginTime }).min() else { return false }
        CATransaction.begin()
        CATransaction.setDisableActions(true)
        for (chip, _) in chips {
            // Production groups share a timeline origin plus each chip's
            // stagger. Preserve those offsets while holding a mid-wave phase.
            chip.speed = 0
            chip.timeOffset = firstStart + 0.42
        }
        CATransaction.commit()
        CATransaction.flush()
        return true
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
        var clearInteriorPixels = 0
        for y in stride(from: minimumY, to: maximumY, by: 2) {
            for x in stride(from: minimumX, to: maximumX, by: 2) {
                guard let color = bitmap.colorAt(x: x, y: y)?.usingColorSpace(.sRGB) else { continue }
                if color.alphaComponent > 0.08,
                   color.blueComponent > 0.18,
                   color.blueComponent - color.redComponent > 0.05,
                   color.blueComponent - color.greenComponent > 0.015 {
                    sourcePixels += 1
                }
                // The intact source fills the card. Require holes through its
                // central area as independent evidence that the compositor is
                // presenting displaced fragments rather than a static model
                // layer. This band remains inside the card with either bitmap
                // row orientation, including captures that contain a titlebar.
                if y >= bitmap.pixelsHigh * 30 / 100,
                   y < bitmap.pixelsHigh * 70 / 100,
                   color.alphaComponent < 0.03 {
                    clearInteriorPixels += 1
                }
            }
        }
        return sourcePixels >= 120 && clearInteriorPixels >= 30
    }

    private static func hasSharedFixtureFeature(_ image: NSImage) -> Bool {
        guard let bitmap = NSBitmapImageRep(data: image.tiffRepresentation ?? Data()) else { return false }
        return hasSharedFixtureFeature(bitmap)
    }

    private static func hasSharedFixtureFeature(_ bitmap: NSBitmapImageRep) -> Bool {
        // The AVPlayer compositor presents the source darker than its decoded
        // RGB values (the fixture moon is about 247/230/184 before presentation
        // and 129/123/101 in the CI window capture). Match its relative warm
        // chroma rather than the source luminance, and independently require a
        // substantial population from the blue scene. Neutral editor chrome,
        // the yellow accent, and an empty/black player cannot satisfy both.
        let minimumX = bitmap.pixelsWide * 15 / 100
        let maximumX = bitmap.pixelsWide * 85 / 100
        // Keep this vertical crop symmetric because NSBitmapImageRep and image
        // inspection tools need not expose the same row origin.
        let minimumY = bitmap.pixelsHigh * 10 / 100
        let maximumY = bitmap.pixelsHigh * 90 / 100
        var warmPixels = 0
        var bluePixels = 0
        for y in stride(from: minimumY, to: maximumY, by: 2) {
            for x in stride(from: minimumX, to: maximumX, by: 2) {
                guard let color = bitmap.colorAt(x: x, y: y)?.usingColorSpace(.sRGB) else { continue }
                if color.alphaComponent > 0.08,
                   color.redComponent > 0.42,
                   color.greenComponent > 0.38,
                   color.blueComponent > 0.30,
                   color.redComponent - color.greenComponent < 0.09,
                   color.greenComponent - color.blueComponent > 0.045 {
                    warmPixels += 1
                }
                if color.alphaComponent > 0.08,
                   color.blueComponent > 0.18,
                   color.blueComponent - color.redComponent > 0.05,
                   color.blueComponent - color.greenComponent > 0.015 {
                    bluePixels += 1
                }
            }
        }
        return warmPixels >= 80 && bluePixels >= 500
    }

    private static func hasTimelineFilmstrip(_ image: NSImage) -> Bool {
        guard let bitmap = NSBitmapImageRep(data: image.tiffRepresentation ?? Data()) else { return false }
        return hasTimelineFilmstrip(bitmap)
    }

    private static func hasTimelineFilmstrip(_ bitmap: NSBitmapImageRep) -> Bool {
        // The preview is centered and letterboxed, while the filmstrip spans
        // nearly the full editor width. Count the source fixture's blue only
        // in the extreme side bands at the timeline's vertical position, so
        // a decoded preview cannot make empty thumbnail cells pass. Test both
        // vertical orientations because NSBitmapImageRep row origins vary by
        // capture path and backing representation.
        let xRanges = [
            bitmap.pixelsWide * 5 / 100..<bitmap.pixelsWide * 15 / 100,
            bitmap.pixelsWide * 85 / 100..<bitmap.pixelsWide * 95 / 100,
        ]
        let yRanges = [
            bitmap.pixelsHigh * 72 / 100..<bitmap.pixelsHigh * 87 / 100,
            bitmap.pixelsHigh * 13 / 100..<bitmap.pixelsHigh * 28 / 100,
        ]
        for yRange in yRanges {
            var sourcePixels = [0, 0]
            for y in stride(from: yRange.lowerBound, to: yRange.upperBound, by: 2) {
                for (side, xRange) in xRanges.enumerated() {
                    for x in stride(from: xRange.lowerBound, to: xRange.upperBound, by: 2) {
                        guard let color = bitmap.colorAt(x: x, y: y)?.usingColorSpace(.sRGB) else { continue }
                        if color.alphaComponent > 0.08,
                           color.blueComponent > 0.18,
                           color.blueComponent - color.redComponent > 0.05,
                           color.blueComponent - color.greenComponent > 0.015 {
                            sourcePixels[side] += 1
                        }
                    }
                }
            }
            if sourcePixels.allSatisfy({ $0 >= 40 }) {
                print("Decoded filmstrip source samples, left/right: \(sourcePixels)")
                return true
            }
        }
        return false
    }
}
