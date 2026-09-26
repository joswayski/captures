import AppKit
import XCTest
@testable import CapturesNative

/// The first position after `start` of a view named `name` (its accessibility
/// label or button title) in a key-view loop.
func keyViewIndex(_ order: [NSView], _ name: String, after start: Int = -1) -> Int? {
    order.indices.first { index in
        index > start && (order[index].accessibilityLabel() == name
            || (order[index] as? NSButton)?.title == name)
    }
}

private final class KeyViewLeafFixture: NSView, KeyViewParticipant {
    var keyViewLeaf: Bool { true }
}

final class KeyViewLoopTests: XCTestCase {
    func testLoopFollowsTheGivenOrderSkipsLabelsAndStopsAtLeaves() {
        _ = NSApplication.shared
        let frame = NSRect(x: 0, y: 0, width: 400, height: 300)
        let window = NSWindow(contentRect: frame, styleMask: [.titled], backing: .buffered, defer: false)
        window.isReleasedWhenClosed = false
        defer { window.close() }
        let root = NSView(frame: frame)
        window.contentView = root
        let first = NSButton(title: "First", target: nil, action: nil)
        let caption = NSTextField(labelWithString: "Caption")
        let field = NSTextField(string: "")
        let leaf = KeyViewLeafFixture(frame: .zero)
        leaf.addSubview(NSButton(title: "Inside leaf", target: nil, action: nil))
        let group = NSView(frame: .zero)
        group.addSubview(caption); group.addSubview(field)
        [first, group, leaf].forEach(root.addSubview)

        let chain = KeyViewLoop.install([leaf, group, first, leaf], window: window)
        XCTAssertTrue(chain.elementsEqual([leaf, field, first] as [NSView], by: ===),
                      "Given order, labels skipped, leaves not entered, duplicates dropped")
        XCTAssertTrue(first.nextKeyView === leaf, "The loop closes")
        XCTAssertTrue(window.initialFirstResponder === leaf)
        XCTAssertFalse(window.autorecalculatesKeyViewLoop)
        XCTAssertTrue(KeyViewLoop.order(from: leaf).elementsEqual(chain, by: ===))
    }

    func testPreferencesTabOrderFollowsShippingNavHeaderFindThenCards() throws {
        _ = NSApplication.shared
        let directory = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: directory) }
        let frame = NSRect(x: 0, y: 0, width: 1000, height: 600)
        let window = NSWindow(contentRect: frame, styleMask: [.titled], backing: .buffered, defer: false)
        window.isReleasedWhenClosed = false
        defer { window.close() }
        let root = Surface(frame: frame)
        window.contentView = root
        let tokens = try XCTUnwrap(Tokens.variants["light-mustard"])
        let store = try SettingsStore(path: directory.appendingPathComponent("settings.json").path,
                                      debounceInterval: 0)
        let controller = PreferencesController(root: root, store: store, tokens: { tokens },
            appearanceChanged: { _, _, _ in }, showHistory: {})
        defer { withExtendedLifetime(controller) {} }
        let deadline = Date().addingTimeInterval(3)
        while controller.shortcutCard() == nil && Date() < deadline {
            RunLoop.current.run(until: Date().addingTimeInterval(0.01))
        }
        let shortcuts = try XCTUnwrap(controller.shortcutCard())

        func loop() throws -> [NSView] { try KeyViewLoop.order(from: XCTUnwrap(window.initialFirstResponder)) }
        var order = try loop()
        let sections = PreferencesPolicy.sections.map(\.title)
        XCTAssertEqual(order.prefix(sections.count).compactMap { ($0 as? PreferenceNavButton)?.title }, sections,
                       "The section nav comes first and starts focus")
        let history = try XCTUnwrap(keyViewIndex(order, PreferencesPolicy.text("history")))
        XCTAssertEqual(history, sections.count, "Capture History follows the nav")
        let cardControls = KeyViewLoop.candidates(in: shortcuts)
        XCTAssertFalse(cardControls.isEmpty)
        let positions = cardControls.compactMap { control in order.firstIndex { $0 === control } }
        XCTAssertEqual(positions.count, cardControls.count, "Every card control is reachable")
        XCTAssertEqual(positions, positions.sorted(), "Card controls keep their top-to-bottom order")
        XCTAssertGreaterThan(positions.first ?? 0, history)

        controller.showFind()
        order = try loop()
        let find = try XCTUnwrap(keyViewIndex(order, PreferencesPolicy.text("find.placeholder")))
        let firstCard = try XCTUnwrap(order.firstIndex { $0 === cardControls.first })
        let header = try XCTUnwrap(keyViewIndex(order, PreferencesPolicy.text("history")))
        XCTAssertGreaterThan(find, header)
        XCTAssertLessThan(find, firstCard, "The find bar sits in the header, before the cards")
        controller.closeFind()
        order = try loop()
        XCTAssertNil(keyViewIndex(order, PreferencesPolicy.text("find.placeholder")))
    }
}
