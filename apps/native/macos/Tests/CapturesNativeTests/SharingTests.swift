import AppKit
import XCTest
@testable import CapturesNative

final class SharingTests: XCTestCase {
    private final class Transport: SharingTransport {
        var requests: [[String: Any]] = []
        var events: [[String: Any]] = []
        var pollError = false
        var shutdowns = 0
        func request(_ object: [String: Any]) throws -> [String: Any] {
            requests.append(object)
            if object.string("operation") == "poll" {
                if pollError { throw AppBridgeError.invalidResponse }
                let reply = events; events = []; return ["events": reply]
            }
            return ["accepted": true]
        }
        func shutdown(completion: @escaping () -> Void) { shutdowns += 1; completion() }
        var operations: [String] { requests.map { $0.string("operation") }.filter { $0 != "poll" } }
    }

    private func artifact(_ id: String, kind: String = "screenshot", media: String? = nil) -> CaptureArtifact {
        var value: [String: Any] = [
            "entry": ["id": id, "kind": kind, "width": 913, "height": 417,
                      "created_at": "2026-10-05T10:00:00Z", "saved_path": "/Exports/lossy.gif"],
            "image_path": "/private/\(id)/capture.png", "preview_path": "/private/\(id)/preview.jpg"]
        if let media { value["media_path"] = media }
        return CaptureArtifact(value)!
    }

    private func reply(_ auth: String = "signed_in", error: String? = nil, shared: Bool = true) -> [String: Any] {
        let asset: [String: Any] = ["id": "remote", "name": "Capture.png", "deletedAt": NSNull(),
            "share": shared ? ["passwordProtected": true, "expiresAt": "2028-04-02T13:20:00Z",
                              "sharedAt": "2026-10-05T10:01:00Z"] as Any : NSNull()]
        return ["event": "finished", "auth": ["status": auth, "email": "owner@example.com"],
                "opened": ["status": "asset", "asset": asset], "error": error as Any? ?? NSNull(),
                "link": shared && error == nil && auth == "signed_in" ? "https://captur.es/s/shared" as Any : NSNull()]
    }

    func testOriginalSelectionNeverUsesRecordingPosterOrSavedExport() throws {
        XCTAssertEqual(artifact("image").sharingSelection?.string("path"), "/private/image/capture.png")
        let recording = artifact("take", kind: "video", media: "/private/take/media.webm")
        let selection = try XCTUnwrap(recording.sharingSelection)
        XCTAssertEqual(selection.string("path"), "/private/take/media.webm")
        XCTAssertEqual(selection.string("content_type"), "video/webm")
        XCTAssertEqual(selection.string("name"), "Capture-take.webm")
        XCTAssertNil(artifact("missing", kind: "video").sharingSelection)
    }

    func testOpenSignInAndSaveRetryNeverUploadOrRedirectAnAcceptedSelection() {
        let transport = Transport(), model = SharingModel(transport: nil)
        XCTAssertFalse(model.open(artifact("fixture")))
        let live = SharingModel(transport: transport)
        defer { live.shutdown {} }
        XCTAssertTrue(transport.requests.isEmpty)
        XCTAssertTrue(live.open(artifact("first")))
        live.password = "retained-password"; live.expiry = "2028-03-01T09:00:00Z"
        XCTAssertFalse(live.open(artifact("second")))
        XCTAssertEqual(live.artifact?.id, "first")
        XCTAssertEqual(transport.operations, ["open"])
        transport.events = [reply("save_required", error: "Unlock the vault.", shared: false)]
        live.poll()
        XCTAssertEqual(live.auth, "save_required")
        XCTAssertTrue(live.send(["operation": "retry_save"], status: "Saving session…"))
        transport.events = [reply()]; live.poll()
        XCTAssertEqual(live.password, "retained-password")
        XCTAssertEqual(live.expiry, "2028-03-01T09:00:00Z")
        XCTAssertEqual(transport.operations, ["open", "retry_save"])
        XCTAssertNotNil(live.link)
        XCTAssertTrue(live.send(["operation": "upload", "patch": live.patch], status: "Sharing…"))
        XCTAssertNil(live.link)
        transport.events = [reply(error: "Configuration failed.")]; live.poll()
        XCTAssertNil(live.link); XCTAssertEqual(live.password, "retained-password")
        XCTAssertTrue(live.send(["operation": "upload", "patch": live.patch], status: "Retrying…"))
        transport.events = [reply()]; live.poll()
        XCTAssertEqual(live.password, "")
        XCTAssertEqual(live.expiry, "2028-04-02T13:20:00Z")
        XCTAssertEqual(transport.operations, ["open", "retry_save", "upload", "upload"])
    }

    func testPatchSemanticsPollFailurePinsAndShutdownPreventsNewCommands() {
        let transport = Transport(), model = SharingModel(transport: nil)
        XCTAssertTrue(model.patch.isEmpty)
        model.password = "secret-fixture"; model.expiry = " 2028-02-03T14:15:16Z "
        XCTAssertEqual(model.patch["password"] as? String, "secret-fixture")
        XCTAssertEqual(model.patch["expiresAt"] as? String, "2028-02-03T14:15:16Z")
        model.removePassword = true; model.removeExpiry = true
        XCTAssertTrue(model.patch["password"] is NSNull); XCTAssertTrue(model.patch["expiresAt"] is NSNull)
        let live = SharingModel(transport: transport)
        XCTAssertTrue(live.open(artifact("pinned")))
        transport.pollError = true; live.poll()
        XCTAssertTrue(live.busy); XCTAssertNil(live.link)
        XCTAssertFalse(live.open(artifact("replacement")))
        XCTAssertEqual(live.artifact?.id, "pinned")
        var drained = false
        live.shutdown { drained = true }
        XCTAssertTrue(drained); XCTAssertEqual(transport.shutdowns, 1)
        XCTAssertFalse(live.send(["operation": "refresh"], status: "Refresh"))
        XCTAssertEqual(transport.operations, ["open"])
    }

    func testNativeTransportStaysLazyAndCannotRestartAfterShutdown() {
        let transport = NativeSharingTransport(root: "/unused-test-profile")
        XCTAssertThrowsError(try transport.request(["operation": "poll"]))
        var completions = 0
        transport.shutdown { completions += 1 }
        transport.shutdown { completions += 1 }
        XCTAssertEqual(completions, 2)
        XCTAssertThrowsError(try transport.request(["operation": "open"]))
    }

    func testDismissCaptureExclusionAndReopenRetainAcceptedWorkAndSettings() throws {
        _ = NSApplication.shared
        let transport = Transport()
        let form = SharingController(tokens: try XCTUnwrap(Tokens.variants["dark-mustard"]), transport: transport)
        defer { form.model.shutdown {}; form.window.delegate = nil; form.window.close() }
        form.present(artifact("chosen"))
        form.model.password = "kept-password"; form.model.expiry = "2028-05-06T07:08:09Z"; form.refresh()
        form.dismiss()
        XCTAssertTrue(form.model.busy)
        XCTAssertEqual(transport.operations, ["open"])
        transport.events = [reply("code_sent", shared: false)]; form.model.poll()
        form.present(artifact("chosen"))
        XCTAssertEqual(form.password.stringValue, "kept-password")
        XCTAssertEqual(form.expiry.stringValue, "2028-05-06T07:08:09Z")
        XCTAssertEqual(form.window.sharingType, .none)
        form.setCaptureHidden(true); XCTAssertFalse(form.window.isVisible)
        form.setCaptureHidden(false); XCTAssertTrue(form.window.isVisible)
        form.setCaptureHidden(true); form.dismiss(); form.setCaptureHidden(false)
        XCTAssertFalse(form.window.isVisible, "a dismissed popup must not resurrect after capture")
        XCTAssertEqual(form.model.artifact?.id, "chosen")
        XCTAssertEqual(transport.operations, ["open", "open"])
    }

    func testFixtureControlsAreNamedDisabledAndFooterFitsEveryStateAtMinimumSize() throws {
        _ = NSApplication.shared
        for appearance in ["light", "dark"] {
            let form = SharingController(tokens: try XCTUnwrap(Tokens.variants["\(appearance)-mustard"]))
            defer { form.window.delegate = nil; form.window.close() }
            for size in [NSSize(width: 480, height: 720), NSSize(width: 380, height: 520)] {
                form.window.setContentSize(size); form.root.setFrameSize(size)
                for state in ["", "otp", "vault", "shared", "uploading", "trash", "error"] {
                    form.model.loadFixture(state); form.refresh(); form.layoutForm()
                    XCTAssertGreaterThanOrEqual(form.footer.frame.minY, 0, "\(appearance)/\(state)/\(size)")
                    XCTAssertEqual(form.footer.frame.maxY, size.height, accuracy: 0.01)
                    XCTAssertLessThanOrEqual(form.scroll.frame.maxY, form.footer.frame.minY)
                    let actions = form.root.subviewsRecursive.compactMap { $0 as? CaptureButton }
                    XCTAssertFalse(actions.isEmpty)
                    XCTAssertTrue(actions.allSatisfy { !$0.isEnabled })
                    for action in actions {
                        XCTAssertEqual(action.accessibilityLabel(), action.title)
                        if !action.isHidden && action.superview === form.footer {
                            XCTAssertTrue(form.footer.bounds.contains(action.frame), action.title)
                        }
                        action.performClick(nil)
                    }
                    for field in [form.email, form.code as NSTextField, form.password as NSTextField, form.expiry] {
                        XCTAssertFalse(field.isEnabled); XCTAssertNotNil(field.accessibilityLabel())
                    }
                    try render(form, name: "sharing-\(appearance)-\(state.isEmpty ? "signed-out" : state)-\(Int(size.width))x\(Int(size.height)).png")
                }
            }
        }
    }

    func testSharingFixtureCannotRunAsALiveOrExercisedHost() throws {
        XCTAssertEqual(try Options(["--scene", "sharing"]).scene, "sharing")
        XCTAssertThrowsError(try Options(["--scene", "sharing", "--live"]))
        XCTAssertThrowsError(try Options(["--scene", "sharing", "--exercise"]))
        XCTAssertThrowsError(try Options(["--scene", "sharing"], bundled: true))
    }

    private func render(_ form: SharingController, name: String) throws {
        guard let directory = ProcessInfo.processInfo.environment["CAPTURES_TEST_ARTIFACTS"] else { return }
        form.root.displayIfNeeded()
        let bitmap = try XCTUnwrap(form.root.bitmapImageRepForCachingDisplay(in: form.root.bounds))
        form.root.cacheDisplay(in: form.root.bounds, to: bitmap)
        let png = try XCTUnwrap(bitmap.representation(using: .png, properties: [:]))
        let root = URL(fileURLWithPath: directory)
        try FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)
        try png.write(to: root.appendingPathComponent(name))
    }
}
