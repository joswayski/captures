import AppKit
import ImageIO
import XCTest

@MainActor
final class EditorModelTests: XCTestCase {
    func testRecordingEstimateLifecycleSchedulesForEitherProbeAndAppearanceOrder() {
        var probeFirst = RecordingEstimateLifecycle()
        XCTAssertFalse(probeFirst.probeChanged(isReady: true))
        XCTAssertTrue(probeFirst.viewAppeared(probeReady: true))
        XCTAssertFalse(probeFirst.probeChanged(isReady: true))

        var appearanceFirst = RecordingEstimateLifecycle()
        XCTAssertFalse(appearanceFirst.viewAppeared(probeReady: false))
        XCTAssertTrue(appearanceFirst.probeChanged(isReady: true))
        XCTAssertFalse(appearanceFirst.probeChanged(isReady: true))

        appearanceFirst.viewDisappeared()
        XCTAssertTrue(appearanceFirst.viewAppeared(probeReady: true))
    }

    func testViewportEventRoutingConsumesHandledEventAndPreservesUnhandledEvent() throws {
        let event = try XCTUnwrap(NSEvent.otherEvent(
            with: .applicationDefined,
            location: CGPoint(x: 31, y: 47),
            modifierFlags: [],
            timestamp: 0,
            windowNumber: 0,
            context: nil,
            subtype: 0,
            data1: 0,
            data2: 0
        ))

        XCTAssertNil(routeEditorViewportEvent(event) { _ in nil })
        XCTAssertTrue(routeEditorViewportEvent(event) { $0 } === event)
        XCTAssertTrue(routeEditorViewportEvent(event, handler: nil) === event)
    }

    func testViewportZoomKeepsAsymmetricDocumentPointUnderPointerAfterRelayout() {
        let anchor = CGPoint(x: 317, y: 229)
        let documentPoint = EditorViewportMath.documentPoint(
            anchor: anchor,
            canvasOrigin: CGPoint(x: 83, y: 41),
            zoom: 0.65
        )
        let relaidOutOrigin = CGPoint(x: -127, y: 76)
        let correction = EditorViewportMath.anchorCorrection(
            anchor: anchor,
            canvasOrigin: relaidOutOrigin,
            documentPoint: documentPoint,
            zoom: 1.7
        )

        XCTAssertEqual(
            relaidOutOrigin.x + correction.width + documentPoint.x * 1.7,
            anchor.x,
            accuracy: 0.0001
        )
        XCTAssertEqual(
            relaidOutOrigin.y + correction.height + documentPoint.y * 1.7,
            anchor.y,
            accuracy: 0.0001
        )
    }

    func testViewportComposesTwoAnchorZoomsBeforeLayoutUpdates() {
        let first = EditorViewportMath.pendingAnchor(
            replacing: nil,
            viewportPoint: CGPoint(x: 100, y: 100),
            measuredCanvasOrigin: .zero,
            currentZoom: 1,
            nextZoom: 2
        )
        let sameAnchor = EditorViewportMath.pendingAnchor(
            replacing: first,
            viewportPoint: CGPoint(x: 100, y: 100),
            measuredCanvasOrigin: .zero,
            currentZoom: 2,
            nextZoom: 4
        )
        XCTAssertEqual(sameAnchor.documentPoint.x, 100, accuracy: 0.0001)
        XCTAssertEqual(sameAnchor.documentPoint.y, 100, accuracy: 0.0001)

        let movedAnchor = EditorViewportMath.pendingAnchor(
            replacing: first,
            viewportPoint: CGPoint(x: 160, y: 130),
            measuredCanvasOrigin: .zero,
            currentZoom: 2,
            nextZoom: 4
        )
        XCTAssertEqual(movedAnchor.documentPoint.x, 130, accuracy: 0.0001)
        XCTAssertEqual(movedAnchor.documentPoint.y, 115, accuracy: 0.0001)

        let relaidOutOrigin = CGPoint(x: -370, y: -281)
        let correction = EditorViewportMath.anchorCorrection(
            anchor: movedAnchor.viewportPoint,
            canvasOrigin: relaidOutOrigin,
            documentPoint: movedAnchor.documentPoint,
            zoom: movedAnchor.zoom
        )
        XCTAssertEqual(
            relaidOutOrigin.x + correction.width + movedAnchor.documentPoint.x * movedAnchor.zoom,
            movedAnchor.viewportPoint.x,
            accuracy: 0.0001
        )
        XCTAssertEqual(
            relaidOutOrigin.y + correction.height + movedAnchor.documentPoint.y * movedAnchor.zoom,
            movedAnchor.viewportPoint.y,
            accuracy: 0.0001
        )
        XCTAssertFalse(EditorViewportMath.frame(
            CGRect(x: -100, y: -100, width: 800, height: 600),
            matches: CGSize(width: 400, height: 300),
            zoom: movedAnchor.zoom
        ), "An intermediate 2× layout must not resolve the pending 4× anchor")
        XCTAssertTrue(EditorViewportMath.frame(
            CGRect(x: -370, y: -281, width: 1_600, height: 1_200),
            matches: CGSize(width: 400, height: 300),
            zoom: movedAnchor.zoom
        ))
    }

    func testViewportRetainsVirtualAnchorBetweenPanCorrectionAndGeometryConfirmation() throws {
        let zoomTwo = EditorViewportMath.pendingAnchor(
            replacing: nil,
            viewportPoint: CGPoint(x: 100, y: 100),
            measuredCanvasOrigin: .zero,
            currentZoom: 1,
            nextZoom: 2
        )
        let firstLayout = EditorViewportMath.resolvePendingAnchor(zoomTwo, canvasOrigin: .zero)
        XCTAssertEqual(firstLayout.correction.width, -100, accuracy: 0.0001)
        XCTAssertEqual(firstLayout.correction.height, -100, accuracy: 0.0001)
        let retained = try XCTUnwrap(firstLayout.pending)

        let zoomFourBeforeCorrectedLayout = EditorViewportMath.pendingAnchor(
            replacing: retained,
            viewportPoint: CGPoint(x: 100, y: 100),
            measuredCanvasOrigin: .zero,
            currentZoom: 2,
            nextZoom: 4
        )
        XCTAssertEqual(zoomFourBeforeCorrectedLayout.documentPoint.x, 100, accuracy: 0.0001)
        XCTAssertEqual(zoomFourBeforeCorrectedLayout.documentPoint.y, 100, accuracy: 0.0001)

        let duplicateOldFrame = EditorViewportMath.resolvePendingAnchor(retained, canvasOrigin: .zero)
        XCTAssertEqual(duplicateOldFrame.correction, .zero, "Do not apply the same pan correction twice")
        XCTAssertNotNil(duplicateOldFrame.pending)
        let confirmed = EditorViewportMath.resolvePendingAnchor(retained, canvasOrigin: CGPoint(x: -100, y: -100))
        XCTAssertEqual(confirmed.correction, .zero)
        XCTAssertNil(confirmed.pending, "Only corrected geometry clears the virtual anchor")
    }

    func testViewportZoomBoundsAndOppositeWheelDeltas() {
        XCTAssertEqual(EditorViewportMath.clampedZoom(0.001), 0.05)
        XCTAssertEqual(EditorViewportMath.clampedZoom(12), 8)
        let zoomIn = EditorViewportMath.wheelZoomFactor(scrollingDeltaY: 73, precise: true)
        let zoomOut = EditorViewportMath.wheelZoomFactor(scrollingDeltaY: -73, precise: true)
        XCTAssertGreaterThan(zoomIn, 1)
        XCTAssertEqual(zoomIn * zoomOut, 1, accuracy: 0.0001)
        XCTAssertEqual(
            EditorViewportMath.wheelZoomFactor(scrollingDeltaY: 1000, precise: true),
            EditorViewportMath.wheelZoomFactor(scrollingDeltaY: 240, precise: true),
            accuracy: 0.0001
        )
        let asymmetricZoom: CGFloat = 1.75
        XCTAssertEqual(
            EditorViewportMath.zoom(forSliderPosition: EditorViewportMath.sliderPosition(for: asymmetricZoom)),
            asymmetricZoom,
            accuracy: 0.0001
        )
        XCTAssertGreaterThan(EditorViewportMath.sliderPosition(for: 1), 0.45)
        XCTAssertLessThan(EditorViewportMath.sliderPosition(for: 1), 0.7)
    }

    func testViewportRecenterThresholdDistinguishesAreaFromThinSliver() {
        let viewport = CGSize(width: 800, height: 600)
        XCTAssertFalse(EditorViewportMath.isMostlyOffscreen(
            viewportSize: .zero,
            canvasFrame: CGRect(x: 901, y: -200, width: 400, height: 300)
        ))
        XCTAssertFalse(EditorViewportMath.isMostlyOffscreen(
            viewportSize: viewport,
            canvasFrame: .zero
        ))
        XCTAssertFalse(EditorViewportMath.isMostlyOffscreen(
            viewportSize: viewport,
            canvasFrame: CGRect(x: -317, y: 71, width: 400, height: 300)
        ))
        XCTAssertTrue(EditorViewportMath.isMostlyOffscreen(
            viewportSize: viewport,
            canvasFrame: CGRect(x: 789, y: 541, width: 400, height: 300)
        ))
        XCTAssertTrue(EditorViewportMath.isMostlyOffscreen(
            viewportSize: viewport,
            canvasFrame: CGRect(x: 901, y: -200, width: 400, height: 300)
        ))
    }

    func testNewAnnotationsUseToolDefaults() {
        let model = EditorModel(
            document: EditorDocument(width: 400, height: 300, layers: []),
            sourceURL: URL(fileURLWithPath: "/tmp/source.png")
        )
        let color = EditorColor(red: 0.24, green: 0.48, blue: 0.95)

        model.addShape(.rectangle, color: color, lineWidth: 11, fill: true, opacity: 0.65)
        let shape = model.document.layers[0]
        XCTAssertEqual(shape.color, color)
        XCTAssertEqual(shape.fill, color)
        XCTAssertEqual(shape.lineWidth, 11)
        XCTAssertEqual(shape.opacity, 0.65)

        model.addShape(.arrow, color: color, lineWidth: 9, fill: true, opacity: 0.8)
        let arrow = model.document.layers[1]
        XCTAssertNil(arrow.fill, "Open shapes never use the closed-shape fill default")
        XCTAssertEqual(arrow.lineWidth, 9)

        model.addStroke(
            [CGPoint(x: 10, y: 12), CGPoint(x: 40, y: 42)],
            color: color, lineWidth: 7, opacity: 0.45
        )
        let stroke = model.document.layers[2]
        XCTAssertEqual(stroke.color, color)
        XCTAssertEqual(stroke.lineWidth, 7)
        XCTAssertEqual(stroke.opacity, 0.45)
    }

    func testCropTranslatesLayersAndSupportsUndo() {
        let source = EditorLayer(
            name: "Source",
            content: .shape(.rectangle),
            frame: EditorRect(x: 10, y: 20, width: 80, height: 60),
            locked: true
        )
        let annotation = EditorLayer(
            name: "Arrow",
            content: .shape(.arrow),
            frame: EditorRect(x: 40, y: 50, width: 40, height: 20)
        )
        let model = EditorModel(
            document: EditorDocument(width: 200, height: 120, layers: [source, annotation]),
            sourceURL: URL(fileURLWithPath: "/tmp/source.png")
        )

        model.crop(to: CGRect(x: 25, y: 30, width: 100, height: 70))

        XCTAssertEqual(model.document.width, 100)
        XCTAssertEqual(model.document.height, 70)
        XCTAssertEqual(model.document.layers[0].frame.x, -15)
        XCTAssertEqual(model.document.layers[0].frame.y, -10)
        XCTAssertEqual(model.document.layers[1].frame.x, 15)
        XCTAssertEqual(model.document.layers[1].frame.y, 20)

        model.undo()
        XCTAssertEqual(model.document.width, 200)
        XCTAssertEqual(model.document.layers[1].frame, annotation.frame)
        XCTAssertTrue(model.canRedo)
    }

    func testLockedLayerCannotBeDeletedOrReordered() {
        let locked = EditorLayer(
            name: "Original", content: .shape(.rectangle),
            frame: EditorRect(x: 0, y: 0, width: 100, height: 100), locked: true
        )
        let editable = EditorLayer(
            name: "Text", content: .text("hello"),
            frame: EditorRect(x: 10, y: 10, width: 30, height: 20)
        )
        let model = EditorModel(
            document: EditorDocument(width: 100, height: 100, layers: [locked, editable]),
            sourceURL: URL(fileURLWithPath: "/tmp/source.png")
        )

        model.selectedLayerID = locked.id
        model.deleteSelected()
        model.reorderSelected(by: 1)
        model.updateSelected { $0.rotation = .pi }
        model.beginInteractiveEdit()
        model.updateSelectedLive { $0.frame.width = 31 }
        model.endInteractiveEdit()

        XCTAssertEqual(model.document.layers.map(\.id), [locked.id, editable.id])
        XCTAssertEqual(model.document.layers[0], locked)
        XCTAssertFalse(model.canUndo)
    }

    func testTranslatedLayerSnapsNearestEdgesAndReportsGuides() {
        let moving = EditorLayer(
            name: "Moving", content: .shape(.rectangle),
            frame: EditorRect(x: 10, y: 30, width: 10, height: 10)
        )
        let peer = EditorLayer(
            name: "Peer", content: .shape(.rectangle),
            frame: EditorRect(x: 80, y: 60, width: 20, height: 20)
        )
        let model = EditorModel(
            document: EditorDocument(width: 200, height: 120, layers: [moving, peer]),
            sourceURL: URL(fileURLWithPath: "/tmp/snap.png")
        )

        let snapped = model.snapTranslatedFrame(
            EditorRect(x: 67, y: 33, width: 10, height: 10),
            layerID: moving.id, threshold: 4
        )

        XCTAssertEqual(snapped.frame, EditorRect(x: 70, y: 33, width: 10, height: 10))
        XCTAssertEqual(snapped.guides, [EditorAlignmentGuide(axis: .vertical, position: 80)])
    }

    func testCanvasExpansionShiftsAllLayersAndFitsAsymmetricOverflow() throws {
        let anchor = EditorLayer(
            name: "Anchor", content: .shape(.rectangle),
            frame: EditorRect(x: 20, y: 15, width: 10, height: 10)
        )
        let overflow = EditorLayer(
            name: "Overflow", content: .shape(.rectangle),
            frame: EditorRect(x: -7.2, y: 85, width: 120.6, height: 24.4)
        )
        let model = EditorModel(
            document: EditorDocument(width: 100, height: 100, layers: [anchor, overflow]),
            sourceURL: URL(fileURLWithPath: "/tmp/expand.png")
        )

        XCTAssertEqual(
            model.canvasExpansion(for: overflow.id),
            CGRect(x: -8, y: 0, width: 122, height: 110)
        )
        try model.expandCanvasToFit(layerID: overflow.id)

        XCTAssertEqual(model.document.width, 122)
        XCTAssertEqual(model.document.height, 110)
        XCTAssertEqual(model.document.layers[0].frame, EditorRect(x: 28, y: 15, width: 10, height: 10))
        let expandedOverflow = model.document.layers[1].frame
        XCTAssertEqual(expandedOverflow.x, 0.8, accuracy: 0.0001)
        XCTAssertEqual(expandedOverflow.y, 85)
        XCTAssertEqual(expandedOverflow.width, 120.6)
        XCTAssertEqual(expandedOverflow.height, 24.4)
        XCTAssertNil(model.canvasExpansion(for: overflow.id))
        model.undo()
        XCTAssertEqual(model.document.layers, [anchor, overflow])
    }

    func testCanvasExpansionAcceptsExactLimitButRejectsEitherOversizedDimensionAtomically() throws {
        let atLimit = EditorLayer(
            name: "At limit", content: .shape(.rectangle),
            frame: EditorRect(x: -0.5, y: -0.5, width: 10, height: 10)
        )
        let accepted = EditorModel(
            document: EditorDocument(width: 16_383, height: 16_383, layers: [atLimit]),
            sourceURL: URL(fileURLWithPath: "/tmp/expand-limit.png")
        )

        try accepted.expandCanvasToFit(layerID: atLimit.id)

        XCTAssertEqual(accepted.document.width, 16_384)
        XCTAssertEqual(accepted.document.height, 16_384)
        XCTAssertEqual(accepted.document.layers[0].frame.x, 0.5, accuracy: 0.0001)
        XCTAssertEqual(accepted.document.layers[0].frame.y, 0.5, accuracy: 0.0001)

        for (width, height, frame) in [
            (16_384, 100, EditorRect(x: -0.5, y: 10, width: 10, height: 10)),
            (100, 16_384, EditorRect(x: 10, y: -0.5, width: 10, height: 10)),
        ] {
            let overflow = EditorLayer(name: "Overflow", content: .shape(.rectangle), frame: frame)
            let rejected = EditorModel(
                document: EditorDocument(width: width, height: height, layers: [overflow]),
                sourceURL: URL(fileURLWithPath: "/tmp/expand-over-limit.png")
            )
            let original = rejected.document

            XCTAssertThrowsError(try rejected.expandCanvasToFit(layerID: overflow.id)) { error in
                XCTAssertEqual(error as? EditorError, EditorError.canvasTooLarge)
            }
            XCTAssertEqual(rejected.document, original)
            XCTAssertFalse(rejected.canUndo)
            XCTAssertFalse(rejected.dirty)
        }
    }

    func testRotateClockwisePreservesLocalFrameAndTransformsWorldBounds() {
        let layer = EditorLayer(
            name: "Layer", content: .shape(.ellipse),
            frame: EditorRect(x: 10, y: 20, width: 30, height: 40)
        )
        let model = EditorModel(
            document: EditorDocument(width: 200, height: 100, layers: [layer]),
            sourceURL: URL(fileURLWithPath: "/tmp/source.png")
        )

        model.rotateCanvas(clockwise: true)

        XCTAssertEqual(model.document.width, 100)
        XCTAssertEqual(model.document.height, 200)
        let clockwise = model.document.layers[0]
        XCTAssertEqual(clockwise.frame, EditorRect(x: 45, y: 5, width: 30, height: 40))
        XCTAssertEqual(clockwise.rotation, .pi / 2, accuracy: 0.0001)
        assertRect(worldBounds(of: clockwise), equals: CGRect(x: 40, y: 10, width: 40, height: 30))

        model.rotateCanvas(clockwise: false)
        XCTAssertEqual(model.document.width, 200)
        XCTAssertEqual(model.document.height, 100)
        XCTAssertEqual(model.document.layers[0].frame, layer.frame)
        XCTAssertEqual(model.document.layers[0].rotation, 0, accuracy: 0.0001)

        model.undo()
        XCTAssertEqual(model.document.layers[0], clockwise)
        model.undo()
        XCTAssertEqual(model.document.layers[0], layer)
    }

    func testPixelRendererUsesExactDimensionsAndRotatesAsymmetricColorsClockwise() throws {
        let red = try solidPNG(.red, rgba: [255, 0, 0, 255])
        let blue = try solidPNG(.blue, rgba: [0, 0, 255, 255])
        let layers = [
            EditorLayer(name: "top-left", content: .image(red, original: red), frame: EditorRect(x: 0, y: 0, width: 1, height: 1)),
            EditorLayer(name: "bottom-right", content: .image(blue, original: blue), frame: EditorRect(x: 3, y: 2, width: 1, height: 1)),
        ]
        let model = EditorModel(
            document: EditorDocument(width: 4, height: 3, layers: layers),
            sourceURL: URL(fileURLWithPath: "/tmp/source.png")
        )

        model.rotateCanvas(clockwise: true)
        let rendered = try XCTUnwrap(model.renderedImage().cgImage(forProposedRect: nil, context: nil, hints: nil))
        XCTAssertEqual(try pixel(rendered, x: 2, y: 0), [255, 0, 0, 255], "Rendered red before encoding")
        XCTAssertEqual(try pixel(rendered, x: 0, y: 3), [0, 0, 255, 255], "Rendered blue before encoding")
        let png = try model.exportData(format: "png")
        let image = try decodePNG(png)

        XCTAssertEqual(image.width, 3)
        XCTAssertEqual(image.height, 4)
        // Bitmap pixel coordinates start at the top left: clockwise maps
        // top-left to top-right and bottom-right to bottom-left.
        XCTAssertEqual(try pixel(image, x: 2, y: 0), [255, 0, 0, 255], "Exported red")
        XCTAssertEqual(try pixel(image, x: 0, y: 3), [0, 0, 255, 255], "Exported blue")
    }

    func testEraseInverseTransformsRotatedLayerAndEditsLockedBackground() throws {
        let opaque = try solidPNG(.white, rgba: [255, 255, 255, 255], width: 5, height: 3)
        let frame = EditorRect(x: 10, y: 20, width: 50, height: 30)
        let layer = EditorLayer(name: "Rotated", content: .image(opaque, original: opaque), frame: frame, rotation: .pi / 2)
        let model = EditorModel(
            document: EditorDocument(width: 80, height: 80, layers: [layer]),
            sourceURL: URL(fileURLWithPath: "/tmp/source.png")
        )
        model.selectedLayerID = layer.id

        // Local point (15, 35) rotates to world point (35, 15), outside the
        // unrotated frame. Erasing it therefore proves inverse hit testing.
        try model.erase(at: CGPoint(x: 35, y: 15), radius: 5, restore: false)
        guard case let .image(edited, _) = model.document.layers[0].content else {
            return XCTFail("Expected image layer")
        }
        let editedImage = try decodePNG(edited)
        XCTAssertEqual(try pixel(editedImage, x: 0, y: 1), [0, 0, 0, 0])
        XCTAssertEqual(try pixel(editedImage, x: 4, y: 0), [255, 255, 255, 255])

        try model.erase(at: CGPoint(x: 35, y: 15), radius: 5, restore: true)
        guard case let .image(restored, _) = model.document.layers[0].content else {
            return XCTFail("Expected restored image layer")
        }
        XCTAssertEqual(try pixel(decodePNG(restored), x: 0, y: 1), [255, 255, 255, 255])

        var locked = layer
        locked.locked = true
        let lockedModel = EditorModel(
            document: EditorDocument(width: 80, height: 80, layers: [locked]),
            sourceURL: URL(fileURLWithPath: "/tmp/source.png")
        )
        lockedModel.selectedLayerID = locked.id
        try lockedModel.erase(at: CGPoint(x: 35, y: 15), radius: 5, restore: false)
        guard case let .image(lockedEdited, _) = lockedModel.document.layers[0].content else {
            return XCTFail("Expected locked image layer to remain an image")
        }
        XCTAssertEqual(try pixel(decodePNG(lockedEdited), x: 0, y: 1), [0, 0, 0, 0])
        XCTAssertTrue(lockedModel.canUndo)
    }

    func testEraseSoftnessFeathersTheBrushEdge() throws {
        let opaque = try solidPNG(.white, rgba: [255, 255, 255, 255], width: 9, height: 9)
        let layer = EditorLayer(
            name: "Background", content: .image(opaque, original: opaque),
            frame: EditorRect(x: 0, y: 0, width: 9, height: 9)
        )
        let model = EditorModel(
            document: EditorDocument(width: 9, height: 9, layers: [layer]),
            sourceURL: URL(fileURLWithPath: "/tmp/soft-erase.png")
        )
        model.selectedLayerID = layer.id

        try model.erase(at: CGPoint(x: 4.5, y: 4.5), radius: 4.5, softness: 1, restore: false)

        guard case let .image(edited, _) = model.document.layers[0].content else {
            return XCTFail("Expected edited image layer")
        }
        let image = try decodePNG(edited)
        XCTAssertEqual(try pixel(image, x: 4, y: 4), [0, 0, 0, 0])
        let feathered = try pixel(image, x: 1, y: 4)
        XCTAssertGreaterThan(feathered[3], 0)
        XCTAssertLessThan(feathered[3], 255)
        XCTAssertLessThanOrEqual(abs(Int(feathered[0]) - Int(feathered[3])), 1)
    }

    func testEraseStrokeInterpolatesAcrossFastPointerMovementAsOneUndo() throws {
        let opaque = try solidPNG(.white, rgba: [255, 255, 255, 255], width: 21, height: 5)
        let layer = EditorLayer(
            name: "Background", content: .image(opaque, original: opaque),
            frame: EditorRect(x: 0, y: 0, width: 21, height: 5), locked: true
        )
        let model = EditorModel(
            document: EditorDocument(width: 21, height: 5, layers: [layer]),
            sourceURL: URL(fileURLWithPath: "/tmp/interpolated-erase.png")
        )
        model.selectedLayerID = layer.id
        model.beginInteractiveEdit()

        try model.eraseStroke(
            from: CGPoint(x: 2.5, y: 2.5), to: CGPoint(x: 18.5, y: 2.5),
            radius: 1, softness: 0, restore: false
        )
        model.endInteractiveEdit()

        guard case let .image(edited, _) = model.document.layers[0].content else {
            return XCTFail("Expected edited image layer")
        }
        let image = try decodePNG(edited)
        for x in 2...18 {
            XCTAssertEqual(try pixel(image, x: x, y: 2), [0, 0, 0, 0], "Gap at x=\(x)")
        }
        model.undo()
        XCTAssertEqual(model.document.layers[0], layer)
        XCTAssertFalse(model.canUndo)
    }

    func testEraseStrokeSafelyClipsSegmentsThatLeaveAndReenterImage() throws {
        let opaque = try solidPNG(.white, rgba: [255, 255, 255, 255], width: 21, height: 5)
        let layer = EditorLayer(
            name: "Background", content: .image(opaque, original: opaque),
            frame: EditorRect(x: 0, y: 0, width: 21, height: 5)
        )
        let model = EditorModel(
            document: EditorDocument(width: 21, height: 5, layers: [layer]),
            sourceURL: URL(fileURLWithPath: "/tmp/clipped-erase.png")
        )
        model.selectedLayerID = layer.id
        model.beginInteractiveEdit()
        try model.eraseStroke(
            from: CGPoint(x: 2.5, y: 2.5), to: CGPoint(x: 100, y: 2.5),
            radius: 1, restore: false
        )
        try model.eraseStroke(
            from: CGPoint(x: 100, y: 2.5), to: CGPoint(x: 18.5, y: 2.5),
            radius: 1, restore: false
        )
        model.endInteractiveEdit()
        guard case let .image(edited, _) = model.document.layers[0].content else {
            return XCTFail("Expected edited image layer")
        }
        XCTAssertEqual(try pixel(decodePNG(edited), x: 20, y: 2), [0, 0, 0, 0])

        let outside = EditorModel(
            document: EditorDocument(width: 21, height: 5, layers: [layer]),
            sourceURL: URL(fileURLWithPath: "/tmp/outside-erase.png")
        )
        outside.selectedLayerID = layer.id
        try outside.eraseStroke(
            from: CGPoint(x: 100, y: 100), to: CGPoint(x: 120, y: 120),
            radius: 1, restore: false
        )
        XCTAssertEqual(outside.document.layers[0], layer)
        XCTAssertFalse(outside.canUndo)
    }

    func testImportedFileDraftSurvivesANewArtifactIdentity() throws {
        let directory = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: directory) }
        let source = directory.appendingPathComponent("imported.png")
        let original = try solidPNG(.blue, rgba: [0, 0, 255, 255], width: 7, height: 3)
        try original.write(to: source)
        let first = try EditorModel(artifact: Artifact(path: source.path, kind: "image"))
        defer { first.clearDraft() }
        first.resizeCanvas(width: 11, height: 5)
        let reopened = try EditorModel(artifact: Artifact(path: source.path, kind: "image"))
        XCTAssertTrue(reopened.restoredDraft)
        XCTAssertEqual(reopened.document.width, 11)
        XCTAssertEqual(reopened.document.height, 5)
        XCTAssertEqual(try Data(contentsOf: source), original)
    }

    func testReferenceModelsRemainIndependentWhenBuiltInEitherStateOrder() throws {
        let directory = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: directory) }
        let source = directory.appendingPathComponent("shared-reference.png")
        try solidPNG(.blue, rgba: [0, 0, 255, 255], width: 7, height: 3).write(to: source)
        let artifact = Artifact(path: source.path, kind: "image", width: 7, height: 3)

        let propertiesFirst = try makeImageEditorReferenceModel(artifact: artifact)
        defer { propertiesFirst.clearDraft() }
        propertiesFirst.addShape(.rectangle, at: CGPoint(x: 2, y: 1))
        let overflowSecond = try makeImageEditorReferenceModel(artifact: artifact)
        defer { overflowSecond.clearDraft() }

        XCTAssertEqual(propertiesFirst.document.layers.count, 2)
        XCTAssertEqual(overflowSecond.document.layers.count, 1)
        XCTAssertFalse(overflowSecond.restoredDraft)

        overflowSecond.addShape(.rectangle, at: CGPoint(x: 8, y: 1))
        let propertiesThird = try makeImageEditorReferenceModel(artifact: artifact)
        defer { propertiesThird.clearDraft() }
        XCTAssertEqual(propertiesThird.document.layers.count, 1)
        XCTAssertFalse(propertiesThird.restoredDraft)
    }

    func testBackgroundWandRemovesOnlyConnectedMatchingColorAndSupportsUndo() throws {
        let source = try splitPNG()
        let layer = EditorLayer(
            name: "Background", content: .image(source, original: source),
            frame: EditorRect(x: 0, y: 0, width: 4, height: 2), locked: true
        )
        let model = EditorModel(
            document: EditorDocument(width: 4, height: 2, layers: [layer]),
            sourceURL: URL(fileURLWithPath: "/tmp/wand.png")
        )
        model.selectedLayerID = layer.id

        try model.removeBackgroundColor(at: CGPoint(x: 0.5, y: 0.5), tolerance: 0.01)

        guard case let .image(edited, _) = model.document.layers[0].content else {
            return XCTFail("Expected edited image layer")
        }
        let image = try decodePNG(edited)
        XCTAssertEqual(try pixel(image, x: 0, y: 0), [0, 0, 0, 0])
        XCTAssertEqual(try pixel(image, x: 3, y: 0), [0, 0, 255, 255])
        model.undo()
        XCTAssertEqual(model.document.layers[0], layer)
    }

    func testBackgroundWandCanRemoveDisconnectedMatchingColors() throws {
        let source = try separatedColorPNG()
        let layer = EditorLayer(
            name: "Background", content: .image(source, original: source),
            frame: EditorRect(x: 0, y: 0, width: 5, height: 1)
        )
        let model = EditorModel(
            document: EditorDocument(width: 5, height: 1, layers: [layer]),
            sourceURL: URL(fileURLWithPath: "/tmp/global-wand.png")
        )
        model.selectedLayerID = layer.id

        try model.removeBackgroundColor(
            at: CGPoint(x: 0.5, y: 0.5), tolerance: 0.01, contiguous: false
        )

        guard case let .image(edited, _) = model.document.layers[0].content else {
            return XCTFail("Expected edited image layer")
        }
        let image = try decodePNG(edited)
        XCTAssertEqual(try pixel(image, x: 0, y: 0), [0, 0, 0, 0])
        XCTAssertEqual(try pixel(image, x: 2, y: 0), [0, 0, 255, 255])
        XCTAssertEqual(try pixel(image, x: 4, y: 0), [0, 0, 0, 0])
    }

    func testLayerShadowRendersOutsideShapeBounds() throws {
        let layer = EditorLayer(
            name: "Shadowed", content: .shape(.rectangle),
            frame: EditorRect(x: 4, y: 4, width: 5, height: 5),
            fill: .white,
            shadow: EditorShadow(radius: 0, offsetX: 4, offsetY: 0, opacity: 1)
        )
        let model = EditorModel(
            document: EditorDocument(width: 16, height: 16, layers: [layer]),
            sourceURL: URL(fileURLWithPath: "/tmp/shadow.png")
        )
        let image = try XCTUnwrap(model.renderedImage().cgImage(forProposedRect: nil, context: nil, hints: nil))
        XCTAssertGreaterThan(try pixel(image, x: 11, y: 6)[3], 0)
    }

    func testLayerBlendModesMatchShippingSetAndMultiplyPixels() throws {
        XCTAssertEqual(
            Set(EditorBlendMode.allCases.map(\.rawValue)),
            Set(["source-over", "multiply", "screen", "overlay", "darken", "lighten"])
        )
        let bottomData = try solidPNG(
            NSColor(srgbRed: 64.0 / 255, green: 128.0 / 255, blue: 192.0 / 255, alpha: 1),
            rgba: [64, 128, 192, 255], width: 2, height: 2
        )
        let topData = try solidPNG(
            NSColor(srgbRed: 128.0 / 255, green: 64.0 / 255, blue: 192.0 / 255, alpha: 1),
            rgba: [128, 64, 192, 255], width: 2, height: 2
        )
        let frame = EditorRect(x: 0, y: 0, width: 2, height: 2)
        let model = EditorModel(
            document: EditorDocument(width: 2, height: 2, layers: [
                EditorLayer(name: "Bottom", content: .image(bottomData, original: bottomData), frame: frame),
                EditorLayer(
                    name: "Top", content: .image(topData, original: topData), frame: frame,
                    blendMode: .multiply
                ),
            ]),
            sourceURL: URL(fileURLWithPath: "/tmp/blend.png")
        )

        let image = try XCTUnwrap(model.renderedImage().cgImage(forProposedRect: nil, context: nil, hints: nil))
        try assertPixel(image, x: 0, y: 0, approximately: [32, 32, 145, 255], tolerance: 3)
    }

    func testImageOpacityRendersAndMergeDownRasterizesStackOrderAsOneUndoableImage() throws {
        let bottomData = try solidPNG(
            NSColor(srgbRed: 200.0 / 255, green: 40.0 / 255, blue: 20.0 / 255, alpha: 1),
            rgba: [200, 40, 20, 255], width: 2, height: 2
        )
        let topData = try solidPNG(
            NSColor(srgbRed: 20.0 / 255, green: 80.0 / 255, blue: 220.0 / 255, alpha: 1),
            rgba: [20, 80, 220, 255], width: 2, height: 2
        )
        let frame = EditorRect(x: 0, y: 0, width: 2, height: 2)
        let bottom = EditorLayer(name: "Photo", content: .image(bottomData, original: bottomData), frame: frame)
        let top = EditorLayer(
            name: "Tint", content: .image(topData, original: topData), frame: frame, opacity: 0.5
        )
        let original = EditorDocument(width: 2, height: 2, layers: [bottom, top])
        let model = EditorModel(document: original, sourceURL: URL(fileURLWithPath: "/tmp/merge.png"))
        model.selectedLayerID = top.id

        let preview = try XCTUnwrap(model.renderedImage().cgImage(forProposedRect: nil, context: nil, hints: nil))
        try assertPixel(preview, x: 1, y: 1, approximately: [110, 60, 120, 255], tolerance: 3)
        XCTAssertTrue(model.canMergeSelectedDown)
        try model.mergeSelectedDown()

        XCTAssertEqual(model.document.layers.count, 1)
        XCTAssertEqual(model.document.layers[0].name, "Photo")
        XCTAssertEqual(model.document.layers[0].frame, frame)
        XCTAssertEqual(model.selectedLayerID, model.document.layers[0].id)
        let merged = try XCTUnwrap(model.renderedImage().cgImage(forProposedRect: nil, context: nil, hints: nil))
        try assertPixel(merged, x: 1, y: 1, approximately: [110, 60, 120, 255], tolerance: 3)
        model.undo()
        XCTAssertEqual(model.document, original)

        var lockedBottom = bottom
        lockedBottom.locked = true
        let lockedDocument = EditorDocument(width: 2, height: 2, layers: [lockedBottom, top])
        let lockedModel = EditorModel(
            document: lockedDocument,
            sourceURL: URL(fileURLWithPath: "/tmp/locked-merge.png")
        )
        lockedModel.selectedLayerID = top.id
        XCTAssertFalse(lockedModel.canMergeSelectedDown)
        try lockedModel.mergeSelectedDown()
        XCTAssertEqual(lockedModel.document, lockedDocument)
    }

    func testMergeVisibleKeepsHiddenLayersInPlaceAndFlattenDiscardsThem() throws {
        let red = try solidPNG(.red, rgba: [255, 0, 0, 255], width: 2, height: 2)
        let blue = try solidPNG(.blue, rgba: [0, 0, 255, 255], width: 1, height: 2)
        let hiddenBelow = EditorLayer(
            name: "Hidden below", content: .image(red, original: red),
            frame: EditorRect(x: 0, y: 0, width: 2, height: 2), visible: false
        )
        let visibleBottom = EditorLayer(
            name: "Visible bottom", content: .image(red, original: red),
            frame: EditorRect(x: 0, y: 0, width: 1, height: 2)
        )
        let hiddenMiddle = EditorLayer(
            name: "Hidden middle", content: .image(blue, original: blue),
            frame: EditorRect(x: 1, y: 0, width: 1, height: 2), visible: false
        )
        let visibleTop = EditorLayer(
            name: "Visible top", content: .image(blue, original: blue),
            frame: EditorRect(x: 1, y: 0, width: 1, height: 2)
        )
        let original = EditorDocument(
            width: 3, height: 2,
            layers: [hiddenBelow, visibleBottom, hiddenMiddle, visibleTop],
            background: .white
        )
        let model = EditorModel(document: original, sourceURL: URL(fileURLWithPath: "/tmp/visible.png"))

        XCTAssertTrue(model.canMergeVisible)
        try model.mergeVisible()
        XCTAssertEqual(model.document.layers.count, 3)
        XCTAssertEqual(model.document.layers[0].id, hiddenBelow.id)
        XCTAssertEqual(model.document.layers[1].name, "Merged")
        XCTAssertEqual(model.document.layers[2].id, hiddenMiddle.id)
        XCTAssertFalse(model.document.layers[0].visible)
        XCTAssertFalse(model.document.layers[2].visible)
        let merged = try XCTUnwrap(model.renderedImage().cgImage(forProposedRect: nil, context: nil, hints: nil))
        try assertPixel(merged, x: 0, y: 0, approximately: [255, 0, 0, 255], tolerance: 1)
        try assertPixel(merged, x: 1, y: 0, approximately: [0, 0, 255, 255], tolerance: 1)
        model.undo()
        XCTAssertEqual(model.document, original)

        XCTAssertTrue(model.canFlatten)
        try model.flatten()
        XCTAssertNil(model.document.background)
        XCTAssertEqual(model.document.layers.count, 1)
        XCTAssertEqual(model.document.layers[0].name, "Flattened")
        XCTAssertTrue(model.document.layers[0].locked)
        let flattened = try XCTUnwrap(model.renderedImage().cgImage(forProposedRect: nil, context: nil, hints: nil))
        try assertPixel(flattened, x: 0, y: 0, approximately: [255, 0, 0, 255], tolerance: 1)
        try assertPixel(flattened, x: 1, y: 0, approximately: [0, 0, 255, 255], tolerance: 1)
        try assertPixel(flattened, x: 2, y: 0, approximately: [255, 255, 255, 255], tolerance: 1)
        model.undo()
        XCTAssertEqual(model.document, original)
    }

    func testLegacyDraftWithoutBackgroundOrShadowStillDecodes() throws {
        let document = EditorDocument(width: 8, height: 6, layers: [
            EditorLayer(name: "Shape", content: .shape(.ellipse), frame: EditorRect(x: 1, y: 1, width: 4, height: 3)),
        ])
        var json = try XCTUnwrap(JSONSerialization.jsonObject(with: JSONEncoder().encode(document)) as? [String: Any])
        json.removeValue(forKey: "background")
        var layers = try XCTUnwrap(json["layers"] as? [[String: Any]])
        layers[0].removeValue(forKey: "shadow")
        layers[0].removeValue(forKey: "blendMode")
        json["layers"] = layers

        let decoded = try JSONDecoder().decode(EditorDocument.self, from: JSONSerialization.data(withJSONObject: json))

        XCTAssertNil(decoded.background)
        XCTAssertNil(decoded.layers[0].shadow)
        XCTAssertNil(decoded.layers[0].blendMode)
        XCTAssertEqual(decoded.width, 8)
    }

    func testExportRefusesExistingFileWithoutChangingIt() throws {
        let directory = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: directory) }
        let output = directory.appendingPathComponent("existing.png")
        let sentinel = Data("do not replace".utf8)
        try sentinel.write(to: output)
        let layer = EditorLayer(
            name: "Layer", content: .shape(.rectangle),
            frame: EditorRect(x: 0, y: 0, width: 16, height: 16), fill: .signal
        )
        let model = EditorModel(
            document: EditorDocument(width: 16, height: 16, layers: [layer]),
            sourceURL: directory.appendingPathComponent("source.png")
        )

        XCTAssertThrowsError(try model.writeExport(to: output, format: "png")) { error in
            XCTAssertEqual(error as? EditorError, .existingFile)
        }
        XCTAssertEqual(try Data(contentsOf: output), sentinel)
    }

    private func worldBounds(of layer: EditorLayer) -> CGRect {
        let frame = layer.frame.cgRect
        let center = CGPoint(x: frame.midX, y: frame.midY)
        let cosine = cos(layer.rotation)
        let sine = sin(layer.rotation)
        let corners = [
            CGPoint(x: frame.minX, y: frame.minY), CGPoint(x: frame.maxX, y: frame.minY),
            CGPoint(x: frame.maxX, y: frame.maxY), CGPoint(x: frame.minX, y: frame.maxY),
        ].map { point -> CGPoint in
            let dx = point.x - center.x
            let dy = point.y - center.y
            return CGPoint(x: center.x + dx * cosine - dy * sine, y: center.y + dx * sine + dy * cosine)
        }
        let xs = corners.map(\.x)
        let ys = corners.map(\.y)
        return CGRect(x: xs.min()!, y: ys.min()!, width: xs.max()! - xs.min()!, height: ys.max()! - ys.min()!)
    }

    private func assertRect(_ actual: CGRect, equals expected: CGRect, file: StaticString = #filePath, line: UInt = #line) {
        XCTAssertEqual(actual.minX, expected.minX, accuracy: 0.0001, file: file, line: line)
        XCTAssertEqual(actual.minY, expected.minY, accuracy: 0.0001, file: file, line: line)
        XCTAssertEqual(actual.width, expected.width, accuracy: 0.0001, file: file, line: line)
        XCTAssertEqual(actual.height, expected.height, accuracy: 0.0001, file: file, line: line)
    }

    private func assertPixel(
        _ image: CGImage, x: Int, y: Int, approximately expected: [UInt8], tolerance: Int,
        file: StaticString = #filePath, line: UInt = #line
    ) throws {
        let actual = try pixel(image, x: x, y: y)
        XCTAssertEqual(actual.count, expected.count, file: file, line: line)
        for (component, pair) in zip(["red", "green", "blue", "alpha"], zip(actual, expected)) {
            XCTAssertLessThanOrEqual(
                abs(Int(pair.0) - Int(pair.1)), tolerance,
                "\(component): expected \(pair.1), got \(pair.0)",
                file: file, line: line
            )
        }
    }

    private func solidPNG(_ color: NSColor, rgba: [UInt8], width: Int = 1, height: Int = 1) throws -> Data {
        let space = try XCTUnwrap(CGColorSpace(name: CGColorSpace.sRGB))
        let context = try XCTUnwrap(CGContext(
            data: nil, width: width, height: height, bitsPerComponent: 8,
            bytesPerRow: width * 4, space: space,
            bitmapInfo: CGBitmapInfo.byteOrder32Big.rawValue | CGImageAlphaInfo.premultipliedLast.rawValue
        ))
        // Use an explicit Core Graphics color space rather than AppKit's legacy
        // device-RGB bitmap initializer/setColor path. Verify the encoded input.
        context.setFillColor(try XCTUnwrap(color.usingColorSpace(.sRGB)).cgColor)
        context.fill(CGRect(x: 0, y: 0, width: width, height: height))
        let original = try XCTUnwrap(context.makeImage())
        XCTAssertEqual(try pixel(original, x: 0, y: 0), rgba, "Fixture before PNG encoding")
        let rep = NSBitmapImageRep(cgImage: original)
        let png = try XCTUnwrap(rep.representation(using: .png, properties: [:]))
        let decoded = try decodePNG(png)
        XCTAssertEqual(try pixel(decoded, x: 0, y: 0), rgba, "Fixture after PNG encoding")
        return png
    }

    private func decodePNG(_ data: Data) throws -> CGImage {
        let source = try XCTUnwrap(CGImageSourceCreateWithData(data as CFData, nil))
        return try XCTUnwrap(CGImageSourceCreateImageAtIndex(source, 0, nil))
    }

    private func splitPNG() throws -> Data {
        let context = try XCTUnwrap(CGContext(
            data: nil, width: 4, height: 2, bitsPerComponent: 8, bytesPerRow: 16,
            space: CGColorSpaceCreateDeviceRGB(),
            bitmapInfo: CGBitmapInfo.byteOrder32Big.rawValue | CGImageAlphaInfo.premultipliedLast.rawValue
        ))
        context.setFillColor(NSColor.yellow.cgColor)
        context.fill(CGRect(x: 0, y: 0, width: 3, height: 2))
        context.setFillColor(NSColor.blue.cgColor)
        context.fill(CGRect(x: 3, y: 0, width: 1, height: 2))
        let image = try XCTUnwrap(context.makeImage())
        return try XCTUnwrap(NSBitmapImageRep(cgImage: image).representation(using: .png, properties: [:]))
    }

    private func separatedColorPNG() throws -> Data {
        let context = try XCTUnwrap(CGContext(
            data: nil, width: 5, height: 1, bitsPerComponent: 8, bytesPerRow: 20,
            space: CGColorSpaceCreateDeviceRGB(),
            bitmapInfo: CGBitmapInfo.byteOrder32Big.rawValue | CGImageAlphaInfo.premultipliedLast.rawValue
        ))
        context.setFillColor(NSColor.yellow.cgColor)
        context.fill(CGRect(x: 0, y: 0, width: 2, height: 1))
        context.fill(CGRect(x: 3, y: 0, width: 2, height: 1))
        context.setFillColor(NSColor.blue.cgColor)
        context.fill(CGRect(x: 2, y: 0, width: 1, height: 1))
        let image = try XCTUnwrap(context.makeImage())
        return try XCTUnwrap(NSBitmapImageRep(cgImage: image).representation(using: .png, properties: [:]))
    }

    /// Honor the decoded profile, then inspect literal sRGB RGBA8 bytes without
    /// NSBitmapImageRep.colorAt's additional NSColor color-space interpretation.
    private func pixel(_ image: CGImage, x: Int, y: Int) throws -> [UInt8] {
        guard (0..<image.width).contains(x), (0..<image.height).contains(y) else {
            XCTFail("Pixel (\(x), \(y)) outside \(image.width) × \(image.height)")
            return []
        }
        let space = try XCTUnwrap(CGColorSpace(name: CGColorSpace.sRGB))
        let context = try XCTUnwrap(CGContext(
            data: nil, width: image.width, height: image.height, bitsPerComponent: 8,
            bytesPerRow: image.width * 4, space: space,
            bitmapInfo: CGBitmapInfo.byteOrder32Big.rawValue | CGImageAlphaInfo.premultipliedLast.rawValue
        ))
        context.setBlendMode(.copy)
        context.interpolationQuality = .none
        context.draw(image, in: CGRect(x: 0, y: 0, width: image.width, height: image.height))
        let bytes = try XCTUnwrap(context.data).assumingMemoryBound(to: UInt8.self)
        let offset = y * context.bytesPerRow + x * 4
        return withExtendedLifetime(context) { (0..<4).map { bytes[offset + $0] } }
    }
}

/// Direct swiftc entry point. Compile this file together with EditorModel.swift
/// and the files that define Artifact/AppStore, excluding the application's @main.
@main
@MainActor
struct EditorModelTestRunner {
    static func main() {
        let reporter = FailureReporter()
        XCTestObservationCenter.shared.addTestObserver(reporter)
        let suite = XCTestSuite(forTestCaseClass: EditorModelTests.self)
        suite.run()
        guard let run = suite.testRun, run.executionCount > 0 else {
            fputs("EditorModelTests: no tests executed\n", stderr)
            exit(1)
        }
        print("EditorModelTests: \(run.executionCount) executed, \(run.totalFailureCount) failures")
        exit(run.hasSucceeded && run.executionCount == suite.testCaseCount ? 0 : 1)
    }

    private final class FailureReporter: NSObject, XCTestObservation {
        func testCase(_ testCase: XCTestCase, didFailWithDescription description: String,
                      inFile filePath: String?, atLine lineNumber: Int) {
            fputs("\(testCase.name): \(filePath ?? "unknown"):\(lineNumber): \(description)\n", stderr)
        }
    }
}
