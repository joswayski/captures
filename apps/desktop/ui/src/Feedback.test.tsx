import { invoke } from "@tauri-apps/api/core";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";

import { Feedback } from "./Feedback";

vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn(),
  isTauri: () => false,
}));

describe("Feedback", () => {
  beforeEach(() => {
    vi.mocked(invoke).mockImplementation(async (command, args) => {
      if (command === "get_feedback_context") {
        return {
          app_version: "2026.08.06.1",
          os: "macos",
          os_version: "15.5",
          arch: "aarch64",
        };
      }
      if (command === "submit_feedback") {
        const draft = (args as { draft: { message: string; category: string; contact: string | null } })
          .draft;
        expect(draft.category).toBe("bug");
        expect(draft.message).toContain("freeze");
        return { ok: true };
      }
      throw new Error(`unexpected command: ${command}`);
    });
  });

  afterEach(() => {
    vi.clearAllMocks();
  });

  it("defaults to the bug category and autofills system details", async () => {
    render(<Feedback />);

    expect(await screen.findByText("2026.08.06.1")).toBeInTheDocument();
    expect(screen.getByText(/macos · 15\.5 · aarch64/i)).toBeInTheDocument();

    const bug = screen.getByRole("radio", { name: /Bug/i });
    expect(bug).toHaveAttribute("aria-checked", "true");
    expect(screen.getByLabelText("Message")).toHaveAttribute(
      "placeholder",
      "What happened? What did you expect?",
    );
    expect(screen.getByText(/follow-up question/i)).toBeInTheDocument();

    const submit = screen.getByRole("button", { name: "Send feedback" });
    expect(submit).toBeDisabled();

    fireEvent.change(screen.getByLabelText("Message"), {
      target: { value: "Recording freezes when switching displays" },
    });
    expect(submit).not.toBeDisabled();

    fireEvent.change(screen.getByLabelText(/Contact/), {
      target: { value: "@joswayski" },
    });
    fireEvent.click(submit);

    await waitFor(() => {
      expect(invoke).toHaveBeenCalledWith("submit_feedback", {
        draft: {
          message: "Recording freezes when switching displays",
          contact: "@joswayski",
          category: "bug",
        },
      });
    });
    expect(await screen.findByText(/feedback sent/i)).toBeInTheDocument();
  });

  it("updates the message placeholder when the category changes", () => {
    render(<Feedback />);

    const message = screen.getByLabelText("Message");

    fireEvent.click(screen.getByRole("radio", { name: /Idea/i }));
    expect(message).toHaveAttribute("placeholder", "What's the idea? What problem would it solve?");

    fireEvent.click(screen.getByRole("radio", { name: /Other/i }));
    expect(message).toHaveAttribute("placeholder", "What would you like us to know?");

    fireEvent.click(screen.getByRole("radio", { name: /Bug/i }));
    expect(message).toHaveAttribute("placeholder", "What happened? What did you expect?");
  });
});
