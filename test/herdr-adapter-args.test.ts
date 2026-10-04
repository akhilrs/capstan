import assert from "node:assert/strict";
import { test } from "node:test";
import { InvalidArgumentError, claudeArguments } from "../src/herdr/adapter.js";

const HOOKS_OFF_SETTINGS = JSON.stringify({
  disableAllHooks: true,
  includeCoAuthoredBy: false,
  attribution: { commit: "", pr: "" },
});
test("claude settings turn Claude attribution off with hooks on and off", () => {
  for (const hooks of ["inherit", "off"] as const) {
    const args = claudeArguments({
      model: null,
      permissionMode: "default",
      allow: [],
      deny: [],
      hooks,
    });
    const settings = JSON.parse(args[args.indexOf("--settings") + 1] as string);
    assert.equal(settings.includeCoAuthoredBy, false);
    assert.deepEqual(settings.attribution, { commit: "", pr: "" });
    assert.equal(settings.disableAllHooks === true, hooks === "off");
    assert.equal(settings.hooks !== undefined, hooks === "inherit");
  }
});
test("claudeArguments puts --mcp-config and --strict-mcp-config before --settings and the prompt file", () => {
  const base = {
    model: null,
    permissionMode: "default",
    allow: ["WebSearch"],
    deny: ["Agent"],
    hooks: "off",
  } as const;
  const mcp = [
    {
      name: "playwright",
      command: "npx",
      args: ["-y", "@playwright/mcp@0.0.83", "--headless"],
    },
  ];
  const args = claudeArguments({ ...base, mcp }, "/tmp/p.md");
  const at = args.indexOf("--mcp-config");
  assert.deepEqual(args.slice(0, at), [
    "--permission-mode",
    "default",
    "--allowedTools",
    "WebSearch",
    "--disallowedTools",
    "Agent",
  ]);
  assert.deepEqual(args.slice(at + 2), [
    "--strict-mcp-config",
    "--settings",
    HOOKS_OFF_SETTINGS,
    "--append-system-prompt-file",
    "/tmp/p.md",
  ]);
  assert.deepEqual(JSON.parse(args[at + 1] as string), {
    mcpServers: {
      playwright: {
        type: "stdio",
        command: "npx",
        args: ["-y", "@playwright/mcp@0.0.83", "--headless"],
      },
    },
  });
  assert.deepEqual(
    claudeArguments({ ...base, mcp: [] }, "/tmp/p.md"),
    claudeArguments(base, "/tmp/p.md"),
  );
  assert.ok(!claudeArguments(base).includes("--mcp-config"));
  for (const command of ["bad\ncommand", "-x"])
    assert.throws(
      () =>
        claudeArguments({ ...base, mcp: [{ name: "p", command, args: [] }] }),
      InvalidArgumentError,
    );
  assert.throws(
    () =>
      claudeArguments({
        ...base,
        mcp: [{ name: "p", command: "npx", args: ["a\nb"] }],
      }),
    InvalidArgumentError,
  );
});
test("claudeArguments builds the per-role list and refuses control characters", () => {
  const base = {
    model: null,
    permissionMode: "default",
    allow: [],
    deny: [],
    hooks: "inherit",
  } as const;
  const inherited = claudeArguments(base);
  assert.deepEqual(inherited.slice(0, 2), ["--permission-mode", "default"]);
  assert.equal(inherited.filter((arg) => arg === "--settings").length, 1);
  assert.deepEqual(
    JSON.parse(inherited[inherited.indexOf("--settings") + 1]!),
    {
      includeCoAuthoredBy: false,
      attribution: { commit: "", pr: "" },
      hooks: {
        PostToolUse: [
          {
            matcher: "*",
            hooks: [
              { type: "command", command: "cstan inbox --hook", timeout: 5 },
            ],
          },
        ],
      },
    },
  );
  assert.deepEqual(
    claudeArguments(
      {
        model: "opus",
        permissionMode: "acceptEdits",
        allow: ["Bash(cstan *)", "Read"],
        deny: ["Bash(rm *)"],
        hooks: "off",
      },
      "/tmp/p.md",
    ),
    [
      "--model",
      "opus",
      "--permission-mode",
      "acceptEdits",
      "--allowedTools",
      "Bash(cstan *)",
      "Read",
      "--disallowedTools",
      "Bash(rm *)",
      "--settings",
      HOOKS_OFF_SETTINGS,
      "--append-system-prompt-file",
      "/tmp/p.md",
    ],
  );
  assert.throws(
    () => claudeArguments({ ...base, allow: ["a\nb"] }),
    InvalidArgumentError,
  );
  assert.throws(
    () => claudeArguments({ ...base, model: "x\u0000" }),
    InvalidArgumentError,
  );
});
