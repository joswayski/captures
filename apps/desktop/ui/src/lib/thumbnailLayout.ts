export function shouldScrollThumbnailStackToEnd(
  previousCount: number,
  nextCount: number,
): boolean {
  return nextCount > previousCount;
}
