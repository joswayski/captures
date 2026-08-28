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
    const after = screen.getByAltText("After compression");
    expect(after.parentElement).toHaveClass("compression-preview-after-clip");
    expect(screen.getByRole("group", { name: "Compression comparison" }))
      .toHaveClass("is-live");
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

    expect(screen.getByText("After · Encoding…")).toBeInTheDocument();
    expect(screen.queryByRole("slider", { name: "Before and after comparison" }))
      .not.toBeInTheDocument();
  });
});
