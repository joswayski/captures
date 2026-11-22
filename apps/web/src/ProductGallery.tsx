import { useId, useState, type MouseEvent } from "react";

import captureSelection from "../../../docs/images/capture-selection.jpg";
import collapsedMiniPreviews from "../../../docs/images/collapsed-mini-previews.jpg";
import preferences from "../../../docs/images/preferences.jpg";
import screenshotEditor from "../../../docs/images/screenshot-editor.jpg";
import videoEditor from "../../../docs/images/video-editor.jpg";
import ProductLightbox from "./ProductLightbox";
import { PRODUCT_SHOTS, type ProductShot } from "./productShots";

const SHOT_SRC = {
  "capture-selection.jpg": captureSelection,
  "collapsed-mini-previews.jpg": collapsedMiniPreviews,
  "preferences.jpg": preferences,
  "screenshot-editor.jpg": screenshotEditor,
  "video-editor.jpg": videoEditor,
} as const satisfies Record<ProductShot["file"], string>;

/**
 * Anybase-style layout: the first still spans the gallery, the rest sit in
 * pairs below it. Each pair's widths follow the shots' aspect ratios, so both
 * stills in a row share one height without cropping or letterboxing. Clicking
 * a shot opens the lightbox.
 */
export default function ProductGallery() {
  const headingId = useId();
  const [openIndex, setOpenIndex] = useState<number | null>(null);
  const [openedByPointer, setOpenedByPointer] = useState(false);

  function open(index: number, event: MouseEvent) {
    setOpenedByPointer(event.detail > 0);
    setOpenIndex(index);
  }

  const openShot = openIndex === null ? PRODUCT_SHOTS[0] : PRODUCT_SHOTS[openIndex];
  const [hero, ...rest] = PRODUCT_SHOTS.map((shot, index) => ({ shot, index }));
  const rows = [[hero]];
  for (let i = 0; i < rest.length; i += 2) rows.push(rest.slice(i, i + 2));

  return (
    <section aria-labelledby={headingId} className="product-gallery mt-16">
      <h2
        id={headingId}
        className="text-[0.6875rem] font-medium uppercase tracking-[0.14em] text-accent-readable"
      >
        A look at Captures
      </h2>

      <div className="mt-5 flex flex-col gap-6 sm:gap-5">
        {rows.map((row) => (
          <div key={row[0].shot.id} className="product-gallery-row">
            {row.map(({ shot, index }) => (
              <figure
                key={shot.id}
                className="product-gallery-item"
                style={{ flexGrow: shot.width / shot.height }}
              >
                <button
                  type="button"
                  className="product-gallery-shot"
                  aria-haspopup="dialog"
                  aria-label={`Open larger view: ${shot.title}`}
                  onClick={(event) => open(index, event)}
                >
                  <img
                    src={SHOT_SRC[shot.file]}
                    alt={shot.alt}
                    width={shot.width}
                    height={shot.height}
                    decoding="async"
                    loading={index < 1 ? "eager" : "lazy"}
                    draggable={false}
                  />
                </button>
                <figcaption className="product-gallery-caption">
                  <span className="block font-medium text-ink">{shot.title}</span>
                  {shot.description}
                </figcaption>
              </figure>
            ))}
          </div>
        ))}
      </div>

      <ProductLightbox
        open={openIndex !== null}
        src={SHOT_SRC[openShot.file]}
        shot={openShot}
        onClose={() => setOpenIndex(null)}
        suppressFocusRing={openedByPointer}
      />
    </section>
  );
}
