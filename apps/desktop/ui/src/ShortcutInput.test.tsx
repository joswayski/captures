import { fireEvent, render, screen } from "@testing-library/react";
import { useState } from "react";

import { ShortcutInput } from "./App";

function ShortcutHarness() {
  const [value, setValue] = useState("Ctrl+Shift+4");
  const [recording, setRecording] = useState(false);
  return (
    <ShortcutInput
      id="region-shortcut"
      label="Region"
      value={value}
      recording={recording}
      onRecordingChange={setRecording}
      onChange={setValue}
    />
  );
}

describe("ShortcutInput", () => {
  it("records a physical key combination instead of exposing a text field", () => {
    render(<ShortcutHarness />);

    expect(screen.queryByRole("textbox")).not.toBeInTheDocument();
    const recorder = screen.getByRole("button", { name: "Region" });
    fireEvent.click(recorder);
    expect(screen.getByText("Press shortcut…")).toBeInTheDocument();
    expect(recorder).toHaveFocus();

    fireEvent.keyDown(document.activeElement!, {
      code: "ShiftLeft",
      key: "Shift",
      ctrlKey: true,
      shiftKey: true,
    });
    expect(screen.getByText("Ctrl")).toBeInTheDocument();
    expect(screen.getByText("Shift")).toBeInTheDocument();

    fireEvent.keyDown(document.activeElement!, {
      code: "Digit3",
      key: "#",
      ctrlKey: true,
      shiftKey: true,
    });
    expect(screen.getByText("3")).toBeInTheDocument();
    expect(recorder).toHaveAttribute("aria-pressed", "false");
  });

  it("cancels recording without changing the shortcut", () => {
    render(<ShortcutHarness />);

    const recorder = screen.getByRole("button", { name: "Region" });
    fireEvent.click(recorder);
    fireEvent.keyDown(recorder, { code: "Escape", key: "Escape" });

    expect(recorder).toHaveAttribute("aria-pressed", "false");
    expect(screen.getByText("4")).toBeInTheDocument();
  });
});
