import { describe, it, expect, vi, beforeEach } from "vitest";
import { logger } from "@/core/mcp/logger/mcpLogger";
import { toast } from "sonner";
import type { App } from "@modelcontextprotocol/ext-apps";

vi.mock("sonner", () => ({
  toast: {
    error: vi.fn(),
    warning: vi.fn(),
    info: vi.fn(),
  },
}));

describe("logger", () => {
  let mockApp: App;

  beforeEach(() => {
    vi.clearAllMocks();
    mockApp = {
      sendLog: vi.fn().mockResolvedValue(undefined),
    } as unknown as App;
    logger.setApp(mockApp);
    vi.spyOn(console, "error").mockImplementation(() => {});
    vi.spyOn(console, "warn").mockImplementation(() => {});
    vi.spyOn(console, "info").mockImplementation(() => {});
    vi.spyOn(console, "debug").mockImplementation(() => {});
  });

  it("should log an error and forward it to the host", async () => {
    await logger.error("test error", { test: 1 });
    expect(console.error).toHaveBeenCalledWith("test error", { test: 1 });
    expect(toast.error).toHaveBeenCalledWith("test error", {
      description: '{"test":1}',
    });
    expect(mockApp.sendLog).toHaveBeenCalledWith({
      level: "error",
      data: { level: "error", message: "test error", data: { test: 1 } },
    });
  });

  it("should log a warning and forward it to the host", async () => {
    await logger.warn("test warn", { test: 2 });
    expect(console.warn).toHaveBeenCalledWith("test warn", { test: 2 });
    expect(toast.warning).toHaveBeenCalledWith("test warn", {
      description: '{"test":2}',
    });
    expect(mockApp.sendLog).toHaveBeenCalledWith({
      level: "warn",
      data: { level: "warn", message: "test warn", data: { test: 2 } },
    });
  });

  it("should log info and forward it to the host", async () => {
    await logger.info("test info");
    expect(console.info).toHaveBeenCalledWith("test info", undefined);
    expect(toast.info).toHaveBeenCalledWith("test info", {
      description: undefined,
    });
    expect(mockApp.sendLog).toHaveBeenCalledWith({
      level: "info",
      data: { level: "info", message: "test info", data: undefined },
    });
  });

  it("should log debug data and forward it to the host", async () => {
    await logger.debug("test debug");
    expect(console.debug).toHaveBeenCalledWith("test debug", undefined);
    expect(mockApp.sendLog).toHaveBeenCalledWith({
      level: "debug",
      data: { level: "debug", message: "test debug", data: undefined },
    });
  });

  it("should handle host logging failures gracefully", async () => {
    mockApp.sendLog = vi.fn().mockRejectedValue(new Error("Network error"));
    await logger.info("test info with failing sendLog");
    expect(console.error).toHaveBeenCalledWith(
      "Failed to send log to MCP host",
      expect.any(Error)
    );
  });

  it("should still work without an app instance", async () => {
    logger.setApp(null as unknown as App);
    await logger.info("test info without app");
    expect(console.info).toHaveBeenCalledWith(
      "test info without app",
      undefined
    );
  });
});
