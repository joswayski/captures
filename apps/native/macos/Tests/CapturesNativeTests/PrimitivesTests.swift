import AppKit
import XCTest
@testable import CapturesNative

final class PrimitivesTests: XCTestCase {
    private func bitmap(width: Int, height: Int, draw: () -> Void) throws -> NSBitmapImageRep {
        let rep = try XCTUnwrap(NSBitmapImageRep(bitmapDataPlanes: nil, pixelsWide: width, pixelsHigh: height,
            bitsPerSample: 8, samplesPerPixel: 4, hasAlpha: true, isPlanar: false,
            colorSpaceName: .deviceRGB, bytesPerRow: 0, bitsPerPixel: 0))
        NSGraphicsContext.saveGraphicsState()
        NSGraphicsContext.current = NSGraphicsContext(bitmapImageRep: rep)
        draw()
        NSGraphicsContext.restoreGraphicsState()
        return rep
    }

    func testFocusRingIsATranslucentAccentBandInsideTheRect() throws {
        let tokens = try XCTUnwrap(Tokens.variants["light-mustard"])
        let rep = try bitmap(width: 60, height: 30) {
            drawFocusRing(tokens, in: NSRect(x: 0, y: 0, width: 60, height: 30), radius: 6)
        }
        let edge = try XCTUnwrap(rep.colorAt(x: 30, y: 1))
        let middle = try XCTUnwrap(rep.colorAt(x: 30, y: 15))
        XCTAssertGreaterThan(edge.alphaComponent, 0.4, "a 2 pt ring hugs the edge")
        XCTAssertLessThan(edge.alphaComponent, 0.8, "the shipping ring is translucent")
        XCTAssertEqual(middle.alphaComponent, 0, accuracy: 0.01, "the ring leaves the control's face alone")
    }

    func testCaptureButtonReplacesTheSystemRingWithTheTokenRing() throws {
        _ = NSApplication.shared
        let tokens = try XCTUnwrap(Tokens.variants["dark-mustard"])
        let frame = NSRect(x: 0, y: 0, width: 240, height: 120)
        let window = NSWindow(contentRect: frame, styleMask: [.titled], backing: .buffered, defer: false)
        window.isReleasedWhenClosed = false
        defer { window.close() }
        let root = Surface(frame: frame)
        window.contentView = root
        let button = CaptureButton("Send feedback", frame: NSRect(x: 20, y: 20, width: 140, height: 32),
                                   tokens: tokens) {}
        root.addSubview(button)
        XCTAssertEqual(button.focusRingType, .none)
        XCTAssertFalse(button.focusRingVisible, "no ring without keyboard focus")
        XCTAssertEqual(button.accessibilityLabel(), "Send feedback")
    }

    func testTokenScrollersKeepAxesAndRestyleWithTheAppearance() throws {
        _ = NSApplication.shared
        let light = try XCTUnwrap(Tokens.variants["light-mustard"])
        let dark = try XCTUnwrap(Tokens.variants["dark-mustard"])
        let frame = NSRect(x: 0, y: 0, width: 300, height: 200)
        let window = NSWindow(contentRect: frame, styleMask: [.titled], backing: .buffered, defer: false)
        window.isReleasedWhenClosed = false
        defer { window.close() }
        let root = Surface(frame: frame)
        window.contentView = root
        let scroll = NSScrollView(frame: frame)
        scroll.hasVerticalScroller = true
        scroll.documentView = NSView(frame: NSRect(x: 0, y: 0, width: 280, height: 900))
        scroll.useTokenScrollers(light)
        root.addSubview(scroll)
        XCTAssertTrue(scroll.hasVerticalScroller); XCTAssertFalse(scroll.hasHorizontalScroller)
        let vertical = try XCTUnwrap(scroll.verticalScroller as? TokenScroller)
        XCTAssertTrue(scroll.horizontalScroller is TokenScroller)
        XCTAssertEqual(TokenScroller.scrollerWidth(for: .regular, scrollerStyle: .legacy), 10)
        XCTAssertEqual(TokenScroller.scrollerWidth(for: .regular, scrollerStyle: .overlay), 10)
        XCTAssertTrue(TokenScroller.isCompatibleWithOverlayScrollers)
        XCTAssertFalse(vertical.thumbHovered)
        window.display()
        let thumb = vertical.thumbRect
        if thumb.width > 0 {
            XCTAssertEqual(thumb.width, 4, accuracy: 0.5, "a 4 pt thumb inside the 10 pt bar")
        }
        restyleTokenScrollers(in: root, dark)
        XCTAssertTrue(vertical.tokens?.color("text") == dark.color("text"))
    }

    func testTokenSelectOpensShippingListboxWithDescriptionsAndHomeEndKeys() throws {
        _ = NSApplication.shared
        let tokens = try XCTUnwrap(Tokens.variants["light-mustard"])
        let frame = NSRect(x: 0, y: 0, width: 300, height: 120)
        let window = NSWindow(contentRect: frame, styleMask: [.titled], backing: .buffered, defer: false)
        window.isReleasedWhenClosed = false
        defer { window.close() }
        let root = Surface(frame: frame)
        window.contentView = root
        let select = ClosurePopUpButton(frame: NSRect(x: 20, y: 20, width: 200, height: 32), pullsDown: false)
        select.tokens = tokens
        select.addItems(withTitles: ["Preserve quality", "Compress", "Maximum file size"])
        select.item(at: 1)?.toolTip = "Smaller file with Tiny through Highest quality presets."
        root.addSubview(select)
        defer { select.closeListbox() }
        XCTAssertEqual(select.focusRingType, .none, "the token ring replaces the system ring")
        XCTAssertEqual(select.selectStyle, .field)
        var changed: [Int] = []
        select.bindChange { changed.append($0) }
        // A click (or Space) opens shipping's listbox instead of the native menu.
        select.performClick(nil)
        let list = try XCTUnwrap(select.listbox)
        XCTAssertTrue(select.isListboxOpen)
        XCTAssertEqual(list.options.map(\.title), ["Preserve quality", "Compress", "Maximum file size"])
        XCTAssertNil(list.options[0].detail, "items without a description stay plain")
        XCTAssertEqual(list.options[1].detail, "Smaller file with Tiny through Highest quality presets.")
        XCTAssertEqual(TokenSelectListView.rowHeight(list.options[0]), 30)
        XCTAssertEqual(TokenSelectListView.rowHeight(list.options[1]), 46, "the description takes its own line")
        XCTAssertEqual(list.rowRect(1).minY, list.rowRect(0).maxY)
        XCTAssertEqual(list.active, 0, "opening highlights the selected option")
        XCTAssertGreaterThanOrEqual(list.frame.width, select.bounds.width)
        XCTAssertEqual(select.item(at: 1)?.title, "Compress", "titles and accessibility names are unchanged")
        list.display()
        // Shipping `CustomSelect` keys from the shared controls model.
        XCTAssertTrue(select.handleSelectKey("end"))
        XCTAssertEqual(list.active, 2, "End jumps to the last option")
        XCTAssertTrue(select.handleSelectKey("home"))
        XCTAssertEqual(list.active, 0, "Home jumps to the first option")
        XCTAssertTrue(select.handleSelectKey("arrow_down"))
        XCTAssertEqual(list.active, 1)
        XCTAssertTrue(select.handleSelectKey("enter"))
        XCTAssertFalse(select.isListboxOpen)
        XCTAssertEqual(changed, [1])
        XCTAssertEqual(select.titleOfSelectedItem, "Compress")
        XCTAssertFalse(select.handleSelectKey("home"), "Home and End only move an open listbox")
        XCTAssertTrue(select.handleSelectKey("arrow_up"))
        XCTAssertTrue(select.isListboxOpen, "arrows open a closed select")
        XCTAssertEqual(select.listbox?.active, 1, "on the selected option")
        XCTAssertTrue(select.handleSelectKey("escape"))
        XCTAssertFalse(select.isListboxOpen)
        XCTAssertEqual(changed, [1], "Escape closes without choosing")
        let home = try XCTUnwrap(NSEvent.keyEvent(with: .keyDown, location: .zero, modifierFlags: [],
            timestamp: 0, windowNumber: window.windowNumber, context: nil,
            characters: "\u{F729}", charactersIgnoringModifiers: "\u{F729}", isARepeat: false, keyCode: 115))
        select.performClick(nil)
        select.keyDown(with: home)
        XCTAssertEqual(select.listbox?.active, 0, "the Home key event moves the open listbox")
        select.listbox?.choose(2)
        XCTAssertFalse(select.isListboxOpen, "a click on an option chooses it and closes")
        XCTAssertEqual(changed, [1, 2])
        window.display()
    }

    func testTokenSelectAccessibilityRowsAreStableActionableAndTrackActiveSelection() throws {
        _ = NSApplication.shared
        let tokens = try XCTUnwrap(Tokens.variants["light-mustard"])
        for style in [TokenSelectStyle.field, .glass, .inline] {
            let frame = NSRect(x: 0, y: 0, width: 300, height: 120)
            let window = NSWindow(contentRect: frame, styleMask: [.titled], backing: .buffered, defer: false)
            window.isReleasedWhenClosed = false
            defer { window.close() }
            let root = Surface(frame: frame)
            window.contentView = root
            let select = ClosurePopUpButton(frame: NSRect(x: 20, y: 20, width: 200, height: 32), pullsDown: false)
            select.tokens = tokens; select.selectStyle = style
            select.setAccessibilityLabel("Numbers")
            select.addItems(withTitles: ["One", "Two", "Three"])
            select.menu?.autoenablesItems = false
            select.item(at: 1)?.isEnabled = false
            select.item(at: 2)?.toolTip = "The third option"
            root.addSubview(select)
            defer { select.closeListbox() }
            var changed: [Int] = []
            select.bindChange { changed.append($0) }
            _ = window.makeFirstResponder(select)
            let responder = window.firstResponder
            XCTAssertFalse(select.isAccessibilityExpanded())
            XCTAssertTrue(select.accessibilityLinkedUIElements()?.isEmpty == true)
            XCTAssertTrue(select.accessibilityPerformShowMenu())
            let list = try XCTUnwrap(select.listbox)
            XCTAssertEqual(list.accessibilityRole(), .list)
            XCTAssertEqual(list.accessibilityLabel(), "Numbers")
            XCTAssertTrue(select.isAccessibilityExpanded())
            XCTAssertTrue(select.accessibilityLinkedUIElements()?.first as? NSView === list)
            let rows = try XCTUnwrap(list.accessibilityChildren() as? [NSAccessibilityElement])
            let repeated = try XCTUnwrap(list.accessibilityChildren() as? [NSAccessibilityElement])
            XCTAssertEqual(rows.count, 3)
            for index in rows.indices {
                XCTAssertTrue(rows[index] === repeated[index], "AX rows retain their identity while open")
                XCTAssertEqual(rows[index].accessibilityRole(), .row)
                XCTAssertTrue(rows[index].accessibilityParent() as? NSView === list)
                XCTAssertGreaterThan(rows[index].accessibilityFrame().width, 0)
            }
            XCTAssertEqual(rows.map { $0.accessibilityLabel() }, ["One", "Two", "Three"])
            XCTAssertTrue(rows[0].isAccessibilitySelected())
            XCTAssertFalse(rows[2].isAccessibilitySelected())
            XCTAssertFalse(rows[1].isAccessibilityEnabled())
            XCTAssertEqual(rows[2].accessibilityHelp(), "The third option")
            XCTAssertTrue(list.accessibilitySelectedChildren()?.first as? NSAccessibilityElement === rows[0])
            XCTAssertFalse(rows[1].accessibilityPerformPress())
            XCTAssertTrue(select.accessibilitySharedFocusElements()?.first as? NSAccessibilityElement === rows[0])
            rows[2].setAccessibilityFocused(true)
            XCTAssertEqual(list.active, 2)
            XCTAssertTrue(rows[2].isAccessibilityFocused())
            XCTAssertFalse(rows[0].isAccessibilityFocused())
            XCTAssertTrue(select.accessibilitySharedFocusElements()?.first as? NSAccessibilityElement === rows[2])
            XCTAssertTrue(list.accessibilitySharedFocusElements()?.first as? NSAccessibilityElement === rows[2])
            XCTAssertEqual(select.indexOfSelectedItem, 0, "assistive focus does not select")
            XCTAssertTrue(window.firstResponder === responder, "active navigation retains keyboard focus")
            rows[1].setAccessibilityFocused(true)
            XCTAssertEqual(list.active, 2, "disabled rows cannot become active")
            XCTAssertTrue(changed.isEmpty)
            XCTAssertTrue(rows[2].accessibilityPerformPress())
            XCTAssertEqual(changed, [2])
            XCTAssertEqual(select.indexOfSelectedItem, 2)
            XCTAssertFalse(select.isAccessibilityExpanded())
            XCTAssertTrue(select.accessibilityLinkedUIElements()?.isEmpty == true)
            XCTAssertTrue(list.accessibilitySharedFocusElements()?.isEmpty == true)
            XCTAssertFalse(rows[2].isAccessibilityFocused(), "closed rows no longer share focus")
            XCTAssertFalse(rows[0].accessibilityPerformPress(), "closed rows cannot act")
            XCTAssertTrue(select.accessibilityPerformShowMenu())
            XCTAssertFalse(rows[2].accessibilityPerformPress(), "old rows cannot act on a replacement list")
            let replacement = try XCTUnwrap(select.listbox)
            let newRows = try XCTUnwrap(replacement.accessibilityChildren() as? [NSAccessibilityElement])
            XCTAssertTrue(newRows[2].isAccessibilitySelected())
            select.isEnabled = false
            XCTAssertFalse(select.isListboxOpen, "disabling the owner closes its child panel")
            XCTAssertFalse(select.isAccessibilityExpanded())
            XCTAssertFalse(newRows[2].accessibilityPerformPress())
            XCTAssertFalse(select.accessibilityPerformShowMenu())
            XCTAssertEqual(changed, [2])
        }
    }

    func testNumberFieldStepsWithinBoundsAndNamesItsSteppers() throws {
        _ = NSApplication.shared
        let tokens = try XCTUnwrap(Tokens.variants["dark-mustard"])
        let frame = NSRect(x: 0, y: 0, width: 300, height: 120)
        let window = NSWindow(contentRect: frame, styleMask: [.titled], backing: .buffered, defer: false)
        window.isReleasedWhenClosed = false
        defer { window.close() }
        let root = Surface(frame: frame)
        window.contentView = root
        let field = TokenNumberField(frame: NSRect(x: 20, y: 20, width: 120, height: 32))
        field.tokens = tokens
        field.minimum = { 0 }; field.maximum = { 10 }
        field.setAccessibilityLabel("Recording crop X")
        var steps: [String] = []
        field.stepped = { steps.append($0.stringValue) }
        field.stringValue = "9"
        root.addSubview(field)
        XCTAssertEqual(field.increaseHalf.accessibilityLabel(), "Increase Recording crop X")
        XCTAssertEqual(field.decreaseHalf.accessibilityLabel(), "Decrease Recording crop X")
        XCTAssertFalse(KeyViewLoop.isCandidate(field.increaseHalf), "steppers stay out of the Tab order")
        XCTAssertTrue(field.increaseHalf.accessibilityPerformPress())
        XCTAssertEqual(field.stringValue, "10")
        XCTAssertFalse(field.increaseHalf.available, "Increase is disabled at the maximum")
        XCTAssertFalse(field.increaseHalf.accessibilityPerformPress())
        field.step(up: false)
        XCTAssertEqual(steps, ["10", "9"])
        XCTAssertEqual(field.focusRingType, .none)
        field.isEnabled = false
        XCTAssertTrue(field.increaseHalf.isHidden, "disabled fields hide their steppers")
        field.isEnabled = true
        XCTAssertFalse(field.decreaseHalf.isHidden)
        window.display()
    }

    func testTokenSliderDrawsTheShippingThumb() throws {
        _ = NSApplication.shared
        let tokens = try XCTUnwrap(Tokens.variants["light-mustard"])
        let slider = TokenSlider(value: 100, minValue: 0, maxValue: 200, target: nil, action: nil)
        slider.frame = NSRect(x: 0, y: 0, width: 200, height: 20)
        slider.tokens = tokens
        let cell = try XCTUnwrap(slider.cell as? TokenSliderCell)
        XCTAssertNotNil(cell.tokens)
        XCTAssertEqual(slider.focusRingType, .none)
        let knob = cell.knobRect(flipped: slider.isFlipped)
        XCTAssertEqual(knob.width, 14); XCTAssertEqual(knob.height, 14)
        XCTAssertEqual(slider.doubleValue, 100)
    }
}
