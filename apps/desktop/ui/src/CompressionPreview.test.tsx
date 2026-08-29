import { fireEvent, render, screen } from "@testing-library/react";

import { CompressionPreview } from "./CompressionPreview";

describe("CompressionPreview", () => {
  it("shows before/after sizes and reports savings", () => {
    render(
      <CompressionPreview
        beforeUrl="blob:before"
        afterUrl="blob:after"
        beforeBytes={1_000_000}
        afterBytes={250_000}
        pending={false}
      />,
    );

    expect(screen.getByRole("group", { name: "Compression comparison" }))
      .toBeInTheDocument();
    expect(screen.getByAltText("Before compression")).toHaveAttribute("src", "blob:before");
    expect(screen.getByAltText("After compression")).toHaveAttribute("src", "blob:after");
    expect(screen.getByText((_, node) => node?.textContent === "Before · 1.0 MB")).toBeInTheDocument();
    expect(screen.getByText((_, node) => (
      node?.textContent === "After · 250 KB · 75% smaller"
    ))).toBeInTheDocument();
    expect(screen.getByText((_, node) => (
      node?.classList.contains("compression-preview-savings") === true
      && node.textContent === " · 75% smaller"
    ))).toBeInTheDocument();
    expect(screen.getByRole("slider", { name: "Before and after comparison" }))
      .toHaveValue("50");
  });

  it("shows the original left of the divider and keeps the handle inside the badges", () => {
    render(
      <CompressionPreview
        beforeUrl="blob:before"
        afterUrl="blob:after"
        beforeBytes={1_000_000}
        afterBytes={250_000}
        pending={false}
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

  it("clips the compressed encode to the right when the editor is the before view", () => {
    render(
      <CompressionPreview
        beforeUrl={null}
        afterUrl="blob:after"
        beforeBytes={1_700_000}
        afterBytes={330_000}
        pending={false}
        liveBefore
      />,
    );

    expect(screen.queryByAltText("Before compression")).not.toBeInTheDocument();
    const after = screen.getByRole("img", { name: "After compression" });
    expect(after.tagName).toBe("CANVAS");
    expect(after.parentElement).toHaveClass("compression-preview-after-clip");
    expect(screen.getByRole("group", { name: "Compression comparison" }))
      .toHaveClass("is-live");
  });

  it("sizes the live after canvas to the editor frame so text is not stretched", () => {
    class MockResizeObserver {
      private readonly callback: ResizeObserverCallback;

      constructor(callback: ResizeObserverCallback) {
        this.callback = callback;
      }

      observe(target: Element) {
        Object.defineProperty(target, "clientWidth", { configurable: true, value: 640 });
        Object.defineProperty(target, "clientHeight", { configurable: true, value: 480 });
        this.callback(
          [{ target, contentRect: target.getBoundingClientRect() } as ResizeObserverEntry],
          this,
        );
      }

      unobserve() {}

      disconnect() {}
    }
    const previous = globalThis.ResizeObserver;
    vi.stubGlobal("ResizeObserver", MockResizeObserver);

    try {
      render(
        <CompressionPreview
          beforeUrl={null}
          afterUrl="blob:after"
          beforeBytes={1_700_000}
          afterBytes={330_000}
          pending={false}
          liveBefore
        />,
      );
      const after = screen.getByRole("img", { name: "After compression" });
      expect(after).toHaveStyle({ width: "640px", height: "480px" });
    } finally {
      vi.stubGlobal("ResizeObserver", previous);
    }
  });

  it("shows encoding status while the after frame is pending", () => {
    render(
      <CompressionPreview
        beforeUrl={null}
        afterUrl={null}
        beforeBytes={null}
        afterBytes={null}
        pending
        liveBefore
      />,
    );

    expect(screen.getByText("After · Processing…")).toBeInTheDocument();
    expect(screen.queryByRole("slider", { name: "Before and after comparison" }))
      .not.toBeInTheDocument();
  });

  it("does not cover the editor while the before/after images are empty", () => {
    render(
      <CompressionPreview
        className="is-embed is-cover"
        beforeUrl={null}
        afterUrl={null}
        beforeBytes={null}
        afterBytes={null}
        pending
      />,
    );

    const frame = screen.getByRole("group", { name: "Compression comparison" });
    expect(frame).toHaveClass("is-cover", "is-waiting");
    expect(screen.queryByText("Preparing preview…")).not.toBeInTheDocument();
    expect(screen.queryByAltText("Before compression")).not.toBeInTheDocument();
    expect(screen.queryByAltText("After compression")).not.toBeInTheDocument();
    expect(screen.getByText("After · Processing…")).toBeInTheDocument();
  });

  it("keeps the split when the after image is replaced during a refresh", () => {
    const { rerender } = render(
      <CompressionPreview
        beforeUrl="blob:before"
        afterUrl="blob:after-1"
        beforeBytes={1_000_000}
        afterBytes={250_000}
        pending={false}
      />,
    );

    const split = screen.getByRole("slider", { name: "Before and after comparison" });
    fireEvent.change(split, { target: { value: "72" } });
    expect(split).toHaveValue("72");

    rerender(
      <CompressionPreview
        beforeUrl="blob:before"
        afterUrl="blob:after-2"
        beforeBytes={1_000_000}
        afterBytes={180_000}
        pending={false}
      />,
    );

    expect(screen.getByRole("slider", { name: "Before and after comparison" }))
      .toHaveValue("72");
    expect(screen.getByAltText("After compression")).toHaveAttribute("src", "blob:after-2");
  });

  it("keeps the last comparison visible while a refresh is pending", () => {
    render(
      <CompressionPreview
        className="is-embed is-cover"
        beforeUrl="blob:before"
        afterUrl="blob:after"
        beforeBytes={1_000_000}
        afterBytes={250_000}
        pending
      />,
    );

    const frame = screen.getByRole("group", { name: "Compression comparison" });
    expect(frame).toHaveAttribute("data-pending", "true");
    expect(frame).toHaveClass("is-processing", "is-draw-locked");
    expect(frame).not.toHaveClass("is-waiting");
    expect(screen.getByAltText("Before compression")).toBeInTheDocument();
    expect(screen.getByAltText("After compression")).toBeInTheDocument();
    expect(screen.getByText("After · Processing…")).toBeInTheDocument();
    expect(screen.getByRole("status")).toHaveTextContent("Processing");
    const split = screen.getByRole("slider", { name: "Before and after comparison" });
    expect(split).toBeDisabled();
    expect(screen.getByRole("button", { name: "Drag to compare before and after" })).toBeDisabled();
    fireEvent.change(split, { target: { value: "80" } });
    expect(split).toHaveValue("50");
  });

  it("shows a cursor hint on the compressed side while a drawing tool is active", () => {
    render(
      <CompressionPreview
        beforeUrl="blob:before"
        afterUrl="blob:after"
        beforeBytes={1_000_000}
        afterBytes={250_000}
        pending={false}
        afterHint="Edits apply to the original. This side updates after you finish."
      />,
    );

    const frame = screen.getByRole("group", { name: "Compression comparison" });
    vi.spyOn(frame, "getBoundingClientRect").mockReturnValue({
      x: 0,
      y: 0,
      top: 0,
      left: 0,
      right: 200,
      bottom: 100,
      width: 200,
      height: 100,
      toJSON: () => ({}),
    });

    fireEvent.pointerMove(window, { clientX: 160, clientY: 40 });
    expect(screen.getByRole("tooltip")).toHaveTextContent(
      "Edits apply to the original. This side updates after you finish.",
    );

    fireEvent.pointerMove(window, { clientX: 20, clientY: 40 });
    expect(screen.queryByRole("tooltip")).not.toBeInTheDocument();
  });

  it("keeps the after-side hint inside the preview near the far edge", () => {
    render(
      <CompressionPreview
        beforeUrl="blob:before"
        afterUrl="blob:after"
        beforeBytes={1_000_000}
        afterBytes={250_000}
        pending={false}
        afterHint="Edits apply to the original. This side updates after you finish."
      />,
    );

    const frame = screen.getByRole("group", { name: "Compression comparison" });
    vi.spyOn(frame, "getBoundingClientRect").mockReturnValue({
      x: 0,
      y: 0,
      top: 0,
      left: 0,
      right: 200,
      bottom: 100,
      width: 200,
      height: 100,
      toJSON: () => ({}),
    });

    fireEvent.pointerMove(window, { clientX: 190, clientY: 92 });
    const hint = screen.getByRole("tooltip");
    const left = Number.parseFloat(hint.style.left);
    const top = Number.parseFloat(hint.style.top);
    expect(left).toBeGreaterThanOrEqual(8);
    expect(top).toBeGreaterThanOrEqual(8);
    expect(left).toBeLessThanOrEqual(192);
    expect(top).toBeLessThanOrEqual(92);
  });

  it("can hide the comparison without changing save quality", () => {
    const onDismiss = vi.fn();
    render(
      <CompressionPreview
        beforeUrl="blob:before"
        afterUrl="blob:after"
        beforeBytes={1_000_000}
        afterBytes={250_000}
        pending={false}
        onDismiss={onDismiss}
      />,
    );

    fireEvent.click(screen.getByRole("button", { name: "Hide compression comparison" }));
    expect(onDismiss).toHaveBeenCalledTimes(1);
  });

  it("fades the overlay away while the editor is drawing underneath", () => {
    render(
      <CompressionPreview
        className="is-embed is-cover"
        beforeUrl="blob:before"
        afterUrl="blob:after"
        beforeBytes={1_000_000}
        afterBytes={250_000}
        pending={false}
        suppressed
      />,
    );

    expect(screen.getByRole("group", { name: "Compression comparison" }))
      .toHaveClass("is-suppressed");
  });

  it("restores a saved split and locks the handle while drawing", () => {
    const onSplitChange = vi.fn();
    const { rerender } = render(
      <CompressionPreview
        beforeUrl="blob:before"
        afterUrl="blob:after"
        beforeBytes={1_000_000}
        afterBytes={250_000}
        pending={false}
        initialSplit={72}
        onSplitChange={onSplitChange}
        splitDragEnabled={false}
      />,
    );

    const frame = screen.getByRole("group", { name: "Compression comparison" });
    expect(frame).toHaveClass("is-draw-locked");
    expect(screen.getByRole("slider", { name: "Before and after comparison" }))
      .toHaveValue("72");

    rerender(
      <CompressionPreview
        beforeUrl="blob:before"
        afterUrl="blob:after-2"
        beforeBytes={1_000_000}
        afterBytes={180_000}
        pending={false}
        initialSplit={72}
        onSplitChange={onSplitChange}
        splitDragEnabled={false}
      />,
    );
    expect(screen.getByRole("slider", { name: "Before and after comparison" }))
      .toHaveValue("72");
  });
});
