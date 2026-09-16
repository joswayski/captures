import Foundation

private struct PoseInput: Decodable {
  let particles: [ThumbnailDustParticle]
  let times: [Double]
}

@main
enum Main {
  static func main() {
    do {
      let arguments = CommandLine.arguments
      if arguments.count == 4, arguments[1] == "--poses" {
        let input = try JSONDecoder().decode(
          PoseInput.self, from: Data(contentsOf: URL(fileURLWithPath: arguments[2])))
        let poses = input.times.map { time in
          input.particles.map { thumbnailDustVisualAt($0, elapsedMs: time) }
        }
        try JSONEncoder().encode(poses).write(
          to: URL(fileURLWithPath: arguments[3]), options: .atomic)
        return
      }
      #if canImport(AppKit)
        guard arguments.count == 3 else {
          throw Failure("usage: native CONFIG_JSON OUTPUT_JSON | native --poses INPUT OUTPUT")
        }
        let config = try JSONDecoder().decode(
          BenchmarkConfig.self, from: Data(contentsOf: URL(fileURLWithPath: arguments[1])))
        try NativeRenderer(config: config, output: URL(fileURLWithPath: arguments[2])).start()
      #else
        throw NSError(
          domain: "NativeBenchmark", code: 1,
          userInfo: [NSLocalizedDescriptionKey: "renderer requires macOS; --poses is portable"])
      #endif
    } catch {
      #if canImport(AppKit)
        fail(error)
      #else
        FileHandle.standardError.write(Data("native benchmark: \(error)\n".utf8))
        exit(1)
      #endif
    }
  }
}
