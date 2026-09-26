import AppKit
import Metal
import XCTest
import CCapturesSettings
@testable import CapturesNative

/// Preview stack motion from `captures_app::preview_motion`: exits holding
/// their slots, survivors settling, the list ↔ pile flight, the Show less
/// morph, micro-motion and the card warnings.
final class PreviewMotionTests: XCTestCase {
    private let tokens = Tokens.variants["dark-mustard"]!

    func testShippingPreviewMotionCrossesTheABI() throws {
        let catalog = NativeMotion.catalog
        for name in ["preview_dismiss", "preview_dismiss_streak", "preview_delete_fallback",
                     "preview_capture_highlight", "preview_action_icon_pop",
                     "preview_clipboard_chip_arrive", "preview_pile_sparkle",
                     "preview_pile_sparkle_late", "preview_toolbar_in", "preview_toolbar_out",
                     "preview_toolbar_exit", "preview_toolbar_clear"] {
            let spec = try XCTUnwrap(catalog.keyframes[name], name)
            XCTAssertEqual(spec.frames.first?.offset, 0, name)
            XCTAssertEqual(spec.frames.last?.offset, 1, name)
        }
        for name in ["preview_minimize_morph", "preview_minimize_swap", "preview_stack_settle",
                     "preview_stack_fly", "preview_delete_frame_fade"] {
            XCTAssertNotNil(catalog.transitions[name], name)
        }
        let dismiss = try XCTUnwrap(catalog.keyframes["preview_dismiss"])
        XCTAssertEqual(dismiss.duration, .millis(1030))
        XCTAssertEqual(dismiss.frames[1].translateX, -118)
        XCTAssertEqual(dismiss.frames[1].offset, 0.44, accuracy: 1e-9)
        let streak = try XCTUnwrap(catalog.keyframes["preview_dismiss_streak"])
        XCTAssertEqual(streak.frames.last?.scaleX, 1.13)
        XCTAssertEqual(streak.frames.last?.blur, 14)
        let morph = try XCTUnwrap(catalog.transitions["preview_minimize_morph"])
        XCTAssertEqual(morph.duration, .millis(240))
        XCTAssertEqual(morph.easing, .token("ease-out"))

        let tables = PreviewMotionTables.shared
        XCTAssertEqual(tables.dismiss.hold, 1.03, accuracy: 1e-9)
        XCTAssertEqual(tables.dismiss.settleDelay, 0.45, accuracy: 1e-9)
        XCTAssertEqual(tables.dust.hold, 2.9, accuracy: 1e-9)
        XCTAssertEqual(tables.dust.settleDelay, 1.8, accuracy: 1e-9)
        XCTAssertEqual(tables.dustPad, 120)
        XCTAssertEqual(tables.deleteOriginFirstX, 22.5)
        XCTAssertEqual(tables.deleteOriginAfterCloseX, 57.5)
        XCTAssertEqual(tables.early.count, 6)
        XCTAssertEqual(tables.late.count, 5)
        XCTAssertEqual(tables.reach, 96)
        let particles = PreviewMotionTables.dustParticles(card: CGSize(width: 284, height: 160),
            image: CGSize(width: 800, height: 600), origin: CGPoint(x: 22.5, y: 22.5), seed: 3)
        XCTAssertEqual(particles.count, 198, "the shipping 18 × 11 grid")
        XCTAssertTrue(particles.allSatisfy { $0.dy < 0 }, "ash never falls")
        XCTAssertEqual(captures_preview_clear_delay_ms_v1(3, 0, false), 72)
    }

    func testTransformMirrorsTheDismissSlideAndStretchesHorizontally() {
        let layer = CALayer()
        layer.bounds = CGRect(x: 0, y: 0, width: 100, height: 40)
        let slid = MotionKeyframes.Frame(offset: 1, opacity: 0, translateY: 0, scale: 1, translateX: -118)
        XCTAssertEqual(NativeMotion.transform(slid, layer: layer, down: 1).m41, -118)
        XCTAssertEqual(NativeMotion.transform(slid, layer: layer, down: 1, mirrorX: true).m41, 118)
        let stretched = MotionKeyframes.Frame(offset: 1, opacity: 1, translateY: 0, scale: 1, scaleX: 1.13)
        let t = NativeMotion.transform(stretched, layer: layer, down: 1)
        XCTAssertEqual(t.m11, 1.13, accuracy: 1e-9)
        XCTAssertEqual(t.m22, 1, accuracy: 1e-9)
        XCTAssertFalse(slid.isRest)
        XCTAssertTrue(MotionKeyframes.Frame(offset: 0, opacity: 1, translateY: 0, scale: 1).isRest)
    }

    func testSelfDrawnPosesSampleTheSharedKeyframes() throws {
        let start = try XCTUnwrap(NativeMotion.pose("preview_action_icon_pop", at: 0, tokens: tokens, reduced: false))
        XCTAssertEqual(start.opacity, 0.25, accuracy: 1e-9)
        XCTAssertEqual(start.scale, 0.65, accuracy: 1e-9)
        let end = try XCTUnwrap(NativeMotion.pose("preview_action_icon_pop", at: 0.2, tokens: tokens, reduced: false))
        XCTAssertTrue(end.isRest)
        let mid = try XCTUnwrap(NativeMotion.pose("preview_action_icon_pop", at: 0.1, tokens: tokens, reduced: false))
        XCTAssertGreaterThan(mid.scale, 0.65)
        XCTAssertLessThan(mid.scale, 1)
        let reduced = try XCTUnwrap(NativeMotion.pose("preview_action_icon_pop", at: 0, tokens: tokens, reduced: true))
        XCTAssertTrue(reduced.isRest, "reduced motion lands on the final keyframe")
        let late = try XCTUnwrap(NativeMotion.pose("preview_pile_sparkle_late", at: 0.4, tokens: tokens, reduced: false))
        XCTAssertEqual(late.opacity, 0, "the late sparkle waits 0.5 s")
        XCTAssertEqual(NativeMotion.duration("preview_toolbar_in", tokens: tokens, reduced: false), 0.52, accuracy: 1e-9)
        XCTAssertEqual(NativeMotion.ease(CAMediaTimingFunction(name: .linear), 0.3), 0.3, accuracy: 1e-6)
    }

    func testCloseHoldsItsSlotWhileOlderCardsSettleThenRebuilds() throws {
        _ = NSApplication.shared
        try XCTSkipIf(NativeMotion.reduceMotion, "Reduce Motion removes cards at once")
        let controller = try presentedController(ids: ["oldest", "middle", "newest"])
        defer { controller.close() }
        let view = try XCTUnwrap(controller.previewView)
        let oldest = try XCTUnwrap(view.card(for: "oldest"))
        let restY = oldest.frame.minY
        let slot = try XCTUnwrap(view.card(for: "middle")).frame.minY - restY

        controller.dismiss("newest")
        XCTAssertEqual(controller.presentedArtifactIDs, ["oldest", "middle"])
        XCTAssertTrue(controller.isTransitioning)
        XCTAssertTrue(controller.previewView === view, "the stack keeps its layout during the exit")
        XCTAssertEqual(view.renderedArtifactIDs, ["oldest", "middle", "newest"])
        XCTAssertEqual(view.exitingArtifactIDs, ["newest"])
        XCTAssertNil(view.card(for: "newest")?.hitTest(NSPoint(x: 20, y: 20)), "an exiting card ignores input")
        XCTAssertEqual(oldest.frame.minY, restY, "survivors wait for the streak")
        // Bottom-anchored: older cards slide down into the hole.
        try waitUntil { oldest.frame.minY == restY + slot }
        try waitUntil { !controller.isTransitioning && controller.previewView !== view }
        XCTAssertEqual(controller.previewView?.renderedArtifactIDs, ["oldest", "middle"])
    }

    func testDeleteDissolvesIntoDustAndClearAllStreaksEveryCard() throws {
        _ = NSApplication.shared
        try XCTSkipIf(NativeMotion.reduceMotion, "Reduce Motion removes cards at once")
        let controller = try presentedController(ids: ["first", "second", "third"])
        defer { controller.close() }
        let view = try XCTUnwrap(controller.previewView)
        let toolbar = view.stackToolbarButtons
        XCTAssertEqual(toolbar.count, 2)

        controller.dismiss("third", exit: .dust)
        XCTAssertEqual(view.exitingArtifactIDs, ["third"])
        if MTLCreateSystemDefaultDevice() != nil {
            XCTAssertEqual(view.dustChipCount, 198, "the shipping 18 × 11 ash grid")
        }

        controller.clearAll()
        XCTAssertTrue(controller.presentedArtifactIDs.isEmpty)
        XCTAssertEqual(view.exitingArtifactIDs, ["first", "second", "third"])
        XCTAssertTrue(controller.isPanelVisible, "the last streak still plays")
        XCTAssertTrue(toolbar.allSatisfy { !$0.isEnabled }, "the toolbar leaves with the first streak")
        try waitUntil { !controller.isPanelVisible }
    }

    func testShowLessMorphsOverTheShippingTimingOnScreen() throws {
        _ = NSApplication.shared
        try XCTSkipIf(NativeMotion.reduceMotion, "Reduce Motion snaps the morph")
        let controller = try presentedController(ids: ["older", "newer"])
        defer { controller.close() }
        let view = try XCTUnwrap(controller.previewView)
        let minimize = try XCTUnwrap(view.stackToolbarButtons.first { $0.kind == .collapse })
        let rest = minimize.frame
        let event = try XCTUnwrap(NSEvent.mouseEvent(with: .mouseMoved, location: .zero,
            modifierFlags: [], timestamp: 1, windowNumber: view.window?.windowNumber ?? 0, context: nil,
            eventNumber: 0, clickCount: 0, pressure: 0))
        minimize.mouseEntered(with: event)
        XCTAssertTrue(minimize.showsHoverLabel)
        XCTAssertLessThan(minimize.frame.width, 92, "the pill widens instead of jumping")
        try waitUntil { minimize.frame.width == 92 && minimize.morphSwap == 1 }
        XCTAssertEqual(minimize.frame.maxX, rest.maxX, "right-anchored: grows away from the screen edge")
        minimize.mouseExited(with: event)
        try waitUntil { minimize.frame == rest && minimize.morphSwap == 0 }
    }

    func testShowLessAndExpandFlyBetweenTheListAndThePile() throws {
        _ = NSApplication.shared
        try XCTSkipIf(NativeMotion.reduceMotion, "Reduce Motion rebuilds at once")
        let controller = try presentedController(ids: ["older", "newer"])
        defer { controller.close() }
        let list = try XCTUnwrap(controller.previewView)
        controller.setCollapsed(true)
        XCTAssertTrue(controller.isCollapsed)
        XCTAssertTrue(controller.isTransitioning, "cards fly to the pile before the rebuild")
        XCTAssertEqual(list.visibleCardLabelCount, 0, "flying cards are compact")
        try waitUntil { !controller.isTransitioning && controller.previewView !== list }

        controller.setCollapsed(false)
        let expanded = try XCTUnwrap(controller.previewView)
        XCTAssertEqual(expanded.visibleCardLabelCount, 0, "cards fly out of the pile compact")
        try waitUntil { expanded.visibleCardLabelCount == 2 }
    }

    func testCopyFailureWarnsUntilACopyWorksAndHighlightFades() throws {
        _ = NSApplication.shared
        let controller = try presentedController(ids: ["capture"])
        defer { controller.close() }
        XCTAssertNil(controller.warningText(for: "capture"))
        controller.recordCopyResult(artifactID: "capture", succeeded: false)
        XCTAssertEqual(controller.warningText(for: "capture"), "Clipboard unavailable")
        controller.recordCopyResult(artifactID: "capture", succeeded: true)
        XCTAssertNil(controller.warningText(for: "capture"))
        let card = try XCTUnwrap(controller.previewView?.card(for: "capture"))
        XCTAssertEqual(card.isHighlighting, !NativeMotion.reduceMotion,
                       "a new card shows the capture highlight")
        try waitUntil(timeout: 7) { !card.isHighlighting }
    }

    // MARK: - Helpers

    private func presentedController(ids: [String]) throws -> MiniPreviewController {
        let image = NSImage(cgImage: PreviewView.fixtureImage(scale: 1), size: NSSize(width: 284, height: 160))
        let controller = MiniPreviewController(tokens: tokens, imageLoader: { _ in image })
        let settings = MiniPreviewSettings(enabled: true, placement: "bottom_right", includeInCaptures: false)
        for id in ids {
            let generation = try XCTUnwrap(controller.beginCapture(settings: settings))
            controller.present(artifact(id: id), on: screenID(), settings: settings, generation: generation)
            try waitUntil { controller.decodedArtifactIDs.contains(id) }
        }
        try waitUntil { controller.isPanelVisible && controller.previewView?.window?.isVisible == true }
        return controller
    }

    private func artifact(id: String) -> CaptureArtifact {
        CaptureArtifact(["entry": ["id": id, "width": 800, "height": 600,
            "created_at": "2026-09-18T00:00:00Z", "saved_path": NSNull()],
            "image_path": "/\(id).png", "preview_path": "/\(id)-preview.png"])!
    }

    private func screenID() -> String {
        NSScreen.screens.first.flatMap(MiniPreviewController.displayID(for:)) ?? "0"
    }

    private func waitUntil(timeout: TimeInterval = 5, _ condition: () -> Bool) throws {
        let deadline = Date().addingTimeInterval(timeout)
        while !condition() && Date() < deadline {
            RunLoop.current.run(until: Date().addingTimeInterval(0.01))
        }
        XCTAssertTrue(condition(), "preview motion did not settle")
        guard condition() else { throw AppBridgeError.invalidResponse }
    }
}
