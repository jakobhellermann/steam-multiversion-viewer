// TODO(ai-review): review for style and correctness
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

import js from "@eslint/js";
import tseslint from "typescript-eslint";
import reactHooks from "eslint-plugin-react-hooks";
import tailwind from "eslint-plugin-tailwindcss";

const __dirname = dirname(fileURLToPath(import.meta.url));

export default tseslint.config(
  // Ignore generated, build artifacts, the eslint config itself.
  {
    ignores: ["dist/**", "src/routeTree.gen.ts", "node_modules/**", "eslint.config.js"],
  },

  // Base + typescript-eslint recommended rules.
  js.configs.recommended,
  ...tseslint.configs.recommended,

  // Treat any identifier starting with `_` as intentionally unused, so
  // `(_e: Event) => {}` doesn't lint. Standard rust-style convention.
  {
    rules: {
      "@typescript-eslint/no-unused-vars": [
        "error",
        {
          argsIgnorePattern: "^_",
          varsIgnorePattern: "^_",
          caughtErrorsIgnorePattern: "^_",
          destructuredArrayIgnorePattern: "^_",
        },
      ],
      "no-unused-vars": "off",
    },
  },

  // React hooks rules (rules of hooks + exhaustive-deps).
  {
    plugins: { "react-hooks": reactHooks },
    rules: {
      "react-hooks/rules-of-hooks": "error",
      "react-hooks/exhaustive-deps": "warn",
    },
  },

  // Tailwind plugin (v4-beta API). At alpha.2 only `classnames-order`
  // and `no-custom-classname` are actually wired up; the LSP's
  // `suggestCanonicalClasses` doesn't have a lint counterpart yet.
  // We only enable the sort rule — `no-custom-classname` is too noisy
  // with arbitrary values.
  {
    plugins: { tailwindcss: tailwind },
    settings: {
      tailwindcss: {
        // v4 reads its config from CSS, not JS.
        cssConfigPath: resolve(__dirname, "src/styles.css"),
      },
    },
    rules: {
      "tailwindcss/classnames-order": "warn",
    },
  },
);
