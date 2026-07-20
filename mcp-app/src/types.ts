export interface LogMessage {
  level: "info" | "warn" | "error" | "debug";
  message: string;
  data?: unknown;
}

export interface AppState {
  serverTime: string;
  status: "idle" | "loading" | "error";
}
