import AppKit
import XCTest
import CCapturesSettings
@testable import CapturesNative

final class MiniPreviewTests: XCTestCase {
    private let tokens = Tokens.variants["dark-mustard"]!

    func testMainActionRenderedFeedbackKeepsGlassAndHitFrame() throws {
        _ = NSApplication.shared
        let preference = NativeMotion.motionPreference
        defer { NativeMotion.motionPreference = preference }
        for reduced in [false, true] {
            NativeMotion.motionPreference = { reduced }
            var copies = 0
            let panel = fixturePanel(ids: ["card"], images: ["card": solidImage(.white)],
                                     copy: { _ in copies += 1 })
            defer { panel.close() }
            panel.orderFrontRegardless()
            let card = try XCTUnwrap(panel.previewView.card(for: "card"))
            let event = try hoverEvent(for: panel)
            card.mouseEntered(with: event)
            let button = try XCTUnwrap(card.subviews.compactMap { $0 as? MiniPreviewButton }
                .first { $0.kind == .copy })
            let frame = button.frame
            let rest = try XCTUnwrap(tokens.color("glass-strong").usingColorSpace(.sRGB))
            button.mouseEntered(with: event)
            try waitUntil { abs(button.chromeBackground.usingColorSpace(.sRGB)!.redComponent
                - min(1, rest.redComponent * 1.12)) < 0.001 }
            func assertPose(scale: Double, dy: Double) throws {
                _ = try renderPresentation(panel)
                let layer = try XCTUnwrap(button.layer?.presentation())
                XCTAssertEqual(layer.transform.m11, CGFloat(scale), accuracy: 0.001)
                let down: CGFloat = card.isFlipped ? 1 : -1
                // Account for AppKit's backing-layer anchor when measuring the centre.
                let centre = CGPoint(x: layer.bounds.midX, y: layer.bounds.midY)
                let t = layer.transform
                let transformedY = (centre.y - layer.anchorPoint.y * layer.bounds.height) * t.m22
                    + t.m42 + layer.position.y
                XCTAssertEqual(transformedY, frame.midY + down * CGFloat(dy), accuracy: 0.01)
                XCTAssertEqual(button.frame, frame)
            }
            try assertPose(scale: 1, dy: -1)
            // Exercise the public cell highlight used by native tracking, not
            // NSButton.highlight (which mouse tracking need not call).
            button.cell?.isHighlighted = true
            if !reduced { RunLoop.main.run(until: Date().addingTimeInterval(0.2)) }
            try assertPose(scale: 0.97, dy: 1)
            button.cell?.isHighlighted = false // cancelled or released tracking
            if !reduced { RunLoop.main.run(until: Date().addingTimeInterval(0.2)) }
            try assertPose(scale: 1, dy: -1)
            button.performClick(nil)
            XCTAssertEqual(copies, 1)
            card.setMainActionBusy(true)
            XCTAssertEqual(button.alphaValue, 0.7, accuracy: 0.001)
            button.performClick(nil)
            XCTAssertEqual(copies, 1, "disabled actions must not dispatch")
            button.mouseExited(with: event)
            button.mouseEntered(with: event)
            if !reduced { RunLoop.main.run(until: Date().addingTimeInterval(0.2)) }
            try assertPose(scale: 1, dy: -1)
            XCTAssertEqual(button.chromeBackground.usingColorSpace(.sRGB)!.alphaComponent,
                           rest.alphaComponent, accuracy: 0.001)
            let bitmap = try XCTUnwrap(renderPresentation(panel))
            let rect = button.convert(button.bounds, to: panel.previewView)
            let pixel = try XCTUnwrap(pixelColor(in: bitmap, of: panel,
                x: rect.minX + 4, y: rect.midY))
            XCTAssertGreaterThan(pixel.alphaComponent, 0, "the transformed glass face is rendered")
            card.setMainActionBusy(false)
            XCTAssertEqual(button.alphaValue, 1)
        }
    }

    func testPreviewKindsResolveSharedShippingGeometry() {
        XCTAssertEqual(MiniPreviewButtonKind.allCases.map(\.iconName),
                       ["close", "trash", "edit", "copy", "save", "folder", "preview-stack", "close", "check"])
        for kind in MiniPreviewButtonKind.allCases {
            XCTAssertFalse(ShippingIcons.polylines(kind.iconName).isEmpty, kind.iconName)
        }
        XCTAssertEqual(ShippingIcons.polylines("preview-overflow-up"),
                       [[NSPoint(x: 5.25, y: 15), NSPoint(x: 12, y: 8.25), NSPoint(x: 18.75, y: 15)]])
        XCTAssertEqual(ShippingIcons.polylines("check"),
                       [[NSPoint(x: 5, y: 12), NSPoint(x: 9, y: 16), NSPoint(x: 19, y: 6)]])
    }

    func testCoordinateConversionPreservesNegativeOriginAndTopLeftGeometry() {
        let monitor = CapturesPreviewMonitor(work_x: -2880, work_y: 50,
            work_width: 2880, work_height: 1700, full_x: -2880, full_y: -100,
            full_width: 2880, full_height: 1800, scale_factor: 2)
        let geometry = CapturesPreviewGeometry(x: -1420, y: 25, width: 340, height: 240,
            card_height: 160, padding: 28, control_gutter: 52, anchor: 1)
        let frame = MiniPreviewController.appKitFrame(geometry: geometry, monitor: monitor,
            screenFrame: NSRect(x: -1440, y: 120, width: 1440, height: 900))
        XCTAssertEqual(frame, NSRect(x: -1420, y: 705, width: 340, height: 240))
    }

    func testPanelIsFixedGlassNonactivatingAndActionsStayBoundToArtifact() throws {
        _ = NSApplication.shared
        var actions: [String] = []
        let image = NSImage(cgImage: PreviewView.fixtureImage(scale: 1),
                            size: NSSize(width: 284, height: 160))
        let panel = fixturePanel(ids: ["latest"], images: ["latest": image],
            copy: { _ in actions.append("copy") }, save: { _ in actions.append("save") },
            open: { _ in actions.append("open") }, trash: { _ in actions.append("trash") },
            dismiss: { _ in actions.append("dismiss") }, discard: { _ in actions.append("discard") })
        defer { panel.close() }

        XCTAssertEqual(panel.canBecomeKey, NSApp.isActive, "Keyboard focus only while Captures is active")
        XCTAssertTrue(panel.becomesKeyOnlyIfNeeded, "Clicks never take keyboard focus"); XCTAssertFalse(panel.canBecomeMain)
        XCTAssertTrue(panel.styleMask.contains(.nonactivatingPanel))
        XCTAssertEqual(panel.previewView.artifactIDs, ["latest"])
        let buttons = panel.previewView.subviewsRecursive.compactMap { $0 as? MiniPreviewButton }
        XCTAssertEqual(buttons.map(\.title), ["Close", "Delete", "Edit", "Copy", "Save file"])
        XCTAssertEqual(buttons.first { $0.title == "Edit" }?.accessibilityLabel(), "Edit")
        XCTAssertTrue(buttons.allSatisfy(\.isHidden), "idle chrome must not leave click traps")
        buttons.forEach { $0.performClick(nil) }
        // An unsaved card's Delete discards (dissolves) only the preview.
        XCTAssertEqual(actions, ["dismiss", "discard", "open", "copy", "save"])
        try write(render(panel), name: "mini-preview-single-unsaved-edit-trash.png")
    }

    func testSavedCardUsesRevealWithShowInFolderAccessibility() throws {
        _ = NSApplication.shared
        let panel = fixturePanel(ids: ["saved"], images: ["saved": solidImage(.systemBlue)],
                                 savedPaths: ["saved": "/Exports/Café image.png"])
        defer { panel.close() }
        let buttons = panel.previewView.subviewsRecursive.compactMap { $0 as? MiniPreviewButton }
        let reveal = try XCTUnwrap(buttons.first { $0.title == "Show in Folder" })
        XCTAssertEqual(buttons.map(\.title), ["Close", "Delete", "Edit", "Copy", "Show in Folder"])
        XCTAssertEqual(reveal.accessibilityLabel(), "Show in Folder")
        // Labelled actions carry no tooltip, and no control uses the system one.
        XCTAssertNil(reveal.tooltipText)
        XCTAssertTrue(buttons.allSatisfy { $0.toolTip == nil })
        // Shipping icon buttons use short glass tips that match their names.
        for name in ["Close", "Delete", "Edit"] {
            XCTAssertEqual(buttons.first { $0.title == name }?.tooltipText, name)
        }
        try write(render(panel), name: "mini-preview-single-saved-reveal.png")
    }

    func testDeleteHoverUsesSignalAndWhiteThenClearsWhenHidden() throws {
        _ = NSApplication.shared
        let panel = fixturePanel(ids: ["card"], images: ["card": solidImage(.white)])
        defer { panel.close() }
        let card = try XCTUnwrap(panel.previewView.card(for: "card"))
        let hover = try hoverEvent(for: panel)
        card.mouseEntered(with: hover)
        let controls = card.subviews.compactMap { $0 as? MiniPreviewButton }
        let delete = try XCTUnwrap(controls.first { $0.kind == .trash })
        let edit = try XCTUnwrap(controls.first { $0.kind == .edit })
        func assertColor(_ actual: NSColor, _ expected: NSColor, file: StaticString = #filePath, line: UInt = #line) {
            let actual = actual.usingColorSpace(.sRGB)!, expected = expected.usingColorSpace(.sRGB)!
            XCTAssertEqual(actual.redComponent, expected.redComponent, accuracy: 0.01, file: file, line: line)
            XCTAssertEqual(actual.greenComponent, expected.greenComponent, accuracy: 0.01, file: file, line: line)
            XCTAssertEqual(actual.blueComponent, expected.blueComponent, accuracy: 0.01, file: file, line: line)
            XCTAssertEqual(actual.alphaComponent, expected.alphaComponent, accuracy: 0.01, file: file, line: line)
        }
        assertColor(delete.chromeBackground, tokens.color("glass-strong"))
        assertColor(delete.chromeForeground, tokens.color("theme-signal-text"))
        let restingFrame = delete.frame
        delete.mouseEntered(with: hover)
        try waitUntil { delete.iconHover == 1 }
        assertColor(delete.chromeBackground, tokens.color("theme-signal"))
        assertColor(delete.chromeForeground, .white)
        XCTAssertEqual(delete.frame, restingFrame, "hover must not move the hit target")
        let lifted = try XCTUnwrap(renderPresentation(panel))
        let anchor = delete.convert(delete.bounds, to: panel.previewView)
        // The resting top border is now inside the lifted, scaled face.
        assertColor(try XCTUnwrap(pixelColor(in: lifted, of: panel, x: anchor.midX, y: anchor.minY)),
                    tokens.color("theme-signal"))
        try write(lifted, name: "mini-preview-delete-hover.png")
        delete.mouseExited(with: hover)
        try waitUntil { delete.iconHover == 0 }
        XCTAssertEqual(delete.frame, restingFrame)
        assertColor(delete.chromeBackground, tokens.color("glass-strong"))
        edit.mouseEntered(with: hover)
        try waitUntil { edit.iconHover == 1 }
        assertColor(edit.chromeBackground, tokens.color("glass-raised"))
        delete.mouseEntered(with: hover)
        try waitUntil { delete.iconHover == 1 }
        card.mouseExited(with: hover)
        XCTAssertEqual(delete.iconHover, 0, "hidden controls must not retain destructive hover chrome")
        card.mouseEntered(with: hover)
        assertColor(delete.chromeBackground, tokens.color("glass-strong"))
        delete.isEnabled = false
        delete.mouseEntered(with: hover)
        XCTAssertEqual(delete.iconHover, 0)
    }

    func testHoverChromeMirrorsAndSaveUpdatesWithoutMovingCenterActions() throws {
        _ = NSApplication.shared
        for right in [false, true] {
            let panel = fixturePanel(ids: ["card"], images: ["card": solidImage(.white)],
                                     rightAnchor: right)
            defer { panel.close() }
            let card = try XCTUnwrap(panel.previewView.subviewsRecursive.compactMap { $0 as? MiniPreviewCardView }.first)
            let controls = card.subviews.compactMap { $0 as? MiniPreviewButton }
            func button(_ name: String) throws -> MiniPreviewButton {
                try XCTUnwrap(controls.first { $0.title == name })
            }
            let event = try XCTUnwrap(NSEvent.mouseEvent(with: .mouseMoved, location: .zero,
                modifierFlags: [], timestamp: 1, windowNumber: panel.windowNumber, context: nil,
                eventNumber: 0, clickCount: 0, pressure: 0))
            XCTAssertTrue(controls.allSatisfy(\.isHidden))
            card.mouseEntered(with: event)
            XCTAssertEqual(controls.filter { !$0.isHidden }.map(\.title), ["Delete", "Edit", "Copy", "Save file"])
            XCTAssertEqual(try button("Delete").frame, NSRect(x: right ? 248 : 8, y: 8, width: 28, height: 28))
            XCTAssertEqual(try button("Edit").frame.minX, right ? 8 : 248)
            XCTAssertEqual(try button("Copy").frame, NSRect(x: 72, y: 45, width: 140, height: 32))
            let saveFrame = try button("Save file").frame
            XCTAssertEqual(saveFrame, NSRect(x: 72, y: 83, width: 140, height: 32))
            XCTAssertFalse(card.hasVisibleLabels)
            try write(render(panel), name: "mini-preview-hover-\(right ? "right" : "left")")
            card.updateSaveButton(saved: true)
            XCTAssertEqual(try button("Show in Folder").frame, saveFrame)
            XCTAssertFalse(try button("Close").isHidden)
            XCTAssertEqual(try button("Close").frame.minX, right ? 214 : 8)
            XCTAssertEqual(try button("Delete").frame.minX, right ? 248 : 42)
            card.mouseExited(with: event)
            XCTAssertTrue(controls.allSatisfy(\.isHidden))
            XCTAssertTrue(card.hasVisibleLabels)
        }
    }

    func testCollapsedPointerDragTracksDesktopWithoutExpandingAndClickStillExpands() throws {
        _ = NSApplication.shared
        var moves: [NSPoint] = []
        var expanded = 0
        let panel = fixturePanel(ids: ["one", "two"],
            images: ["one": solidImage(.red), "two": solidImage(.blue)], collapsed: true,
            setCollapsed: { if !$0 { expanded += 1 } }, move: { moves.append($0) })
        defer { panel.close() }
        panel.setFrameOrigin(NSPoint(x: 300, y: 200))
        let button = try XCTUnwrap(panel.previewView.subviewsRecursive.compactMap { $0 as? NSButton }
            .first { $0.accessibilityLabel() == "Expand 2 previews" })
        let point = button.convert(NSPoint(x: 100, y: 80), to: nil)
        func event(_ type: NSEvent.EventType, _ point: NSPoint) throws -> NSEvent {
            try XCTUnwrap(NSEvent.mouseEvent(with: type, location: point, modifierFlags: [],
                timestamp: 1, windowNumber: panel.windowNumber, context: nil,
                eventNumber: 0, clickCount: 1, pressure: 1))
        }
        let rear = try XCTUnwrap(panel.previewView.subviewsRecursive.compactMap { $0 as? MiniPreviewCardView }
            .first { $0.artifactID == "one" })
        let front = try XCTUnwrap(panel.previewView.subviewsRecursive.compactMap { $0 as? MiniPreviewCardView }
            .first { $0.artifactID == "two" })
        let restY = rear.frame.minY, frontFrame = front.frame, windowFrame = panel.frame
        // The shared pile pose moves the rear card between its rest and fan poses.
        let rest = try XCTUnwrap(panel.previewView.pilePose(for: "one", depth: 1, hovered: false))
        let fan = try XCTUnwrap(panel.previewView.pilePose(for: "one", depth: 1, hovered: true))
        XCTAssertLessThan(fan.dy, rest.dy, "the hover fan lifts the rear card further")
        button.mouseEntered(with: try event(.leftMouseDown, point))
        try waitUntil { abs(rear.frame.minY - (restY + CGFloat(fan.dy - rest.dy))) < 0.001 }
        XCTAssertEqual(front.frame, frontFrame)
        XCTAssertEqual(panel.frame, windowFrame)
        XCTAssertEqual(rear.pileGlowOpacity, 1, "the hovered pile takes its accent ring and glow")
        XCTAssertEqual(rear.depthBlurRadius, captures_preview_pile_media_blur_v1(1, true), accuracy: 1e-9)
        try write(render(panel), name: "mini-preview-stack-hovered.png")
        for delta in [NSPoint(x: 7, y: 0), NSPoint(x: 5, y: -5)] {
            moves.removeAll()
            button.mouseDown(with: try event(.leftMouseDown, point))
            let jitter = NSPoint(x: point.x + delta.x, y: point.y + delta.y)
            button.mouseDragged(with: try event(.leftMouseDragged, jitter))
            XCTAssertTrue(moves.isEmpty, "sub-eight-point jitter must remain an Expand click")
            let before = expanded
            button.mouseUp(with: try event(.leftMouseUp, jitter))
            XCTAssertEqual(expanded, before + 1)
        }
        for delta in [NSPoint(x: 8, y: 0), NSPoint(x: 6, y: -6)] {
            moves.removeAll()
            button.mouseDown(with: try event(.leftMouseDown, point))
            let moved = NSPoint(x: point.x + delta.x, y: point.y + delta.y)
            button.mouseDragged(with: try event(.leftMouseDragged, moved))
            XCTAssertEqual(moves.last, NSPoint(x: 300 + delta.x, y: 200 + delta.y))
            let before = expanded
            button.mouseUp(with: try event(.leftMouseUp, moved))
            XCTAssertEqual(expanded, before, "crossing eight points must not expand on release")
        }
        moves.removeAll(); expanded = 0
        button.mouseDown(with: try event(.leftMouseDown, point))
        button.mouseExited(with: try event(.leftMouseDragged, point))
        XCTAssertTrue(panel.previewView.pileHovered, "Press retains hover outside the front card")
        button.mouseDragged(with: try event(.leftMouseDragged, NSPoint(x: point.x + 2, y: point.y - 1)))
        XCTAssertTrue(moves.isEmpty)
        button.mouseDragged(with: try event(.leftMouseDragged, NSPoint(x: point.x + 40, y: point.y - 25)))
        XCTAssertEqual(moves.last, NSPoint(x: 340, y: 175))
        panel.setFrameOrigin(try XCTUnwrap(moves.last))
        // The stationary desktop pointer returns to its original local position.
        button.mouseDragged(with: try event(.leftMouseDragged, point))
        XCTAssertEqual(moves.last, NSPoint(x: 340, y: 175))
        button.mouseUp(with: try event(.leftMouseUp, point))
        XCTAssertEqual(expanded, 0)
        button.mouseDown(with: try event(.leftMouseDown, point))
        button.mouseUp(with: try event(.leftMouseUp, point))
        XCTAssertEqual(expanded, 1)
        button.performClick(nil)
        XCTAssertEqual(expanded, 2, "Accessibility activation must remain a click")
        button.mouseExited(with: try event(.leftMouseDragged, point))
        try waitUntil { abs(rear.frame.minY - restY) < 0.001 }
        XCTAssertFalse(panel.previewView.pileHovered)
        XCTAssertTrue(panel.becomesKeyOnlyIfNeeded)
    }

    func testCarriedPileLeansAfterItsGatherAndDropsTheLean() throws {
        _ = NSApplication.shared
        let ids = ["oldest", "middle", "newest"]
        let images = Dictionary(uniqueKeysWithValues: ids.map { ($0, solidImage(.systemBlue)) })
        let panel = fixturePanel(ids: ids, images: images, collapsed: true)
        defer { panel.close() }
        let view = panel.previewView
        let rear = try XCTUnwrap(view.card(for: "oldest"))
        view.carryPile(at: NSPoint(x: 100, y: 100))
        let start = CACurrentMediaTime()
        view.stepCarry(now: start + 0.01)
        XCTAssertEqual(view.pileLean.x, 0, "the fanned pose holds while the fan gathers")
        view.stepCarry(now: start + 1)
        let still = rear.frame.origin
        for step in 1...8 {
            view.carryPile(at: NSPoint(x: 100 + 30 * CGFloat(step), y: 100))
            view.stepCarry(now: start + 1 + 0.016 * Double(step))
        }
        if NativeMotion.reduceMotion {
            XCTAssertEqual(view.pileLean.x, 0, "reduced motion never leans")
        } else {
            XCTAssertLessThan(view.pileLean.x, -1, "a rightward carry leans the rear cards left")
            XCTAssertLessThan(rear.frame.minX, still.x, "the rear card trails the carry")
        }
        view.carryPile(at: nil)
        XCTAssertEqual(view.pileLean.x, 0)
        XCTAssertEqual(view.pileLean.y, 0)
    }

    func testCollapsedPileAppliesTheSharedDepthPose() throws {
        _ = NSApplication.shared
        let ids = ["oldest", "middle", "newest"]
        let images = Dictionary(uniqueKeysWithValues: ids.map { ($0, solidImage(.systemBlue)) })
        for top in [false, true] {
            let panel = fixturePanel(ids: ids, images: images, collapsed: true, topAnchor: top)
            defer { panel.close() }
            let view = panel.previewView
            XCTAssertEqual(view.pileGravity, top ? -1 : 1, "corner piles sit at full gravity")
            let front = try XCTUnwrap(view.card(for: "newest"))
            XCTAssertNil(view.pilePose(for: "newest", depth: 0, hovered: false))
            XCTAssertTrue(CATransform3DIsIdentity(try XCTUnwrap(front.layer).transform))
            var previousScale = 1.0
            for (depth, id) in [(1, "middle"), (2, "oldest")] {
                let pose = try XCTUnwrap(view.pilePose(for: id, depth: depth, hovered: false))
                XCTAssertEqual(pose.rotation_deg, 0, "no spin at the screen edge")
                XCTAssertLessThan(pose.scale_x, previousScale)
                previousScale = pose.scale_x
                let card = try XCTUnwrap(view.card(for: id))
                // Rear cards peek past the front card, toward the stack's open side.
                let offset = (card.frame.midY - front.frame.midY) * (top ? 1 : -1)
                XCTAssertEqual(card.frame.midY - front.frame.midY, CGFloat(pose.dy), accuracy: 0.001)
                XCTAssertGreaterThan(offset, 0)
                XCTAssertFalse(CATransform3DIsIdentity(try XCTUnwrap(card.layer).transform))
                XCTAssertEqual(card.depthBlurRadius, captures_preview_pile_media_blur_v1(depth, false),
                               accuracy: 1e-9, "rear media blurs with depth")
                let projection = try XCTUnwrap(view.pileProjection(for: id, depth: depth, hovered: false))
                XCTAssertNotEqual(projection[7], 0, "the rotateX tilt keeps its keystone")
            }
            XCTAssertEqual(front.depthBlurRadius, 0)
            XCTAssertEqual(front.layer?.sublayers?.filter { $0.name == BoxShadowLayers.layerName }.count, 2,
                           "--thumbnail-card-shadow has two layers")
            let stack = NativePreviewStack()
            ids.forEach { stack.insert($0) }; stack.setCollapsed(true)
            let layouts = Dictionary(uniqueKeysWithValues: ids.enumerated().compactMap { index, id in
                stack.cardLayout(index: index, topAnchor: top).map { (id, $0) }
            })
            let hoverLayouts = Dictionary(uniqueKeysWithValues: ids.enumerated().compactMap { index, id in
                stack.cardLayout(index: index, topAnchor: top, hovered: true).map { (id, $0) }
            })
            view.updatePileGravity(0, topAnchor: top, layouts: layouts, hoverLayouts: hoverLayouts)
            let middle = try XCTUnwrap(view.pilePose(for: "middle", depth: 1, hovered: false))
            XCTAssertGreaterThanOrEqual(abs(middle.rotation_deg), 2.7)
            XCTAssertLessThanOrEqual(abs(middle.rotation_deg), 3)
        }
    }

    func testPileTransformAppliesTheProjectionAboutTheCardCentre() {
        // Homogeneous row-vector product, as Core Animation applies it.
        func mapped(_ t: CATransform3D, _ p: CGPoint) -> CGPoint {
            let w = p.x * t.m14 + p.y * t.m24 + t.m44
            return CGPoint(x: (p.x * t.m11 + p.y * t.m21 + t.m41) / w, y: (p.x * t.m12 + p.y * t.m22 + t.m42) / w)
        }
        let size = CGSize(width: 200, height: 100)
        // Half scale, a quarter turn clockwise in y-down space.
        let spin = MiniPreviewView.pileTransform(projection: [0, -0.5, 0, 0.5, 0, 0, 0, 0, 1], size: size,
                                                 anchorPoint: .zero, flipped: true)
        let centre = mapped(spin, CGPoint(x: 100, y: 50))
        XCTAssertEqual(centre.x, 100, accuracy: 1e-9); XCTAssertEqual(centre.y, 50, accuracy: 1e-9)
        let edge = mapped(spin, CGPoint(x: 200, y: 50))
        XCTAssertEqual(edge.x, 100, accuracy: 1e-9); XCTAssertEqual(edge.y, 100, accuracy: 1e-9)
        // A tilt's w row narrows one edge and widens the other.
        let tilt = MiniPreviewView.pileTransform(projection: [1, 0, 0, 0, 1, 0, 0, -0.001, 1], size: size,
                                                 anchorPoint: CGPoint(x: 0.5, y: 0.5), flipped: true)
        let top = mapped(tilt, CGPoint(x: 100, y: -50)).x - mapped(tilt, CGPoint(x: -100, y: -50)).x
        let bottom = mapped(tilt, CGPoint(x: 100, y: 50)).x - mapped(tilt, CGPoint(x: -100, y: 50)).x
        // w = 1 - 0.001·y: the y = -50 edge divides by 1.05, the y = 50 edge by 0.95.
        XCTAssertEqual(top, 200 / 1.05, accuracy: 1e-9)
        XCTAssertEqual(bottom, 200 / 0.95, accuracy: 1e-9)
        XCTAssertTrue(CATransform3DIsIdentity(MiniPreviewView.pileTransform(
            projection: [1, 0, 0, 0, 1, 0, 0, 0, 1], size: size, anchorPoint: CGPoint(x: 0.5, y: 0.5),
            flipped: false)))
    }

    func testCardControlsFollowShippingTabOrderAndFocusRevealsThem() throws {
        _ = NSApplication.shared
        let image = NSImage(cgImage: PreviewView.fixtureImage(scale: 1), size: NSSize(width: 284, height: 160))
        let panel = fixturePanel(ids: ["latest"], images: ["latest": image])
        defer { panel.close() }
        let card = try XCTUnwrap(panel.previewView.subviewsRecursive.compactMap { $0 as? MiniPreviewCardView }.first)
        XCTAssertTrue(panel.initialFirstResponder === card)
        XCTAssertEqual(KeyViewLoop.order(from: card).compactMap { ($0 as? MiniPreviewButton)?.title },
                       ["Close", "Delete", "Edit", "Copy", "Save file"])
        let buttons = panel.previewView.subviewsRecursive.compactMap { $0 as? MiniPreviewButton }
        XCTAssertTrue(buttons.allSatisfy(\.isHidden))
        XCTAssertFalse(panel.isKeyWindow)
        XCTAssertTrue(panel.makeFirstResponder(card))
        RunLoop.current.run(until: Date().addingTimeInterval(0.05))
        XCTAssertTrue(buttons.allSatisfy(\.isHidden),
                      "Focus in a panel that is not key shows no chrome, like :focus-within in a background window")
        XCTAssertTrue(card.hasVisibleLabels)
    }

    func testFocusRevealsChromeOnlyWhileThePanelIsKey() throws {
        _ = NSApplication.shared
        let image = NSImage(cgImage: PreviewView.fixtureImage(scale: 1), size: NSSize(width: 284, height: 160))
        let panel = fixturePanel(ids: ["older", "newer"], images: ["older": image, "newer": image])
        let other = NSWindow(contentRect: NSRect(x: 0, y: 0, width: 200, height: 120),
            styleMask: [.titled], backing: .buffered, defer: false)
        other.isReleasedWhenClosed = false
        defer { panel.close(); other.close() }
        NSApp.activate(ignoringOtherApps: true)
        let deadline = Date().addingTimeInterval(2)
        while !NSApp.isActive && Date() < deadline { RunLoop.current.run(until: Date().addingTimeInterval(0.01)) }
        let cards = panel.previewView.subviewsRecursive.compactMap { $0 as? MiniPreviewCardView }
        XCTAssertEqual(cards.count, 2)
        func copyHidden(_ card: MiniPreviewCardView) -> Bool {
            card.subviews.compactMap { $0 as? MiniPreviewButton }.first { $0.title == "Copy" }?.isHidden != false
        }
        // Ordering the panel in hands its initial card first responder while
        // Captures is active, but that alone must not raise any card's chrome.
        panel.orderFrontRegardless()
        RunLoop.current.run(until: Date().addingTimeInterval(0.1))
        XCTAssertFalse(panel.isKeyWindow)
        for card in cards {
            XCTAssertTrue(copyHidden(card), "a shown panel puts up no chrome until it is key")
            XCTAssertTrue(card.hasVisibleLabels, "the dimensions badge stays up")
        }

        try XCTSkipUnless(NSApp.isActive, "Making the panel key needs Captures active in a window-server session")
        let first = try XCTUnwrap(panel.initialFirstResponder as? MiniPreviewCardView)
        panel.makeKey()
        try waitUntil { panel.isKeyWindow }
        if panel.firstResponder !== first { XCTAssertTrue(panel.makeFirstResponder(first)) }
        try waitUntil { !copyHidden(first) }
        XCTAssertFalse(first.hasVisibleLabels, "chrome replaces the dimensions badge")
        for card in cards where card !== first {
            XCTAssertTrue(copyHidden(card), "only the focused card shows its chrome")
        }

        // Focus stays on the card, but its chrome leaves with key status.
        other.makeKeyAndOrderFront(nil)
        try waitUntil { !panel.isKeyWindow }
        XCTAssertTrue(panel.firstResponder === first)
        try waitUntil { copyHidden(first) && first.hasVisibleLabels }
    }

    func testOutboundCardDragIsCopyOnlyAndCompactPileRemainsMoveOnly() throws {
        _ = NSApplication.shared
        let image = solidImage(.systemBlue)
        let expanded = fixturePanel(ids: ["expanded"], images: ["expanded": image])
        let collapsed = fixturePanel(ids: ["older", "newer"],
            images: ["older": image, "newer": image], collapsed: true)
        defer { expanded.close(); collapsed.close() }
        let expandedCard = try XCTUnwrap(expanded.previewView.subviewsRecursive
            .compactMap { $0 as? MiniPreviewCardView }.first)
        expandedCard.preparedDragPath = "/tmp/exact-source.png"
        XCTAssertEqual(expandedCard.sourceOperationMask(for: .outsideApplication), .copy)
        XCTAssertEqual(expandedCard.sourceOperationMask(for: .withinApplication), .copy)
        XCTAssertTrue(expandedCard.isOutboundFileDragEnabled)

        let compactCards = collapsed.previewView.subviewsRecursive
            .compactMap { $0 as? MiniPreviewCardView }
        compactCards.forEach { $0.preparedDragPath = "/tmp/\($0.artifactID).png" }
        XCTAssertTrue(compactCards.allSatisfy { !$0.isOutboundFileDragEnabled },
            "compact cards remain move-only even after their files are prepared")
    }

    func testWorkspaceReconfigurationDoesNotDeleteLiveDragExports() {
        let transport = MiniPreviewActionTransport()
        let actions = MiniPreviewActions(settingsPath: nil, transport: transport)
        actions.configure(historyRoot: "/profile/history")
        LiveCaptureController.queue.sync {}
        XCTAssertEqual(transport.cleanupCount, 1)
        actions.configure(historyRoot: "/profile/history")
        LiveCaptureController.queue.sync {}
        XCTAssertEqual(transport.cleanupCount, 1)
    }

    func testHistoryRestoreJoinsTheStackOnceWithoutACaptureGeneration() throws {
        _ = NSApplication.shared
        let controller = MiniPreviewController(tokens: tokens, imageLoader: { _ in self.solidImage(.blue) })
        defer { controller.close() }
        var outcomes: [String] = []
        func record(_ outcome: MiniPreviewRestoreOutcome) {
            switch outcome {
            case .shown: outcomes.append("shown")
            case .alreadyShowing: outcomes.append("already")
            case .cancelled: outcomes.append("cancelled")
            case .failed: outcomes.append("failed")
            }
        }
        let captured = artifact(id: "captured", previewPath: "/captured.png")
        let generation = try XCTUnwrap(controller.beginCapture(settings: previewSettings()))
        controller.present(captured, on: screenID(), settings: previewSettings(), generation: generation)
        try waitUntil { controller.decodedArtifactIDs == ["captured"] }

        let restored = artifact(id: "restored", previewPath: "/restored.png")
        controller.restore(restored, on: nil, settings: previewSettings(), completion: record)
        try waitUntil { outcomes == ["shown"] }
        XCTAssertEqual(controller.presentedArtifactIDs, ["captured", "restored"], "Restore lands in front")
        XCTAssertTrue(controller.isPanelVisible)
        controller.restore(captured, on: nil, settings: previewSettings(), completion: record)
        XCTAssertEqual(outcomes, ["shown", "already"])
        XCTAssertEqual(controller.presentedArtifactIDs, ["captured", "restored"], "an existing card never moves")

        controller.dismiss("restored")
        controller.restore(restored, on: nil, settings: previewSettings(), completion: record)
        controller.dismiss("restored")
        try waitUntil { outcomes.count == 3 }
        XCTAssertEqual(outcomes.last, "cancelled")
        XCTAssertEqual(controller.presentedArtifactIDs, ["captured"])
        // Restore never begins a capture: the next capture still arrives.
        let nextCapture = try XCTUnwrap(controller.beginCapture(settings: previewSettings()))
        controller.restoreCapture(generation: nextCapture)

        let export = FileManager.default.temporaryDirectory.appendingPathComponent("Café-\(UUID()).png")
        defer { try? FileManager.default.removeItem(at: export) }
        let retained = artifact(id: "retained", previewPath: "/retained.png", savedPath: export.path)
        controller.restore(retained, on: nil, settings: previewSettings()) { _ in }
        try waitUntil { controller.decodedArtifactIDs.contains(retained.id) }
        func saveTitle() throws -> String {
            let card = try XCTUnwrap(controller.previewView?.card(for: retained.id))
            return try XCTUnwrap(card.subviews.compactMap { $0 as? MiniPreviewButton }
                .first { $0.kind == .save || $0.kind == .folder }).title
        }
        XCTAssertEqual(try saveTitle(), "Save file", "a missing export does not remove the retained capture")
        controller.refreshArtifacts([retained])
        XCTAssertEqual(try saveTitle(), "Save file", "raw History metadata cannot reattach a missing file")
        XCTAssertEqual(retained.savedPath, export.path, "Restore does not mutate History's snapshot")

        _ = controller.dismiss(retained.id, exit: nil)
        try Data([1, 2, 3]).write(to: export)
        controller.restore(retained, on: nil, settings: previewSettings()) { _ in }
        try waitUntil { controller.decodedArtifactIDs.contains(retained.id) }
        XCTAssertEqual(try saveTitle(), "Show in Folder", "an existing export keeps Reveal")
        try FileManager.default.removeItem(at: export)
        controller.restore(retained, on: nil, settings: previewSettings()) { _ in }
        XCTAssertEqual(try saveTitle(), "Show in Folder", "an existing card retains Export missing behavior")
        _ = controller.dismiss(retained.id, exit: nil)
        try FileManager.default.createDirectory(at: export, withIntermediateDirectories: false)
        controller.restore(retained, on: nil, settings: previewSettings()) { _ in }
        try waitUntil { controller.decodedArtifactIDs.contains(retained.id) }
        XCTAssertEqual(try saveTitle(), "Save file", "a directory is not a saved screenshot")
    }

    func testStaleAsyncDragPreparationCannotAttachToReplacementIdentity() throws {
        _ = NSApplication.shared
        let controller = MiniPreviewController(tokens: tokens, imageLoader: { _ in self.solidImage(.blue) })
        defer { controller.close() }
        var preparations: [(CaptureArtifact, (String?) -> Void)] = []
        controller.prepareDrag = { preparations.append(($0, $1)) }
        let old = artifact(id: "same", previewPath: "/old-preview.png", imagePath: "/old.png")
        let oldGeneration = try XCTUnwrap(controller.beginCapture(settings: previewSettings()))
        controller.present(old, on: screenID(), settings: previewSettings(), generation: oldGeneration)
        try waitUntil { preparations.count == 1 }

        controller.dismiss(old.id)
        let replacement = artifact(id: "same", previewPath: "/new-preview.png", imagePath: "/new.png")
        let newGeneration = try XCTUnwrap(controller.beginCapture(settings: previewSettings()))
        controller.present(replacement, on: screenID(), settings: previewSettings(), generation: newGeneration)
        try waitUntil { preparations.count == 2 }
        preparations[0].1("/tmp/stale.png")
        // The dismissed panel is closed but may still be listed in NSApp.windows.
        let panel = try XCTUnwrap(NSApp.windows.compactMap { $0 as? MiniPreviewPanel }
            .first { $0.isVisible && $0.previewView.artifactIDs == ["same"] })
        XCTAssertFalse(panel.previewView.containsPreparedDragPath("/tmp/stale.png"))
        preparations[1].1("/tmp/current.png")
        XCTAssertTrue(panel.previewView.containsPreparedDragPath("/tmp/current.png"))
        XCTAssertTrue(controller.updateSavedPath("/Exports/Café.png", for: replacement))
        XCTAssertEqual(preparations.count, 3)
        XCTAssertEqual(preparations[2].0.savedPath, "/Exports/Café.png")
        XCTAssertFalse(panel.previewView.containsPreparedDragPath("/tmp/current.png"))
        preparations[1].1("/tmp/late-unsaved.png")
        XCTAssertFalse(panel.previewView.containsPreparedDragPath("/tmp/late-unsaved.png"))
        preparations[2].1("/Exports/Café.png")
        XCTAssertTrue(panel.previewView.containsPreparedDragPath("/Exports/Café.png"))
    }

    func testDragCompletionRequiresExactSourceAndClassifiesDestinations() throws {
        _ = NSApplication.shared
        let controller = try presentedController(artifact(id: "source", previewPath: "/source.png"))
        defer { controller.close() }
        func card() throws -> (MiniPreviewPanel, MiniPreviewCardView) {
            // Closed panels stay in NSApp.windows while this test still holds them.
            let panel = try XCTUnwrap(NSApp.windows.compactMap { $0 as? MiniPreviewPanel }
                .first { $0.isVisible && $0.previewView === controller.previewView })
            return (panel, try XCTUnwrap(panel.previewView.subviewsRecursive
                .compactMap { $0 as? MiniPreviewCardView }.first))
        }
        var current = try card()
        current.1.finishDrag(at: NSPoint(x: current.0.frame.midX, y: current.0.frame.midY),
                             operation: .copy)
        XCTAssertEqual(controller.presentedArtifactIDs, ["source"], "own-panel drops are rejected")

        let ownWindow = NSWindow(contentRect: NSRect(x: 20_000, y: 20_000, width: 100, height: 100),
            styleMask: .borderless, backing: .buffered, defer: false)
        ownWindow.isReleasedWhenClosed = false; defer { ownWindow.close() }
        ownWindow.orderFrontRegardless()
        current.1.finishDrag(at: NSPoint(x: ownWindow.frame.midX, y: ownWindow.frame.midY),
                             operation: .copy)
        XCTAssertEqual(controller.presentedArtifactIDs, ["source"], "another app window keeps the card")
        current.1.finishDrag(at: NSPoint(x: -20_000, y: -20_000), operation: [])
        XCTAssertEqual(controller.presentedArtifactIDs, ["source"], "cancelled external drags keep the card")

        // The second replacement has the same path but a new source generation.
        for path in ["/replacement.png", "/replacement.png"] {
            let staleCard = current.1
            staleCard.setFileDragging(true)
            controller.dismiss("source")
            let replacement = artifact(id: "source", previewPath: path)
            var restored = false
            controller.restore(replacement, on: screenID(), settings: previewSettings()) {
                if case .shown = $0 { restored = true }
            }
            try waitUntil { restored }
            staleCard.finishDrag(at: NSPoint(x: -20_000, y: -20_000), operation: .copy)
            XCTAssertEqual(controller.presentedArtifactIDs, ["source"],
                "late completion cannot dismiss a different path or a new generation of the same path")
            current = try card()
            let point = current.1.convert(NSPoint(x: 30, y: 70), to: current.0.previewView)
            let pixel = try XCTUnwrap(pixelColor(in: render(current.0), of: current.0, x: point.x, y: point.y))
            XCTAssertGreaterThan(pixel.alphaComponent, 0.99,
                "a replacement must not inherit concealment from the stale OS drag")
        }

        current.1.finishDrag(at: NSPoint(x: -20_000, y: -20_000), operation: .copy)
        XCTAssertTrue(controller.presentedArtifactIDs.isEmpty,
            "an accepted external copy dismisses the exact source card")
    }

    func testSelfDropRejectionMatchesShippingPresentationAndReducedMotion() throws {
        _ = NSApplication.shared
        let preference = NativeMotion.motionPreference
        defer { NativeMotion.motionPreference = preference }
        for reduced in [false, true] {
            NativeMotion.motionPreference = { reduced }
            let controller = try presentedController(artifact(id: "source", previewPath: "/source.png"))
            defer { controller.close() }
            let panel = try XCTUnwrap(NSApp.windows.compactMap { $0 as? MiniPreviewPanel }
                .first { $0.isVisible && $0.previewView === controller.previewView })
            let card = try XCTUnwrap(panel.previewView.card(for: "source"))
            let layer = try XCTUnwrap(card.layer)
            CATransaction.flush()
            RunLoop.current.run(until: Date().addingTimeInterval(0.6))
            for operation in [NSDragOperation(), .copy] {
                panel.previewView.samplePointer(nil) // A real drag has released the stale-pointer lock.
                card.mouseExited(with: try hoverEvent(for: panel))
                // Hold the real layer clock so slow CI rendering cannot skip a peak.
                let started = layer.convertTime(CACurrentMediaTime(), from: nil)
                layer.speed = 0; layer.timeOffset = started
                card.finishDrag(at: NSPoint(x: panel.frame.midX, y: panel.frame.midY), operation: operation)
                XCTAssertEqual(controller.presentedArtifactIDs, ["source"],
                    "a self-drop is rejected even when the OS refuses COPY")
                XCTAssertTrue(panel.previewView.visibleCardActionTitles.contains("Copy"),
                    "the drop point restores hover without another mouse-enter event")
                XCTAssertFalse(card.mediaHovered, "rejection keeps the restored action overlay's media sharp")
                // Independently sampled from shipping CSS's uneven keyframe times.
                for (seconds, offset) in [(0.042, -12.0), (0.126, 12.0), (0.210, -8.0),
                                          (0.273, 6.0), (0.336, -4.0), (0.378, 4.0), (0.420, 0.0)] {
                    layer.timeOffset = started + seconds
                    let bitmap = try XCTUnwrap(renderPresentation(panel))
                    XCTAssertEqual(try XCTUnwrap(layer.presentation()).transform.m41,
                                   reduced ? 0 : CGFloat(offset), accuracy: 0.2,
                                   "self-drop at \(seconds)s, reduced motion \(reduced), operation \(operation)")
                    if seconds == 0.042 && operation.isEmpty {
                        try write(bitmap, name: "mini-preview-self-drop-\(reduced ? "reduced" : "shake").png")
                    }
                }
            }
            card.finishDrag(at: NSPoint(x: -20_000, y: -20_000), operation: [])
            XCTAssertFalse(panel.previewView.visibleCardActionTitles.contains("Copy"),
                "a cancelled external landing must not leave source hover stuck on")
        }
    }

    func testFileDragConcealsOnlySourceMediaAndRestoresRebuiltSurvivor() throws {
        _ = NSApplication.shared
        let controller = try presentedController(artifact(id: "source", previewPath: "/source.png"))
        defer { controller.close() }
        func panel() throws -> MiniPreviewPanel {
            try XCTUnwrap(NSApp.windows.compactMap { $0 as? MiniPreviewPanel }
                .first { $0.isVisible && $0.previewView === controller.previewView })
        }
        func mediaPixel(_ panel: MiniPreviewPanel) throws -> NSColor {
            let card = try XCTUnwrap(panel.previewView.card(for: "source"))
            let point = card.convert(NSPoint(x: 30, y: 70), to: panel.previewView)
            return try XCTUnwrap(pixelColor(in: render(panel), of: panel, x: point.x, y: point.y))
        }
        let original = try panel()
        let source = try XCTUnwrap(original.previewView.card(for: "source"))
        let visible = try mediaPixel(original)
        source.setFileDragging(true)
        let concealed = try mediaPixel(original)
        XCTAssertNotEqual(concealed, visible, "the capture does not paint under its OS drag ghost")
        XCTAssertLessThan(concealed.alphaComponent, 0.01, "shipping leaves the source media area transparent")
        XCTAssertTrue(source.hasVisibleLabels, "concealing media retains its metadata")
        try write(render(original), name: "mini-preview-file-drag-source-hidden.png")
        controller.setEditorArtifacts(["source"])
        RunLoop.current.run(until: Date().addingTimeInterval(0.3))
        let glowing = try XCTUnwrap(renderPresentation(original))
        let inside = source.convert(NSPoint(x: 4, y: 70), to: original.previewView)
        XCTAssertLessThan(try XCTUnwrap(pixelColor(in: glowing, of: original,
            x: inside.x, y: inside.y)).alphaComponent, 0.01,
            "the editor glow stays outside the concealed source, like CSS box-shadow")
        try write(glowing, name: "mini-preview-file-drag-editor-glow.png")
        controller.setEditorArtifacts([])
        controller.updateSettings(previewSettings())
        let rebuilt = try panel()
        XCTAssertEqual(try mediaPixel(rebuilt), concealed, "settings must not reintroduce the source media")
        source.finishDrag(at: NSPoint(x: -20_000, y: -20_000), operation: [])
        XCTAssertEqual(try mediaPixel(rebuilt), visible, "the original source completion restores its survivor")
        XCTAssertEqual(controller.presentedArtifactIDs, ["source"])
        try write(render(rebuilt), name: "mini-preview-file-drag-source-restored.png")
    }

    func testSelfDropSurvivesSettingsRebuildWithoutReplayingItsShake() throws {
        _ = NSApplication.shared
        let preference = NativeMotion.motionPreference
        NativeMotion.motionPreference = { false }
        defer { NativeMotion.motionPreference = preference }
        let controller = try presentedController(artifact(id: "source", previewPath: "/source.png"))
        defer { controller.close() }
        let original = try XCTUnwrap(NSApp.windows.compactMap { $0 as? MiniPreviewPanel }
            .first { $0.isVisible && $0.previewView === controller.previewView })
        let card = try XCTUnwrap(original.previewView.card(for: "source"))
        RunLoop.current.run(until: Date().addingTimeInterval(0.6))
        let started = CACurrentMediaTime()
        card.finishDrag(at: NSPoint(x: original.frame.midX, y: original.frame.midY), operation: [])
        RunLoop.current.run(until: Date().addingTimeInterval(0.06))
        controller.updateSettings(previewSettings())
        let rebuilt = try XCTUnwrap(NSApp.windows.compactMap { $0 as? MiniPreviewPanel }
            .first { $0.isVisible && $0.previewView === controller.previewView })
        let survivor = try XCTUnwrap(rebuilt.previewView.card(for: "source"))
        survivor.mouseEntered(with: try hoverEvent(for: rebuilt))
        XCTAssertFalse(survivor.mediaHovered, "a surviving rejection keeps media sharp")
        XCTAssertTrue(rebuilt.previewView.visibleCardActionTitles.contains("Copy"))
        let layer = try XCTUnwrap(survivor.layer)
        let peak = layer.convertTime(started, from: nil) + 0.126
        layer.speed = 0; layer.timeOffset = peak
        try write(XCTUnwrap(renderPresentation(rebuilt)), name: "mini-preview-self-drop-after-settings.png")
        XCTAssertEqual(try XCTUnwrap(layer.presentation()).transform.m41, 12, accuracy: 0.6,
            "the original 30% peak survives; replaying or losing the shake changes this pose")
        try waitUntil { survivor.mediaHovered }
    }

    func testPendingFirstDecodeIsInvalidatedWhenHistoryIsCleared() throws {
        _ = NSApplication.shared
        let releaseDecode = DispatchSemaphore(value: 0)
        let decodeReturned = expectation(description: "blocked preview decode returned")
        let image = NSImage(cgImage: PreviewView.fixtureImage(scale: 1),
                            size: NSSize(width: 284, height: 160))
        let controller = MiniPreviewController(tokens: tokens, imageLoader: { _ in
            releaseDecode.wait(); decodeReturned.fulfill(); return image
        })
        let settings = MiniPreviewSettings(enabled: true, placement: "bottom_left",
                                           includeInCaptures: false)
        let pending = artifact(id: "pending", previewPath: "/pending-preview.png")
        let generation = try XCTUnwrap(controller.beginCapture(settings: settings))
        controller.present(pending, on: screenID(), settings: settings, generation: generation)
        controller.reconcileHistory(ids: [])
        releaseDecode.signal()
        wait(for: [decodeReturned], timeout: 5)
        RunLoop.current.run(until: Date().addingTimeInterval(0.05))
        XCTAssertNil(controller.presentedArtifactID)
        XCTAssertFalse(controller.isPanelVisible)
    }

    func testCopyAndSaveRemainAvailableAfterWorkspaceSwitchesToPreferences() throws {
        _ = NSApplication.shared
        let directory = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: directory) }
        let png = try XCTUnwrap(NSBitmapImageRep(cgImage: PreviewView.fixtureImage(scale: 1))
            .representation(using: .png, properties: [:]))
        let imagePath = directory.appendingPathComponent("full.png")
        try png.write(to: imagePath)
        let transport = MiniPreviewActionTransport()
        transport.cleanupGate = DispatchSemaphore(value: 0)
        defer { transport.cleanupGate?.signal() }
        let pasteboard = NSPasteboard(name: NSPasteboard.Name("es.captures.tests.\(UUID())"))
        pasteboard.clearContents()
        let actions = MiniPreviewActions(settingsPath: nil, transport: transport,
            loadPreferences: { try self.preferences() }, pasteboard: { pasteboard })
        actions.configure(historyRoot: directory.path)
        XCTAssertEqual(transport.cleanupStarted.wait(timeout: .now() + 5), .success)
        var workspaceOwner: NSObject? = NSObject()
        workspaceOwner = nil // Preferences replaced the live workspace controller.
        XCTAssertNil(workspaceOwner)
        let captured = artifact(id: "latest", previewPath: imagePath.path,
                                imagePath: imagePath.path)
        let controller = MiniPreviewController(tokens: tokens,
            imageLoader: { _ in self.solidImage(.systemBlue) })
        defer { controller.close() }
        controller.prepareDrag = { _, completion in completion(imagePath.path) }
        let generation = try XCTUnwrap(controller.beginCapture(settings: previewSettings()))
        controller.present(captured, on: screenID(), settings: previewSettings(), generation: generation)
        try waitUntil { controller.previewView?.card(for: captured.id)?.isOutboundFileDragEnabled == true }
        actions.bind(previews: controller)
        controller.copyArtifact = { actions.copy($0) }
        controller.saveArtifact = { actions.save($0) }
        let panel = try XCTUnwrap(controller.previewView?.window as? MiniPreviewPanel)
        let card = try XCTUnwrap(panel.previewView.card(for: captured.id))
        let copy = try XCTUnwrap(card.subviews.compactMap { $0 as? MiniPreviewButton }
            .first { $0.kind == .copy })

        copy.performClick(nil)
        // The persistent queue is held before the real PNG read. A second
        // main action must not be accepted while Copy owns these controls.
        actions.save(captured)
        XCTAssertFalse(copy.isEnabled)
        controller.updateSettings(previewSettings())
        let rebuiltPanel = try XCTUnwrap(controller.previewView?.window as? MiniPreviewPanel)
        let rebuiltCard = try XCTUnwrap(rebuiltPanel.previewView.card(for: captured.id))
        let controls = rebuiltCard.subviews.compactMap { $0 as? MiniPreviewButton }
        let save = try XCTUnwrap(controls.first { $0.kind == .save })
        XCTAssertFalse(save.isEnabled, "Copy's busy state must survive a panel rebuild")
        XCTAssertFalse(try XCTUnwrap(controls.first { $0.kind == .copy }).isEnabled)
        XCTAssertTrue(controls.filter { $0.kind == .edit || $0.kind == .trash }.allSatisfy(\.isEnabled),
            "shipping keeps Edit and removal available during main actions")
        XCTAssertTrue(rebuiltCard.isOutboundFileDragEnabled)
        rebuiltPanel.previewView.samplePointer(nil)
        rebuiltCard.mouseEntered(with: try hoverEvent(for: rebuiltPanel))
        try write(render(rebuiltPanel), name: "mini-preview-copy-busy-after-rebuild.png")
        transport.cleanupGate?.signal()
        LiveCaptureController.flush()
        try waitUntil { pasteboard.data(forType: .png) == png }
        XCTAssertEqual(pasteboard.data(forType: .png), png,
            "Copy must publish the full-resolution PNG, not the thumbnail")
        XCTAssertTrue(save.isEnabled)
        XCTAssertEqual(transport.saveCount, 0, "Save must wait for Copy to finish")
        save.performClick(nil)
        XCTAssertFalse(save.isEnabled)
        LiveCaptureController.flush()
        try waitUntil { save.title == "Saved" }
        XCTAssertTrue(save.isEnabled)
        XCTAssertEqual(transport.saveCount, 1, "a later explicit Save uses the persistent queue")
        XCTAssertEqual(transport.savedRoot, directory.path)
        try write(render(rebuiltPanel), name: "mini-preview-main-action-recovered.png")
    }

    func testLateCopyCannotReleaseSameIDReplacementSaveOrAdoptItsClipboardChip() throws {
        _ = NSApplication.shared
        let path = FileManager.default.temporaryDirectory.appendingPathComponent("\(UUID()).png")
        let png = try XCTUnwrap(NSBitmapImageRep(cgImage: PreviewView.fixtureImage(scale: 1))
            .representation(using: .png, properties: [:]))
        try png.write(to: path)
        defer { try? FileManager.default.removeItem(at: path) }
        let captured = artifact(id: "same-id", previewPath: path.path, imagePath: path.path)
        let controller = try presentedController(captured)
        defer { controller.close() }
        let pasteboard = NSPasteboard(name: NSPasteboard.Name("es.captures.tests.\(UUID())"))
        pasteboard.clearContents()
        let transport = MiniPreviewActionTransport()
        transport.cleanupGate = DispatchSemaphore(value: 0)
        transport.gate = DispatchSemaphore(value: 0)
        defer { transport.cleanupGate?.signal(); transport.gate?.signal() }
        let actions = MiniPreviewActions(settingsPath: nil, transport: transport,
            loadPreferences: { try self.preferences() }, pasteboard: { pasteboard })
        actions.bind(previews: controller); actions.configure(historyRoot: path.deletingLastPathComponent().path)
        XCTAssertEqual(transport.cleanupStarted.wait(timeout: .now() + 5), .success)
        actions.copy(captured)
        controller.dismiss(captured.id, exit: nil)
        controller.restore(captured, on: screenID(), settings: previewSettings()) { _ in }
        try waitUntil { controller.isPanelVisible }
        let replacement = try XCTUnwrap(controller.previewView?.card(for: captured.id))
        let controls = replacement.subviews.compactMap { $0 as? MiniPreviewButton }
        let save = try XCTUnwrap(controls.first { $0.kind == .save })
        let copy = try XCTUnwrap(controls.first { $0.kind == .copy })
        XCTAssertTrue(save.isEnabled, "new generations do not inherit an old action lock")
        actions.save(captured)
        transport.cleanupGate?.signal()
        XCTAssertEqual(transport.started.wait(timeout: .now() + 5), .success)
        try waitUntil { pasteboard.data(forType: .png) == png }
        XCTAssertFalse(save.isEnabled, "the old Copy cannot unlock the new Save")
        let panel = try XCTUnwrap(controller.previewView?.window as? MiniPreviewPanel)
        panel.previewView.samplePointer(nil)
        replacement.mouseEntered(with: try hoverEvent(for: panel))
        XCTAssertFalse(copy.isHidden, "a stale Copy cannot claim the replacement's clipboard chip")
        transport.gate?.signal()
        LiveCaptureController.flush()
        try waitUntil { save.title == "Saved" }
        XCTAssertTrue(save.isEnabled)
    }

    func testSavedActionRevealsExactUnicodePathWithoutSavingAgain() throws {
        let path = "/Exports/Client shots/Café 東京.png"
        let transport = MiniPreviewActionTransport()
        var checkedPath: String?
        var revealed: [URL] = []
        let actions = MiniPreviewActions(settingsPath: nil, transport: transport,
            loadPreferences: { try self.preferences() }, fileExists: {
                checkedPath = $0; return true
            }, revealFiles: { revealed = $0 })

        actions.save(artifact(id: "stable-id", previewPath: "/preview.png", savedPath: path))
        LiveCaptureController.flush()
        try waitUntil { !revealed.isEmpty }
        XCTAssertEqual(checkedPath, path)
        XCTAssertEqual(revealed, [URL(fileURLWithPath: path)])
        XCTAssertEqual(transport.saveCount, 0, "Reveal must not create another export")
    }

    func testClipboardOwnerShowsChipHidesCopyAndReleasesOnExternalWrite() throws {
        _ = NSApplication.shared
        let controller = try presentedController(artifact(id: "owned", previewPath: "/owned.png"))
        defer { controller.close() }
        XCTAssertEqual(controller.metadataText(for: "owned"), "800 × 600 · 0 B")
        let panel = try XCTUnwrap(NSApp.windows.compactMap { $0 as? MiniPreviewPanel }
            .first { $0.isVisible && $0.previewView.artifactIDs == ["owned"] })
        let card = try XCTUnwrap(panel.previewView.subviewsRecursive
            .compactMap { $0 as? MiniPreviewCardView }.first)
        let chip = try XCTUnwrap(card.subviews.compactMap { $0 as? MiniPreviewClipboardChip }.first)
        let buttons = card.subviews.compactMap { $0 as? MiniPreviewButton }
        let copy = try XCTUnwrap(buttons.first { $0.title == "Copy" })
        let save = try XCTUnwrap(buttons.first { $0.title == "Save file" })
        let pairedSaveY = save.frame.minY
        XCTAssertTrue(chip.isHidden)

        let pasteboard = NSPasteboard(name: NSPasteboard.Name("es.captures.tests.\(UUID())"))
        pasteboard.clearContents(); pasteboard.setString("capture", forType: .string)
        controller.recordClipboardCopy(artifactID: "owned", pasteboard: pasteboard)
        XCTAssertTrue(controller.isClipboardCurrent(for: "owned"))
        XCTAssertFalse(chip.isHidden)
        XCTAssertTrue(copy.isHidden)
        XCTAssertEqual(save.frame.minY, (card.bounds.height - 32) / 2,
            "the remaining action centers when Copy hides")

        pasteboard.clearContents(); pasteboard.setString("another app", forType: .string)
        controller.refreshClipboardOwner()
        XCTAssertFalse(controller.isClipboardCurrent(for: "owned"))
        XCTAssertTrue(chip.isHidden)
        XCTAssertEqual(save.frame.minY, pairedSaveY)
    }

    func testClipboardChipDoesNotReplayWhenAnotherCardArrivesOrSettingsRebuild() throws {
        _ = NSApplication.shared
        let preference = NativeMotion.motionPreference
        NativeMotion.motionPreference = { false }
        defer { NativeMotion.motionPreference = preference }
        let controller = try presentedController(artifact(id: "owned", previewPath: "/owned.png"))
        defer { controller.close() }
        let pasteboard = NSPasteboard(name: NSPasteboard.Name("es.captures.tests.\(UUID())"))
        pasteboard.clearContents(); pasteboard.setString("capture", forType: .string)
        controller.recordClipboardCopy(artifactID: "owned", pasteboard: pasteboard)
        CATransaction.flush()
        // Opacity has reached 1, but the 1.15-second scale bounce is still running.
        RunLoop.current.run(until: Date().addingTimeInterval(0.6))
        let original = try XCTUnwrap(NSApp.windows.compactMap { $0 as? MiniPreviewPanel }
            .first { $0.isVisible && $0.previewView === controller.previewView })
        let chip = try XCTUnwrap(original.previewView.card(for: "owned")?.subviews
            .compactMap { $0 as? MiniPreviewClipboardChip }.first)
        XCTAssertEqual(try XCTUnwrap(chip.layer?.presentation()).opacity, 1, accuracy: 0.01)

        var restored = false
        controller.restore(artifact(id: "other", previewPath: "/other.png"), on: screenID(),
                           settings: previewSettings()) { _ in restored = true }
        try waitUntil { restored }
        let arrival = try XCTUnwrap(NSApp.windows.compactMap { $0 as? MiniPreviewPanel }
            .first { $0.isVisible && $0.previewView === controller.previewView })
        let continued = try XCTUnwrap(arrival.previewView.card(for: "owned")?.subviews
            .compactMap { $0 as? MiniPreviewClipboardChip }.first)
        // Hold the sampled pose before slow bitmap rendering can age the bounce.
        let continuedLayer = try XCTUnwrap(continued.layer)
        let continuedTime = continuedLayer.convertTime(CACurrentMediaTime(), from: nil) + 0.03
        continuedLayer.speed = 0; continuedLayer.timeOffset = continuedTime
        try write(XCTUnwrap(renderPresentation(arrival)), name: "mini-preview-clipboard-after-arrival.png")
        let continuedPose = try XCTUnwrap(continued.layer?.presentation())
        XCTAssertEqual(continuedPose.opacity, 1, accuracy: 0.01,
            "an unrelated arrival must not fade the surviving clipboard badge in again")
        XCTAssertGreaterThan(continuedPose.transform.m11, 0.985)
        XCTAssertLessThan(continuedPose.transform.m11, 0.999,
            "the original scale bounce must finish, not snap to its resting scale")
        XCTAssertTrue(controller.isClipboardCurrent(for: "owned"))
        XCTAssertFalse(controller.isClipboardCurrent(for: "other"))

        RunLoop.current.run(until: Date().addingTimeInterval(0.7))
        controller.updateSettings(previewSettings())
        let settled = try XCTUnwrap(NSApp.windows.compactMap { $0 as? MiniPreviewPanel }
            .first { $0.isVisible && $0.previewView === controller.previewView })
        let settledChip = try XCTUnwrap(settled.previewView.card(for: "owned")?.subviews
            .compactMap { $0 as? MiniPreviewClipboardChip }.first)
        try write(XCTUnwrap(renderPresentation(settled)), name: "mini-preview-clipboard-after-settings.png")
        XCTAssertEqual(try XCTUnwrap(settledChip.layer?.presentation()).opacity, 1, accuracy: 0.01,
            "a settled badge must remain visible through a settings rebuild")

        pasteboard.clearContents(); pasteboard.setString("other capture", forType: .string)
        controller.recordClipboardCopy(artifactID: "other", pasteboard: pasteboard)
        let fresh = try XCTUnwrap(settled.previewView.card(for: "other")?.subviews
            .compactMap { $0 as? MiniPreviewClipboardChip }.first)
        let freshLayer = try XCTUnwrap(fresh.layer)
        let freshTime = freshLayer.convertTime(CACurrentMediaTime(), from: nil) + 0.03
        freshLayer.speed = 0; freshLayer.timeOffset = freshTime
        try write(XCTUnwrap(renderPresentation(settled)), name: "mini-preview-clipboard-new-owner.png")
        XCTAssertFalse(controller.isClipboardCurrent(for: "owned"))
        XCTAssertTrue(controller.isClipboardCurrent(for: "other"))
        XCTAssertLessThan(try XCTUnwrap(fresh.layer?.presentation()).opacity, 0.9,
            "a new clipboard owner must still play its entrance")
    }

    func testSaveUpdatesExistingCardThenRevealAndMissingExportNeverSaveAgain() throws {
        _ = NSApplication.shared
        let image = solidImage(.systemBlue)
        let controller = MiniPreviewController(tokens: tokens, imageLoader: { _ in image })
        defer { controller.close() }
        let settings = MiniPreviewSettings(enabled: true, placement: "bottom_right",
                                           includeInCaptures: false)
        let captured = artifact(id: "save-reveal", previewPath: "/save-reveal-preview.png")
        let generation = try XCTUnwrap(controller.beginCapture(settings: settings))
        controller.present(captured, on: screenID(), settings: settings, generation: generation)
        try waitUntil { controller.isPanelVisible }
        let panel = try XCTUnwrap(NSApp.windows.compactMap { $0 as? MiniPreviewPanel }
            .first { $0.previewView.artifactIDs == [captured.id] })
        let button = try XCTUnwrap(panel.previewView.subviewsRecursive.compactMap { $0 as? MiniPreviewButton }
            .first { $0.title == "Save file" })
        let originalFrame = panel.frame
        let transport = MiniPreviewActionTransport()
        transport.gate = DispatchSemaphore(value: 0)
        var exists = true
        var revealed: [URL] = []
        let actions = MiniPreviewActions(settingsPath: nil, transport: transport,
            loadPreferences: { try self.preferences() }, fileExists: { path in
                XCTAssertFalse(Thread.isMainThread)
                XCTAssertEqual(path, "/exports/latest.png")
                return exists
            }, revealFiles: { revealed.append(contentsOf: $0) })
        actions.bind(previews: controller); actions.configure(historyRoot: "/History")
        controller.saveArtifact = { actions.save($0) }
        button.performClick(nil)
        XCTAssertEqual(transport.started.wait(timeout: .now() + 5), .success)
        button.performClick(nil)
        actions.trash(captured)
        XCTAssertEqual(transport.saveCount, 1, "in-flight Save must not dispatch twice")
        XCTAssertEqual(transport.trashCount, 0, "Trash must not race an accepted Save")
        transport.gate?.signal()
        LiveCaptureController.flush()
        try waitUntil { button.title == "Show in Folder" }
        XCTAssertEqual(panel.frame, originalFrame)
        XCTAssertTrue(panel.isVisible, "Save updates the existing panel")
        XCTAssertEqual(button.accessibilityLabel(), "Show in Folder")
        controller.refreshArtifacts([captured])
        XCTAssertEqual(button.title, "Show in Folder", "an older unsaved snapshot cannot erase export state")
        button.performClick(nil)
        try waitUntil { revealed.count == 1 }
        XCTAssertEqual(revealed, [URL(fileURLWithPath: "/exports/latest.png")])
        exists = false
        button.performClick(nil)
        try waitUntil { controller.statusText(for: captured.id) == "Export missing" }
        let status = try XCTUnwrap(panel.previewView.subviewsRecursive.compactMap { $0 as? NSTextField }
            .first { $0.stringValue == "Export missing" })
        XCTAssertLessThanOrEqual(status.attributedStringValue.size().width, status.bounds.width,
                                 "the missing-export status must fit without truncation")
        XCTAssertEqual(revealed.count, 1)
        XCTAssertEqual(transport.saveCount, 1)
        XCTAssertEqual(button.title, "Show in Folder")
        XCTAssertEqual(controller.presentedArtifactIDs, [captured.id])
        try write(render(panel), name: "mini-preview-reveal-missing-export.png")
    }

    func testSavedFeedbackSurvivesAnArrivalWithoutExtendingItsDeadline() throws {
        _ = NSApplication.shared
        let captured = artifact(id: "saved", previewPath: "/saved-preview.png")
        let controller = try presentedController(captured)
        defer { controller.close() }
        let transport = MiniPreviewActionTransport()
        let actions = MiniPreviewActions(settingsPath: nil, transport: transport,
            loadPreferences: { try self.preferences() })
        actions.bind(previews: controller); actions.configure(historyRoot: "/History")
        actions.save(captured)
        LiveCaptureController.flush()
        try waitUntil {
            controller.previewView?.card(for: captured.id)?.subviews
                .compactMap { $0 as? MiniPreviewButton }.contains { $0.title == "Saved" } == true
        }
        let confirmedAt = Date()
        RunLoop.current.run(until: Date().addingTimeInterval(0.35))
        var restored = false
        controller.restore(artifact(id: "other", previewPath: "/other.png"), on: screenID(),
                           settings: previewSettings()) { _ in restored = true }
        try waitUntil { restored }
        let view = try XCTUnwrap(controller.previewView)
        let card = try XCTUnwrap(view.card(for: captured.id))
        let button = try XCTUnwrap(card.subviews.compactMap { $0 as? MiniPreviewButton }
            .first { $0.kind == .check || $0.kind == .folder })
        XCTAssertEqual(button.title, "Saved", "an unrelated arrival must not truncate Save confirmation")
        let panel = try XCTUnwrap(view.window as? MiniPreviewPanel)
        card.mouseEntered(with: try hoverEvent(for: panel))
        try write(render(panel), name: "mini-preview-saved-after-arrival.png")
        RunLoop.current.run(until: confirmedAt.addingTimeInterval(1.1))
        XCTAssertLessThan(Date().timeIntervalSince(confirmedAt), 1.25,
            "the expiry probe must precede a wrongly restarted 1.35-second deadline")
        XCTAssertEqual(button.title, "Show in Folder", "a rebuild must not restart the one-second deadline")
        controller.updateSettings(previewSettings())
        let settled = try XCTUnwrap(controller.previewView?.card(for: captured.id))
        XCTAssertFalse(settled.subviews.compactMap { $0 as? MiniPreviewButton }.contains { $0.title == "Saved" })
    }

    func testSaveFailureStaysSaveAndCompletionAfterDismissalIsRejected() throws {
        _ = NSApplication.shared
        let image = solidImage(.systemBlue)
        let controller = MiniPreviewController(tokens: tokens, imageLoader: { _ in image })
        let settings = MiniPreviewSettings(enabled: true, placement: "bottom_right",
                                           includeInCaptures: false)
        let captured = artifact(id: "stable", previewPath: "/stable-preview.png")
        let generation = try XCTUnwrap(controller.beginCapture(settings: settings))
        controller.present(captured, on: screenID(), settings: settings, generation: generation)
        try waitUntil { controller.isPanelVisible }

        let failedTransport = MiniPreviewActionTransport(); failedTransport.fail = true
        let failedActions = MiniPreviewActions(settingsPath: nil, transport: failedTransport,
                                               loadPreferences: { try self.preferences() })
        failedActions.bind(previews: controller); failedActions.configure(historyRoot: "/History")
        failedActions.save(captured)
        let busyControls = try XCTUnwrap(controller.previewView?.card(for: captured.id))
            .subviews.compactMap { $0 as? MiniPreviewButton }
        XCTAssertTrue(busyControls.filter { $0.kind == .copy || $0.kind == .save }
            .allSatisfy { !$0.isEnabled })
        LiveCaptureController.flush()
        try waitUntil { controller.statusText(for: captured.id) == "Save failed" }
        XCTAssertTrue(busyControls.allSatisfy(\.isEnabled), "a failed Save must release the controls")

        var restored = false
        controller.restore(artifact(id: "other", previewPath: "/other.png"), on: screenID(),
                           settings: settings) { _ in restored = true }
        try waitUntil { restored }
        XCTAssertEqual(controller.statusText(for: captured.id), "Save failed",
            "an unrelated arrival must not erase an action failure")
        let rebuilt = try XCTUnwrap(controller.previewView?.window as? MiniPreviewPanel)
        try write(render(rebuilt), name: "mini-preview-save-failure-after-arrival.png")
        controller.dismiss("other", exit: nil)
        XCTAssertEqual(controller.statusText(for: captured.id), "Save failed",
            "removing a sibling must not erase the survivor's action failure")

        let blockedTransport = MiniPreviewActionTransport()
        blockedTransport.gate = DispatchSemaphore(value: 0)
        let blockedActions = MiniPreviewActions(settingsPath: nil, transport: blockedTransport,
                                                loadPreferences: { try self.preferences() })
        blockedActions.bind(previews: controller); blockedActions.configure(historyRoot: "/History")
        blockedActions.save(captured)
        XCTAssertEqual(blockedTransport.started.wait(timeout: .now() + 5), .success)
        controller.dismiss(captured.id)
        blockedTransport.gate?.signal()
        LiveCaptureController.flush()
        RunLoop.current.run(until: Date().addingTimeInterval(0.05))
        XCTAssertEqual(controller.presentedArtifactIDs, [],
            "a dismissed card must not be recreated by a late save completion")
    }

    func testTrashSendsExactUnicodeSnapshotAndSerializesWithSave() throws {
        _ = NSApplication.shared
        let savedPath = "/Exports/客户/Café 東京.png"
        let captured = artifact(id: "截图-id", previewPath: "/preview.png", savedPath: savedPath)
        let controller = try presentedController(captured)
        defer { controller.close() }
        let transport = MiniPreviewActionTransport()
        transport.gate = DispatchSemaphore(value: 0)
        let actions = MiniPreviewActions(settingsPath: nil, transport: transport,
            loadPreferences: { try self.preferences() })
        actions.bind(previews: controller); actions.configure(historyRoot: "/历史/History")

        actions.trash(captured)
        XCTAssertEqual(controller.presentedArtifactIDs, [],
                       "shipping dissolves the card before its Trash request")
        if NativeMotion.reduceMotion {
            XCTAssertFalse(controller.isTransitioning, "Reduce Motion removes the card without a dust delay")
        } else {
            XCTAssertTrue(controller.isTransitioning, "the dust holds the card's slot")
            XCTAssertEqual(transport.requestCount, 0, "Trash waits for the dust")
        }
        XCTAssertEqual(transport.started.wait(timeout: .now() + 5), .success)
        actions.save(captured)
        XCTAssertEqual(transport.saveCount, 0, "Save and Trash share the artifact busy guard")
        XCTAssertEqual(transport.lastRequest?["operation"] as? String, "trash_preview")
        XCTAssertEqual(transport.lastRequest?["root"] as? String, "/历史/History")
        XCTAssertEqual(transport.lastRequest?["id"] as? String, "截图-id")
        XCTAssertEqual(transport.lastRequest?["saved_path"] as? String, savedPath)
        transport.gate?.signal(); LiveCaptureController.flush()
        try waitUntil { controller.presentedArtifactIDs.isEmpty }
        XCTAssertEqual(transport.trashCount, 1)
    }

    func testSecondDustDismissalBeforeSettleKeepsTheTopCardInItsPresentationSlot() throws {
        _ = NSApplication.shared
        let motionPreference = NativeMotion.motionPreference
        NativeMotion.motionPreference = { false }
        defer { NativeMotion.motionPreference = motionPreference }
        let controller = MiniPreviewController(tokens: tokens, imageLoader: { path in
            let colors = ["top.png": NSColor.systemBlue, "middle.png": NSColor.systemOrange,
                          "bottom.png": NSColor.systemPurple]
            return self.solidImage(colors[(path as NSString).lastPathComponent] ?? .systemGray)
        })
        defer { controller.close() }
        let settings = previewSettings()
        for id in ["top", "middle", "bottom"] {
            let generation = try XCTUnwrap(controller.beginCapture(settings: settings))
            controller.present(artifact(id: id, previewPath: "/\(id).png"), on: screenID(),
                               settings: settings, generation: generation)
            try waitUntil { controller.decodedArtifactIDs.contains(id) && controller.isPanelVisible }
        }
        let view = try XCTUnwrap(controller.previewView)
        let panel = try XCTUnwrap(view.window as? MiniPreviewPanel)
        let top = try XCTUnwrap(view.subviewsRecursive.compactMap { $0 as? MiniPreviewCardView }
            .first { $0.artifactID == "top" })
        let layer = try XCTUnwrap(top.layer)
        let original = layer.position.y
        func presentationY() -> CGFloat { (layer.presentation() ?? layer).position.y }
        let originalPanel = panel.frame
        XCTAssertEqual(controller.dismiss("bottom", exit: .dust), 2.9, accuracy: 0.01,
                       "exercise actual dust, not the short no-Metal fallback")
        // The source fades in 550 ms; survivors wait 1800 ms before sliding.
        RunLoop.current.run(until: Date().addingTimeInterval(0.8))
        XCTAssertEqual(presentationY(), original, accuracy: 0.1)
        try write(render(panel), name: "mini-preview-reentrant-before-second-dust.png")
        XCTAssertEqual(controller.dismiss("middle", exit: .dust), 2.9, accuracy: 0.01)
        XCTAssertTrue(controller.previewView === view, "the second exit must retain the presentation surface")
        XCTAssertEqual(panel.frame, originalPanel, "held slots must not resize/reposition the native window")
        // Cross the FIRST settle deadline, but stay within the SECOND hold.
        // Reading the model frame here would test the target, not what paints.
        RunLoop.current.run(until: Date().addingTimeInterval(1.35))
        XCTAssertEqual(presentationY(), original, accuracy: 0.1,
                       "the first settle must not drift during the second dust hold")
        try write(render(panel), name: "mini-preview-reentrant-after-second-dust.png")
        try waitUntil { !controller.isTransitioning }
        XCTAssertEqual(controller.presentedArtifactIDs, ["top"])
        let final = try XCTUnwrap(controller.previewView)
        XCTAssertEqual(final.artifactIDs, ["top"])
        try write(render(try XCTUnwrap(final.window as? MiniPreviewPanel)),
                  name: "mini-preview-reentrant-settled.png")
    }

    func testArrivalsRetainDustAndCloseSlotsWithoutRestartingTheirDeadlineAtEveryCorner() throws {
        _ = NSApplication.shared
        let preference = NativeMotion.motionPreference
        NativeMotion.motionPreference = { false }
        defer { NativeMotion.motionPreference = preference }
        for placement in ["bottom_left", "bottom_right", "top_left", "top_right"] {
            let controller = MiniPreviewController(tokens: tokens, imageLoader: { _ in
                NSImage(cgImage: PreviewView.fixtureImage(scale: 1), size: NSSize(width: 284, height: 160))
            })
            defer { controller.close() }
            let settings = MiniPreviewSettings(enabled: true, placement: placement, includeInCaptures: false)
            func capture(_ id: String) throws {
                let generation = try XCTUnwrap(controller.beginCapture(settings: settings))
                controller.present(artifact(id: id, previewPath: "/\(id).png"), on: screenID(),
                                   settings: settings, generation: generation)
                try waitUntil { controller.decodedArtifactIDs.contains(id) && controller.isPanelVisible }
            }
            for id in ["older", "dust", "close"] { try capture(id) }
            controller.recordCopyResult(artifactID: "older", succeeded: false)
            XCTAssertEqual(controller.dismiss("dust", exit: .dust), 2.9, accuracy: 0.01)
            let started = CACurrentMediaTime()
            XCTAssertGreaterThan(controller.dismiss("close"), 0)
            let chips = try XCTUnwrap(controller.previewView).dustChipCount
            XCTAssertGreaterThan(chips, 0, "exercise real dust, not the fallback")
            RunLoop.current.run(until: Date().addingTimeInterval(0.4))
            try capture("incoming")
            let view = try XCTUnwrap(controller.previewView)
            XCTAssertEqual(view.renderedArtifactIDs, ["older", "dust", "close", "incoming"])
            XCTAssertEqual(view.dustChipCount, chips, "the existing chips must survive the arrival")
            for overlay in view.subviewsRecursive.compactMap({ $0 as? PreviewMotionOverlay }) {
                XCTAssertTrue(overlay.frame.minX.isFinite && overlay.frame.minY.isFinite,
                              "hidden/reattaching layer positions must not move dust to infinity")
                XCTAssertTrue(overlay.content.superlayer === overlay.layer, "retained dust must still paint")
            }
            XCTAssertTrue(controller.isTransitioning)
            XCTAssertEqual(controller.warningText(for: "older"), "Clipboard unavailable")
            for id in ["dust", "close"] {
                let card = try XCTUnwrap(view.card(for: id))
                XCTAssertNil(card.hitTest(.zero), "ghosts cannot intercept input")
                for button in card.subviews.compactMap({ $0 as? MiniPreviewButton }) {
                    XCTAssertFalse(button.acceptsFirstResponder)
                    XCTAssertFalse(button.canBecomeKeyView, "Tab must skip every exiting control")
                }
            }
            try write(render(try XCTUnwrap(view.window as? MiniPreviewPanel)),
                      name: "mini-preview-overlap-arrival-\(placement).png")
            if let bitmap = try renderPresentation(try XCTUnwrap(view.window as? MiniPreviewPanel)) {
                try write(bitmap, name: "mini-preview-overlap-presentation-\(placement).png")
            }
            RunLoop.current.run(until: Date().addingTimeInterval(0.4))
            try capture("again")
            XCTAssertEqual(try XCTUnwrap(controller.previewView).dustChipCount, chips)
            try waitUntil { !controller.isTransitioning }
            XCTAssertLessThan(CACurrentMediaTime() - started, 3.4, "arrivals must not restart a 2.9 s exit")
            XCTAssertEqual(controller.presentedArtifactIDs, ["older", "incoming", "again"])
            let final = try XCTUnwrap(controller.previewView)
            XCTAssertEqual(final.renderedArtifactIDs, controller.presentedArtifactIDs)
            XCTAssertEqual(final.dustChipCount, 0)
            try write(render(try XCTUnwrap(final.window as? MiniPreviewPanel)),
                      name: "mini-preview-overlap-settled-\(placement).png")
        }
    }

    func testRestoreCancelsOnlyItsOwnExitAndRetainsPendingTrashRecovery() throws {
        _ = NSApplication.shared
        let preference = NativeMotion.motionPreference
        NativeMotion.motionPreference = { false }
        defer { NativeMotion.motionPreference = preference }
        let controller = MiniPreviewController(tokens: tokens, imageLoader: { _ in self.solidImage(.systemBlue) })
        defer { controller.close() }
        let settings = previewSettings()
        let captures = ["older", "restore", "trash"].map {
            artifact(id: $0, previewPath: "/\($0).png", savedPath: "/exports/\($0).png")
        }
        for captured in captures {
            controller.restore(captured, on: screenID(), settings: settings) { _ in }
            try waitUntil { controller.decodedArtifactIDs.contains(captured.id) }
        }
        XCTAssertEqual(controller.dismiss("restore", exit: .dust), 2.9, accuracy: 0.01)
        XCTAssertEqual(try XCTUnwrap(controller.beginTrash("trash")), 2.9, accuracy: 0.01)
        controller.restore(captures[1], on: screenID(), settings: settings) { _ in }
        try waitUntil { controller.decodedArtifactIDs.contains("restore") }
        let view = try XCTUnwrap(controller.previewView)
        XCTAssertEqual(controller.presentedArtifactIDs, ["older", "restore"])
        XCTAssertEqual(view.renderedArtifactIDs, ["older", "trash", "restore"])
        XCTAssertFalse(try XCTUnwrap(view.card(for: "restore")).isExiting)
        XCTAssertTrue(try XCTUnwrap(view.card(for: "trash")).isExiting)
        XCTAssertGreaterThan(view.dustChipCount, 0)
        XCTAssertTrue(controller.isTransitioning)
        XCTAssertTrue(controller.restoreTrashed("trash", status: "Trash failed"))
        XCTAssertEqual(controller.presentedArtifactIDs, ["older", "trash", "restore"],
                       "Trash restores slot 1, recorded after the earlier dismissal; History Restore is new")
        XCTAssertEqual(controller.statusText(for: "trash"), "Trash failed")
        XCTAssertFalse(controller.isTransitioning)
        XCTAssertEqual(try XCTUnwrap(controller.previewView).dustChipCount, 0)
        XCTAssertTrue(controller.isPanelVisible)
    }

    func testArrivalCancelsClearSlotsButNotAnEarlierSavedDelete() throws {
        _ = NSApplication.shared
        let preference = NativeMotion.motionPreference
        NativeMotion.motionPreference = { false }
        defer { NativeMotion.motionPreference = preference }
        let controller = MiniPreviewController(tokens: tokens, imageLoader: { _ in self.solidImage(.systemOrange) })
        defer { controller.close() }
        let settings = previewSettings()
        for id in ["clear", "saved"] {
            controller.restore(artifact(id: id, previewPath: "/\(id).png", savedPath: "/exports/\(id).png"),
                               on: screenID(), settings: settings) { _ in }
            try waitUntil { controller.decodedArtifactIDs.contains(id) }
        }
        XCTAssertEqual(try XCTUnwrap(controller.beginTrash("saved")), 2.9, accuracy: 0.01)
        controller.clearAll()
        controller.restore(artifact(id: "incoming", previewPath: "/incoming.png"),
                           on: screenID(), settings: settings) { _ in }
        try waitUntil { controller.decodedArtifactIDs == ["incoming"] && controller.isPanelVisible }
        let view = try XCTUnwrap(controller.previewView)
        XCTAssertEqual(view.renderedArtifactIDs, ["saved", "incoming"])
        XCTAssertGreaterThan(view.dustChipCount, 0)
        XCTAssertTrue(view.stackToolbarButtons.isEmpty, "one live card must not inherit a ghost toolbar")
        controller.finishTrash("saved")
        try waitUntil { !controller.isTransitioning }
        XCTAssertEqual(controller.presentedArtifactIDs, ["incoming"])
        XCTAssertEqual(try XCTUnwrap(controller.previewView).renderedArtifactIDs, ["incoming"])
        XCTAssertFalse(controller.restoreTrashed("saved", status: "must not return"))
    }

    func testReducedMotionArrivalDoesNotRetainGhostsOrHoldSlots() throws {
        _ = NSApplication.shared
        let preference = NativeMotion.motionPreference
        NativeMotion.motionPreference = { true }
        defer { NativeMotion.motionPreference = preference }
        let controller = MiniPreviewController(tokens: tokens, imageLoader: { _ in self.solidImage(.systemPurple) })
        defer { controller.close() }
        for id in ["old", "new"] {
            controller.restore(artifact(id: id, previewPath: "/\(id).png"),
                               on: screenID(), settings: previewSettings()) { _ in }
            try waitUntil { controller.decodedArtifactIDs.contains(id) }
            if id == "old" { XCTAssertEqual(controller.dismiss(id, exit: .dust), 0) }
        }
        XCTAssertFalse(controller.isTransitioning)
        XCTAssertEqual(controller.presentedArtifactIDs, ["new"])
        XCTAssertEqual(try XCTUnwrap(controller.previewView).renderedArtifactIDs, ["new"])
        XCTAssertEqual(try XCTUnwrap(controller.previewView).dustChipCount, 0)
    }

    func testZeroPresentationRetargetsBothHeldSlotsForEitherAnchor() throws {
        _ = NSApplication.shared
        let motionPreference = NativeMotion.motionPreference
        NativeMotion.motionPreference = { false }
        defer { NativeMotion.motionPreference = motionPreference }
        for topAnchor in [false, true] {
            let panel = fixturePanel(ids: ["top", "middle", "bottom"], images: [
                "top": solidImage(.systemBlue), "middle": solidImage(.systemOrange),
                "bottom": solidImage(.systemPurple)], topAnchor: topAnchor)
            defer { panel.close() }
            panel.orderFrontRegardless(); panel.display()
            let view = panel.previewView
            let top = try XCTUnwrap(view.card(for: "top"))
            let middle = try XCTUnwrap(view.card(for: "middle"))
            let layer = try XCTUnwrap(top.layer)
            let original = layer.position.y
            let middleOrigin = middle.frame.minY
            view.settleSurvivors(into: "bottom", after: 1.8)
            let firstMove = try XCTUnwrap(layer.animation(forKey: "preview-settle-position"))
            CATransaction.flush()
            RunLoop.current.run(until: Date().addingTimeInterval(0.05))
            // A valid rendered zero while the model already targets one slot.
            XCTAssertEqual((layer.presentation() ?? layer).position.y, original, accuracy: 0.1)
            XCTAssertEqual(abs(layer.position.y - original), 184, accuracy: 0.1)
            XCTAssertNotNil(view.playExit(for: "middle", kind: .dust))
            XCTAssertEqual(middle.frame.minY, middleOrigin, accuracy: 0.1,
                           "the exiting card must freeze its presentation too")
            view.settleSurvivors(into: "middle", after: 0.45)
            let move = try XCTUnwrap(layer.animation(forKey: "preview-settle-position") as? CABasicAnimation)
            let from = try XCTUnwrap(move.fromValue as? NSValue).pointValue.y
            let to = try XCTUnwrap(move.toValue as? NSValue).pointValue.y
            XCTAssertEqual(from, original, accuracy: 0.1)
            XCTAssertEqual(to - from, topAnchor ? -368 : 368, accuracy: 0.1)
            XCTAssertEqual(move.beginTime, firstMove.beginTime, accuracy: 0.001,
                           "a shorter second exit must not advance the first dust deadline")
            RunLoop.current.run(until: Date().addingTimeInterval(0.1))
            XCTAssertEqual((layer.presentation() ?? layer).position.y, original, accuracy: 0.1)
        }
    }

    func testTrashFailureCanRetryAndKeepsReadableErrorDetail() throws {
        _ = NSApplication.shared
        let captured = artifact(id: "retry", previewPath: "/preview.png",
                                savedPath: "/Exports/retry.png")
        let controller = try presentedController(captured)
        defer { controller.close() }
        let transport = MiniPreviewActionTransport(); transport.fail = true
        let actions = MiniPreviewActions(settingsPath: nil, transport: transport,
            loadPreferences: { try self.preferences() })
        actions.bind(previews: controller); actions.configure(historyRoot: "/History")
        actions.trash(captured); LiveCaptureController.flush()
        try waitUntil { controller.statusText(for: captured.id) == "Trash failed" }
        // The failed card returns in the rebuilt stack's panel; an earlier
        // panel can linger in NSApp.windows, so read the one on screen now.
        let view = try XCTUnwrap(controller.previewView)
        XCTAssertEqual(view.artifactIDs, [captured.id])
        let panel = try XCTUnwrap(view.window as? MiniPreviewPanel)
        let status = try XCTUnwrap(view.subviewsRecursive.compactMap { $0 as? NSTextField }
            .first { $0.stringValue == "Trash failed" })
        XCTAssertFalse(try XCTUnwrap(status.toolTip).isEmpty)
        XCTAssertTrue(panel.isVisible)
        try write(render(panel), name: "mini-preview-trash-error.png")
        transport.fail = false
        actions.trash(captured); LiveCaptureController.flush()
        try waitUntil { controller.presentedArtifactIDs.isEmpty }
        try waitUntil { transport.trashCount == 2 }
    }

    func testTrashRejectsStaleSavedPathAndMalformedResponse() throws {
        _ = NSApplication.shared
        let original = artifact(id: "snapshot", previewPath: "/preview.png", savedPath: "/old.png")
        let controller = try presentedController(original)
        defer { controller.close() }
        let currentExport = FileManager.default.temporaryDirectory.appendingPathComponent("new-\(UUID()).png")
        try Data([1, 2, 3]).write(to: currentExport)
        defer { try? FileManager.default.removeItem(at: currentExport) }
        let transport = MiniPreviewActionTransport()
        let actions = MiniPreviewActions(settingsPath: nil, transport: transport,
            loadPreferences: { try self.preferences() })
        actions.bind(previews: controller); actions.configure(historyRoot: "/History")
        controller.refreshArtifacts([
            artifact(id: "snapshot", previewPath: "/preview.png", savedPath: currentExport.path),
        ])
        actions.trash(original)
        XCTAssertEqual(transport.requestCount, 0, "stale saved-path snapshots cannot dispatch")

        transport.malformed = true
        let current = artifact(id: "snapshot", previewPath: "/preview.png", savedPath: currentExport.path)
        actions.trash(current); LiveCaptureController.flush()
        try waitUntil { controller.statusText(for: current.id) == "Trash failed" }
        XCTAssertEqual(controller.presentedArtifactIDs, [current.id],
            "a malformed success response restores the dissolved card")
    }

    func testUnsavedTrashOnlyDismissesAndLateSavedCompletionCannotRemoveReplacement() throws {
        _ = NSApplication.shared
        let unsaved = artifact(id: "draft", previewPath: "/draft-preview.png")
        let unsavedController = try presentedController(unsaved)
        let unsavedTransport = MiniPreviewActionTransport()
        let unsavedActions = MiniPreviewActions(settingsPath: nil, transport: unsavedTransport,
            loadPreferences: { try self.preferences() })
        unsavedActions.bind(previews: unsavedController); unsavedActions.configure(historyRoot: "/History")
        unsavedActions.trash(unsaved)
        LiveCaptureController.flush()
        try waitUntil { unsavedController.presentedArtifactIDs.isEmpty }
        try waitUntil { unsavedTransport.requestCount == 1 }
        XCTAssertEqual(unsavedTransport.lastRequest?["operation"] as? String, "trash_preview")
        XCTAssertTrue(unsavedTransport.lastRequest?["saved_path"] is NSNull,
            "unsaved Trash must never send a private file as the exported path")
        unsavedController.close()

        let saved = artifact(id: "same", previewPath: "/old-preview.png", savedPath: "/old.png")
        let controller = try presentedController(saved)
        defer { controller.close() }
        let transport = MiniPreviewActionTransport(); transport.gate = DispatchSemaphore(value: 0)
        let actions = MiniPreviewActions(settingsPath: nil, transport: transport,
            loadPreferences: { try self.preferences() })
        actions.bind(previews: controller); actions.configure(historyRoot: "/History")
        actions.trash(saved)
        XCTAssertEqual(controller.presentedArtifactIDs, [],
                       "shipping dissolves the card before its Trash request")
        XCTAssertEqual(transport.started.wait(timeout: .now() + 5), .success)
        let replacement = artifact(id: "same", previewPath: "/replacement-preview.png")
        let generation = try XCTUnwrap(controller.beginCapture(settings: previewSettings()))
        controller.present(replacement, on: screenID(), settings: previewSettings(), generation: generation)
        try waitUntil { controller.decodedArtifactIDs == ["same"] }
        // Even a late failure must not restore the dissolved card over it.
        transport.fail = true
        transport.gate?.signal(); LiveCaptureController.flush()
        var completionDrained = false
        DispatchQueue.main.async { completionDrained = true }
        try waitUntil { completionDrained }
        XCTAssertEqual(controller.presentedArtifactIDs, ["same"],
            "late Trash completion must not remove an identity/path replacement")
        XCTAssertNotEqual(controller.statusText(for: "same"), "Trash failed")
    }

    func testOutOfOrderDecodesPreserveCaptureOrderAndCaptureCancellationRestoresPanel() throws {
        _ = NSApplication.shared
        let oldDecode = DispatchSemaphore(value: 0)
        let image = NSImage(cgImage: PreviewView.fixtureImage(scale: 1),
                            size: NSSize(width: 284, height: 160))
        let controller = MiniPreviewController(tokens: tokens, imageLoader: { path in
            if path == "/old-preview.png" { oldDecode.wait() }
            return image
        })
        let visible = MiniPreviewSettings(enabled: true, placement: "bottom_right",
                                          includeInCaptures: false)
        let old = artifact(id: "old", previewPath: "/old-preview.png")
        let new = artifact(id: "new", previewPath: "/new-preview.png")

        let first = try XCTUnwrap(controller.beginCapture(settings: visible))
        controller.present(old, on: screenID(), settings: visible, generation: first)
        let second = try XCTUnwrap(controller.beginCapture(settings: visible))
        controller.present(new, on: screenID(), settings: visible, generation: second)
        try waitUntil { controller.presentedArtifactID == "new" && controller.isPanelVisible }
        XCTAssertEqual(controller.decodedArtifactIDs, ["new"])
        oldDecode.signal()
        try waitUntil { controller.decodedArtifactIDs == ["old", "new"] }
        XCTAssertEqual(controller.presentedArtifactID, "new",
            "decode completion order must not change chronological membership")

        let cancelled = try XCTUnwrap(controller.beginCapture(settings: visible))
        XCTAssertFalse(controller.isPanelVisible)
        controller.restoreCapture(generation: cancelled)
        XCTAssertTrue(controller.isPanelVisible)
        let included = MiniPreviewSettings(enabled: true, placement: "bottom_right",
                                           includeInCaptures: true)
        let includedCapture = try XCTUnwrap(controller.beginCapture(settings: included))
        XCTAssertTrue(controller.isPanelVisible,
            "the explicit inclusion preference keeps the preview visible during capture")
        controller.restoreCapture(generation: includedCapture)
        controller.updateSettings(MiniPreviewSettings(enabled: false, placement: "top_left",
                                                       includeInCaptures: false))
        XCTAssertFalse(controller.isPanelVisible)
        controller.reconcileHistory(ids: [])
        XCTAssertNil(controller.presentedArtifactID)
    }

    func testClearAllUsesSnapshotAndDoesNotRemoveLaterArrival() throws {
        _ = NSApplication.shared
        let oldDecode = DispatchSemaphore(value: 0)
        let image = NSImage(cgImage: PreviewView.fixtureImage(scale: 1),
                            size: NSSize(width: 284, height: 160))
        let controller = MiniPreviewController(tokens: tokens, imageLoader: { path in
            if path == "/old-preview.png" { oldDecode.wait() }
            return image
        })
        let settings = MiniPreviewSettings(enabled: true, placement: "bottom_right",
                                           includeInCaptures: false)
        let first = try XCTUnwrap(controller.beginCapture(settings: settings))
        controller.present(artifact(id: "old", previewPath: "/old-preview.png"),
                           on: screenID(), settings: settings, generation: first)
        XCTAssertEqual(controller.presentedArtifactIDs, ["old"])
        controller.clearAll()
        XCTAssertEqual(controller.presentedArtifactIDs, [])

        let second = try XCTUnwrap(controller.beginCapture(settings: settings))
        controller.present(artifact(id: "new", previewPath: "/new-preview.png"),
                           on: screenID(), settings: settings, generation: second)
        try waitUntil { controller.presentedArtifactIDs == ["new"] && controller.isPanelVisible }
        oldDecode.signal()
        RunLoop.current.run(until: Date().addingTimeInterval(0.05))
        XCTAssertEqual(controller.presentedArtifactIDs, ["new"],
            "late decode from the cleared snapshot cannot remove or resurrect previews")
    }

    func testClearExitPreservesDistinctMediaAndPartialCardOpacityThroughDelay() throws {
        _ = NSApplication.shared
        let preference = NativeMotion.motionPreference
        NativeMotion.motionPreference = { false }
        defer { NativeMotion.motionPreference = preference }
        let panel = fixturePanel(ids: ["idle", "hovered", "partial"], images: [
            "idle": solidImage(.white), "hovered": solidImage(.white), "partial": solidImage(.white)])
        defer { panel.close() }
        panel.orderFrontRegardless(); panel.display()
        let view = panel.previewView
        let hover = try hoverEvent(for: panel)
        let hovered = try XCTUnwrap(view.card(for: "hovered"))
        hovered.mouseEntered(with: hover)
        RunLoop.current.run(until: Date().addingTimeInterval(0.3))
        XCTAssertEqual(hovered.mediaDimOpacity, 0.5, accuracy: 0.01)
        for (id, blur, scale, opacity) in [
            ("idle", 0.0, 1.0, 1.0), ("hovered", 2.0, 1.015, 1.0), ("partial", 0.8, 1.006, 0.73)] {
            let card = try XCTUnwrap(view.card(for: id))
            let image = try XCTUnwrap(card.subviewsRecursive.first {
                $0.accessibilityLabel() == "Screenshot thumbnail"
            })
            let layer = try XCTUnwrap(image.layer)
            // A deterministic partial presentation, independent of CI frame timing.
            CATransaction.begin(); CATransaction.setDisableActions(true)
            layer.removeAllAnimations()
            layer.setValue(blur, forKeyPath: "filters.blur.inputRadius")
            let clip = try XCTUnwrap(image.superview)
            let size = NSSize(width: clip.bounds.width * scale, height: clip.bounds.height * scale)
            image.frame = NSRect(x: (clip.bounds.width - size.width) / 2,
                                 y: (clip.bounds.height - size.height) / 2,
                                 width: size.width, height: size.height)
            image.alphaValue = opacity
            card.layer?.removeAllAnimations(); card.layer?.opacity = 0.61
            CATransaction.commit(); CATransaction.flush()
            RunLoop.current.run(until: Date().addingTimeInterval(0.05))
            let dim = card.mediaDimOpacity
            XCTAssertNotNil(view.playExit(for: id, kind: .dismiss, delay: 0.5, preserveAppearance: true))
            XCTAssertEqual(card.mediaDimOpacity, dim, accuracy: 0.01,
                           "Clear must not force idle/partial media to hover dimming")
            XCTAssertEqual(card.mediaScale, scale, accuracy: 0.001)
            XCTAssertEqual(image.alphaValue, opacity, accuracy: 0.01)
            XCTAssertEqual(try XCTUnwrap(layer.value(forKeyPath: "filters.blur.inputRadius") as? NSNumber)
                .doubleValue, blur, accuracy: 0.01)
            let exit = try XCTUnwrap(card.layer?.animation(forKey: "preview-exit") as? CAAnimationGroup)
            let cardFade = try XCTUnwrap(exit.animations?.first as? CAKeyframeAnimation)
            XCTAssertEqual(try XCTUnwrap(cardFade.values?.first as? NSNumber).doubleValue, 0.61, accuracy: 0.01)
            XCTAssertEqual(exit.fillMode, .both, "snapshot must hold before the stagger starts")
            let streak = try XCTUnwrap(layer.animation(forKey: "preview-dismiss-streak") as? CAAnimationGroup)
            let mediaFade = try XCTUnwrap(streak.animations?.first as? CAKeyframeAnimation)
            XCTAssertEqual(try XCTUnwrap(mediaFade.values?.first as? NSNumber).doubleValue, opacity, accuracy: 0.01)
            let mediaMove = try XCTUnwrap(streak.animations?.last as? CAKeyframeAnimation)
            let firstMove = try XCTUnwrap(mediaMove.values?.first as? NSValue).caTransform3DValue
            XCTAssertEqual(firstMove.m11, 1, accuracy: 0.001, "do not double-apply locked-hover scale")
            XCTAssertEqual(firstMove.m22, 1, accuracy: 0.001)
            RunLoop.current.run(until: Date().addingTimeInterval(0.1))
            XCTAssertEqual(card.mediaDimOpacity, dim, accuracy: 0.01)
            XCTAssertEqual(card.mediaScale, scale, accuracy: 0.001)
        }
        try write(render(panel), name: "mini-preview-clear-preserved-presentation.png")
    }

    func testIncomingCapturePreservesCollapsedState() throws {
        _ = NSApplication.shared
        let image = NSImage(cgImage: PreviewView.fixtureImage(scale: 1),
                            size: NSSize(width: 284, height: 160))
        let controller = MiniPreviewController(tokens: tokens, imageLoader: { _ in image })
        let settings = MiniPreviewSettings(enabled: true, placement: "top_left",
                                           includeInCaptures: false)
        let first = try XCTUnwrap(controller.beginCapture(settings: settings))
        controller.present(artifact(id: "first", previewPath: "/first-preview.png"),
                           on: screenID(), settings: settings, generation: first)
        try waitUntil { controller.isPanelVisible }
        controller.setCollapsed(true)
        controller.moveStack(to: NSPoint(x: 240, y: 270))
        let moved = try XCTUnwrap(controller.stackOrigin)

        let second = try XCTUnwrap(controller.beginCapture(settings: settings))
        controller.present(artifact(id: "second", previewPath: "/second-preview.png"),
                           on: screenID(), settings: settings, generation: second)
        try waitUntil { controller.decodedArtifactIDs == ["first", "second"] }
        XCTAssertTrue(controller.isCollapsed)
        XCTAssertEqual(controller.stackOrigin?.x, moved.x)
        XCTAssertEqual(controller.stackOrigin?.edge, moved.edge)
        controller.setCollapsed(false)
        XCTAssertEqual(controller.stackOrigin?.edge, moved.edge)

        controller.updateSettings(MiniPreviewSettings(enabled: false, placement: "top_left",
                                                       includeInCaptures: false))
        XCTAssertNil(controller.stackOrigin)
        XCTAssertFalse(controller.isPanelVisible)
        XCTAssertFalse(controller.isCollapsed)
        XCTAssertEqual(controller.presentedArtifactIDs, ["first", "second"])
        controller.updateSettings(settings)
        XCTAssertTrue(controller.isPanelVisible)
        XCTAssertEqual(controller.presentedArtifactIDs, ["first", "second"],
            "hiding previews must retain membership for re-enable")
        controller.setCollapsed(true)
        controller.moveStack(to: NSPoint(x: 270, y: 250))
        XCTAssertNotNil(controller.stackOrigin)
        controller.clearAll()
        XCTAssertNil(controller.stackOrigin)
    }

    func testMovedPileChangesExpansionCornerWithoutMovingItsFrontCard() throws {
        _ = NSApplication.shared
        let screen = try XCTUnwrap(NSScreen.screens.first)
        let controller = MiniPreviewController(tokens: tokens, imageLoader: { path in
            self.solidImage(path.contains("older") ? .systemRed : .systemBlue)
        })
        defer { controller.close() }
        let settings = MiniPreviewSettings(enabled: true, placement: "bottom_left", includeInCaptures: false)
        for id in ["older", "latest"] {
            let generation = try XCTUnwrap(controller.beginCapture(settings: settings))
            controller.present(artifact(id: id, previewPath: "/\(id)-preview.png"),
                               on: screenID(), settings: settings, generation: generation)
            try waitUntil { controller.decodedArtifactIDs.contains(id) && controller.isPanelVisible }
        }
        for top in [true, false] {
            controller.setCollapsed(true)
            try waitUntil { controller.previewView?.pileExpandAccessibilityLabel == "Expand 2 previews" }
            let compact = try XCTUnwrap(controller.previewView?.window)
            let work = screen.visibleFrame
            controller.moveStack(to: NSPoint(x: top ? work.maxX - compact.frame.width : work.minX,
                y: top ? work.maxY - compact.frame.height - 20 : work.minY + 20))
            let origin = try XCTUnwrap(controller.stackOrigin)
            XCTAssertEqual(origin.anchor, top ? 1 : 0)
            let front = try XCTUnwrap(controller.previewView?.cardScreenFrames()["latest"])
            controller.setCollapsed(false)
            let view = try XCTUnwrap(controller.previewView)
            let expandedFront = try XCTUnwrap(view.cardScreenFrames()["latest"])
            XCTAssertEqual(expandedFront.minX, front.minX, accuracy: 0.001)
            XCTAssertEqual(expandedFront.minY, front.minY, accuracy: 0.001)
            let clear = try XCTUnwrap(view.stackToolbarButtons.first { $0.kind == .clear })
            XCTAssertEqual(clear.frame.minX, top ? view.bounds.width - 56 : 28)
            XCTAssertEqual(clear.frame.minY, top ? 16 : view.bounds.height - 44)
            let older = try XCTUnwrap(view.card(for: "older"))
            let latest = try XCTUnwrap(view.card(for: "latest"))
            XCTAssertEqual(latest.frame.minY, top ? 52 : 212)
            XCTAssertEqual(older.frame.minY, top ? 236 : 28)
            let panel = try XCTUnwrap(view.window as? MiniPreviewPanel)
            try write(render(panel), name: top ? "mini-preview-moved-expanded-top-right.png"
                                              : "mini-preview-moved-expanded-bottom-left.png")
        }
        controller.clearAll()
        controller.updateSettings(settings)
        XCTAssertNil(controller.stackOrigin)
    }

    func testDeletingBlockedLatestDecodeRestoresVisibleOlderCard() throws {
        _ = NSApplication.shared
        let releaseLatest = DispatchSemaphore(value: 0)
        let image = solidImage(.systemBlue)
        let controller = MiniPreviewController(tokens: tokens, imageLoader: { path in
            if path == "/latest-preview.png" { releaseLatest.wait() }
            return image
        })
        let settings = MiniPreviewSettings(enabled: true, placement: "bottom_right",
                                           includeInCaptures: false)
        let first = try XCTUnwrap(controller.beginCapture(settings: settings))
        controller.present(artifact(id: "older", previewPath: "/older-preview.png"),
                           on: screenID(), settings: settings, generation: first)
        try waitUntil { controller.decodedArtifactIDs == ["older"] && controller.isPanelVisible }

        let second = try XCTUnwrap(controller.beginCapture(settings: settings))
        controller.present(artifact(id: "latest", previewPath: "/latest-preview.png"),
                           on: screenID(), settings: settings, generation: second)
        XCTAssertFalse(controller.isPanelVisible)
        controller.reconcileHistory(ids: ["older"])
        XCTAssertEqual(controller.presentedArtifactIDs, ["older"])
        XCTAssertTrue(controller.isPanelVisible,
            "deleting the pending latest capture must restore decoded survivors")
        releaseLatest.signal()
        RunLoop.current.run(until: Date().addingTimeInterval(0.05))
        XCTAssertEqual(controller.decodedArtifactIDs, ["older"])
    }

    func testCompactDepthShadePreservesFrontAndClearsOnExpansion() throws {
        _ = NSApplication.shared
        let panel = fixturePanel(ids: ["red"],
            images: ["red": solidImage(NSColor(srgbRed: 1, green: 0, blue: 0, alpha: 1))], collapsed: true)
        defer { panel.close() }
        let card = try XCTUnwrap(panel.previewView.subviewsRecursive.compactMap { $0 as? MiniPreviewCardView }.first)
        func red() throws -> CGFloat {
            let bitmap = try render(panel)
            XCTAssertEqual(bitmap.colorSpace, .sRGB)
            XCTAssertEqual(bitmap.bitsPerSample, 8)
            // colorAt returns a calibrated NSColor even for this sRGB bitmap;
            // converting that color again changes the already-correct bytes.
            var pixel = [Int](repeating: 0, count: bitmap.samplesPerPixel)
            let point = bitmapPoint(in: bitmap, of: panel, x: 170, y: 132)
            bitmap.getPixel(&pixel, atX: point.x, y: point.y)
            return CGFloat(pixel[0]) / 255
        }
        XCTAssertEqual(try red(), 1, accuracy: 0.01)
        card.setCompact(true, depth: 1)
        // Source red composited with (15,15,18) at CSS depth1 opacity .13748.
        XCTAssertEqual(try red(), 0.870607, accuracy: 0.015)
        try write(render(panel), name: "mini-preview-depth-one.png")
        card.setCompact(true, depth: 6)
        XCTAssertEqual(try red(), 0.322353, accuracy: 0.015, "deep shade caps at .72")
        card.setCompact(false, depth: 6)
        XCTAssertEqual(try red(), 1, accuracy: 0.01, "expanded images are never shaded")
        card.setCompact(true, depth: 0)
        XCTAssertEqual(try red(), 1, accuracy: 0.01, "new front image is never shaded")
    }

    func testRealThumbnailRendersExpandedAndCollapsedStacks() throws {
        _ = NSApplication.shared
        let ids = ["oldest", "middle", "newest"]
        let images = ["oldest": solidImage(.systemRed), "middle": solidImage(.systemGreen),
                      "newest": solidImage(.systemBlue)]
        let panels: [(MiniPreviewPanel, String)] = [
            (fixturePanel(ids: ids, images: images, topAnchor: true), "top-expanded"),
            (fixturePanel(ids: ids, images: images, collapsed: true, topAnchor: true),
             "top-collapsed"),
            (fixturePanel(ids: ids, images: images), "bottom-expanded"),
            (fixturePanel(ids: ids, images: images, collapsed: true), "bottom-collapsed")
        ]
        defer { panels.forEach { $0.0.close() } }

        for (panel, name) in panels {
            XCTAssertEqual(panel.previewView.cardPaintOrder, ids,
                "chronological subview order paints newest on top")
            let bitmap = try render(panel)
            XCTAssertEqual(try XCTUnwrap(pixelColor(in: bitmap, of: panel, x: 10, y: 10)).alphaComponent, 0,
                           accuracy: 0.01, "stack padding remains transparent")
            let newestSampleY: CGFloat = name == "bottom-expanded" ? 436 : 90
            let frontPixel = try XCTUnwrap(pixelColor(in: bitmap, of: panel, x: 170, y: newestSampleY))
            XCTAssertGreaterThan(frontPixel.alphaComponent, 0.9)
            XCTAssertGreaterThan(frontPixel.blueComponent, frontPixel.redComponent,
                "the newest blue capture must paint over older cards")
            try write(bitmap, name: "mini-preview-stack-\(name).png")
        }

        let asymmetric = NSImage(cgImage: PreviewView.fixtureImage(scale: 2),
                                 size: NSSize(width: 284, height: 160))
        let single = fixturePanel(ids: ["asymmetric"], images: ["asymmetric": asymmetric])
        defer { single.close() }
        try write(render(single), name: "mini-preview-single-asymmetric.png")
        single.orderFrontRegardless()
        let presentation = try XCTUnwrap(renderPresentation(single))
        XCTAssertEqual(presentation.pixelsWide,
                       Int(single.previewView.bounds.width * single.backingScaleFactor))
        XCTAssertEqual(presentation.pixelsHigh,
                       Int(single.previewView.bounds.height * single.backingScaleFactor))
        // Independent asymmetric landmarks catch both a second backing-scale
        // transform and an upside-down layer capture before trusting motion PNGs.
        let landmarks: [(CGFloat, CGFloat, [Int])] = [(100, 110, [217, 84, 105]),
            (260, 65, [245, 189, 74]), (260, 170, [51, 122, 102])]
        for (x, y, expected) in landmarks {
            var pixel = [Int](repeating: 0, count: presentation.samplesPerPixel)
            let point = bitmapPoint(in: presentation, of: single, x: x, y: y)
            presentation.getPixel(&pixel, atX: point.x, y: point.y)
            for channel in 0..<3 {
                XCTAssertEqual(Double(pixel[channel]), Double(expected[channel]), accuracy: 3)
            }
        }
        try write(presentation, name: "mini-preview-single-presentation-asymmetric.png")

        let white = fixturePanel(ids: ["white"], images: ["white": solidImage(.white)])
        defer { white.close() }
        let whiteBitmap = try render(white)
        let whiteImagePixel = try XCTUnwrap(pixelColor(in: whiteBitmap, of: white, x: 170, y: 90))
        XCTAssertGreaterThan(whiteImagePixel.brightnessComponent, 0.9)
        try write(whiteBitmap, name: "mini-preview-single-white.png")
    }

    func testCollapsedPileHidesActionsAndFrontHitTargetExpands() {
        _ = NSApplication.shared
        let ids = ["older", "newer"]
        let image = solidImage(.systemBlue)
        var expanded = false
        let panel = fixturePanel(ids: ids,
            images: Dictionary(uniqueKeysWithValues: ids.map { ($0, image) }), collapsed: true,
            setCollapsed: { expanded = !$0 })
        defer { panel.close() }
        XCTAssertEqual(panel.previewView.visibleCardActionTitles, [])
        XCTAssertEqual(panel.previewView.visibleCardLabelCount, 0)
        panel.previewView.setStatus("Copied", for: "newer")
        XCTAssertEqual(panel.previewView.visibleCardLabelCount, 0,
            "late action feedback cannot restore compact labels")
        XCTAssertTrue(panel.previewView.subviewsRecursive.compactMap { $0 as? MiniPreviewButton }
            .filter { !$0.isHidden }.isEmpty)
        XCTAssertEqual(panel.previewView.pileExpandAccessibilityLabel, "Expand 2 previews")
        panel.previewView.activatePileExpand()
        XCTAssertTrue(expanded)

        let expandedPanel = fixturePanel(ids: ids,
            images: Dictionary(uniqueKeysWithValues: ids.map { ($0, image) }))
        defer { expandedPanel.close() }
        XCTAssertEqual(expandedPanel.previewView.visibleCardLabelCount, ids.count,
            "expanded cards retain the dimensions badge")
    }

    func testOverflowUsesSharedLayoutsAndReservesBottomControlGutter() throws {
        _ = NSApplication.shared
        let ids = (0..<8).map { "capture-\($0)" }
        let image = solidImage(.systemPurple)
        let panel = fixturePanel(ids: ids,
            images: Dictionary(uniqueKeysWithValues: ids.map { ($0, image) }))
        defer { panel.close() }
        XCTAssertEqual(panel.previewView.documentHeight, 1_528)
        XCTAssertGreaterThan(panel.previewView.documentHeight, panel.previewView.bounds.height)
        XCTAssertEqual(panel.previewView.scrollOffsetY,
            panel.previewView.documentHeight - panel.previewView.bounds.height, accuracy: 0.5)

        let collapsed = fixturePanel(ids: ids,
            images: Dictionary(uniqueKeysWithValues: ids.map { ($0, image) }), collapsed: true)
        defer { collapsed.close() }
        XCTAssertLessThanOrEqual(collapsed.previewView.documentHeight,
            collapsed.previewView.viewportHeight + 0.5,
            "collapsed piles never create a hidden scroll range")
        XCTAssertEqual(collapsed.previewView.scrollOffsetY, 0, accuracy: 0.5)

        let distinct = Dictionary(uniqueKeysWithValues: ids.enumerated().map { index, id in
            (id, solidImage(NSColor(calibratedHue: CGFloat(index) / CGFloat(ids.count),
                saturation: 0.8, brightness: 0.9, alpha: 1)))
        })
        let renderPanel = fixturePanel(ids: ids, images: distinct)
        defer { renderPanel.close() }
        XCTAssertEqual(renderPanel.previewView.scrollOffsetY,
            renderPanel.previewView.documentHeight - renderPanel.previewView.bounds.height,
            accuracy: 0.5, "bottom overflow opens with newest capture visible")
        try write(render(renderPanel), name: "mini-preview-stack-bottom-overflow.png")
    }

    func testStackToolbarNeedsTwoPreviewsAndShowLessGrowsInwardOnHover() throws {
        _ = NSApplication.shared
        let single = fixturePanel(ids: ["only"], images: ["only": solidImage(.systemBlue)])
        defer { single.close() }
        XCTAssertTrue(single.previewView.stackToolbarButtons.isEmpty, "one preview has no stack toolbar")

        for right in [false, true] {
            let ids = ["older", "newer"]
            let panel = fixturePanel(ids: ids,
                images: Dictionary(uniqueKeysWithValues: ids.map { ($0, solidImage(.systemBlue)) }),
                rightAnchor: right)
            defer { panel.close() }
            let toolbar = panel.previewView.stackToolbarButtons
            let clear = try XCTUnwrap(toolbar.first { $0.kind == .clear })
            let minimize = try XCTUnwrap(toolbar.first { $0.kind == .collapse })
            XCTAssertEqual(clear.accessibilityLabel(), "Clear all previews")
            XCTAssertNil(clear.toolTip)
            XCTAssertEqual(clear.tooltipText, "Clear all")
            XCTAssertEqual(minimize.accessibilityLabel(), "Minimize previews")
            XCTAssertNil(minimize.toolTip)
            XCTAssertNil(minimize.tooltipText, "Show less swaps in a label instead of a tip")
            let rest = minimize.frame
            XCTAssertEqual(rest.width, 28)
            XCTAssertFalse(minimize.showsHoverLabel)
            let event = try XCTUnwrap(NSEvent.mouseEvent(with: .mouseMoved, location: .zero,
                modifierFlags: [], timestamp: 1, windowNumber: panel.windowNumber, context: nil,
                eventNumber: 0, clickCount: 0, pressure: 0))
            minimize.mouseEntered(with: event)
            XCTAssertTrue(minimize.showsHoverLabel)
            XCTAssertEqual(minimize.frame.width, 92)
            XCTAssertEqual(right ? minimize.frame.maxX : minimize.frame.minX,
                           right ? rest.maxX : rest.minX, "Show less grows away from the screen edge")
            XCTAssertFalse(minimize.frame.intersects(clear.frame), "Show less covered Clear all")
            try write(render(panel), name: "mini-preview-show-less-\(right ? "right" : "left").png")
            minimize.mouseExited(with: event)
            XCTAssertFalse(minimize.showsHoverLabel)
            XCTAssertEqual(minimize.frame, rest)
        }
    }

    func testOverflowCuesFollowHiddenEdgesAndScrollOneCardSlot() throws {
        _ = NSApplication.shared
        let ids = (0..<8).map { "capture-\($0)" }
        let images = Dictionary(uniqueKeysWithValues: ids.map { ($0, solidImage(.systemPurple)) })
        let bottom = fixturePanel(ids: ids, images: images)
        defer { bottom.close() }
        let view = bottom.previewView
        // Bottom stacks open on the newest card; only older captures are hidden above.
        XCTAssertEqual(view.visibleOverflowCueLabels, ["Show older captures"])
        let start = view.scrollOffsetY
        let older = try XCTUnwrap(view.subviewsRecursive.compactMap { $0 as? MiniPreviewOverflowCue }
            .first { $0.above })
        older.performClick(nil)
        try waitUntil { abs(view.scrollOffsetY - (start - 184)) < 0.5 }
        try waitUntil { view.visibleOverflowCueLabels == ["Show older captures", "Show newer captures"] }
        try write(render(bottom), name: "mini-preview-stack-overflow-cues.png")
        view.scrollStack(bySlots: -20)
        try waitUntil { view.scrollOffsetY < 0.5 }
        try waitUntil { view.visibleOverflowCueLabels == ["Show newer captures"] }

        let top = fixturePanel(ids: ids, images: images, topAnchor: true)
        defer { top.close() }
        XCTAssertEqual(top.previewView.visibleOverflowCueLabels, ["Show older captures"],
            "top stacks list newest first, so the lower cue reveals older captures")

        for preview in [view, top.previewView] {
            // A scroll already queued when Clear all starts must not bring
            // its controls back while the cards are still streaking out.
            preview.scrollStack(bySlots: 1)
            preview.playToolbar("preview_toolbar_clear", holdEnd: true)
            XCTAssertEqual(preview.visibleOverflowCueLabels, [])
            try waitUntil { abs(preview.scrollOffsetY - 184) < 0.5 }
            preview.updateOverflowCues()
            XCTAssertEqual(preview.visibleOverflowCueLabels, [])
        }
        try write(render(bottom), name: "mini-preview-clear-overflow-hidden.png")

        let fits = fixturePanel(ids: ["a", "b"], images: ["a": solidImage(.red), "b": solidImage(.blue)])
        defer { fits.close() }
        XCTAssertEqual(fits.previewView.visibleOverflowCueLabels, [])
        let collapsed = fixturePanel(ids: ids, images: images, collapsed: true)
        defer { collapsed.close() }
        XCTAssertEqual(collapsed.previewView.visibleOverflowCueLabels, [])
    }

    func testClosingRootWindowClosesOpenPanelAndRequestsTermination() {
        _ = NSApplication.shared
        let root = NSWindow(contentRect: NSRect(x: 0, y: 0, width: 500, height: 400),
            styleMask: [.titled, .closable], backing: .buffered, defer: false)
        root.isReleasedWhenClosed = false
        let image = NSImage(cgImage: PreviewView.fixtureImage(scale: 1),
                            size: NSSize(width: 284, height: 160))
        let panel = fixturePanel(ids: ["latest"], images: ["latest": image])
        panel.orderFrontRegardless()
        var terminationRequests = 0
        let handler = RootWindowCloseHandler(rootWindow: root,
            closePreviews: { panel.close() }, terminate: { terminationRequests += 1 })
        root.delegate = handler
        root.makeKeyAndOrderFront(nil)
        root.performClose(nil)
        XCTAssertFalse(panel.isVisible)
        XCTAssertEqual(terminationRequests, 1)
        withExtendedLifetime(handler) {}
    }

    func testIconTooltipsAreInstantGlassChipsAndSystemTooltipsAreGone() throws {
        _ = NSApplication.shared
        for top in [false, true] {
            let panel = fixturePanel(ids: ["older", "newer"],
                images: ["older": solidImage(.systemBlue), "newer": solidImage(.systemRed)],
                topAnchor: top, savedPaths: ["newer": "/Exports/newer.png"])
            defer { panel.close() }
            let view = panel.previewView
            XCTAssertTrue(view.subviewsRecursive.allSatisfy { $0.toolTip == nil },
                          "no delayed system tooltips")
            let card = try XCTUnwrap(view.card(for: "newer"))
            let hover = try hoverEvent(for: panel)
            card.mouseEntered(with: hover)
            let controls = card.subviews.compactMap { $0 as? MiniPreviewButton }
            let delete = try XCTUnwrap(controls.first { $0.kind == .trash })
            delete.mouseEntered(with: hover)
            XCTAssertEqual(view.visibleTooltip, "Delete")
            let tip = try XCTUnwrap(view.visibleTooltipFrame)
            let anchor = delete.convert(delete.bounds, to: view)
            XCTAssertEqual(tip.midX, anchor.midX, accuracy: 0.5)
            if top {
                XCTAssertEqual(tip.minY, anchor.maxY + 6, accuracy: 0.5, "top stacks open tips below")
            } else {
                XCTAssertEqual(tip.maxY, anchor.minY - 6, accuracy: 0.5, "bottom stacks open tips above")
            }
            try write(render(panel), name: "mini-preview-tooltip-\(top ? "top" : "bottom").png")
            delete.mouseExited(with: hover)
            XCTAssertNil(view.visibleTooltip)
            let copyAction = try XCTUnwrap(controls.first { $0.kind == .copy })
            copyAction.mouseEntered(with: hover)
            XCTAssertNil(view.visibleTooltip, "labelled actions carry no tip")
            copyAction.mouseExited(with: hover)
            let clear = try XCTUnwrap(view.stackToolbarButtons.first { $0.kind == .clear })
            clear.mouseEntered(with: hover)
            XCTAssertEqual(view.visibleTooltip, "Clear all")
            clear.mouseExited(with: hover)
            XCTAssertNil(view.visibleTooltip)
            // Hiding chrome under a hovered icon also hides its tip.
            let close = try XCTUnwrap(controls.first { $0.kind == .close })
            close.mouseEntered(with: hover)
            XCTAssertEqual(view.visibleTooltip, "Close")
            card.mouseExited(with: hover)
            XCTAssertNil(view.visibleTooltip)
        }
        let pile = fixturePanel(ids: ["a", "b"],
            images: ["a": solidImage(.systemBlue), "b": solidImage(.systemRed)], collapsed: true)
        defer { pile.close() }
        XCTAssertTrue(pile.previewView.subviewsRecursive.allSatisfy { $0.toolTip == nil },
                      "the compact pile carries no expand/drag tooltip")
    }

    func testHoverBlursDarkensAndScalesMediaAndStalePointerLockHoldsItOff() throws {
        _ = NSApplication.shared
        let panel = fixturePanel(ids: ["card"], images: ["card": solidImage(.white)])
        defer { panel.close() }
        let view = panel.previewView
        let card = try XCTUnwrap(view.card(for: "card"))
        let hover = try hoverEvent(for: panel)
        XCTAssertFalse(card.mediaHovered)
        XCTAssertEqual(card.mediaBlurRadius, 0)
        card.mouseEntered(with: hover)
        try waitUntil { card.mediaHovered && abs(card.mediaDimOpacity - 0.5) < 0.01 }
        XCTAssertEqual(card.mediaBlurRadius, 2, accuracy: 0.01)
        try waitUntil { abs(card.mediaScale - 1.015) < 0.001 }
        try write(render(panel), name: "mini-preview-hover-blur.png")
        let preference = NativeMotion.motionPreference
        defer { NativeMotion.motionPreference = preference }
        for reduced in [false, true] {
            NativeMotion.motionPreference = { reduced }
            card.rejectDrop()
            XCTAssertFalse(card.mediaHovered, "rejection resets media without hiding hover actions")
            XCTAssertEqual(card.mediaBlurRadius, 0, accuracy: 0.01)
            XCTAssertEqual(card.mediaDimOpacity, 0, accuracy: 0.01)
            XCTAssertEqual(card.mediaScale, 1, accuracy: 0.001)
            card.mouseEntered(with: hover)
            XCTAssertFalse(card.mediaHovered, "hover cannot reblur the media during rejection")
            XCTAssertTrue(view.visibleCardActionTitles.contains("Copy"))
            try write(render(panel), name: "mini-preview-self-drop-media-\(reduced ? "reduced" : "normal").png")
            try waitUntil { card.mediaHovered && abs(card.mediaDimOpacity - 0.5) < 0.01 }
            XCTAssertEqual(card.mediaBlurRadius, 2, accuracy: 0.01)
            try waitUntil { abs(card.mediaScale - 1.015) < 0.001 }
        }
        card.mouseExited(with: hover)
        try waitUntil { !card.mediaHovered && card.mediaDimOpacity < 0.01 }
        XCTAssertEqual(card.mediaBlurRadius, 0, accuracy: 0.01)
        try waitUntil { abs(card.mediaScale - 1) < 0.001 }

        // After an expand, the resting pointer does not count as hover.
        view.lockCardHover(releaseIfPointerOutside: false)
        card.mouseEntered(with: hover)
        XCTAssertTrue(view.isCardHoverLocked)
        XCTAssertFalse(card.mediaHovered)
        XCTAssertEqual(view.visibleCardActionTitles, [])
        view.samplePointer(NSPoint(x: 100, y: 100))
        view.samplePointer(NSPoint(x: 102, y: 101))
        XCTAssertTrue(view.isCardHoverLocked, "sub-slop jitter keeps hover idle")
        XCTAssertFalse(card.mediaHovered)
        view.samplePointer(NSPoint(x: 100, y: 105))
        XCTAssertFalse(view.isCardHoverLocked)
        XCTAssertTrue(card.mediaHovered)
        XCTAssertEqual(Set(view.visibleCardActionTitles), ["Delete", "Edit", "Copy", "Save file"])
        // Leaving the stack releases a lock too.
        card.mouseExited(with: hover)
        view.lockCardHover(releaseIfPointerOutside: false)
        XCTAssertTrue(view.isCardHoverLocked)
        view.samplePointer(nil)
        XCTAssertFalse(view.isCardHoverLocked)
    }

    func testEditorPresencePinsInEditorPillAndRingThenLingers() throws {
        _ = NSApplication.shared
        let present = UInt32(CAPTURES_EDITOR_PHASE_PRESENT)
        for right in [false, true] {
            let panel = fixturePanel(ids: ["card"], images: ["card": solidImage(.systemBlue)],
                                     rightAnchor: right)
            defer { panel.close() }
            let view = panel.previewView
            let card = try XCTUnwrap(view.card(for: "card"))
            let edit = try XCTUnwrap(view.editControl(for: "card"))
            let rest = edit.frame
            XCTAssertTrue(edit.isHidden)
            XCTAssertEqual(edit.accessibilityLabel(), "Edit")
            view.setEditorPhase(present, for: "card", animated: false)
            XCTAssertFalse(edit.isHidden, "an open editor pins the control without hover")
            XCTAssertTrue(edit.editorPresent)
            XCTAssertEqual(edit.editorLabel, "In editor")
            XCTAssertEqual(edit.accessibilityLabel(), "Show in editor")
            XCTAssertNil(edit.tooltipText, "the pill carries no tip")
            XCTAssertEqual(card.editorRingOpacity, 1)
            XCTAssertGreaterThan(edit.frame.width, 60)
            XCTAssertEqual(right ? edit.frame.minX : edit.frame.maxX, right ? rest.minX : rest.maxX,
                           "the pill widens away from its corner")
            XCTAssertEqual(view.visibleCardActionTitles, ["Edit"])
            XCTAssertTrue(card.hasVisibleLabels, "idle metadata stays beside the pill")
            try write(render(panel), name: "mini-preview-in-editor-\(right ? "right" : "left").png")
            let hover = try hoverEvent(for: panel)
            let pillFrame = edit.frame
            edit.mouseEntered(with: hover)
            try waitUntil { edit.iconHover == 1 }
            XCTAssertEqual(edit.editorLabel, "Show in editor")
            XCTAssertEqual(edit.frame, pillFrame, "hover keeps the pill's hit target fixed")
            let lifted = try XCTUnwrap(renderPresentation(panel))
            let anchor = edit.convert(edit.bounds, to: view)
            let top = try XCTUnwrap(pixelColor(in: lifted, of: panel, x: anchor.midX, y: anchor.minY)?.usingColorSpace(.sRGB))
            let accent = try XCTUnwrap(tokens.color("theme-accent").usingColorSpace(.sRGB))
            XCTAssertEqual(top.redComponent, accent.redComponent, accuracy: 0.01, "the resting border is inside the lifted face")
            XCTAssertEqual(top.greenComponent, accent.greenComponent, accuracy: 0.01)
            XCTAssertEqual(top.blueComponent, accent.blueComponent, accuracy: 0.01)
            try write(lifted, name: "mini-preview-editor-hover-\(right ? "right" : "left").png")
            edit.performClick(nil)
            XCTAssertEqual(edit.editorLabel, "In editor", "the opening click keeps the passive label")
            XCTAssertEqual(edit.iconHover, 1, "just-opened mutes the action, not the hover pose")
            let passive = try XCTUnwrap(renderPresentation(panel))
            let passiveTop = try XCTUnwrap(pixelColor(in: passive, of: panel, x: anchor.midX, y: anchor.minY)?.usingColorSpace(.sRGB))
            let passiveInside = try XCTUnwrap(pixelColor(in: passive, of: panel, x: anchor.midX, y: anchor.minY + 4)?.usingColorSpace(.sRGB))
            XCTAssertEqual(passiveTop.redComponent, passiveInside.redComponent, accuracy: 0.01)
            XCTAssertEqual(passiveTop.greenComponent, passiveInside.greenComponent, accuracy: 0.01)
            XCTAssertEqual(passiveTop.blueComponent, passiveInside.blueComponent, accuracy: 0.01)
            try write(passive, name: "mini-preview-editor-just-opened-\(right ? "right" : "left").png")
            edit.mouseExited(with: hover)
            edit.mouseEntered(with: hover)
            XCTAssertEqual(edit.editorLabel, "Show in editor")
            try waitUntil { edit.iconHover == 1 }

            view.setEditorPhase(UInt32(CAPTURES_EDITOR_PHASE_LEAVING), for: "card", animated: false)
            XCTAssertFalse(edit.isHidden)
            XCTAssertFalse(edit.isEnabled, "a leaving pill ignores clicks")
            XCTAssertEqual(edit.iconHover, 0, "leaving clears the lifted pose even with the pointer held")
            XCTAssertEqual(card.editorRingOpacity, 0)
            XCTAssertEqual(edit.frame, rest)
            edit.mouseExited(with: hover)
            view.setEditorPhase(UInt32(CAPTURES_EDITOR_PHASE_LINGERING), for: "card", animated: false)
            XCTAssertFalse(edit.isHidden, "the Edit icon lingers without hover")
            XCTAssertTrue(edit.isEnabled)
            XCTAssertEqual(edit.tooltipText, "Edit")
            XCTAssertEqual(edit.accessibilityLabel(), "Edit")
            view.setEditorPhase(UInt32(CAPTURES_EDITOR_PHASE_IDLE), for: "card", animated: false)
            XCTAssertTrue(edit.isHidden)
        }
    }

    func testControllerFollowsOpenEditorsThenLeavesAndLingers() throws {
        _ = NSApplication.shared
        let preference = NativeMotion.motionPreference
        defer { NativeMotion.motionPreference = preference }
        for reduced in [false, true] {
            NativeMotion.motionPreference = { reduced }
            let captured = artifact(id: "edited", previewPath: "/edited-preview.png")
            let controller = try presentedController(captured)
            defer { controller.close() }
            func panel() throws -> MiniPreviewPanel {
                try XCTUnwrap(NSApp.windows.compactMap { $0 as? MiniPreviewPanel }
                    .first { $0.isVisible && $0.previewView === controller.previewView })
            }
            func width() throws -> CGFloat {
                let edit = try XCTUnwrap(controller.previewView?.editControl(for: "edited"))
                // The ticker changes the hit target directly. AppKit's backing
                // presentation can lag that frame until the next display pass.
                return edit.frame.width
            }
            func pump(until deadline: CFTimeInterval) {
                while CACurrentMediaTime() < deadline {
                    let remaining = deadline - CACurrentMediaTime()
                    _ = RunLoop.current.run(mode: .default,
                        before: Date().addingTimeInterval(min(0.005, max(0, remaining))))
                }
            }
            func inFlightWidth(started: CFTimeInterval, fullWidth: CGFloat) throws -> CGFloat {
                if reduced { return try width() }
                // A run-loop deadline does not guarantee a timer delivery.
                // Observe actual geometry within the original 280 ms morph,
                // keeping enough distance from both endpoints to catch a snap.
                // Leave at least 70 ms between the original completion and a
                // restarted clock so a late timer can still distinguish them.
                while CACurrentMediaTime() - started < 0.28 {
                    let sampled = try width()
                    let elapsed = CACurrentMediaTime() - started
                    if elapsed >= 0.07, elapsed < 0.28, sampled > 30, sampled < fullWidth - 2 {
                        return sampled
                    }
                    pump(until: min(started + 0.28, CACurrentMediaTime() + 0.005))
                }
                let finalWidth = try width()
                XCTFail("No intermediate badge frame within 280 ms; width=\(finalWidth), elapsed=\(CACurrentMediaTime() - started)s")
                throw AppBridgeError.invalidResponse
            }
            func waitForSettledWidth(_ target: CGFloat, before deadline: CFTimeInterval) throws {
                // Reaching a run-loop deadline can leave a due timer unserviced.
                // Observe its endpoint, not the last frame before that deadline.
                // Start observing immediately after rebuild: an unconditional
                // pump to +300 ms can service unrelated work past the deadline
                // even if the endpoint was already delivered on time.
                // Exact equality matters: easing can approach within 0.1 pt of
                // the endpoint before a wrongly restarted morph has finished.
                while !reduced, try width() != target, CACurrentMediaTime() < deadline {
                    pump(until: min(deadline, CACurrentMediaTime() + 0.005))
                }
                if !reduced {
                    XCTAssertLessThan(CACurrentMediaTime(), deadline,
                        "an overdue sample cannot distinguish the original clock from a restarted one")
                }
                XCTAssertEqual(try width(), target,
                    "the original morph must finish before a restarted 280 ms clock could finish")
            }
            let idle = UInt32(CAPTURES_EDITOR_PHASE_IDLE)
            let present = UInt32(CAPTURES_EDITOR_PHASE_PRESENT)
            let lingering = UInt32(CAPTURES_EDITOR_PHASE_LINGERING)
            XCTAssertEqual(controller.editorPhase(for: "edited"), idle)
            // Measure this control's endpoints without starting a transition.
            let view = try XCTUnwrap(controller.previewView)
            view.setEditorPhase(present, for: "edited", animated: false)
            let fullWidth = try width()
            view.setEditorPhase(idle, for: "edited", animated: false)
            XCTAssertEqual(try width(), 28, accuracy: 0.1)
            // Measure rebuild continuity on an already-serviced preview, not
            // AppKit's first presentation. Keep setup outside the morph clock;
            // this fixture does not establish acceptable cold-preview latency.
            do {
                let initial = try panel()
                initial.previewView.layoutSubtreeIfNeeded()
                initial.displayIfNeeded()
                CATransaction.flush()
                _ = RunLoop.current.run(mode: .default, before: Date().addingTimeInterval(0.01))
            }
            let enterStarted = CACurrentMediaTime()
            controller.setEditorArtifacts(["edited"])
            XCTAssertEqual(controller.editorPhase(for: "edited"), present)
            let enteringWidth = try inFlightWidth(started: enterStarted, fullWidth: fullWidth)
            let enterRebuilt = CACurrentMediaTime()
            controller.updateSettings(previewSettings())
            XCTAssertEqual(try width(), enteringWidth, accuracy: 1,
                "rebuilding a survivor must not finish or restart its entering badge")
            if reduced { XCTAssertEqual(enteringWidth, fullWidth, accuracy: 0.1) }
            try waitForSettledWidth(fullWidth, before: enterRebuilt + 0.28)
            // Render only after the timing assertions, never between sampling
            // a frame and rebuilding or while checking the original deadline.
            try write(XCTUnwrap(renderPresentation(panel())),
                      name: "mini-preview-editor-present-rebuild-\(reduced ? "reduced" : "motion").png")
            let leaveStarted = CACurrentMediaTime()
            controller.setEditorArtifacts([])
            XCTAssertTrue([UInt32(CAPTURES_EDITOR_PHASE_LEAVING), lingering]
                .contains(controller.editorPhase(for: "edited")))
            let leavingWidth = try inFlightWidth(started: leaveStarted, fullWidth: fullWidth)
            let leaveRebuilt = CACurrentMediaTime()
            controller.updateSettings(previewSettings())
            XCTAssertEqual(try width(), leavingWidth, accuracy: 1,
                "rebuilding a survivor must not finish or restart its leaving badge")
            if reduced { XCTAssertEqual(leavingWidth, 28, accuracy: 0.1) }
            try waitForSettledWidth(28, before: leaveRebuilt + 0.28)
            try write(XCTUnwrap(renderPresentation(panel())),
                      name: "mini-preview-editor-leave-rebuild-\(reduced ? "reduced" : "motion").png")
            if !reduced { try waitUntil { controller.editorPhase(for: "edited") == lingering } }
            try waitUntil { controller.editorPhase(for: "edited") == idle }
        }
    }

    private func hoverEvent(for panel: NSWindow) throws -> NSEvent {
        try XCTUnwrap(NSEvent.mouseEvent(with: .mouseMoved, location: .zero,
            modifierFlags: [], timestamp: 1, windowNumber: panel.windowNumber, context: nil,
            eventNumber: 0, clickCount: 0, pressure: 0))
    }

    private func artifact(id: String, previewPath: String,
                          imagePath: String? = nil, savedPath: String? = nil) -> CaptureArtifact {
        CaptureArtifact(["entry": ["id": id, "width": 800, "height": 600,
            "created_at": "2026-09-18T00:00:00Z",
            "saved_path": savedPath.map { $0 as Any } ?? NSNull()],
            "image_path": imagePath ?? "/\(id).png", "preview_path": previewPath])!
    }

    private func fixturePanel(ids: [String], images: [String: NSImage],
                              collapsed: Bool = false, topAnchor: Bool = false,
                              rightAnchor: Bool = false,
                              savedPaths: [String: String] = [:],
                              copy: @escaping (String) -> Void = { _ in },
                              save: @escaping (String) -> Void = { _ in },
                              open: @escaping (String) -> Void = { _ in },
                              trash: @escaping (String) -> Void = { _ in },
                              dismiss: @escaping (String) -> Void = { _ in },
                              discard: @escaping (String) -> Void = { _ in },
                              setCollapsed: @escaping (Bool) -> Void = { _ in },
                              move: @escaping (NSPoint) -> Void = { _ in }) -> MiniPreviewPanel {
        let stack = NativePreviewStack()
        ids.forEach { XCTAssertTrue(stack.insert($0)) }
        if collapsed { stack.setCollapsed(true) }
        let placement = (topAnchor ? "top_" : "bottom_") + (rightAnchor ? "right" : "left")
        let monitor = CapturesPreviewMonitor(work_x: 0, work_y: 0,
            work_width: 800, work_height: 700, full_x: 0, full_y: 0,
            full_width: 800, full_height: 700, scale_factor: 1)
        let geometry = NativePreviewLayout.geometry(monitor: monitor, count: ids.count,
            collapsed: collapsed, placement: placement)!
        let resources = Dictionary(uniqueKeysWithValues: ids.compactMap { id in
            images[id].map { (id, MiniPreviewResource(artifact: artifact(id: id,
                previewPath: "/\(id)-preview.png", savedPath: savedPaths[id]), image: $0)) }
        })
        let layouts = Dictionary(uniqueKeysWithValues: ids.enumerated().compactMap { index, id in
            stack.cardLayout(index: index, topAnchor: topAnchor).map { (id, $0) }
        })
        let hoverLayouts = Dictionary(uniqueKeysWithValues: ids.enumerated().compactMap { index, id in
            stack.cardLayout(index: index, topAnchor: topAnchor, hovered: true).map { (id, $0) }
        })
        return MiniPreviewPanel(frame: NSRect(x: 0, y: 0,
            width: geometry.width, height: geometry.height), geometry: geometry,
            contentHeight: stack.contentHeight,
            resources: resources, ids: ids, layouts: layouts, hoverLayouts: hoverLayouts, collapsed: collapsed,
            topAnchor: topAnchor, rightAnchor: placement.hasSuffix("_right"), tokens: tokens,
            copy: copy, save: save, open: open,
            trash: trash, dismiss: dismiss, discard: discard, setCollapsed: setCollapsed,
            clearAll: {}, move: move)
    }

    private func previewSettings() -> MiniPreviewSettings {
        MiniPreviewSettings(enabled: true, placement: "bottom_right", includeInCaptures: false)
    }

    private func presentedController(_ captured: CaptureArtifact) throws -> MiniPreviewController {
        let controller = MiniPreviewController(tokens: tokens, imageLoader: { _ in self.solidImage(.systemBlue) })
        let settings = previewSettings()
        let generation = try XCTUnwrap(controller.beginCapture(settings: settings))
        controller.present(captured, on: screenID(), settings: settings, generation: generation)
        try waitUntil { controller.isPanelVisible }
        return controller
    }

    private func solidImage(_ color: NSColor) -> NSImage {
        NSImage(size: NSSize(width: 284, height: 160), flipped: true) { rect in
            color.setFill(); rect.fill(); return true
        }
    }

    private func render(_ panel: MiniPreviewPanel) throws -> NSBitmapImageRep {
        panel.display(); panel.previewView.layoutSubtreeIfNeeded()
        let view = panel.previewView
        // Set the destination profile BEFORE drawing. Display-dependent blending
        // followed by conversion to sRGB does not give the same channel values.
        let bitmap = try XCTUnwrap(view.bitmapImageRepForCachingDisplay(in: view.bounds)?
            .retagging(with: .sRGB))
        view.cacheDisplay(in: view.bounds, to: bitmap)
        return bitmap
    }

    /// A current layer-tree fixture, not cacheDisplay's model-only drawing.
    /// Core Graphics does not reproduce compositor filters/backdrop materials;
    /// this records dust's prefiltered chips, transforms, clipping and opacity.
    private func renderPresentation(_ panel: MiniPreviewPanel) throws -> NSBitmapImageRep? {
        panel.display(); panel.previewView.layoutSubtreeIfNeeded()
        CATransaction.flush()
        RunLoop.current.run(until: Date().addingTimeInterval(0.03))
        guard let presentation = panel.previewView.layer?.presentation() else { return nil }
        // Use an explicit point-to-pixel transform instead of relying on
        // NSGraphicsContext's display-dependent bitmap transform. Start from
        // identity and apply backing scale and top-left orientation once.
        let scale = panel.backingScaleFactor
        let width = Int(ceil(panel.previewView.bounds.width * scale))
        let height = Int(ceil(panel.previewView.bounds.height * scale))
        let context = try XCTUnwrap(CGContext(data: nil, width: width, height: height,
            bitsPerComponent: 8, bytesPerRow: width * 4,
            space: CGColorSpace(name: CGColorSpace.sRGB)!,
            bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue))
        context.translateBy(x: 0, y: CGFloat(height))
        context.scaleBy(x: scale, y: -scale)
        presentation.render(in: context)
        return NSBitmapImageRep(cgImage: try XCTUnwrap(context.makeImage()))
    }

    /// The bitmap pixel under view point (`x`, `y`) from the top left. `render`
    /// caches at the window's backing scale, so a Retina display (2x) has
    /// twice the pixels of a 1x one; sample in points, not raw pixels.
    private func bitmapPoint(in bitmap: NSBitmapImageRep, of panel: MiniPreviewPanel,
                             x: CGFloat, y: CGFloat) -> (x: Int, y: Int) {
        let scale = CGFloat(bitmap.pixelsWide) / panel.previewView.bounds.width
        return (Int(x * scale), Int(y * scale))
    }

    private func pixelColor(in bitmap: NSBitmapImageRep, of panel: MiniPreviewPanel,
                            x: CGFloat, y: CGFloat) -> NSColor? {
        let point = bitmapPoint(in: bitmap, of: panel, x: x, y: y)
        // Both renderers produce 8-bit sRGB pixels. colorAt returns a calibrated
        // NSColor; converting it to sRGB again changes the already-correct bytes.
        var pixel = [Int](repeating: 0, count: bitmap.samplesPerPixel)
        bitmap.getPixel(&pixel, atX: point.x, y: point.y)
        let alpha = CGFloat(pixel[3]) / 255
        let divisor = bitmap.bitmapFormat.contains(.alphaNonpremultiplied)
            ? CGFloat(255) : CGFloat(max(1, pixel[3]))
        return NSColor(srgbRed: CGFloat(pixel[0]) / divisor,
                       green: CGFloat(pixel[1]) / divisor,
                       blue: CGFloat(pixel[2]) / divisor, alpha: alpha)
    }

    private func write(_ bitmap: NSBitmapImageRep, name: String) throws {
        guard let directory = ProcessInfo.processInfo.environment["CAPTURES_TEST_ARTIFACTS"] else { return }
        let url = URL(fileURLWithPath: directory).appendingPathComponent(name)
        try FileManager.default.createDirectory(at: url.deletingLastPathComponent(),
                                                withIntermediateDirectories: true)
        try XCTUnwrap(bitmap.representation(using: .png, properties: [:])).write(to: url)
    }

    private func preferences() throws -> CapturePreferences {
        try CapturePreferences(["auto_copy_to_clipboard": false,
            "show_cursor_in_screenshots": false, "freeze_screen": true,
            "auto_start_on_selection": false, "show_mini_previews": true,
            "mini_preview_placement": "bottom_right",
            "include_mini_previews_in_captures": false,
            "output_directory": "/exports", "screenshot_format": "png",
            "screenshot_countdown_seconds": 0])
    }

    private func screenID() -> String {
        NSScreen.screens.first.flatMap(MiniPreviewController.displayID(for:)) ?? "0"
    }

    private func waitUntil(_ condition: () -> Bool) throws {
        let deadline = Date().addingTimeInterval(5)
        while !condition() && Date() < deadline {
            RunLoop.current.run(until: Date().addingTimeInterval(0.01))
        }
        XCTAssertTrue(condition(), "mini preview operation did not settle")
        guard condition() else { throw AppBridgeError.invalidResponse }
    }
}

private final class MiniPreviewActionTransport: AppTransport {
    private let lock = NSLock()
    private var saves = 0
    private var root: String?
    private var trashes = 0
    private var requests = 0
    private var cleanups = 0
    private var recordedRequest: [String: Any]?
    var fail = false
    var malformed = false
    var gate: DispatchSemaphore?
    var cleanupGate: DispatchSemaphore?
    let started = DispatchSemaphore(value: 0)
    let cleanupStarted = DispatchSemaphore(value: 0)
    var saveCount: Int { lock.lock(); defer { lock.unlock() }; return saves }
    var savedRoot: String? { lock.lock(); defer { lock.unlock() }; return root }
    var trashCount: Int { lock.lock(); defer { lock.unlock() }; return trashes }
    var requestCount: Int { lock.lock(); defer { lock.unlock() }; return requests }
    var cleanupCount: Int { lock.lock(); defer { lock.unlock() }; return cleanups }
    var lastRequest: [String: Any]? { lock.lock(); defer { lock.unlock() }; return recordedRequest }

    func request(_ object: [String: Any]) throws -> [String: Any] {
        lock.lock(); defer { lock.unlock() }
        let operation = object["operation"] as? String
        if operation == "clear_previous_preview_drags" {
            cleanups += 1
            let gate = cleanupGate
            cleanupStarted.signal()
            lock.unlock(); gate?.wait(); lock.lock()
            return ["kind": "previous_preview_drags_cleared"]
        }
        requests += 1; recordedRequest = object
        guard operation == "save_screenshot" || operation == "trash_preview" else {
            throw AppBridgeError.invalidResponse
        }
        if operation == "save_screenshot" { saves += 1 } else { trashes += 1 }
        root = object["root"] as? String; started.signal()
        let gate = self.gate
        lock.unlock(); gate?.wait(); lock.lock()
        if fail { throw AppBridgeError.invalidResponse }
        if operation == "trash_preview" {
            if malformed { return ["kind": "wrong", "id": object["id"] as Any] }
            return ["kind": "preview_trashed", "id": object["id"] as Any]
        }
        return ["path": "/exports/latest.png"]
    }
}

private extension NSView {
    var subviewsRecursive: [NSView] {
        subviews + subviews.flatMap(\.subviewsRecursive)
    }
}
