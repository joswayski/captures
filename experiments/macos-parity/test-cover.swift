import Foundation

@main
enum CoverGeometryTests {
  static func main() {
    // Explicit destinations, not values derived from the implementation. These
    // distinguish snapping both edges from rounding size or recentering afterward.
    let cases: [(CGSize, CGSize, CGRect)] = [
      (
        CGSize(width: 397, height: 251), CGSize(width: 568, height: 320),
        CGRect(x: 0, y: -20, width: 568, height: 360)
      ),
      (
        CGSize(width: 251, height: 397), CGSize(width: 320, height: 568),
        CGRect(x: -20, y: 0, width: 360, height: 568)
      ),
      (
        CGSize(width: 3, height: 2), CGSize(width: 5, height: 5),
        CGRect(x: -1, y: 0, width: 7, height: 5)
      ),
      // Halfway negative origins round up, not away from zero. Do not recenter.
      (
        CGSize(width: 3, height: 2), CGSize(width: 6, height: 6),
        CGRect(x: -1, y: 0, width: 9, height: 6)
      ),
      (
        CGSize(width: 2, height: 3), CGSize(width: 6, height: 6),
        CGRect(x: 0, y: -1, width: 6, height: 9)
      ),
    ]
    for (source, target, expected) in cases {
      let actual = thumbnailCoverRect(source: source, target: target, snapToPixels: true)
      precondition(actual == expected, "snapped cover: \(actual) != \(expected)")
    }
    let floating = thumbnailCoverRect(
      source: CGSize(width: 3, height: 2), target: CGSize(width: 5, height: 5),
      snapToPixels: false)
    precondition(floating == CGRect(x: -1.25, y: 0, width: 7.5, height: 5))
    print("6 cover geometry checks passed (snapped media, floating dust, both axes and half ties)")
  }
}
