/// <reference types="vitest/config" />
import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";
import tailwindcss from "@tailwindcss/vite";
import path from "path";
import { viteSingleFile } from "vite-plugin-singlefile";

// More info at: https://storybook.js.org/docs/next/writing-tests/integrations/vitest-addon
const INPUT = process.env.INPUT;
const isStorybook =
  process.argv.some((arg) => arg.includes("storybook")) ||
  process.env.STORYBOOK ||
  process.env.npm_lifecycle_event?.includes("storybook");

if (
  !INPUT &&
  !isStorybook &&
  process.env.NODE_ENV !== "test" &&
  !process.env.VITEST
) {
  throw new Error("INPUT environment variable is not set");
}
const isDevelopment = process.env.NODE_ENV === "development";
export default defineConfig({
  plugins: [
    react(),
    tailwindcss(),
    !isStorybook && !process.env.VITEST && viteSingleFile(),
  ].filter(Boolean),
  resolve: {
    alias: {
      "@": path.resolve(__dirname, "./src"),
    },
  },
  test: {
    include: ["src/**/*.test.{ts,tsx}", "src/**/*.spec.{ts,tsx}"],
    environment: "jsdom",
    setupFiles: ["./vitest.setup.ts"],
    coverage: {
      provider: "v8",
      include: ["src/**/*.{ts,tsx}"],
      exclude: [
        "test/**",
        "src/**/*.d.ts",
        "src/**/*.test.{ts,tsx}",
        "src/shared/components/ui/**",
        "src/stories/**",
        "src/tools/**/view.tsx",
        "src/tools/**/components/**",
        "src/tools/**/libs/**",
        "src/tools/**/pages/**",
        "src/tools/**/index.ts",
        "src/tools/**/*.tsx",
        "src/shared/hooks/**",
        "src/core/framework/**",
        "src/types/**",
      ],
      reporter: ["text", "json", "html"],
    },
  },
  build: {
    sourcemap: isDevelopment ? "inline" : undefined,
    cssMinify: !isDevelopment,
    minify: !isDevelopment,
    rollupOptions: {
      input: INPUT,
    },
    outDir: "dist",
    emptyOutDir: false,
  },
});
