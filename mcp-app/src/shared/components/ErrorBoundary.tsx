import { Component, type ReactNode } from "react";

interface Props {
  toolName: string;
  children: ReactNode;
}

interface State {
  error: Error | null;
}

/**
 * Without this, any render crash in a tool view (a data/component
 * mismatch, a bad payload, anything) takes the whole React tree down
 * to a blank white screen with zero diagnostics — React 18+ unmounts
 * on an uncaught render error and there's nothing here to catch it
 * otherwise. Keyed by `toolName` in mcp-app.tsx so switching to a
 * different tool always mounts a fresh boundary instead of staying
 * stuck on a previous tool's crash.
 */
export class ToolErrorBoundary extends Component<Props, State> {
  state: State = { error: null };

  static getDerivedStateFromError(error: Error): State {
    return { error };
  }

  render() {
    if (this.state.error) {
      return (
        <div className="mx-auto flex w-full max-w-3xl flex-col gap-3 p-6">
          <div className="border-2 border-destructive bg-card p-4 shadow-brutal-sm">
            <p className="text-sm font-bold tracking-wide uppercase text-destructive">
              {this.props.toolName} crashed while rendering
            </p>
            <pre className="mt-2 overflow-auto text-xs whitespace-pre-wrap">
              {this.state.error.message}
            </pre>
          </div>
        </div>
      );
    }
    return this.props.children;
  }
}
