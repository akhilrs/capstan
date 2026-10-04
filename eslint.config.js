import { builtinRules } from "eslint/use-at-your-own-risk";
import tseslint from "typescript-eslint";

// File-size guard: a hard cap (error) and an earlier soft cap (warn).
// The soft rule reuses ESLint's built-in max-lines under another name so both
// can run at once. `eslint/use-at-your-own-risk` is not a stable API; if a
// future ESLint drops it, replace it with a ~30-line local rule that counts
// lines that are neither blank nor comments.
const capstan = { rules: { "max-lines-soft": builtinRules.get("max-lines") } };

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
  { files: ["src/**"], rules: limits(1500, 800) },
  { files: ["test/**"], rules: limits(3000, 1500) },
);
