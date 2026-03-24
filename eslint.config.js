import js from "@eslint/js";
import tseslint from "typescript-eslint";
import reactHooks from "eslint-plugin-react-hooks";

export default tseslint.config(
  js.configs.recommended,
  ...tseslint.configs.recommended,
  {
    plugins: {
      "react-hooks": reactHooks,
    },
    rules: {
      ...reactHooks.configs.recommended.rules,
      // React 19 strict rules — warn for now, tighten to error incrementally
      "react-hooks/set-state-in-effect": "warn",
      "react-hooks/refs": "warn",
      // Align with project convention: logError for catches, not empty blocks
      "@typescript-eslint/no-unused-vars": [
        "error",
        { argsIgnorePattern: "^_", varsIgnorePattern: "^_" },
      ],
    },
  },
  {
    ignores: ["dist/", "src-tauri/", "*.config.js", "*.config.ts"],
  },
);
