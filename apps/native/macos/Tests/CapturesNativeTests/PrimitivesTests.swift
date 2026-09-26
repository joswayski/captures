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

    func testTokenSelectDescribesItemsAndKeepsPlainTitles() throws {
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
        select.addItems(withTitles: ["Preserve quality", "Compress"])
        select.item(at: 1)?.toolTip = "Smaller file with Tiny through Highest quality presets."
        root.addSubview(select)
        XCTAssertEqual(select.focusRingType, .none, "the token ring replaces the system ring")
        XCTAssertEqual(select.selectStyle, .field)
        var changed: [Int] = []
        select.bindChange { changed.append($0) }
        select.describeItems()
        XCTAssertFalse(select.item(at: 0)?.attributedTitle?.string.contains("\n") ?? false,
                       "items without a description stay plain")
        let described = try XCTUnwrap(select.item(at: 1)?.attributedTitle?.string)
        XCTAssertEqual(described, "Compress\nSmaller file with Tiny through Highest quality presets.")
        select.restoreItems()
        XCTAssertEqual(select.item(at: 1)?.title, "Compress", "titles and accessibility names are unchanged")
        XCTAssertFalse(select.item(at: 1)?.attributedTitle?.string.contains("\n") ?? false)
        select.selectItem(at: 1); select.selectedValue()
        XCTAssertEqual(changed, [1])
        XCTAssertEqual(select.titleOfSelectedItem, "Compress")
        window.display()
    }
}
