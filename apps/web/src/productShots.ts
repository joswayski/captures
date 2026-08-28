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
    alt: "Captures region capture over an aerial satellite view of the Ever Given in the Suez Canal, with a highlighted box, corner handles, and the capture menu",
  },
  {
    id: "screenshot-editor",
    file: "screenshot-editor.jpg",
    width: 1440,
    height: 900,
    title: "Edit screenshots",
    description: "Text, arrows, and stickers. Hover a hanging layer to expand the canvas.",
    alt: "Captures screenshot editor with the Suez Canal, a Choke point label, a tiger on the left bank, and an Evergreen ship hanging off the right edge with an Expand canvas button",
  },
  {
    id: "video-editor",
    file: "video-editor.jpg",
    width: 1440,
    height: 980,
    title: "Polish recordings",
    description: "Preview, trim, crop, and export video with size and audio controls.",
    alt: "Captures video editor trimming a total solar eclipse to a few seconds of totality, with crop handles and save controls",
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
