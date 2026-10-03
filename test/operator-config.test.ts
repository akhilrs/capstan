import assert from "node:assert/strict";
import { chmodSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import { test } from "node:test";
import {
  CONFIG_FILE_NAME,
  ConfigError,
  STARTER_CONFIG,
  loadCapstanConfig,
  type CapstanConfig,
} from "../src/config/capstan-config.js";

const DENY = `["Write", "Edit", "NotebookEdit", "Agent", "Task", "Read", "Glob", "Grep"]`;

const BASE = `schema_version = 1

[hosts.claude]
kind = "claude"

[hosts.omp]
kind = "omp"

[roles.pm]
kind = "PM"
host = "claude"

[roles.architect]
kind = "Developer"
host = "claude"

[roles.reviewer]
kind = "Verifier"
host = "claude"
`;

function operatorRole(
  options: {
    name?: string;
    kind?: string;
    host?: string;
    allow?: string;
    deny?: string;
    extra?: string;
  } = {},
): string {
  return `
[roles.${options.name ?? "operator"}]
kind = "${options.kind ?? "Developer"}"
host = "${options.host ?? "claude"}"
allow = ${options.allow ?? `["Bash(cstan *)"]`}
deny = ${options.deny ?? DENY}
${options.extra ?? ""}`;
}

function load(content: string): CapstanConfig {
  const directory = mkdtempSync(path.join(tmpdir(), "capstan-operator-"));
  try {
    const file = path.join(directory, CONFIG_FILE_NAME);
    writeFileSync(file, content, { mode: 0o600 });
    chmodSync(file, 0o600);
    return loadCapstanConfig(directory);
  } finally {
    rmSync(directory, { recursive: true, force: true });
  }
}

function rejects(content: string, pattern: RegExp): void {
  assert.throws(
    () => load(content),
    (error: unknown) => {
      assert.ok(error instanceof ConfigError, String(error));
      assert.match(error.message, pattern);
      return true;
    },
  );
}

const ENABLED = `${BASE}\n[operator]\nenabled = true\n`;

test("without an [operator] table the operator is disabled with the defaults", () => {
  const config = load(BASE);
  assert.deepEqual(config.operator, {
    configured: false,
    enabled: false,
    role: "operator",
    autoApprove: [],
    autoApprovePrefix: [],
    timeoutSeconds: 300,
    maxTimeoutSeconds: 1800,
    outputTailBytes: 8192,
    proposalTtlMinutes: 60,
    approvalTtlMinutes: 10,
    maxPendingProposals: 5,
    countTowardWorkerLimit: false,
    restartHealthTimeoutSeconds: 60,
    restartIdleWaitSeconds: 120,
    sessionGrantMaxMinutes: 60,
    fullAutoDefaultMinutes: 30,
    fullAutoMaxMinutes: 120,
  });
});

test("a table that is present is configured, even when disabled", () => {
  assert.equal(load(`${BASE}\n[operator]\n`).operator.configured, true);
  assert.equal(load(`${ENABLED}${operatorRole()}`).operator.configured, true);
});

test("an enabled operator with a valid role loads", () => {
  const config = load(`${ENABLED}${operatorRole()}`);
  assert.equal(config.operator.enabled, true);
  assert.equal(config.operator.role, "operator");
});

test("enabled = true with a bad role fails with a ConfigError naming the key", () => {
  rejects(ENABLED, /operator\.role "operator" does not name a configured role/);
  rejects(
    `${ENABLED}${operatorRole({ kind: "Verifier" })}`,
    /operator\.role "operator" must be a Developer role/,
  );
  rejects(
    `${ENABLED}${operatorRole({
      host: "omp",
      allow: "[]",
      deny: "[]",
      extra: 'permission_mode = "acceptEdits"',
    })}`,
    /roles\.operator.*claude host/s,
  );
  rejects(
    `${BASE}\n[operator]\nenabled = true\nrole = "architect"\n${operatorRole({ name: "x" })}`,
    /operator\.role "architect" must differ from architect\.role/,
  );
  rejects(
    `${BASE}\n[architect]\nenabled = true\nrole = "operator"\n[operator]\nenabled = true\n${operatorRole()}`,
    /operator\.role "operator" must differ from architect\.role/,
  );
});

test("an allow of bare Bash, Bash(*) or anything but Bash(cstan ...) is refused", () => {
  for (const allow of [
    `["Bash"]`,
    `["Bash(*)"]`,
    `["Bash(git *)"]`,
    `["Bash(cstan *; rm x)"]`,
    `["Read"]`,
    `["Bash(cstan *)", "Bash"]`,
  ])
    rejects(
      `${ENABLED}${operatorRole({ allow })}`,
      /roles\.operator\.allow\[\d+\] must be a Bash\(cstan/,
    );
});

test("a deny list lacking any required entry is refused and names the entry", () => {
  for (const missing of [
    "Write",
    "Edit",
    "NotebookEdit",
    "Agent",
    "Task",
    "Read",
    "Glob",
    "Grep",
  ]) {
    const deny = JSON.stringify(
      (JSON.parse(DENY) as string[]).filter((entry) => entry !== missing),
    );
    rejects(
      `${ENABLED}${operatorRole({ deny })}`,
      new RegExp(`roles\\.operator\\.deny must include ${missing}`),
    );
  }
});

test("timeouts, ranges and unknown keys are validated", () => {
  const withRole = (table: string): string =>
    `${BASE}\n[operator]\n${table}\n${operatorRole()}`;
  const config = load(
    withRole(
      "enabled = true\ntimeout_seconds = 60\nmax_timeout_seconds = 3600\noutput_tail_bytes = 12288",
    ),
  );
  assert.equal(config.operator.maxTimeoutSeconds, 3600);
  assert.equal(config.operator.outputTailBytes, 12288);
  rejects(
    withRole("timeout_seconds = 400\nmax_timeout_seconds = 300"),
    /operator\.timeout_seconds/,
  );
  rejects(
    withRole("max_timeout_seconds = 3601"),
    /operator\.max_timeout_seconds/,
  );
  rejects(withRole("output_tail_bytes = 12289"), /operator\.output_tail_bytes/);
  rejects(
    withRole("max_pending_proposals = 0"),
    /operator\.max_pending_proposals/,
  );
  rejects(withRole("shell = true"), /operator/);
});

test("auto_approve defaults to [] and accepts only allowlisted exact commands", () => {
  assert.deepEqual(load(BASE).operator.autoApprove, []);
  const ok = load(
    `${BASE}\n[operator]\nauto_approve = ["ls -l", "git rev-parse --short HEAD"]\nauto_approve_prefix = ["ls -l"]\n`,
  );
  assert.deepEqual(ok.operator.autoApprove, [
    "ls -l",
    "git rev-parse --short HEAD",
  ]);
  assert.deepEqual(ok.operator.autoApprovePrefix, ["ls -l"]);
});

const BAD_RULES = [
  "git status",
  "git diff",
  "git show HEAD:.env",
  "git log --ext-diff",
  "git status; rm x",
  "ls $(x)",
  "ls > f",
  'sh -c "ls"',
  "FOO=1 ls",
  "git -c core.pager=x rev-parse",
  "git push",
  "rm -rf x",
  "git reset --hard",
  "ls\nrm x",
  "ls\u0000",
  "ls \u001b[31m",
  "git diff --output=/p",
  "git grep -Ocmd",
  "npm run build --script-shell=x",
  "npm test",
  "make",
  "/bin/rm x",
  "./rm x",
  "git clean -xdf",
  "git push --force=1",
  "git push --exec=x",
  "git branch -D x",
  "git branch -d x",
  "git branch d",
  "x --delete",
  "ls  -l",
  " ls",
];

test("a bad auto_approve or auto_approve_prefix entry makes the load throw", () => {
  for (const rule of BAD_RULES)
    for (const key of ["auto_approve", "auto_approve_prefix"])
      rejects(
        `${BASE}\n[operator]\n${key} = [${JSON.stringify(rule)}]\n`,
        new RegExp(`operator\\.${key}\\[0\\]`),
      );
});

test("a rule that looks like a credential is refused", () => {
  rejects(
    `${BASE}\n[operator]\nauto_approve = ["ls sk-ant-api03-abcdefghijklmnopqrstuvwxyz0123456789"]\n`,
    /operator\.auto_approve\[0\]/,
  );
});

test("the starter operator tables load once uncommented", () => {
  const lines = STARTER_CONFIG.split("\n");
  const uncomment = (start: string): string[] => {
    const at = lines.findIndex((line) => line === start);
    assert.notEqual(at, -1, start);
    const out: string[] = [];
    for (let i = at; i < lines.length && lines[i]!.startsWith("# "); i += 1)
      out.push(lines[i]!.slice(2));
    return out;
  };
  const operatorTable = uncomment("# [operator]");
  const roleTable = uncomment("# [roles.operator]");
  assert.ok(operatorTable.length > 2 && roleTable.length > 2);
  const text = `${STARTER_CONFIG}\n${operatorTable.join("\n")}\n${roleTable.join("\n")}\n`;
  const config = load(text);
  assert.equal(config.operator.enabled, true);
  assert.deepEqual(config.operator.autoApprove, [
    "ls -l",
    "git rev-parse --short HEAD",
  ]);
  assert.deepEqual(load(STARTER_CONFIG).operator.enabled, false);
});

test("the session grant and full auto keys have defaults, bounds and an order", () => {
  const config = load(`${ENABLED}${operatorRole()}`);
  assert.equal(config.operator.sessionGrantMaxMinutes, 60);
  assert.equal(config.operator.fullAutoDefaultMinutes, 30);
  assert.equal(config.operator.fullAutoMaxMinutes, 120);
  const custom = load(
    `${ENABLED}session_grant_max_minutes = 480\nfull_auto_default_minutes = 5\nfull_auto_max_minutes = 480\n${operatorRole()}`,
  );
  assert.equal(custom.operator.sessionGrantMaxMinutes, 480);
  assert.equal(custom.operator.fullAutoDefaultMinutes, 5);
  assert.equal(custom.operator.fullAutoMaxMinutes, 480);
  for (const key of [
    "session_grant_max_minutes",
    "full_auto_default_minutes",
    "full_auto_max_minutes",
  ])
    for (const value of ["0", "481", '"x"'])
      rejects(
        `${ENABLED}${key} = ${value}\n${operatorRole()}`,
        new RegExp(`operator\\.${key}`),
      );
  rejects(
    `${ENABLED}full_auto_default_minutes = 90\nfull_auto_max_minutes = 60\n${operatorRole()}`,
    /full_auto_default_minutes \(90\) must not exceed operator\.full_auto_max_minutes \(60\)/,
  );
});
