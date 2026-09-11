import Foundation

@main
enum ModelsTests {
    static func main() throws {
        let now = Date(timeIntervalSince1970: 2_000_000_000)
        var boundary = Artifact(path: "/tmp/boundary.png", kind: "image", width: 73, height: 41)
        boundary.createdAt = now.addingTimeInterval(-30 * 24 * 60 * 60)
        var expired = Artifact(path: "/tmp/expired.png", kind: "image")
        expired.createdAt = boundary.createdAt.addingTimeInterval(-0.001)
        var newest = Artifact(path: "/tmp/newest.mp4", kind: "video")
        newest.createdAt = now
        let recent = NativeStorage.recent([expired, boundary, newest], now: now)
        precondition(recent.map(\.path) == ["/tmp/newest.mp4", "/tmp/boundary.png"], "30-day boundary or descending ordering is wrong")
        let parsed = try Artifact(response: ["path":"/tmp/asymmetric.png", "kind":"image", "width":73, "height":41])
        precondition(parsed.width == 73 && parsed.height == 41)
        for bad: [String: Any] in [["path":"relative.png", "kind":"image"], ["path":"/tmp/a", "kind":"unknown"], [:]] {
            do { _ = try Artifact(response: bad); preconditionFailure("Accepted invalid engine artifact") }
            catch is NativeFailure {}
        }
        let directory = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        defer { try? FileManager.default.removeItem(at: directory) }
        let file = directory.appendingPathComponent("history.json")
        try NativeStorage.write(recent, to: file)
        let loaded = try NativeStorage.read([Artifact].self, from: file)
        precondition(loaded == recent)
        let attributes = try FileManager.default.attributesOfItem(atPath: file.path)
        precondition((attributes[.posixPermissions] as? NSNumber)?.intValue == 0o600)
        try NativeStorage.write([boundary], to: file)
        let replaced = try NativeStorage.read([Artifact].self, from: file)
        precondition(replaced == [boundary], "Atomic replacement retained stale records")
        let defaults = NativeSettings()
        let decoded = try JSONDecoder().decode(NativeSettings.self, from: JSONEncoder().encode(defaults))
        precondition(decoded == defaults)
        precondition(Set(defaults.shortcuts.map(\.id)).count == defaults.shortcuts.count)
        print("ModelsTests: artifact validation, 30-day boundary, ordering, private atomic storage, settings round-trip passed")
    }
}
