import AppKit
import XCTest
@testable import CapturesNative

final class EditorPickersTests: XCTestCase {
    private let shippingSwatches = ["#ff3b5c", "#ff8a22", "#ffd22e", "#36c96b",
                                    "#2d9cff", "#8b5cf6", "#111318", "#ffffff"]

    func testSwatchPaletteAndCopyComeFromSharedChrome() {
        XCTAssertEqual(EditorColors.swatches, shippingSwatches)
        XCTAssertEqual(EditorColors.defaultCanvasBackground, "#f7f7f5")
        XCTAssertEqual(EditorColors.text("solid_background"), "Solid background")
        XCTAssertEqual(EditorColors.text("canvas_background"), "Canvas background")
        XCTAssertEqual(EditorColors.text("custom_color"), "Custom color")
        XCTAssertEqual(EditorColors.text("background_tooltip"), "Canvas background color")
        XCTAssertEqual(EditorColors.metric("tile"), 24)
        XCTAssertEqual(EditorColors.metric("cell"), 44)
        XCTAssertEqual(EditorColors.metric("compact_cell"), 36)
        XCTAssertEqual(EditorColors.metric("menu_width"), 248)
        XCTAssertEqual(EditorColors.backgroundLabel("#2d9cff"), "Background color: #2d9cff")
        XCTAssertEqual(EditorColors.backgroundLabel(nil), "Background color: transparent")
        XCTAssertEqual(EditorColors.swatchLabel("Fill color", "#ffffff"), "Fill color: #ffffff")
        XCTAssertTrue(EditorColors.swatchActive("#FF3B5C", "#ff3b5c"))
        XCTAssertTrue(EditorColors.swatchActive("#ff3b5c80", "#ff3b5c"))
        XCTAssertFalse(EditorColors.swatchActive("#ff3b5", "#ff3b5c"))
        XCTAssertFalse(EditorColors.swatchActive("#ffffff", "#ff3b5c"))
    }

    func testSwatchGridMatchesSharedAutoFill() {
        let card = EditorColors.Grid(width: 224, minCell: 36, count: 9)
        XCTAssertEqual(card.columns, 6); XCTAssertEqual(card.rows, 2)
        XCTAssertEqual(card.cellWidth, 224 / 6, accuracy: 1e-9)
        XCTAssertEqual(card.height(tile: 24, rowGap: 6, padding: 4), 62)
        XCTAssertEqual(card.center(6, tile: 24, rowGap: 6, padding: 4).y, 46)
        let panel = EditorColors.Grid(width: 272, minCell: 44, count: 9)
        XCTAssertEqual(panel.columns, 6); XCTAssertEqual(panel.rows, 2)
        let wide = EditorColors.Grid(width: 500, minCell: 44, count: 9)
        XCTAssertEqual(wide.columns, 9); XCTAssertEqual(wide.rows, 1)
        XCTAssertEqual(wide.cellWidth, 500 / 11, accuracy: 1e-9, "auto-fill keeps empty tracks")
        for width in [0, -5, CGFloat.nan, 10] as [CGFloat] {
            let narrow = EditorColors.Grid(width: width, minCell: 44, count: 9)
            XCTAssertEqual(narrow.columns, 1); XCTAssertEqual(narrow.rows, 9)
            XCTAssertTrue(narrow.cellWidth.isFinite && narrow.cellWidth > 0)
        }
    }

    func testSwatchRowLayoutSelectionAndAccessibility() throws {
        _ = NSApplication.shared
        for appearance in ["light", "dark"] {
            let tokens = try XCTUnwrap(Tokens.variants["\(appearance)-mustard"])
            let row = ColorSwatchRow(tokens: tokens, label: "Stroke color", compact: false)
            row.frame = NSRect(x: 0, y: 0, width: 272,
                               height: ColorSwatchRow.height(width: 272, compact: false, tokens: tokens))
            XCTAssertEqual(row.frame.height, 6 + 24 + 8 + 24 + 6, "two rows of six at 272 pt")
            XCTAssertEqual(row.swatchButtons.map(\.swatchHex), shippingSwatches)
            XCTAssertEqual(row.swatchButtons.map { $0.accessibilityLabel() ?? "" },
                           shippingSwatches.map { "Stroke color: \($0)" })
            XCTAssertEqual(row.customWell.accessibilityLabel(), "Custom color")
            XCTAssertEqual(row.accessibilityRole(), .radioGroup)
            // Frames depend only on width and tokens (font-independent).
            let tiles: [NSView] = row.swatchButtons + [row.customWell]
            let grid = ColorSwatchRow.grid(width: 272, compact: false)
            for (index, tile) in tiles.enumerated() {
                let center = grid.center(index, tile: 24, rowGap: 8, padding: 6)
                XCTAssertEqual(tile.frame.midX, center.x, accuracy: 0.001)
                XCTAssertEqual(tile.frame.midY, center.y, accuracy: 0.001)
                XCTAssertTrue(row.bounds.insetBy(dx: -4, dy: -4).contains(tile.frame))
            }
            XCTAssertEqual(tiles[6].frame.midY, tiles[0].frame.midY + 32, "the seventh tile wraps")

            var chosen: [String] = []
            row.changed = { chosen.append($0) }
            row.selectedHex = "#FF3B5C80"
            XCTAssertEqual(row.swatchButtons.map(\.active), [true] + Array(repeating: false, count: 7))
            XCTAssertTrue(chosen.isEmpty, "showing a value is not a choice")
            row.swatchButtons[4].performClick(nil)
            XCTAssertEqual(chosen, ["#2d9cff"])
            XCTAssertTrue(row.swatchButtons[4].active); XCTAssertFalse(row.swatchButtons[0].active)
            XCTAssertEqual(row.swatchButtons[4].accessibilityValue() as? Bool, true)
            row.customWell.color = NSColor(srgbRed: 0.2, green: 0.4, blue: 0.6, alpha: 1)
            _ = row.customWell.sendAction(row.customWell.action, to: row.customWell.target)
            XCTAssertEqual(chosen.last, "#336699", "custom colors are lowercase #rrggbb")
            XCTAssertFalse(row.swatchButtons.contains(where: \.active))
            row.isEnabled = false
            XCTAssertTrue(tiles.allSatisfy { ($0 as? NSControl)?.isEnabled == false })
            row.swatchButtons[0].performClick(nil)
            XCTAssertEqual(chosen.count, 2, "a disabled row makes no choice")

            let compact = ColorSwatchRow(tokens: tokens, label: "Canvas background", compact: true)
            compact.setFrameSize(NSSize(width: 224, height: 62))
            XCTAssertEqual(ColorSwatchRow.height(width: 224, compact: true, tokens: tokens), 62)
            XCTAssertEqual(compact.swatchButtons[5].frame.midX, 224 / 6 * 5.5, accuracy: 0.001)
            XCTAssertEqual(compact.customWell.frame.midX, 224 / 6 * 2.5, accuracy: 0.001)
            XCTAssertEqual(compact.customWell.frame.midY, 46, accuracy: 0.001, "4 + 24 + 6 + 12")

            let window = NSWindow(contentRect: NSRect(x: 0, y: 0, width: 300, height: 120),
                                  styleMask: [.titled], backing: .buffered, defer: false)
            window.appearance = NSAppearance(named: appearance == "dark" ? .darkAqua : .aqua)
            window.contentView?.addSubview(row)
            row.isEnabled = true; row.selectedHex = "#2d9cff"
            // Render into an sRGB bitmap: the default cache bitmap uses the
            // display's space (Display P3 on MacBooks), which shifts samples.
            let scale = window.backingScaleFactor
            let bitmap = try XCTUnwrap(NSBitmapImageRep(
                bitmapDataPlanes: nil, pixelsWide: Int(row.bounds.width * scale),
                pixelsHigh: Int(row.bounds.height * scale), bitsPerSample: 8, samplesPerPixel: 4,
                hasAlpha: true, isPlanar: false, colorSpaceName: .calibratedRGB,
                bytesPerRow: 0, bitsPerPixel: 0)?.retagging(with: .sRGB))
            bitmap.size = row.bounds.size
            row.cacheDisplay(in: row.bounds, to: bitmap)
            let blue = row.swatchButtons[4].frame
            // The bitmap already contains sRGB bytes. colorAt returns a
            // calibrated NSColor; converting that to sRGB transforms them again.
            var sample = [Int](repeating: 0, count: bitmap.samplesPerPixel)
            bitmap.getPixel(&sample, atX: Int(blue.midX * scale), y: Int(blue.midY * scale))
            XCTAssertEqual(sample[3], 255, "the enabled swatch paints opaquely")
            // #2d9cff per channel; a neighbouring swatch is far outside 0.04.
            XCTAssertEqual(CGFloat(sample[0]) / 255, 45 / 255, accuracy: 0.04, "the swatch paints its color")
            XCTAssertEqual(CGFloat(sample[1]) / 255, 156 / 255, accuracy: 0.04, "the swatch paints its color")
            XCTAssertEqual(CGFloat(sample[2]) / 255, 1, accuracy: 0.04, "the swatch paints its color")
            window.orderOut(nil)
        }
    }

    func testCustomPickerKeepsOpacityOnlyWhereAllowedAndNeverOpensTheSystemPanel() throws {
        _ = NSApplication.shared
        func key(_ code: UInt16, _ characters: String) throws -> NSEvent {
            try XCTUnwrap(NSEvent.keyEvent(with: .keyDown, location: .zero, modifierFlags: [], timestamp: 0,
                                           windowNumber: 0, context: nil, characters: characters,
                                           charactersIgnoringModifiers: characters, isARepeat: false,
                                           keyCode: code))
        }
        for appearance in ["light", "dark"] {
            let tokens = try XCTUnwrap(Tokens.variants["\(appearance)-mustard"])
            let window = NSWindow(contentRect: NSRect(x: 200, y: 400, width: 300, height: 220),
                                  styleMask: [.titled], backing: .buffered, defer: false)
            window.appearance = NSAppearance(named: appearance == "dark" ? .darkAqua : .aqua)
            let row = ColorSwatchRow(tokens: tokens, label: "Stroke color", compact: false)
            let shadow = ColorSwatchRow(tokens: tokens, label: "Shadow color", compact: false, allowsAlpha: false)
            row.setFrameSize(NSSize(width: 272, height: 76)); shadow.setFrameSize(NSSize(width: 272, height: 76))
            shadow.setFrameOrigin(NSPoint(x: 0, y: 100))
            window.contentView?.addSubview(row); window.contentView?.addSubview(shadow)
            var chosen: [String] = [], shadowChosen: [String] = []
            row.changed = { chosen.append($0) }; shadow.changed = { shadowChosen.append($0) }
            row.selectedHex = "#336699"; shadow.selectedHex = "#336699"

            row.customWell.activate(true)
            let picker = try XCTUnwrap(row.customWell.picker)
            XCTAssertTrue(row.customWell.isActive)
            XCTAssertNotNil(picker.window?.parent, "the picker floats in a child panel")
            XCTAssertEqual(picker.frame.size, NSSize(width: 208, height: 264), "picker::width() × height(true)")
            XCTAssertEqual(picker.hue, 210.0 / 360, accuracy: 0.002, "seeded from the selected color")
            XCTAssertEqual(picker.brightness, 0.6, accuracy: 0.002)
            XCTAssertEqual((picker.accessibilityChildren() as? [NSAccessibilityElement])?
                .map { $0.accessibilityLabel() ?? "" }, ["Hue", "Saturation", "Brightness", "Opacity"])
            picker.adjust(.opacity, by: -50)
            XCTAssertEqual(chosen.last, "#33669980", "opacity commits as #rrggbbaa")
            XCTAssertFalse(row.swatchButtons.contains(where: \.active))
            picker.adjust(.brightness, by: 100)
            // `pick_wheel`: 12 o'clock half way out is hue 90°, saturation 0.5.
            let wheel = picker.wheelRect
            let point = picker.convert(NSPoint(x: wheel.midX, y: wheel.midY - wheel.width / 4), to: nil)
            picker.mouseDown(with: try XCTUnwrap(NSEvent.mouseEvent(
                with: .leftMouseDown, location: point, modifierFlags: [], timestamp: 0,
                windowNumber: picker.window?.windowNumber ?? 0, context: nil, eventNumber: 0,
                clickCount: 1, pressure: 1)))
            XCTAssertEqual(chosen.last, "#bfff8080")
            XCTAssertTrue(picker.handleKey(try key(48, "\t")), "Tab moves to the next channel")
            XCTAssertTrue(picker.handleKey(try key(124, "")))
            XCTAssertEqual(picker.saturation, 0.51, accuracy: 0.001, "arrows step the focused channel")
            XCTAssertFalse(picker.handleKey(try key(0, "a")), "other keys pass through")
            if let directory = ProcessInfo.processInfo.environment["CAPTURES_TEST_ARTIFACTS"],
               let surface = picker.superview, let bitmap = surface.bitmapImageRepForCachingDisplay(in: surface.bounds) {
                surface.cacheDisplay(in: surface.bounds, to: bitmap)
                let url = URL(fileURLWithPath: directory).appendingPathComponent("color-picker-\(appearance).png")
                try FileManager.default.createDirectory(at: url.deletingLastPathComponent(),
                                                        withIntermediateDirectories: true)
                try XCTUnwrap(bitmap.representation(using: .png, properties: [:])).write(to: url)
            }
            XCTAssertTrue(picker.handleKey(try key(53, "\u{1b}")))
            XCTAssertFalse(row.customWell.isActive, "Escape closes the picker")
            XCTAssertNil(window.childWindows?.first { $0.contentView?.subviews.first is ColorPickerView })

            shadow.customWell.activate(true)
            let opaque = try XCTUnwrap(shadow.customWell.picker)
            XCTAssertEqual(opaque.frame.height, 236, "no opacity row: picker::height(false)")
            XCTAssertFalse(opaque.channels.contains(.opacity))
            opaque.adjust(.opacity, by: -50); opaque.adjust(.hue, by: 1)
            XCTAssertEqual(shadowChosen.last?.count, 7, "shadow colors stay #rrggbb")
            shadow.deactivate()
            XCTAssertFalse(shadow.customWell.isActive)
            window.orderOut(nil)
        }
        XCTAssertFalse(NSColorPanel.sharedColorPanelExists, "the system Colors panel is never built")
    }

    func testCustomTileConicStopsMatchShipping() {
        func rgb(_ color: NSColor) -> [Int] {
            let value = color.usingColorSpace(.sRGB)!
            return [value.redComponent, value.greenComponent, value.blueComponent].map { Int(($0 * 255).rounded()) }
        }
        XCTAssertEqual(rgb(CustomColorWell.conic(0)), [255, 68, 68])
        XCTAssertEqual(rgb(CustomColorWell.conic(1.0 / 6)), [255, 255, 68])
        XCTAssertEqual(rgb(CustomColorWell.conic(0.5)), [68, 255, 255])
        XCTAssertEqual(rgb(CustomColorWell.conic(1)), rgb(CustomColorWell.conic(0)))
    }

    func testTextStyleChipsUseSharedPresetTreatments() throws {
        let tokens = try XCTUnwrap(Tokens.variants["light-mustard"])
        let presets = try ([
            ["id": "standard", "label": "Standard", "fontFamily": "sans", "outlined": false,
             "roundedBackground": false],
            ["id": "mono-box", "label": "Mono Box", "fontFamily": "mono", "background": "#111318",
             "outlined": false, "roundedBackground": false],
            ["id": "rounded-box", "label": "Rounded Box", "fontFamily": "rounded", "background": "#111318",
             "outlined": false, "roundedBackground": true],
        ] as [[String: Any]]).map { try XCTUnwrap(NativeTextPreset($0)) }
        XCTAssertTrue(TextStyleChip.font(for: presets[1]).isFixedPitch)
        XCTAssertEqual(TextStyleChip.font(for: presets[1]).pointSize, 15)
        XCTAssertEqual(TextStyleChip.font(for: presets[0]).pointSize, 17)
        for preset in presets {
            let image = TextStyleChip.image(for: preset, tokens: tokens)
            XCTAssertEqual(image.size, NSSize(width: 72, height: 24))
            XCTAssertEqual(image.accessibilityDescription, preset.label)
        }
        // Box chips fill with `--solid`; plain chips stay transparent at the corner.
        func corner(_ preset: NativeTextPreset) throws -> NSColor {
            let image = TextStyleChip.image(for: preset, tokens: tokens)
            let bitmap = try XCTUnwrap(NSBitmapImageRep(data: XCTUnwrap(image.tiffRepresentation)))
            return try XCTUnwrap(bitmap.colorAt(x: 1, y: bitmap.pixelsHigh / 2))
        }
        XCTAssertEqual(try corner(presets[0]).alphaComponent, 0, accuracy: 0.01)
        XCTAssertEqual(try corner(presets[1]).alphaComponent, 1, accuracy: 0.01)
    }
}
