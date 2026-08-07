import { afterEach, expect } from "vitest";
import * as matchers from "@testing-library/jest-dom/matchers";
import { cleanup } from "@testing-library/react";

expect.extend(matchers);

// `globals: true` isn't set in vite.config.ts, so RTL's own auto-cleanup
// (which relies on detecting a global `afterEach`) never registers. Without
// this, each render() leaves its DOM mounted for the next test in the same
// file — harmless for tests that only assert presence, but it corrupts any
// test asserting *absence* (queryByText(...).not.toBeInTheDocument()),
// since a prior test's leftover node satisfies the query.
afterEach(cleanup);
