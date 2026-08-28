/**
 * Product stills shared with the README gallery in `docs/images/`.
 * Keep this list in lockstep with those files so the website and GitHub
 * page show the same captures.
 */
export const PRODUCT_SHOTS = [
  {
    id: "capture-selection",
    file: "capture-selection.jpg",
    width: 1400,
    height: 875,
    title: "Capture from the desktop",
    description: "One compact menu for screenshots and recordings, then a region, window, or full display.",
    alt: "The Captures menu over a frozen desktop, with screenshot and recording controls in one bar",
  },
  {
    id: "capture-controls",
    file: "capture-controls.jpg",
    width: 1200,
    height: 309,
    title: "Everything in reach",
    description: "Capture type and target controls sit together in one compact menu.",
    alt: "Close-up of the Captures capture menu with screenshot and recording options",
  },
  {
    id: "screenshot-editor",
    file: "screenshot-editor.jpg",
    width: 1280,
    height: 840,
    title: "Edit screenshots",
    description: "Annotations, layers, canvas controls, and flexible export options.",
    alt: "Captures screenshot editor with annotation tools, layers, and export controls",
  },
  {
    id: "video-editor",
    file: "video-editor.jpg",
    width: 1280,
    height: 884,
    title: "Polish recordings",
    description: "Preview, trim, crop, and export video with size and audio controls.",
    alt: "Captures video editor with a preview, trimming timeline, and save controls",
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
