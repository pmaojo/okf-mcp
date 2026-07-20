import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen } from "@testing-library/react";
import { McpProvider, useMcp } from "@/core/mcp/provider/McpProvider";
import * as mcpReact from "@modelcontextprotocol/ext-apps/react";
import type { App, McpUiHostContext } from "@modelcontextprotocol/ext-apps";

vi.mock("@modelcontextprotocol/ext-apps/react", () => ({
  useApp: vi.fn(),
  useHostStyles: vi.fn(),
}));

function TestComponent() {
  const { app, error, hostContext } = useMcp();
  if (error) return <div>Error: {error.message}</div>;
  if (!app) return <div>No app</div>;
  return (
    <div>
      <div>App initialized</div>
      {hostContext && (hostContext as Record<string, unknown>).appInfo ? (
        <div>
          Host:{" "}
          {String(
            (
              (hostContext as Record<string, unknown>).appInfo as {
                name: string;
              }
            ).name
          )}
        </div>
      ) : null}
    </div>
  );
}

describe("McpProvider", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    sessionStorage.clear();
  });

  it("should render an error state through the provider context", () => {
    vi.mocked(mcpReact.useApp).mockReturnValue({
      app: null,
      error: new Error("Test error"),
      isConnected: false,
    });

    render(
      <McpProvider>
        <TestComponent />
      </McpProvider>
    );

    expect(screen.getByText("Error: Test error")).toBeInTheDocument();
  });

  it("should expose the initialized app through the provider context", () => {
    const mockApp = {
      getHostContext: () =>
        ({
          appInfo: { name: "TestHost", version: "1.0.0" },
        }) as unknown as McpUiHostContext,
    } as unknown as App;

    vi.mocked(mcpReact.useApp).mockReturnValue({
      app: mockApp,
      error: null,
      isConnected: true,
    });

    render(
      <McpProvider>
        <TestComponent />
      </McpProvider>
    );

    expect(screen.getByText("App initialized")).toBeInTheDocument();
  });

  it("should throw when useMcp is consumed outside the provider", () => {
    const consoleError = vi
      .spyOn(console, "error")
      .mockImplementation(() => {});
    expect(() => render(<TestComponent />)).toThrow(
      "useMcp must be used within an McpProvider"
    );
    consoleError.mockRestore();
  });
});
