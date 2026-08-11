import { fireEvent, render, screen } from "@testing-library/react";

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
    expect(screen.getByRole("slider", { name: "Before and after comparison" }))
      .toHaveValue("50");
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
