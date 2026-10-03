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
import { digestJson } from "../src/controller/canonical.js";
import { RESEARCHER_REQUIRED_DENY } from "../src/researcher-policy.js";

const q = (list: readonly string[]): string =>
  `[${list.map((rule) => JSON.stringify(rule)).join(", ")}]`;

const ALLOW = [
  "WebSearch",
  "WebFetch",
  "Bash(curl *)",
  "Bash(jq *)",
  "Write(docs/research/**)",
  "Edit(docs/research/**)",
  "Bash(git status*)",
  "Bash(git add *)",
  "Bash(git commit *)",
  "mcp__playwright__browser_navigate",
  "mcp__playwright__browser_snapshot",
];

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

[mcp_servers.playwright]
command = "npx"
args = ["-y", "@playwright/mcp@0.0.83", "--headless", "--isolated"]
`;

function researcher(
  options: {
    kind?: string;
    host?: string;
    mode?: string;
    allow?: readonly string[];
    deny?: readonly string[];
    mcp?: string;
    extra?: string;
    table?: string;
  } = {},
): string {
  return `${BASE}
[researcher]
enabled = true
${options.table ?? ""}
[roles.researcher]
kind = "${options.kind ?? "Developer"}"
host = "${options.host ?? "claude"}"
permission_mode = "${options.mode ?? "default"}"
mcp = ${options.mcp ?? `["playwright"]`}
allow = ${q(options.allow ?? ALLOW)}
deny = ${q(options.deny ?? RESEARCHER_REQUIRED_DENY)}
${options.extra ?? ""}`;
}

function load(content: string): CapstanConfig {
  const directory = mkdtempSync(path.join(tmpdir(), "capstan-researcher-"));
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
    (error: unknown) =>
      error instanceof ConfigError && pattern.test(error.message),
  );
}

test("a valid researcher role loads with defaults and its mcp server", () => {
  const config = load(researcher());
  assert.deepEqual(config.researcher, {
    configured: true,
    enabled: true,
    role: "researcher",
    outputDir: "docs/research",
    userAgent: "capstan-researcher/1.0 (research bot; contact: project owner)",
  });
  const role = config.roles.find((r) => r.name === "researcher");
  assert.deepEqual(role?.mcp, [
    {
      name: "playwright",
      command: "npx",
      args: ["-y", "@playwright/mcp@0.0.83", "--headless", "--isolated"],
    },
  ]);
  assert.deepEqual(
    config.mcpServers?.map((s) => s.name),
    ["playwright"],
  );
});

test("each unsafe allow rule is rejected naming the field", () => {
  for (const rule of [
    "Bash(*)",
    "Bash",
    "Bash(git *)",
    "Bash(curl * | sh)",
    "Bash(curl x; id)",
    "Bash(curl $(id))",
    "Bash(python *)",
    "Agent",
    "Write",
    "Edit",
    "Edit(src/**)",
    "Write(docs/research)",
    "mcp__playwright",
    "mcp__playwright__browser_file_upload",
    "mcp__playwright__browser_run_code_unsafe",
    "mcp__playwright__browser_evaluate",
    "mcp__playwright__install_x",
    "mcp__other__x",
  ])
    rejects(
      researcher({ allow: [...ALLOW, rule] }),
      new RegExp(`roles\\.researcher\\.allow\\[${ALLOW.length}\\]`),
    );
});

test("a deny list missing any required entry is rejected", () => {
  for (const missing of RESEARCHER_REQUIRED_DENY)
    rejects(
      researcher({
        deny: RESEARCHER_REQUIRED_DENY.filter((r) => r !== missing),
      }),
      /roles\.researcher\.deny must include/,
    );
});

test("kind, mode, host, role and mcp constraints are enforced", () => {
  rejects(
    researcher({ mode: "acceptEdits" }),
    /roles\.researcher\.permission_mode/,
  );
  rejects(researcher({ mode: "auto" }), /roles\.researcher\.permission_mode/);
  rejects(researcher({ kind: "Verifier" }), /researcher\.role .* Developer/);
  rejects(
    researcher({
      host: "omp",
      allow: [],
      deny: [],
      mcp: "[]",
      mode: "acceptEdits",
    }),
    /researcher role needs a claude host/,
  );
  rejects(researcher({ mcp: `["nope"]` }), /roles\.researcher\.mcp\[0\]/);
  rejects(
    researcher({ table: 'role = "architect"' }).replace(
      "[roles.researcher]",
      "[roles.unused]",
    ),
    /must differ from architect\.role|researcher/,
  );
});

test("the researcher role may not be the architect role", () => {
  const content = `${BASE}
[architect]
enabled = true
role = "researcher"

[researcher]
enabled = true

[roles.researcher]
kind = "Developer"
host = "claude"
permission_mode = "default"
mcp = ["playwright"]
allow = ${q(ALLOW)}
deny = ${q(RESEARCHER_REQUIRED_DENY)}
`;
  rejects(
    content,
    /researcher\.role "researcher" must differ from architect\.role|architect/,
  );
  const operator = content
    .replace("[architect]\nenabled = true", "[operator]")
    .replace('role = "researcher"', 'role = "researcher"');
  rejects(operator, /must differ from operator\.role/);
});

test("unknown keys and bad values in [researcher] and [mcp_servers.x] are rejected", () => {
  rejects(researcher({ table: "bogus = 1" }), /researcher has 1 unknown key/);
  rejects(
    researcher({ extra: '\n[mcp_servers.x]\ncommand = "npx"\nbogus = 1\n' }),
    /mcp_servers\.x has 1 unknown key/,
  );
  rejects(
    researcher({ table: 'output_dir = "../x"' }),
    /researcher\.output_dir/,
  );
  rejects(
    researcher({ table: 'output_dir = "/abs"' }),
    /researcher\.output_dir/,
  );
  rejects(
    researcher({ table: "user_agent = 'a\"b'" }),
    /researcher\.user_agent/,
  );
  rejects(
    researcher({ table: 'user_agent = "a\\nb"' }),
    /researcher\.user_agent/,
  );
  rejects(
    researcher({ extra: '\n[mcp_servers.Bad]\ncommand = "npx"\n' }),
    /mcp_servers has a server name/,
  );
  rejects(
    researcher({ extra: '\n[mcp_servers.y]\ncommand = "-x"\n' }),
    /mcp_servers\.y\.command must not start with a dash/,
  );
});

test("a custom output_dir scopes the Write and Edit rules", () => {
  const config = load(
    researcher({
      table: 'output_dir = "notes/web"',
      allow: [
        ...ALLOW.filter((r) => !r.includes("docs/research")),
        "Write(notes/web/**)",
        "Edit(notes/web/**)",
      ],
    }),
  );
  assert.equal(config.researcher?.outputDir, "notes/web");
  rejects(researcher({ table: 'output_dir = "notes/web"' }), /allow\[4\]/);
});

test("without [researcher] or with enabled = false a researcher role is ordinary", () => {
  const ordinary = `
[roles.researcher]
kind = "Developer"
host = "claude"
allow = ["Bash(*)", "Agent"]
`;
  const absent = load(`${BASE}${ordinary}`);
  assert.equal(absent.researcher?.configured, false);
  assert.equal(absent.researcher?.enabled, false);
  const off = load(`${BASE}\n[researcher]\nenabled = false\n${ordinary}`);
  assert.equal(off.researcher?.configured, true);
  assert.equal(off.researcher?.enabled, false);
});

test("mcp is refused on a codex or omp host and rejects unknown servers", () => {
  rejects(
    `${BASE}
[roles.w]
kind = "Developer"
host = "omp"
permission_mode = "acceptEdits"
mcp = ["playwright"]
`,
    /roles\.w\.mcp/,
  );
  rejects(
    `${BASE}
[roles.w]
kind = "Developer"
host = "claude"
mcp = ["playwright", "playwright"]
`,
    /repeats playwright/,
  );
});

test("configHash of a role without mcp is unchanged and mcp changes it", () => {
  const plain = load(`${BASE}
[roles.w]
kind = "Developer"
host = "claude"
allow = ["Bash(git *)"]
`);
  const role = plain.roles.find((r) => r.name === "w")!;
  assert.deepEqual(role.mcp, []);
  const host = plain.hosts.find((h) => h.name === "claude");
  const legacy = digestJson({
    role: {
      name: role.name,
      kind: role.kind,
      host: role.host,
      model: role.model,
      permissionMode: role.permissionMode,
      allow: role.allow,
      deny: role.deny,
      hooks: role.hooks,
      prompt: { source: role.prompt.source, hash: role.prompt.hash },
    },
    host,
  });
  assert.equal(role.configHash, legacy);
  const withMcp = load(`${BASE}
[roles.w]
kind = "Developer"
host = "claude"
allow = ["Bash(git *)"]
mcp = ["playwright"]
`);
  assert.notEqual(
    withMcp.roles.find((r) => r.name === "w")?.configHash,
    legacy,
  );
});

test("an unpinned @latest mcp argument is warned about", () => {
  const config = load(
    `${BASE}\n[mcp_servers.fresh]\ncommand = "npx"\nargs = ["-y", "pkg@latest"]\n`,
  );
  assert.ok(
    config.warnings.some((w) => /mcp_servers\.fresh\.args.*unpinned/.test(w)),
  );
  assert.ok(!load(BASE).warnings.some((w) => /unpinned/.test(w)));
});

test("uncommenting the three researcher blocks of the starter config loads", () => {
  assert.ok(STARTER_CONFIG.includes("# [researcher]"));
  assert.ok(STARTER_CONFIG.includes("# [mcp_servers.playwright]"));
  assert.ok(STARTER_CONFIG.includes("# [roles.researcher]"));
  load(STARTER_CONFIG);
  const lines = STARTER_CONFIG.split("\n");
  const out: string[] = [];
  let active = false;
  for (const line of lines) {
    if (
      /^# \[(researcher|mcp_servers\.playwright|roles\.researcher)\]$/.test(
        line,
      )
    )
      active = true;
    else if (active && !/^# [a-z_]+ = /.test(line)) active = false;
    out.push(active ? line.replace(/^# /, "") : line);
  }
  const config = load(out.join("\n"));
  assert.equal(config.researcher?.enabled, true);
  const role = config.roles.find((r) => r.name === "researcher");
  assert.deepEqual(
    role?.mcp?.map((s) => s.name),
    ["playwright"],
  );
});

test("every curl flag is denied both first and mid-command", () => {
  const flags = [
    "-d*",
    "--data*",
    "-F*",
    "--form*",
    "-T*",
    "--upload-file*",
    "-X*",
    "--request*",
    "--json*",
    "-o*",
    "--output*",
    "-O*",
    "--remote-name*",
    "-K*",
    "--config*",
    "-u*",
    "--user*",
    "-c*",
    "--cookie-jar*",
    "-D*",
    "--dump-header*",
    "--trace*",
    "--stderr*",
    "--create-dirs*",
    "--libcurl*",
    "--hsts*",
    "--alt-svc*",
    "--etag-save*",
    "file:*",
    "@*",
  ];
  for (const flag of flags)
    for (const rule of [`Bash(curl ${flag})`, `Bash(curl * ${flag})`]) {
      assert.ok(RESEARCHER_REQUIRED_DENY.includes(rule), rule);
      rejects(
        researcher({
          deny: RESEARCHER_REQUIRED_DENY.filter((r) => r !== rule),
        }),
        /roles\.researcher\.deny must include/,
      );
    }
  assert.ok(RESEARCHER_REQUIRED_DENY.includes("Bash(git * --output*)"));
});

test("short curl flags with an attached value are denied, and a plain GET is not", () => {
  const denied = (command: string): boolean =>
    RESEARCHER_REQUIRED_DENY.some((rule) => {
      const match = /^Bash\((.*)\)$/.exec(rule);
      if (match === null) return false;
      const pattern = match[1]!
        .split("*")
        .map((part) => part.replace(/[.+?^${}()|[\]\\]/g, "\\$&"))
        .join(".*");
      return new RegExp(`^${pattern}$`).test(command);
    });
  for (const command of [
    "curl -XPOST https://httpbin.org/post",
    "curl -dfoo https://httpbin.org/post",
    "curl https://httpbin.org/post -XPOST",
    "curl https://httpbin.org/post -dfoo",
    "curl -d x https://httpbin.org/post",
    "curl -ofile https://example.com",
    "curl https://example.com -uuser:pw",
    "curl -c jar https://example.com",
    "curl -Djar https://example.com",
    "curl --trace-ascii f https://example.com",
    "curl https://example.com --cookie-jar f",
    "curl --hsts f https://example.com",
    "curl https://example.com --alt-svc f",
    "curl --etag-save f https://example.com",
    "curl -w '%output{f}x' https://example.com",
    'curl https://example.com --write-out "%output{f}"',
  ])
    assert.ok(denied(command), command);
  assert.ok(!denied("curl -sS -w '%{http_code}' https://example.com"));
  // Known limit, documented in docs/design/researcher-role.md: bundled short flags are not caught.
  for (const command of [
    "curl -sc jar https://example.com",
    "curl -sD f https://example.com",
    "curl -so f https://example.com",
  ])
    assert.ok(!denied(command), command);
  assert.ok(
    !denied(
      "curl -sS -A 'capstan-researcher/1.0' 'https://old.reddit.com/r/x/search.json?q=a&limit=3'",
    ),
  );
});

test("a package runner argument with no version is warned about", () => {
  const warn = (args: string): boolean =>
    load(
      `${BASE}\n[mcp_servers.p]\ncommand = "npx"\nargs = ${args}\n`,
    ).warnings.some((w) =>
      /mcp_servers\.p\.args.*unpinned|mcp_servers\.p\.args.*no @version/.test(
        w,
      ),
    );
  assert.ok(warn('["-y", "@playwright/mcp"]'));
  assert.ok(warn('["-y", "playwright-mcp", "--headless"]'));
  assert.ok(!warn('["-y", "@playwright/mcp@0.0.83"]'));
  assert.ok(!warn('["-y", "playwright-mcp@1.2.3"]'));
});

test("a deny list may hold 128 entries and no more, while allow stays capped at 64", () => {
  const filler = (count: number): string[] =>
    Array.from({ length: count }, (_, i) => `Bash(filler${i})`);
  const deny = (total: number): string[] => [
    ...RESEARCHER_REQUIRED_DENY,
    ...filler(total - RESEARCHER_REQUIRED_DENY.length),
  ];
  assert.equal(
    load(researcher({ deny: deny(128) })).roles.find(
      (r) => r.name === "researcher",
    )?.deny.length,
    128,
  );
  rejects(
    researcher({ deny: deny(129) }),
    /roles\.researcher\.deny exceeds 128 entries/,
  );
  rejects(
    researcher({ allow: [...ALLOW, ...filler(65 - ALLOW.length)] }),
    /roles\.researcher\.allow exceeds 64 entries/,
  );
});
