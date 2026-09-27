import XCTest
@testable import CapturesNative

/// Never constructs a live shortcut owner or touches WindowServer hotkeys.
final class SystemShortcutTakeoverTests: XCTestCase {
    func testConfigureResultNamesOnlyTheSharedSymbolicHotKeys() {
        XCTAssertEqual(SymbolicHotKeys.ids(in: ["disable_symbolic_hotkeys": [28, 30, 184]]), [28, 30, 184])
        XCTAssertEqual(SymbolicHotKeys.ids(in: ["disable_symbolic_hotkeys": [Int]()]), [])
        XCTAssertEqual(SymbolicHotKeys.ids(in: [String: Any]()), [])
        XCTAssertEqual(SymbolicHotKeys.ids(in: ["disable_symbolic_hotkeys": ["28", 30] as [Any]]), [30])
    }

    func testNothingToDisableNeverCallsWindowServer() {
        XCTAssertEqual(SymbolicHotKeys.disable([]), [])
    }

    func testPreferencesCopyMatchesShippingTakeover() {
        let body = PreferencesPolicy.text("shortcuts.system_body")
        XCTAssertEqual(body, "Captures unbinds overlapping Screenshot app keys (⌘⇧3, ⌘⇧4, ⌘⇧5) so they reach this app instead of the system overlay. Restore them in System Settings if you want both.")
        XCTAssertEqual(PreferencesPolicy.text("shortcuts.system_title"), "macOS Screenshot shortcuts")
    }
}
