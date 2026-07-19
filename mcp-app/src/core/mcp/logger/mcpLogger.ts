import type { App } from "@modelcontextprotocol/ext-apps";
import { toast } from "sonner";

export interface LogMessage {
  level: "debug" | "info" | "warn" | "error";
  message: string;
  data?: unknown;
}

let _app: App | null = null;

const sendLog = async (
  level: LogMessage["level"],
  message: string,
  data?: unknown
) => {
  const logMessage: LogMessage = { level, message, data };

  if (level === "error") {
    console.error(message, data);
    toast.error(message, {
      description: data ? JSON.stringify(data) : undefined,
    });
  } else if (level === "warn") {
    console.warn(message, data);
    toast.warning(message, {
      description: data ? JSON.stringify(data) : undefined,
    });
  } else if (level === "info") {
    console.info(message, data);
    toast.info(message, {
      description: data ? JSON.stringify(data) : undefined,
    });
  } else {
    console.debug(message, data);
  }

  if (_app) {
    try {
      const sendLevel =
        level === "warn" ||
        level === "error" ||
        level === "info" ||
        level === "debug"
          ? level
          : "info";
      // The App['sendLog'] method expects a specific string union for level
      await _app.sendLog({
        level: sendLevel as
          | "debug"
          | "info"
          | "warning"
          | "error"
          | "notice"
          | "alert"
          | "critical"
          | "emergency",
        data: logMessage,
      });
    } catch (e) {
      console.error("Failed to send log to MCP host", e);
    }
  }
};

export const logger = {
  setApp: (app: App | null) => {
    _app = app;
  },

  info: async (message: string, data?: unknown) => {
    await sendLog("info", message, data);
  },

  warn: async (message: string, data?: unknown) => {
    await sendLog("warn", message, data);
  },

  error: async (message: string, data?: unknown) => {
    await sendLog("error", message, data);
  },

  debug: async (message: string, data?: unknown) => {
    await sendLog("debug", message, data);
  },
};
