import XCTest
@testable import CapturesNative

private final class InstanceTransport: NativeInstanceTransport {
    var responses: [[String: Any]]
    private(set) var requests: [[String: Any]] = []

    init(_ responses: [[String: Any]]) { self.responses = responses }

    func request(_ object: [String: Any]) throws -> [String: Any] {
        requests.append(object)
        return responses.removeFirst()
    }
}

final class NativeInstanceTests: XCTestCase {
    func testSecondaryDoesNotReturnAnOwner() throws {
        let transport = InstanceTransport([["primary": false]])
        let result = try NativeInstance.start(historyRoot: nil, paths: ["relative.png"], transport: transport)
        XCTAssertFalse(result.primary)
        XCTAssertNil(result.owner)
        XCTAssertEqual(transport.requests.first?["operation"] as? String, "start")
        XCTAssertTrue(transport.requests.first?["history_root"] is NSNull)
    }

    func testPrimaryDrainsFileAndRelaunchRequestsThenClosesExplicitly() throws {
        let transport = InstanceTransport([
            ["primary": true],
            ["request": ["paths": ["/tmp/a.png", "/tmp/b.mp4"]]],
            ["request": ["paths": [String]()]],
            ["request": NSNull()],
            [:],
            [:],
        ])
        let result = try NativeInstance.start(historyRoot: "/tmp/history", paths: [], transport: transport)
        let owner = try XCTUnwrap(result.owner)
        defer { owner.close() }
        XCTAssertEqual(try owner.nextRequest(), ["/tmp/a.png", "/tmp/b.mp4"])
        XCTAssertEqual(try owner.nextRequest(), [])
        XCTAssertNil(try owner.nextRequest())
        owner.stopAccepting()
        owner.close()
        owner.close()
        XCTAssertNil(try owner.nextRequest())
        XCTAssertEqual(transport.requests.map { $0["operation"] as! String },
            ["start", "next", "next", "next", "stop", "close"])
    }
}
