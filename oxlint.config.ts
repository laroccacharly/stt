import { defineConfig } from "oxlint"
import tsLint from "ts-lint/config"

export default defineConfig({
  extends: [tsLint],
  // Throwaway measurement scripts, run with bun directly.
  ignorePatterns: ["bench/**"],
  options: {
    typeAware: true,
    typeCheck: true,
  },
  rules: {
    "ts-lint/no-unknown": "error",
    "ts-lint/no-try-promise": "off",
  },
})
