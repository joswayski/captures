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
    title: "Capture what you need",
    description: "A region, a window, or a full display. Screenshot or record from the same menu.",
    alt: "Captures New Capture bar in Screenshot mode with a region drawn over an aerial view of a container ship wedged across a canal",
  },
  {
    id: "screenshot-editor",
    file: "screenshot-editor.jpg",
    width: 1440,
    height: 900,
    title: "Edit right away",
    description: "Annotate screenshots with text, arrows, shapes, and crops.",
    alt: "Captures screenshot editor with a desert canal, a Choke point label and arrow, a tiger on the left bank, and a container ship hanging off the right edge with an Expand canvas button",
  },
  {
    id: "collapsed-mini-previews",
    file: "collapsed-mini-previews.jpg",
    width: 340,
    height: 360,
    title: "Keep captures handy",
    description: "Collapse recent captures into a compact corner stack until you need them.",
    alt: "Captures mini preview cards collapsed into a compact corner stack, with the container ship capture on top",
  },
  {
    id: "video-editor",
    file: "video-editor.jpg",
    width: 1440,
    height: 980,
    title: "Trim and export",
    description: "Preview, trim, crop, and export MP4 or GIF with quality and audio controls.",
    alt: "Captures video editor trimming a total solar eclipse to a few seconds of totality, with crop handles and save controls",
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
