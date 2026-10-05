import { builtinRules } from "eslint/use-at-your-own-risk";
import tseslint from "typescript-eslint";

// File-size guard: a hard cap (error) and an earlier soft cap (warn).
// The soft rule reuses ESLint's built-in max-lines under another name so both
// can run at once. `eslint/use-at-your-own-risk` is not a stable API; if a
// future ESLint drops it, replace it with a ~30-line local rule that counts
// lines that are neither blank nor comments.
const capstan = { rules: { "max-lines-soft": builtinRules.get("max-lines") } };

const PUBLIC_CONTROLLER_MODULES = [
  "core",
  "types",
  "auth",
  "canonical",
  "database",
  "messaging",
  "ownership",
  "sqlite",
];

const limits = (hard, soft) => ({
  "max-lines": [
    "error",
    { max: hard, skipBlankLines: true, skipComments: true },
  ],
  "capstan/max-lines-soft": [
    "warn",
    { max: soft, skipBlankLines: true, skipComments: true },
  ],
});

export default tseslint.config(
  { ignores: ["dist/**", "node_modules/**"] },
  ...tseslint.configs.recommended,
  { linterOptions: { reportUnusedDisableDirectives: "error" } },
  { plugins: { capstan } },
  {
    // The landing page is a plain browser script, not TypeScript or Node.
    files: ["site/**/*.js"],
    languageOptions: {
      sourceType: "script",
      globals: Object.fromEntries(
        [
          "clearTimeout",
          "document",
          "localStorage",
          "navigator",
          "requestAnimationFrame",
          "setTimeout",
          "window",
        ].map((name) => [name, "readonly"]),
      ),
    },
    rules: { "no-undef": "error" },
  },
  { files: ["src/**"], rules: limits(1500, 800) },
  { files: ["test/**"], rules: limits(3000, 1500) },
  {
    // Only the facade and the public modules of src/controller/ may be imported from outside it. Every other
    // module (kernel, areas, area files, records, errors, helpers, ...) is internal, so a new area file is
    // protected without touching this rule.
    files: ["src/**", "test/**"],
    ignores: ["src/controller/**"],
    rules: {
      "no-restricted-imports": [
        "error",
        {
          patterns: [
            {
              group: [
                "**/controller/*",
                ...PUBLIC_CONTROLLER_MODULES.map(
                  (name) => `!**/controller/${name}.js`,
                ),
              ],
              message:
                "src/controller/ modules other than the public ones are internal; import ControllerCore from core.ts.",
            },
          ],
        },
      ],
    },
  },
);
