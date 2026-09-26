import AppKit
import XCTest
@testable import CapturesNative

final class NativeMotionTests: XCTestCase {
    private var tokens: Tokens { Tokens.variants["dark-mustard"]! }

    func testCatalogCarriesEveryShippingAnimationFromTheSharedABI() throws {
        let catalog = NativeMotion.catalog
        for name in ["update_notice_in", "update_notice_restart_exit", "startup_notice_in",
                     "startup_notice_in_from_below", "recording_saved_lifecycle",
                     "recording_controls_hidden_lifecycle", "preview_card_arrive",
                     "capture_menu_options_arrive", "countdown_in", "countdown_content_in",
                     "countdown_out", "countdown_content_out", "popover_in"] {
            let spec = try XCTUnwrap(catalog.keyframes[name], name)
            XCTAssertEqual(spec.frames.first?.offset, 0, name)
            XCTAssertEqual(spec.frames.last?.offset, 1, name)
        }
        for name in ["segmented_indicator", "history_card_hover", "tooltip"] {
            XCTAssertNotNil(catalog.transitions[name], name)
        }
        let pop = try XCTUnwrap(catalog.keyframes["update_notice_in"])
        XCTAssertEqual(pop.duration, .token("dur-4"))
        XCTAssertEqual(pop.easing, .token("ease-out"))
        XCTAssertEqual(pop.frames[0].translateY, -4)
        XCTAssertEqual(pop.frames[0].opacity, 0)
        let arrive = try XCTUnwrap(catalog.keyframes["preview_card_arrive"])
        XCTAssertEqual(arrive.duration, .millis(520))
        XCTAssertEqual(arrive.easing, .bezier([0.22, 0.65, 0.28, 1]))
        let saved = try XCTUnwrap(catalog.keyframes["recording_saved_lifecycle"])
        XCTAssertEqual(saved.restOffsets.first, 0.025, accuracy: 1e-9)
        XCTAssertEqual(saved.restOffsets.last, 0.86, accuracy: 1e-9)
    }

    func testTokensResolveDurationsAndEasings() {
        XCTAssertEqual(NativeMotion.seconds(.token("dur-4"), tokens: tokens), 0.28, accuracy: 1e-9)
        XCTAssertEqual(NativeMotion.seconds(.millis(520), tokens: tokens), 0.52, accuracy: 1e-9)
        XCTAssertEqual(tokens.easing("ease-out"), [0.16, 1, 0.3, 1])
        let function = NativeMotion.timingFunction(.token("ease-standard"), tokens: tokens)
        var point: [Float] = [0, 0]
        function.getControlPoint(at: 1, values: &point)
        XCTAssertEqual(point[0], 0.2, accuracy: 1e-6)
        XCTAssertEqual(point[1], 0.8, accuracy: 1e-6)
        let tween = NativeMotion.transition("segmented_indicator", tokens: tokens, reduced: false)
        XCTAssertEqual(tween.duration, 0.28, accuracy: 1e-9)
        XCTAssertEqual(NativeMotion.transition("segmented_indicator", tokens: tokens, reduced: true).duration, 0)
        XCTAssertEqual(NativeMotion.exitDuration("recording_saved_lifecycle", tokens: tokens), 2.1, accuracy: 1e-6)
        XCTAssertEqual(NativeMotion.exitDuration("recording_controls_hidden_lifecycle", tokens: tokens),
                       1.2, accuracy: 1e-6)
    }

    func testPlayIsPresentationOnlyAndReducedMotionSkipsEntrances() throws {
        let view = NSView(frame: NSRect(x: 10, y: 20, width: 200, height: 100))
        let seconds = NativeMotion.play("update_notice_in", on: view, tokens: tokens, reduced: false)
        XCTAssertEqual(seconds, 0.28, accuracy: 1e-6)
        let layer = try XCTUnwrap(view.layer)
        let group = try XCTUnwrap(layer.animation(forKey: NativeMotion.animationKey) as? CAAnimationGroup)
        XCTAssertTrue(group.isRemovedOnCompletion)
        XCTAssertEqual(group.animations?.count, 2)
        // Model values stay at rest, so geometry reads are unaffected.
        XCTAssertEqual(view.frame, NSRect(x: 10, y: 20, width: 200, height: 100))
        XCTAssertEqual(layer.opacity, 1)
        XCTAssertTrue(CATransform3DIsIdentity(layer.transform))
        NativeMotion.cancel(on: view)
        XCTAssertFalse(NativeMotion.isPlaying(on: view))

        XCTAssertEqual(NativeMotion.play("update_notice_in", on: view, tokens: tokens, reduced: true), 0)
        XCTAssertFalse(NativeMotion.isPlaying(on: view), "reduced motion settles entrances at rest")
        XCTAssertEqual(NativeMotion.playEntrance("recording_saved_lifecycle", on: view, tokens: tokens,
                                                 reduced: true), 0)
        XCTAssertEqual(NativeMotion.playExit("recording_saved_lifecycle", on: view, tokens: tokens,
                                             reduced: true), 0, "lifecycle notices stay visible")
    }

    func testExitKeepsItsDelayAndHoldsTheFinalFrameEvenUnderReducedMotion() throws {
        let view = NSView(frame: NSRect(x: 0, y: 0, width: 300, height: 200))
        let seconds = NativeMotion.play("update_notice_restart_exit", on: view, tokens: tokens,
                                        holdEnd: true, reduced: true)
        XCTAssertEqual(seconds, 3, accuracy: 0.001, "shipping keeps the 3 s delay")
        let group = try XCTUnwrap(view.layer?.animation(forKey: NativeMotion.animationKey))
        XCTAssertFalse(group.isRemovedOnCompletion)
        XCTAssertEqual(group.fillMode, .both)
        XCTAssertEqual(view.layer?.opacity, 1)
    }

    func testLifecycleSegmentsCoverEntranceAndExit() throws {
        let view = NSView(frame: NSRect(x: 0, y: 0, width: 440, height: 116))
        XCTAssertEqual(NativeMotion.playEntrance("recording_saved_lifecycle", on: view, tokens: tokens,
                                                 reduced: false), 0.375, accuracy: 1e-6)
        let entrance = try XCTUnwrap(view.layer?.animation(forKey: NativeMotion.animationKey) as? CAAnimationGroup)
        let opacity = try XCTUnwrap(entrance.animations?.first as? CAKeyframeAnimation)
        XCTAssertEqual(opacity.values as? [Double], [0, 1])
        XCTAssertEqual(NativeMotion.playExit("recording_saved_lifecycle", on: view, tokens: tokens,
                                             reduced: false), 2.1, accuracy: 1e-6)
        let exit = try XCTUnwrap(view.layer?.animation(forKey: NativeMotion.animationKey) as? CAAnimationGroup)
        let fade = try XCTUnwrap(exit.animations?.first as? CAKeyframeAnimation)
        XCTAssertEqual(fade.values as? [Double], [1, 0])
        XCTAssertFalse(exit.isRemovedOnCompletion)
    }

    func testTransformScalesAboutTheCentreForEitherAnchor() {
        let layer = CALayer()
        layer.bounds = CGRect(x: 0, y: 0, width: 100, height: 40)
        layer.anchorPoint = .zero
        let frame = MotionKeyframes.Frame(offset: 0, opacity: 0, translateY: 8, scale: 0.5)
        let flipped = NativeMotion.transform(frame, layer: layer, down: 1)
        let centre = CGPoint(x: 50, y: 20).applying(CATransform3DGetAffineTransform(flipped))
        XCTAssertEqual(centre.x, 50, accuracy: 1e-9)
        XCTAssertEqual(centre.y, 28, accuracy: 1e-9)
        let upward = NativeMotion.transform(frame, layer: layer, down: -1)
        XCTAssertEqual(CGPoint(x: 50, y: 20).applying(CATransform3DGetAffineTransform(upward)).y,
                       12, accuracy: 1e-9)
        layer.anchorPoint = CGPoint(x: 0.5, y: 0.5)
        let centred = NativeMotion.transform(frame, layer: layer, down: 1)
        XCTAssertEqual(CGPoint.zero.applying(CATransform3DGetAffineTransform(centred)).y, 8, accuracy: 1e-9)
    }

    func testSavedNoticeEntranceLeavesThePanelGeometryUntouched() throws {
        let screen = try XCTUnwrap(NSScreen.main)
        let controller = RecordingSavedNoticeController(tokens: tokens)
        controller.present(artifactID: "recording", screen: screen)
        let panel = try XCTUnwrap(controller.panel)
        XCTAssertEqual(panel.frame.size, NSSize(width: 440, height: 116))
        XCTAssertEqual(panel.alphaValue, 1)
        XCTAssertEqual(panel.noticeView.layer?.opacity, 1)
        XCTAssertEqual(NativeMotion.isPlaying(on: panel.noticeView), !NativeMotion.reduceMotion)
        controller.dismiss()
        XCTAssertNil(controller.panel)
    }
}
