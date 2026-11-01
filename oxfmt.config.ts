import { defineConfig } from "oxfmt"

export default defineConfig({
  printWidth: 160,
  semi: false,
  trailingComma: "es5",
  sortImports: { ignoreCase: true, newlinesBetween: false, order: "asc" },
  ignorePatterns: ["test/fixtures/**", "migrations/**"],
})
