export type PreviewDownloadId = "macos" | "windows" | "linux-deb" | "linux-appimage";

export type NavigatorHints = {
  userAgent: string;
  platform: string;
  userAgentDataPlatform: string;
  maxTouchPoints: number;
};

/** Pick a Preview installer from browser hints. Mobile browsers return null. */
export function detectPreviewDownloadId(hints: NavigatorHints): PreviewDownloadId | null {
  const { userAgent: ua, platform, userAgentDataPlatform: ch, maxTouchPoints } = hints;
  const isMobileApple =
    /iphone|ipad|ipod/i.test(ua) || (/mac/i.test(`${ch} ${platform} ${ua}`) && maxTouchPoints > 1);
  if (isMobileApple || /android/i.test(ua) || /cros|chrome os/i.test(`${ch} ${ua}`)) return null;

  if (/^mac/i.test(ch) || /mac/i.test(platform) || /macintosh|mac os x/i.test(ua)) {
    return "macos";
  }
  if (/^win/i.test(ch) || /win/i.test(platform) || /windows/i.test(ua)) {
    return "windows";
  }
  if (/^linux/i.test(ch) || /linux/i.test(platform) || (/linux/i.test(ua) && !/android/i.test(ua))) {
    return /ubuntu|debian|linux mint|pop!_os|elementary/i.test(ua) ? "linux-deb" : "linux-appimage";
  }
  return null;
}
