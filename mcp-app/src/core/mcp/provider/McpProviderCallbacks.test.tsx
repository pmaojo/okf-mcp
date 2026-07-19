import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen, act } from "@testing-library/react";
import { McpProvider, useMcp } from "@/core/mcp/provider/McpProvider";
import * as mcpReact from "@modelcontextprotocol/ext-apps/react";
import type { App, McpUiHostContext } from "@modelcontextprotocol/ext-apps";

vi.mock("@modelcontextprotocol/ext-apps/react", () => ({
  useApp: vi.fn(),
  useHostStyles: vi.fn(),
}));

vi.mock("@/core/mcp/logger/mcpLogger", () => ({
  logger: {
    setApp: vi.fn(),
    info: vi.fn(),
    warn: vi.fn(),
    error: vi.fn(),
    debug: vi.fn(),
  },
}));

function TestComponent() {
  const { toolResult } = useMcp();
  return (
    <div>
      <div data-testid="tool-result">
        {toolResult ? "has result" : "no result"}
      </div>
    </div>
  );
}

describe("McpProvider callbacks", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    sessionStorage.clear();
  });

  it("should register lifecycle callbacks and react to them", () => {
    const appCallbacks: Record<string, (...args: unknown[]) => void> = {};
    const mockApp = {
      getHostContext: () =>
        ({ appInfo: { name: "TestHost" } }) as unknown as McpUiHostContext,
      set onteardown(cb: () => void) {
        appCallbacks.onteardown = cb;
      },
      set ontoolinput(cb: (input: unknown) => void) {
        appCallbacks.ontoolinput = cb;
      },
      set ontoolresult(cb: (result: unknown) => void) {
        appCallbacks.ontoolresult = cb;
      },
      set ontoolcancelled(cb: (reason: unknown) => void) {
        appCallbacks.ontoolcancelled = cb;
      },
      set onerror(cb: (error: Error) => void) {
        appCallbacks.onerror = cb as (...args: unknown[]) => void;
      },
      set onhostcontextchanged(cb: () => void) {
        appCallbacks.onhostcontextchanged = cb;
      },
    } as unknown as App;

    vi.mocked(mcpReact.useApp).mockImplementation((options) => {
      options.onAppCreated?.(mockApp);
      return {
        app: mockApp,
        error: null,
        isConnected: true,
      };
    });

    render(
      <McpProvider>
        <TestComponent />
      </McpProvider>
    );

    act(() => {
      appCallbacks.onteardown();
      appCallbacks.ontoolinput({ input: "test" });
      appCallbacks.ontoolcancelled({ reason: "test reason" });
      appCallbacks.onerror(new Error("test error"));
      appCallbacks.onhostcontextchanged();
      appCallbacks.ontoolresult({ result: "test" });
    });

    expect(screen.getByTestId("tool-result").textContent).toBe("has result");
  });
});
