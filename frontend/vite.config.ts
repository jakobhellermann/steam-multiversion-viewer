/// <reference types="vitest/config" />
import { defineConfig, type Plugin } from "vite";
import { devtools } from "@tanstack/devtools-vite";

import { tanstackRouter } from "@tanstack/router-plugin/vite";

import viteReact from "@vitejs/plugin-react";
import tailwindcss from "@tailwindcss/vite";

const BACKEND = "http://127.0.0.1:6556";

/// Embeds the backend's query-cache preload into `index.html`, as the backend does for its own pages.
function backendPreload(): Plugin {
  return {
    name: "backend-preload",
    apply: "serve",
    async transformIndexHtml(html, ctx) {
      const url = ctx.originalUrl ?? ctx.path;
      let entries: unknown[];
      try {
        const r = await fetch(`${BACKEND}/api/preload?url=${encodeURIComponent(url)}`);
        if (!r.ok) throw new Error(`${r.status} ${await r.text()}`);
        entries = (await r.json()) as unknown[];
      } catch (err) {
        // The page still works without it, like on the backend's own pages.
        console.error(`preload for ${url} failed:`, err);
        return html;
      }
      if (entries.length === 0) return html;
      return {
        html,
        tags: [
          {
            tag: "script",
            attrs: { id: "preload", type: "application/json" },
            children: JSON.stringify(entries).replaceAll("<", "\\u003c"),
            injectTo: "head",
          },
        ],
      };
    },
  };
}

const config = defineConfig({
  resolve: { tsconfigPaths: true },
  server: { proxy: { "/api": BACKEND } },
  // The cpp shiki grammar is a large lazy chunk by nature (raw size, but
  // ~56KB gzipped and loaded on demand); keep headroom above it so the
  // warning only fires for genuinely unexpected app-code bloat.
  build: { chunkSizeWarningLimit: 900 },
  plugins: [
    devtools(),
    backendPreload(),
    tailwindcss(),
    tanstackRouter({
      target: "react",
      // Served from localhost: splitting saves no transfer, but adds a chunk fetch per route.
      autoCodeSplitting: false,
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
