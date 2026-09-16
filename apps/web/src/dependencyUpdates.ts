/** Dependency maintenance belongs in GitHub history, not the product change list. */
export function isDependencyUpdateTitle(title: string): boolean {
  return /^Bump\b/iu.test(title.trim());
}
