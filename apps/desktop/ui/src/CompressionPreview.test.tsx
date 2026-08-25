import { fireEvent, render, screen } from "@testing-library/react";
import { vi } from "vitest";

import { CompressionPreview } from "./CompressionPreview";

describe("CompressionPreview", () => {
  it("shows before/after sizes and reports savings", () => {
    render(
      <CompressionPreview
        open
        beforeUrl="blob:before"
        afterUrl="blob:after"
        beforeBytes={1_000_000}
        afterBytes={250_000}
        formatLabel="PNG"
        qualityLabel="Tiny"
        pending={false}
        error=""
        onClose={() => undefined}
      />,
    );

    expect(screen.getByRole("dialog", { name: "Compression preview" })).toBeInTheDocument();
    expect(screen.getByText((_, node) => node?.textContent === "PNG · Tiny · −75%")).toBeInTheDocument();
    expect(screen.getByText("−75%")).toBeInTheDocument();
    expect(screen.getByAltText("Before compression")).toHaveAttribute("src", "blob:before");
    expect(screen.getByAltText("After compression")).toHaveAttribute("src", "blob:after");
    expect(screen.getByText((_, node) => node?.textContent === "Before · 1.0 MB")).toBeInTheDocument();
    expect(screen.getByText((_, node) => node?.textContent === "After · 250 KB")).toBeInTheDocument();
    expect(screen.getByRole("slider", { name: "Before and after comparison" }))
      .toHaveValue("50");
  });

  it("shows the original left of the divider and keeps the handle inside the badges", () => {
    render(
      <CompressionPreview
        open
        beforeUrl="blob:before"
        afterUrl="blob:after"
        beforeBytes={1_000_000}
        afterBytes={250_000}
        formatLabel="PNG"
        qualityLabel="Tiny"
        pending={false}
        error=""
        onClose={() => undefined}
      />,
    );

    // The left clip reveals the original over the full-frame compressed encode.
    const before = screen.getByAltText("Before compression");
    expect(before.parentElement).toHaveClass("compression-preview-before-clip");
    const split = screen.getByRole("slider", { name: "Before and after comparison" });
    fireEvent.change(split, { target: { value: "6" } });
    expect(split).toHaveValue("6");
    expect(split).toHaveAttribute("min", "6");
    expect(split).toHaveAttribute("max", "94");
  });

  it("lets PNG color count be changed from the preview", () => {
    const onPngColorsChange = vi.fn();
    render(
      <CompressionPreview
        open
        beforeUrl="blob:before"
        afterUrl="blob:after"
        beforeBytes={1_700_000}
        afterBytes={330_000}
        formatLabel="PNG"
        qualityLabel="128 colors"
        pending={false}
        error=""
        pngColors={128}
        onPngColorsChange={onPngColorsChange}
        onClose={() => undefined}
      />,
    );

    const colors = screen.getByRole("slider", { name: "PNG palette colors" });
    expect(colors).toHaveValue("128");
    fireEvent.change(colors, { target: { value: "64" } });
    expect(onPngColorsChange).toHaveBeenCalledWith(64);
  });

  it("closes from the dismiss control", () => {
    const onClose = vi.fn();
    render(
      <CompressionPreview
        open
        beforeUrl={null}
        afterUrl={null}
        beforeBytes={null}
        afterBytes={null}
        formatLabel="JPEG"
        qualityLabel="High"
        pending
        error=""
        onClose={onClose}
      />,
    );

    fireEvent.click(screen.getByRole("button", { name: "Close compression preview" }));
    expect(onClose).toHaveBeenCalledTimes(1);
  });

  it("renders nothing when closed", () => {
    const { container } = render(
      <CompressionPreview
        open={false}
        beforeUrl={null}
        afterUrl={null}
        beforeBytes={null}
        afterBytes={null}
        formatLabel="PNG"
        qualityLabel=""
        pending={false}
        error=""
        onClose={() => undefined}
      />,
    );
    expect(container).toBeEmptyDOMElement();
  });
});
