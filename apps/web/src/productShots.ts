/**
 * Product stills shared with the README gallery in `docs/images/`.
 * Keep this list in lockstep with those files so the website and GitHub
 * page show the same captures.
 */
export const PRODUCT_SHOTS = [
  {
    id: "capture-selection",
    file: "capture-selection.jpg",
    width: 1600,
    height: 1000,
    title: "Draw a region",
    description: "Click and drag a box on the desktop, or switch to a window or full display.",
    alt: "Captures region capture over a landscape, with a highlighted box, corner handles, and the capture menu",
  },
  {
    id: "screenshot-editor",
    file: "screenshot-editor.jpg",
    width: 1440,
    height: 900,
    title: "Edit screenshots",
    description: "Text, arrows, and stickers. Drag past an edge to grow the canvas.",
    alt: "Captures screenshot editor with a landscape, a Choke point label on the fjord, a tiger sticker half off the left edge, and an Evergreen ship dragged past the right edge",
  },
  {
    id: "capture-controls",
    file: "capture-controls.jpg",
    width: 1200,
    height: 228,
    title: "Everything in reach",
    description: "Capture type and target controls sit together in one compact menu.",
    alt: "Close-up of the Captures capture menu with screenshot and recording options",
  },
  {
    id: "video-editor",
    file: "video-editor.jpg",
    width: 1440,
    height: 980,
    title: "Polish recordings",
    description: "Preview, trim, crop, and export video with size and audio controls.",
    alt: "Captures video editor with a landscape preview, crop handles, trimming timeline, and save controls",
  },
  {
    id: "preferences",
    file: "preferences.jpg",
    width: 1200,
    height: 900,
    title: "Make it yours",
    description: "Light or dark appearance, accent colors, shortcuts, and capture defaults.",
    alt: "Captures Preferences showing the appearance, accent color, and capture settings",
  },
] as const;

export type ProductShot = (typeof PRODUCT_SHOTS)[number];

/** Tallest still, as height / width. The gallery card uses this so captions stay put. */
export function galleryFrameAspectRatio(
  shots: readonly { width: number; height: number }[] = PRODUCT_SHOTS,
) {
  return shots.reduce((tallest, shot) => Math.max(tallest, shot.height / shot.width), 0);
}
