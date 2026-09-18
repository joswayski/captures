import XCTest
import AppKit
import QuartzCore
@testable import CapturesNative

final class DustTests: XCTestCase {
    func testSwiftPosesMatchShippingTypeScriptAtDelayBoundaries() throws {
        struct Oracle: Decodable {
            let particles: [ThumbnailDustParticle]
            let times: [Double]
            let poses: [[ThumbnailDustVisual]]
        }
        let url = Bundle.module.url(forResource: "poses", withExtension: "json")!
        let fixture = try JSONDecoder().decode(Oracle.self, from: Data(contentsOf: url))
        for (index, time) in fixture.times.enumerated() {
            for (particleIndex, particle) in fixture.particles.enumerated() {
                let actual = thumbnailDustVisualAt(particle, elapsedMs: time)
                let expected = fixture.poses[index][particleIndex]
                XCTAssertEqual(actual.dx, expected.dx, accuracy: 1e-8)
                XCTAssertEqual(actual.dy, expected.dy, accuracy: 1e-8)
                XCTAssertEqual(actual.rotate, expected.rotate, accuracy: 1e-8)
                XCTAssertEqual(actual.opacity, expected.opacity, accuracy: 1e-8)
                XCTAssertEqual(actual.scale, expected.scale, accuracy: 1e-8)
            }
        }
    }

    func testCoverCropDoesNotStretchAndSnapsBothEdges() {
        let source = CGSize(width: 397, height: 251)
        let target = CGSize(width: 568, height: 320)
        let floating = thumbnailCoverRect(source: source, target: target, snapToPixels: false)
        XCTAssertEqual(floating.width, 568, accuracy: 1e-8)
        XCTAssertEqual(floating.height, 359.1133501259446, accuracy: 1e-8)
        XCTAssertEqual(floating.minY, -19.5566750629723, accuracy: 1e-8)
        let snapped = thumbnailCoverRect(source: source, target: target, snapToPixels: true)
        XCTAssertEqual(snapped, CGRect(x: 0, y: -20, width: 568, height: 360))
    }

    func testAtlasPreservesIsolatedChipPixelsAtBothScales() throws {
        let textures: DustTextures
        do { textures = try DustTextures() }
        catch NativeError.noMetal { throw XCTSkip("A Metal device is required for the atlas pixel gate") }
        let particles = try DustFixture.load().particles
        for scale in [CGFloat(1), CGFloat(2)] {
            let source = PreviewView.fixtureImage(scale: scale)
            let reference = try textures.prepare(source: source, particles: particles, scale: scale, atlas: false)
            let atlas = try textures.prepare(source: source, particles: particles, scale: scale, atlas: true)
            XCTAssertEqual(atlas.count, reference.count)
            var totalError = 0, channelCount = 0, largeErrors = 0, nonzero = 0
            var flippedError = 0, croppedError = 0
            for (index, pair) in zip(atlas, reference).enumerated() {
                let (a, b) = pair
                let actual = pixels(a, scale: scale), expected = pixels(b, scale: scale)
                var flippedRect = a.contentsRect
                flippedRect.origin.y = 1 - flippedRect.maxY
                let flipped = pixels(DustTextures.Chip(image: a.image,
                    contentsRect: flippedRect, size: a.size), scale: scale)
                let cropRect = CGRect(x: a.contentsRect.minX * CGFloat(a.image.width),
                    y: a.contentsRect.minY * CGFloat(a.image.height),
                    width: a.contentsRect.width * CGFloat(a.image.width),
                    height: a.contentsRect.height * CGFloat(a.image.height))
                let croppedImage = try XCTUnwrap(a.image.cropping(to: cropRect))
                let cropped = pixels(DustTextures.Chip(image: croppedImage,
                    contentsRect: CGRect(x: 0, y: 0, width: 1, height: 1), size: a.size), scale: scale)
                for (x, y) in zip(flipped, expected) { flippedError += abs(Int(x) - Int(y)) }
                for (x, y) in zip(cropped, expected) { croppedError += abs(Int(x) - Int(y)) }
                if [0, 42, 100, 197].contains(index),
                   let directory = ProcessInfo.processInfo.environment["CAPTURES_TEST_ARTIFACTS"] {
                    let prefix = URL(fileURLWithPath: directory).appendingPathComponent("chip-\(index)-\(Int(scale))x")
                    try writePNG(a.image, to: prefix.appendingPathExtension("atlas.png"))
                    try writePNG(b.image, to: prefix.appendingPathExtension("reference.png"))
                    try writePNG(croppedImage, to: prefix.appendingPathExtension("crop.png"))
                }
                XCTAssertEqual(actual.count, expected.count)
                for (x, y) in zip(actual, expected) {
                    guard x > 0 || y > 0 else { continue }
                    let difference = abs(Int(x) - Int(y))
                    totalError += difference
                    largeErrors += difference > 16 ? 1 : 0
                    nonzero += y > 0 ? 1 : 0
                    channelCount += 1
                }
            }
            print("Atlas diagnostics \(scale)x: contentsRect=\(Double(totalError) / Double(channelCount)), flipped=\(Double(flippedError) / Double(channelCount)), cropped=\(Double(croppedError) / Double(channelCount))")
            XCTAssertGreaterThan(nonzero, 1000, "Blank fixtures cannot pass parity")
            XCTAssertLessThanOrEqual(Double(totalError) / Double(channelCount), 2)
            XCTAssertLessThanOrEqual(Double(largeErrors) / Double(channelCount), 0.01)
        }
    }

    private func writePNG(_ image: CGImage, to url: URL) throws {
        try FileManager.default.createDirectory(at: url.deletingLastPathComponent(), withIntermediateDirectories: true)
        let bitmap = NSBitmapImageRep(cgImage: image)
        try XCTUnwrap(bitmap.representation(using: .png, properties: [:])).write(to: url)
    }

    private func pixels(_ chip: DustTextures.Chip, scale: CGFloat) -> [UInt8] {
        let width = Int(ceil(chip.size.width * scale)), height = Int(ceil(chip.size.height * scale))
        var bytes = [UInt8](repeating: 0, count: width * height * 4)
        bytes.withUnsafeMutableBytes { buffer in
            let context = CGContext(data: buffer.baseAddress, width: width, height: height,
                bitsPerComponent: 8, bytesPerRow: width * 4,
                space: CGColorSpace(name: CGColorSpace.sRGB)!,
                bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue)!
            context.scaleBy(x: scale, y: scale)
            let layer = CALayer()
            layer.bounds = CGRect(origin: .zero, size: chip.size)
            layer.contents = chip.image
            layer.contentsRect = chip.contentsRect
            layer.contentsScale = scale
            layer.render(in: context)
        }
        return bytes
    }
}
