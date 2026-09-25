import AppKit
import XCTest
@testable import CapturesNative

final class BundledLaunchTests: XCTestCase {
    func testBundledDefaultIsLiveButBareFixturesRemainIndependent() throws {
        XCTAssertTrue(try Options([], bundled: true).live)
        XCTAssertFalse(try Options([]).live)
        XCTAssertThrowsError(try Options(["--exercise"], bundled: true))
        let options = try Options(["--open-media", "first.png", "--", "é space.png", "--exercise", "--"], bundled: true)
        XCTAssertEqual(options.openMedia, ["first.png", "é space.png", "--exercise", "--"])
        XCTAssertFalse(options.exercise)
        XCTAssertThrowsError(try Options(["--", "image.png"]))
        XCTAssertThrowsError(try Options(["--live", "--", ""]))
        XCTAssertThrowsError(try Options(["--live", "--typo"]))
        XCTAssertEqual(try Options(["--live", "--"]).openMedia, [])
    }

    func testColdFilesReachElectionInOrderBeforeSecondaryExits() throws {
        var events: [String] = []
        var paths: [String] = []
        let launch = BundledLaunch(options: try Options(["--open-media", "/cli.png"], bundled: true),
            reply: { reply in
                XCTAssertEqual(reply, .success)
                events.append("reply")
            }, finish: { code in
                XCTAssertEqual(code, 0)
                events.append("exit")
            }) { options, _ in
                events.append("elect-and-forward")
                paths = options.openMedia
                return false
            }
        launch.application(NSApplication.shared, openFiles: ["/cold é.png", "/clip.webm"])
        launch.application(NSApplication.shared, openFiles: ["/later.gif"])
        XCTAssertTrue(events.isEmpty)
        launch.applicationDidFinishLaunching(Notification(name: NSApplication.didFinishLaunchingNotification))
        XCTAssertEqual(paths, ["/cli.png", "/cold é.png", "/clip.webm", "/later.gif"])
        XCTAssertEqual(events, ["elect-and-forward", "reply", "exit"])
    }

    func testPrimaryActivationHasNoSpuriousFileReplyOrExit() throws {
        var starts = 0
        let launch = BundledLaunch(options: try Options([], bundled: true),
            reply: { _ in XCTFail("no file event to acknowledge") },
            finish: { _ in XCTFail("primary stays alive") }) { options, _ in
                starts += 1
                XCTAssertTrue(options.openMedia.isEmpty)
                return true
            }
        launch.applicationDidFinishLaunching(Notification(name: NSApplication.didFinishLaunchingNotification))
        XCTAssertEqual(starts, 1)
    }

    func testFailedForwardingNeverAcknowledgesSuccess() throws {
        var replies: [NSApplication.DelegateReply] = []
        var exits: [Int32] = []
        let launch = BundledLaunch(options: try Options([], bundled: true),
            reply: { replies.append($0) }, finish: { exits.append($0) }) { _, _ in
                throw AppBridgeError.invalidResponse
            }
        launch.application(NSApplication.shared, openFiles: ["/lost.png"])
        launch.applicationDidFinishLaunching(Notification(name: NSApplication.didFinishLaunchingNotification))
        XCTAssertEqual(replies, [.failure])
        XCTAssertEqual(exits, [1])
    }
}
