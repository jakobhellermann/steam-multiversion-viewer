/// <reference types="vitest/config" />
import { defineConfig } from "vite";
import { devtools } from "@tanstack/devtools-vite";

import { tanstackRouter } from "@tanstack/router-plugin/vite";

import viteReact from "@vitejs/plugin-react";
import tailwindcss from "@tailwindcss/vite";

const config = defineConfig({
  resolve: { tsconfigPaths: true },
  server: { proxy: { "/api": "http://127.0.0.1:6556" } },
  // The cpp shiki grammar is a ~660KB lazy chunk by nature; raise the limit
  // above it so the warning only fires for genuinely unexpected bloat.
  build: { chunkSizeWarningLimit: 700 },
  plugins: [
    devtools(),
    tailwindcss(),
    tanstackRouter({
      target: "react",
      autoCodeSplitting: true,
      routeFileIgnorePattern: "\\.(test|spec)\\.[cm]?[jt]sx?$",
    }),
    viteReact(),
  ],
  test: {
    environment: "happy-dom",
    globals: true,
    setupFiles: ["./test/setup.ts"],
    restoreMocks: true,
    clearMocks: true,
    unstubGlobals: true,
  },
});

export default config;
