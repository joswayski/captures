import CoreGraphics
import Foundation
import ImageIO

enum EditorImageDecoder {
    private static let maximumDimension = 16_384
    private static let maximumPixels = 100_000_000

    static func decode(_ url: URL) throws -> EditorDecodedImage {
        guard let source = CGImageSourceCreateWithURL(url as CFURL, [
            kCGImageSourceShouldCache: false,
        ] as CFDictionary), CGImageSourceGetCount(source) > 0,
        let properties = CGImageSourceCopyPropertiesAtIndex(source, 0, nil) as? [CFString: Any],
        let sourceWidth = (properties[kCGImagePropertyPixelWidth] as? NSNumber)?.intValue,
        let sourceHeight = (properties[kCGImagePropertyPixelHeight] as? NSNumber)?.intValue,
        sourceWidth > 0, sourceHeight > 0 else {
            throw AppBridgeError.backend("The selected file is not an ImageIO-decodable still image.")
        }
        let orientation = (properties[kCGImagePropertyOrientation] as? NSNumber)?.intValue ?? 1
        let swapsAxes = [5, 6, 7, 8].contains(orientation)
        let width = swapsAxes ? sourceHeight : sourceWidth
        let height = swapsAxes ? sourceWidth : sourceHeight
        guard width <= maximumDimension, height <= maximumDimension,
              width.multipliedReportingOverflow(by: height).overflow == false,
              width * height <= maximumPixels else {
            throw AppBridgeError.backend(
                "Editor images are limited to 16,384 pixels per side and 100,000,000 decoded pixels.")
        }
        let options: [CFString: Any] = [
            kCGImageSourceCreateThumbnailFromImageAlways: true,
            kCGImageSourceCreateThumbnailWithTransform: true,
            kCGImageSourceThumbnailMaxPixelSize: max(sourceWidth, sourceHeight),
            kCGImageSourceShouldCacheImmediately: true,
        ]
        guard let image = CGImageSourceCreateThumbnailAtIndex(source, 0, options as CFDictionary),
              image.width == width, image.height == height,
              let sourceColorSpace = image.colorSpace,
              sourceColorSpace.model != .unknown,
              let targetColorSpace = CGColorSpace(name: CGColorSpace.sRGB) else {
            throw AppBridgeError.backend(
                "The selected image has an unsupported or missing color description.")
        }
        let bytesPerRow = width * 4
        var pixels = Data(count: bytesPerRow * height)
        let rendered = pixels.withUnsafeMutableBytes { bytes -> Bool in
            guard let base = bytes.baseAddress,
                  let context = CGContext(data: base, width: width, height: height,
                    bitsPerComponent: 8, bytesPerRow: bytesPerRow, space: targetColorSpace,
                    bitmapInfo: CGBitmapInfo.byteOrder32Big.rawValue
                        | CGImageAlphaInfo.premultipliedLast.rawValue) else { return false }
            context.interpolationQuality = .high
            context.draw(image, in: CGRect(x: 0, y: 0, width: width, height: height))
            return true
        }
        guard rendered else { throw AppBridgeError.backend("The selected image could not be rendered.") }
        pixels.withUnsafeMutableBytes { bytes in
            let values = bytes.bindMemory(to: UInt8.self)
            for offset in stride(from: 0, to: values.count, by: 4) {
                let alpha = Int(values[offset + 3])
                guard alpha > 0 else {
                    values[offset] = 0; values[offset + 1] = 0; values[offset + 2] = 0
                    continue
                }
                guard alpha < 255 else { continue }
                for channel in 0..<3 {
                    values[offset + channel] = UInt8(min(255,
                        (Int(values[offset + channel]) * 255 + alpha / 2) / alpha))
                }
            }
        }
        let name = url.deletingPathExtension().lastPathComponent
            .trimmingCharacters(in: .whitespacesAndNewlines)
        return EditorDecodedImage(data: pixels, width: width, height: height,
                                  bytesPerRow: bytesPerRow,
                                  name: name.isEmpty ? "Imported image" : name)
    }
}
