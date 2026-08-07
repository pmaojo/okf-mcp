import { describe, it, expect } from "vitest";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { RunPanel } from "./ToolLayout";

describe("RunPanel", () => {
  it("starts expanded by default (no toolResult yet — nothing to show but the form)", () => {
    render(<RunPanel>form fields</RunPanel>);
    expect(screen.getByText("form fields")).toBeInTheDocument();
  });

  it("starts collapsed when the host already provided a result (defaultOpen=false)", () => {
    render(<RunPanel defaultOpen={false}>form fields</RunPanel>);
    expect(screen.queryByText("form fields")).not.toBeInTheDocument();
  });

  it("toggles open and closed on click regardless of the initial state", async () => {
    const user = userEvent.setup();
    render(<RunPanel defaultOpen={false}>form fields</RunPanel>);

    expect(screen.queryByText("form fields")).not.toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: /edit/i }));
    expect(screen.getByText("form fields")).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: /hide/i }));
    expect(screen.queryByText("form fields")).not.toBeInTheDocument();
  });
});
