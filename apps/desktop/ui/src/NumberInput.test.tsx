import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import { NumberInput } from "./NumberInput";

describe("NumberInput", () => {
  it("exposes a spinbutton and larger custom steppers", () => {
    const onChange = vi.fn();
    render(
      <NumberInput
        ariaLabel="Canvas width"
        value={100}
        min={1}
        max={200}
        onChange={onChange}
      />,
    );

    expect(screen.getByRole("spinbutton", { name: "Canvas width" })).toHaveValue(100);
    expect(screen.getByRole("button", { name: "Increase Canvas width" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Decrease Canvas width" })).toBeInTheDocument();
  });

  it("increments and decrements with the custom buttons", () => {
    const onChange = vi.fn();
    const { rerender } = render(
      <NumberInput
        ariaLabel="Size"
        value={10}
        min={0}
        max={20}
        step={2}
        onChange={onChange}
      />,
    );

    fireEvent.click(screen.getByRole("button", { name: "Increase Size" }));
    expect(onChange).toHaveBeenLastCalledWith(12);

    rerender(
      <NumberInput
        ariaLabel="Size"
        value={12}
        min={0}
        max={20}
        step={2}
        onChange={onChange}
      />,
    );

    fireEvent.click(screen.getByRole("button", { name: "Decrease Size" }));
    expect(onChange).toHaveBeenLastCalledWith(10);
  });

  it("supports freeform string fields for size limits", () => {
    const onTextChange = vi.fn();
    render(
      <NumberInput
        ariaLabel="Maximum file size"
        value="1.5"
        min={0.1}
        step={0.1}
        onTextChange={onTextChange}
      />,
    );

    fireEvent.change(screen.getByRole("spinbutton", { name: "Maximum file size" }), {
      target: { value: "2.5" },
    });
    expect(onTextChange).toHaveBeenLastCalledWith("2.5");

    fireEvent.click(screen.getByRole("button", { name: "Increase Maximum file size" }));
    expect(onTextChange).toHaveBeenLastCalledWith("1.6");
  });

  it("commits deferred fields once per entry instead of once per keystroke", () => {
    const onTextChange = vi.fn();
    const onCommit = vi.fn();
    const { rerender } = render(
      <NumberInput
        ariaLabel="Canvas width"
        value="1600"
        min={1}
        max={16_384}
        onTextChange={onTextChange}
        onCommit={onCommit}
      />,
    );

    const field = screen.getByRole("spinbutton", { name: "Canvas width" });
    fireEvent.change(field, { target: { value: "14" } });
    fireEvent.change(field, { target: { value: "1400" } });
    expect(onTextChange).toHaveBeenCalledTimes(2);
    expect(onCommit).not.toHaveBeenCalled();

    rerender(
      <NumberInput
        ariaLabel="Canvas width"
        value="1400"
        min={1}
        max={16_384}
        onTextChange={onTextChange}
        onCommit={onCommit}
      />,
    );

    fireEvent.keyDown(field, { key: "Enter" });
    expect(onCommit).toHaveBeenCalledTimes(1);
    expect(onCommit).toHaveBeenLastCalledWith("1400");

    fireEvent.blur(field);
    expect(onCommit).toHaveBeenCalledTimes(2);

    fireEvent.click(screen.getByRole("button", { name: "Increase Canvas width" }));
    expect(onCommit).toHaveBeenLastCalledWith("1401");
  });

  it("hides steppers when read-only", () => {
    render(<NumberInput ariaLabel="Locked height" value={240} readOnly />);
    expect(screen.getByRole("spinbutton", { name: "Locked height" })).toHaveAttribute("readonly");
    expect(screen.queryByRole("button", { name: /Increase/ })).not.toBeInTheDocument();
  });

  it("hides steppers when disabled", () => {
    render(<NumberInput ariaLabel="Layer width" value={240} disabled onChange={() => undefined} />);
    expect(screen.getByRole("spinbutton", { name: "Layer width" })).toBeDisabled();
    expect(screen.queryByRole("button", { name: /Increase/ })).not.toBeInTheDocument();
  });
});
