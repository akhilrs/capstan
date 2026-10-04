import assert from "node:assert/strict";
import {
  mkdirSync,
  mkdtempSync,
  readFileSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import { test } from "node:test";
import { loadCapstanConfig } from "../src/config/capstan-config.js";
import { MAX_PROMPT_BYTES, buildRolePrompt } from "../src/prompts.js";
import {
  DESIGNER_PROMPT,
  DESIGNER_PROMPT_PATH,
} from "../src/roles/designer-prompt.js";

// The compiled test runs from dist/test, so the repo root is two levels up.
const ROOT = path.resolve(import.meta.dirname, "..", "..");

const SKILL_ALLOWLIST = new Set([
  "high-end-visual-design",
  "minimalist-ui",
  "web-design-guidelines",
  "emil-design-eng",
  "redesign-existing-projects",
  "sleek-design-mobile-apps",
  "stitch-design-taste",
  "gpt-taste",
  "imagegen-frontend-web",
  "imagegen-frontend-mobile",
  "ui-ux-pro-max",
  "frontend-design",
]);

test("DESIGNER_PROMPT equals the tracked roles/designer.md byte for byte", () => {
  const file = readFileSync(path.join(ROOT, DESIGNER_PROMPT_PATH));
  assert.equal(DESIGNER_PROMPT_PATH, "roles/designer.md");
  assert.ok(Buffer.from(DESIGNER_PROMPT, "utf8").equals(file));
  assert.ok(DESIGNER_PROMPT.endsWith("\n") && !DESIGNER_PROMPT.includes("\r"));
  assert.ok(Buffer.byteLength(DESIGNER_PROMPT) < 12 * 1024);
});

test("the designer prompt has its sections in order and the required rules", () => {
  const markers = [
    "## Claude Design",
    "## Brief",
    "## Directions",
    "## Build",
    "## Verify",
    "## Anti-slop standard",
    "## When a tool is missing",
  ];
  let at = -1;
  for (const marker of markers) {
    const next = DESIGNER_PROMPT.indexOf(marker);
    assert.ok(next > at, marker);
    at = next;
  }
  for (const needle of [
    'action "quickstart" and intent "design"',
    "list_projects",
    "get_project",
    "list_files",
    "get_file",
    "Never use DesignSync to make designs",
    "Never create a design system unasked",
    "mcp__playwright__*",
    "375, 768 and 1440",
    "browser_emulate_media",
    "WCAG AA",
    "Tab traversal",
    "Never claim a screenshot or a check that did not run",
    "Work only in your own worktree",
    "Never push and never merge",
  ])
    assert.ok(DESIGNER_PROMPT.includes(needle), needle);
  const avoid = DESIGNER_PROMPT.slice(
    DESIGNER_PROMPT.indexOf("Avoid:"),
    DESIGNER_PROMPT.indexOf("Require:"),
  );
  for (const item of [
    "Purple-to-blue gradients",
    "Glassmorphism everywhere",
    "Emoji or stock-icon grids",
    "Three identical feature cards",
    "A centred hero with vague, generic copy",
    "Everything rounded and soft-shadowed",
    "Lorem ipsum",
    "Generic Inter on white",
  ])
    assert.ok(avoid.includes(item), item);
  const require = DESIGNER_PROMPT.slice(DESIGNER_PROMPT.indexOf("Require:"));
  for (const item of [
    "at most two typefaces",
    "A spacing system",
    "one accent color",
    "Deliberate hierarchy",
    "Real content",
    "Motion only where it carries meaning",
    "Empty, loading, error, hover and focus states",
  ])
    assert.ok(require.includes(item), item);
});

test("every skill the designer prompt names is a verified installed skill", () => {
  const named = new Set<string>();
  for (const match of DESIGNER_PROMPT.matchAll(/^- ([a-z0-9-]+):/gm))
    named.add(match[1]!);
  for (const match of DESIGNER_PROMPT.matchAll(/the ([a-z0-9-]+) skill\b/g))
    named.add(match[1]!);
  for (const name of SKILL_ALLOWLIST)
    assert.ok(DESIGNER_PROMPT.includes(name), `${name} is missing`);
  for (const name of named) assert.ok(SKILL_ALLOWLIST.has(name), name);
  assert.ok(named.size >= 12);
});

test("the designer prompt passes the prompt_file guards and fits in a Developer prompt", () => {
  const directory = mkdtempSync(path.join(tmpdir(), "capstan-designer-"));
  try {
    mkdirSync(path.join(directory, "roles"));
    writeFileSync(path.join(directory, DESIGNER_PROMPT_PATH), DESIGNER_PROMPT);
    writeFileSync(
      path.join(directory, "capstan.toml"),
      `schema_version = 1\n[hosts.claude]\nkind = "claude"\n[roles.pm]\nkind = "PM"\nhost = "claude"\n[roles.d]\nkind = "Developer"\nhost = "claude"\nprompt_file = "${DESIGNER_PROMPT_PATH}"\n`,
      { mode: 0o600 },
    );
    const role = loadCapstanConfig(directory).roles.find(
      (r) => r.name === "d",
    )!;
    assert.equal(role.promptText, DESIGNER_PROMPT);
    const text = buildRolePrompt({
      roleName: "d",
      kind: "Developer",
      agentId: "developer-1",
      waitTimeoutSeconds: 90,
      rolePrompt: role.promptText,
    });
    assert.ok(Buffer.byteLength(text) < MAX_PROMPT_BYTES);
    assert.ok(text.endsWith(DESIGNER_PROMPT));
    assert.ok(text.indexOf("cstan inbox") < text.indexOf(DESIGNER_PROMPT));
  } finally {
    rmSync(directory, { recursive: true, force: true });
  }
});
