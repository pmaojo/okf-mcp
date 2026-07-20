import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen } from "@testing-library/react";
import { AppContent } from "@/mcp-app";
import { useMcp } from "@/core/mcp/provider/McpProvider";

import type { App, McpUiHostContext } from "@modelcontextprotocol/ext-apps";

vi.mock("@/core/mcp/provider/McpProvider", () => ({
  useMcp: vi.fn(),
  McpProvider: ({ children }: { children: React.ReactNode }) => (
    <div>{children}</div>
  ),
}));

vi.mock("../src/tools/registry", () => ({
  TOOL_COMPONENTS: {
    "test-tool": () => <div data-testid="test-tool">Test Tool Component</div>,
  },
}));

describe("AppContent", () => {
  beforeEach(() => {
    vi.clearAllMocks();
  });

  it("should render the error state", () => {
    vi.mocked(useMcp).mockReturnValue({
      error: new Error("Connection failed"),
      app: null,
      hostContext: undefined,
      toolResult: null,
    });

    render(<AppContent />);
    expect(screen.getByText("Error:")).toBeInTheDocument();
    expect(screen.getByText("Connection failed")).toBeInTheDocument();
  });

  it("should render the connecting state", () => {
    vi.mocked(useMcp).mockReturnValue({
      error: null,
      app: null,
      hostContext: undefined,
      toolResult: null,
    });

    render(<AppContent />);
    expect(screen.getByText("Connecting...")).toBeInTheDocument();
  });

  it("should render the missing tool context state", () => {
    vi.mocked(useMcp).mockReturnValue({
      error: null,
      app: {} as App,
      hostContext: undefined,
      toolResult: null,
    });

    render(<AppContent />);
    expect(
      screen.getByText(/waiting for the host to open a tool/i)
    ).toBeInTheDocument();
  });

  it("should render the unknown tool state", () => {
    vi.mocked(useMcp).mockReturnValue({
      error: null,
      app: {} as App,
      hostContext: {
        toolInfo: {
          tool: { name: "unknown-tool" },
        },
      } as unknown as McpUiHostContext,
      toolResult: null,
    });

    render(<AppContent />);
    expect(
      screen.getByText('Tool UI for "unknown-tool" was not found.')
    ).toBeInTheDocument();
  });

  it("should render the resolved tool component", () => {
    vi.mocked(useMcp).mockReturnValue({
      error: null,
      app: {} as App,
      hostContext: {
        toolInfo: {
          tool: { name: "test-tool" },
        },
      } as unknown as McpUiHostContext,
      toolResult: null,
    });

    render(<AppContent />);
    expect(screen.getByTestId("test-tool")).toBeInTheDocument();
  });
});
