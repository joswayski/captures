import Foundation
#if canImport(CoreGraphics)
  import CoreGraphics
#endif

/// Top-left-origin cover destination in physical pixels. WebKit image elements
/// snap both fitted edges before sampling; Canvas/background dust stays floating.
public func thumbnailCoverRect(source: CGSize, target: CGSize, snapToPixels: Bool) -> CGRect {
  let factor = max(target.width / source.width, target.height / source.height)
  let width = source.width * factor
  let height = source.height * factor
  let x = (target.width - width) / 2
  let y = (target.height - height) / 2
  guard snapToPixels else { return CGRect(x: x, y: y, width: width, height: height) }
  // Negative halfway edges round toward positive infinity, as in WebKit.
  let left = floor(x + 0.5)
  let top = floor(y + 0.5)
  return CGRect(
    x: left, y: top, width: floor(x + width + 0.5) - left,
    height: floor(y + height + 0.5) - top)
}

public struct ThumbnailDustParticle: Codable {
  public let id: Int
  public let left, top, width, height: Double
  public let cardWidth, cardHeight: Double
  public let sourceLeft, sourceTop: Double
  public let surfaceWidth, surfaceHeight: Double
  public let surfaceOffsetX, surfaceOffsetY: Double
  public let dx, dy, rotate, delayMs, durationMs: Double
}

public struct ThumbnailDustVisual: Codable {
  public let opacity, dx, dy, rotate, scale: Double
}

private func bezier(_ t: Double, _ a: Double, _ b: Double) -> Double {
  let i = 1 - t
  return 3 * i * i * t * a + 3 * i * t * t * b + t * t * t
}

private func bezierDerivative(_ t: Double, _ a: Double, _ b: Double) -> Double {
  let i = 1 - t
  return 3 * i * i * a + 6 * i * t * (b - a) + 3 * t * t * (1 - b)
}

public func cubicBezierProgress(_ x1: Double, _ y1: Double, _ x2: Double, _ y2: Double, _ x: Double)
  -> Double
{
  if x <= 0 { return 0 }
  if x >= 1 { return 1 }
  var t = x
  for _ in 0..<8 {
    let delta = bezier(t, x1, x2) - x
    if abs(delta) < 1e-6 { return bezier(t, y1, y2) }
    let derivative = bezierDerivative(t, x1, x2)
    if abs(derivative) < 1e-6 { break }
    t = min(1, max(0, t - delta / derivative))
  }
  var low = 0.0
  var high = 1.0
  t = x
  for _ in 0..<12 {
    if bezier(t, x1, x2) < x { low = t } else { high = t }
    t = (low + high) / 2
  }
  return bezier(t, y1, y2)
}

private func mix(_ a: Double, _ b: Double, _ t: Double) -> Double { a + (b - a) * t }

private func opacityAt(_ t: Double) -> Double {
  let keys = [(0.0, 1.0), (0.14, 1.0), (0.5, 0.72), (0.82, 0.0), (1.0, 0.0)]
  if t <= 0 { return 1 }
  for index in 1..<keys.count where t <= keys[index].0 {
    let previous = keys[index - 1]
    let next = keys[index]
    return mix(previous.1, next.1, (t - previous.0) / (next.0 - previous.0))
  }
  return 0
}

/// Exact scalar port of thumbnailDustVisualAt: WAAPI eases the entire duration,
/// then linearly mixes the explicit keyframes.
public func thumbnailDustVisualAt(_ particle: ThumbnailDustParticle, elapsedMs: Double)
  -> ThumbnailDustVisual
{
  let local = elapsedMs - particle.delayMs
  if local <= 0 { return ThumbnailDustVisual(opacity: 1, dx: 0, dy: 0, rotate: 0, scale: 1) }
  let linear = min(1, local / max(1, particle.durationMs))
  let t = cubicBezierProgress(0.28, 0, 0.12, 1, linear)
  let opacity = opacityAt(t)
  if t <= 0.14 {
    let f = t / 0.14
    return ThumbnailDustVisual(
      opacity: opacity, dx: mix(0, particle.dx * 0.06, f), dy: mix(0, particle.dy * 0.06, f),
      rotate: mix(0, particle.rotate * 0.08, f), scale: mix(1, 0.98, f))
  }
  let f = (t - 0.14) / 0.86
  return ThumbnailDustVisual(
    opacity: opacity, dx: mix(particle.dx * 0.06, particle.dx, f),
    dy: mix(particle.dy * 0.06, particle.dy, f),
    rotate: mix(particle.rotate * 0.08, particle.rotate, f), scale: mix(0.98, 0.18, f))
}
