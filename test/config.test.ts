import assert from "node:assert/strict";
import {
  chmodSync,
  mkdirSync,
  mkdtempSync,
  rmSync,
  symlinkSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import { test } from "node:test";
import {
  CONFIG_FILE_NAME,
  ConfigError,
  DEFAULT_HERDR_SESSION,
  DEFAULT_WAIT_TIMEOUT_SECONDS,
  MAX_WAIT_TIMEOUT_SECONDS,
  STARTER_CONFIG,
  loadCapstanConfig,
} from "../src/config/capstan-config.js";

const VALID = `schema_version = 1

[hosts.claude]
kind = "claude"

[roles.pm]
kind = "PM"
host = "claude"

[roles.reviewer]
kind = "Verifier"
host = "claude"
`;

function projectDirectory(): string {
  return mkdtempSync(path.join(tmpdir(), "capstan-config-test-"));
}

function write(
  directory: string,
  content: string | Buffer,
  mode = 0o600,
): void {
  const file = path.join(directory, CONFIG_FILE_NAME);
  writeFileSync(file, content, { mode });
  chmodSync(file, mode);
}

function withConfig<T>(content: string | Buffer, run: (dir: string) => T): T {
  const directory = projectDirectory();
  try {
    write(directory, content);
    return run(directory);
  } finally {
    rmSync(directory, { recursive: true, force: true });
  }
}

function assertRejected(
  content: string | Buffer,
  pattern: RegExp,
  leaked?: string,
): void {
  withConfig(content, (directory) => {
    assert.throws(
      () => loadCapstanConfig(directory),
      (error: unknown) => {
        assert.ok(error instanceof ConfigError, String(error));
        assert.match(error.message, pattern);
        if (leaked !== undefined)
          assert.ok(
            !error.message.includes(leaked),
            `error text leaked: ${error.message}`,
          );
        return true;
      },
    );
  });
}

test("a valid configuration resolves every default", () => {
  withConfig(VALID, (directory) => {
    const config = loadCapstanConfig(directory);
    assert.deepEqual(config.timers, {
      maxDeferralSeconds: 120,
      pmAckTimeoutSeconds: 600,
      pmNotifyAfterSeconds: 300,
      notifyIntervalSeconds: 600,
      stallAfterSeconds: 900,
      workerAckTimeoutSeconds: 600,
    });
    assert.deepEqual(config.hosts, [
      {
        name: "claude",
        kind: "claude",
        command: "claude",
        shellCommandTimeoutSeconds: 120,
        waitTimeoutSeconds: 90,
      },
    ]);
    assert.deepEqual(
      config.roles.map((role) => [
        role.name,
        role.kind,
        role.host,
        role.permissionMode,
        role.hooks,
        role.prompt.source,
      ]),
      [
        ["pm", "PM", "claude", "default", "off", "none"],
        ["reviewer", "Verifier", "claude", "default", "off", "none"],
      ],
    );
    for (const role of config.roles)
      assert.match(role.configHash, /^[0-9a-f]{64}$/);
    assert.equal(config.herdrSession, DEFAULT_HERDR_SESSION);
    assert.deepEqual(config.notifications, { herdr: true, fallback: true });
    assert.equal(DEFAULT_WAIT_TIMEOUT_SECONDS, 90);
    assert.equal(MAX_WAIT_TIMEOUT_SECONDS, 3600);
  });
});

test("herdr_session and the notification channels are read, and validated", () => {
  withConfig(
    `schema_version = 1\nherdr_session = "capstan-work"\n\n[notifications]\nherdr = false\nfallback = true\n\n${VALID.replace("schema_version = 1\n\n", "")}`,
    (directory) => {
      const config = loadCapstanConfig(directory);
      assert.equal(config.herdrSession, "capstan-work");
      assert.deepEqual(config.notifications, { herdr: false, fallback: true });
    },
  );
  withConfig(`${VALID}\n[notifications]\nfallback = false\n`, (directory) =>
    assert.deepEqual(loadCapstanConfig(directory).notifications, {
      herdr: true,
      fallback: false,
    }),
  );
});

test("the starter configuration written by init is valid and gives the PM workers to delegate to", () => {
  withConfig(STARTER_CONFIG, (directory) => {
    const config = loadCapstanConfig(directory);
    assert.deepEqual(
      config.roles.map((role) => [role.name, role.kind]),
      [
        ["pm", "PM"],
        ["developer", "Developer"],
        ["designer", "Developer"],
        ["reviewer", "Verifier"],
        ["tester", "Verifier"],
      ],
    );
    assert.equal(config.limits.maxWorkers, 3);
    assert.equal(config.layout.spawn, "pane");
    assert.equal(config.layout.split, "auto");
    for (const role of config.roles.filter(
      (r) => r.kind !== "PM" && r.name !== "reviewer",
    )) {
      assert.match(role.promptText ?? "", /branch/, role.name);
      assert.equal(role.permissionMode, "acceptEdits");
      assert.deepEqual(role.deny, ["Bash(git push)", "Bash(git push *)"]);
    }
    const reviewer = config.roles.find((r) => r.name === "reviewer")!;
    assert.match(reviewer.promptText ?? "", /do not edit any file/);
    for (const tool of [
      "Write",
      "Edit",
      "NotebookEdit",
      "Agent",
      "Task",
      "Bash(git push)",
    ])
      assert.ok(reviewer.deny.includes(tool), tool);
  });
});

test("limits.max_workers defaults to 3, accepts 1 to 16 and refuses anything else", () => {
  withConfig(VALID, (directory) =>
    assert.equal(loadCapstanConfig(directory).limits.maxWorkers, 3),
  );
  withConfig(`${VALID}\n[limits]\n`, (directory) =>
    assert.equal(loadCapstanConfig(directory).limits.maxWorkers, 3),
  );
  for (const value of ["1", "16"])
    withConfig(`${VALID}\n[limits]\nmax_workers = ${value}\n`, (directory) =>
      assert.equal(
        loadCapstanConfig(directory).limits.maxWorkers,
        Number(value),
      ),
    );
  for (const value of ["0", "17", "-1", "3.5", "3.0", "true", '"3"'])
    assertRejected(
      `${VALID}\n[limits]\nmax_workers = ${value}\n`,
      /limits\.max_workers/,
    );
  assertRejected(`${VALID}\n[limits]\nmax_worker = 2\n`, /limits/);
});

test("env.pass lists the variable names to pass to agents and refuses anything that is not a plain name", () => {
  const load = (text: string) =>
    withConfig(text, (directory) => loadCapstanConfig(directory).env.pass);
  assert.deepEqual(load(VALID), []);
  assert.deepEqual(load(`${VALID}\n[env]\n`), []);
  assert.deepEqual(
    load(`${VALID}\n[env]\npass = ["NEXORA_API_KEY", "_X1", "A"]\n`),
    ["NEXORA_API_KEY", "_X1", "A"],
  );
  assert.deepEqual(load(`${VALID}\n[env]\npass = []\n`), []);
  for (const bad of [
    'pass = ["lower"]',
    'pass = ["1ABC"]',
    'pass = ["A-B"]',
    'pass = ["A B"]',
    'pass = [""]',
    'pass = ["A".repeat(65)]'.replace('"A".repeat(65)', `"${"A".repeat(65)}"`),
    "pass = [1]",
    'pass = "NEXORA_API_KEY"',
  ])
    assertRejected(`${VALID}\n[env]\n${bad}\n`, /env\.pass/);
  assertRejected(`${VALID}\n[env]\npass = ["CAPSTAN_TOKEN"]\n`, /CAPSTAN_/);
  for (const name of [
    "PATH",
    "HOME",
    "TERM",
    "IFS",
    "PS1",
    "PROMPT_COMMAND",
    "BASH_ENV",
    "SHELLOPTS",
    "LD_PRELOAD",
    "NODE_OPTIONS",
    "ZDOTDIR",
    "SHELL",
    "LD_DEBUG",
    "DYLD_INSERT_LIBRARIES",
    "GIT_SSH_COMMAND",
    "GIT_DIR",
    "BASH_FUNC_X",
  ])
    assertRejected(
      `${VALID}\n[env]\npass = ["${name}"]\n`,
      new RegExp(`must not be ${name}`),
    );
  assertRejected(`${VALID}\n[env]\npass = ["A", "B", "A"]\n`, /repeats A/);
  assertRejected(
    `${VALID}\n[env]\npass = [${Array.from({ length: 33 }, (_, i) => `"V${i}"`).join(", ")}]\n`,
    /exceeds 32/,
  );
  assertRejected(`${VALID}\n[env]\nextra = 1\n`, /env/);
});

test("the layout table defaults to tabs, accepts the documented values and refuses everything else", () => {
  const load = (text: string) =>
    withConfig(text, (directory) => loadCapstanConfig(directory).layout);
  const defaults = {
    spawn: "tab",
    split: "auto",
    minPaneColumns: 60,
    minPaneRows: 12,
  };
  assert.deepEqual(load(VALID), defaults);
  assert.deepEqual(load(`${VALID}\n[layout]\n`), defaults);
  assert.deepEqual(
    load(
      `${VALID}\n[layout]\nspawn = "pane"\nsplit = "down"\nmin_pane_columns = 80\nmin_pane_rows = 20\n`,
    ),
    { spawn: "pane", split: "down", minPaneColumns: 80, minPaneRows: 20 },
  );
  for (const bad of [
    'spawn = "Pane"',
    'spawn = " pane"',
    'spawn = ""',
    "spawn = 1",
    'spawn = "window"',
    'split = "vertical"',
    'split = "Right"',
    "split = true",
    "min_pane_columns = 0",
    "min_pane_columns = 501",
    "min_pane_columns = 60.0",
    'min_pane_columns = "60"',
    "min_pane_rows = 0",
    "min_pane_rows = 201",
    "min_pane_rows = -1",
    "min_pane_rows = true",
    "size = 3",
  ])
    assertRejected(`${VALID}\n[layout]\n${bad}\n`, /layout/, undefined);
});

test("a PM without a deny list denies the file-editing and subagent tools, and an explicit deny list replaces that", () => {
  const pmDeny = (text: string) =>
    withConfig(text, (directory) =>
      loadCapstanConfig(directory).roles.find((r) => r.kind === "PM")!,
    );
  const byDefault = pmDeny(VALID);
  assert.deepEqual(byDefault.deny, [
    "Write",
    "Edit",
    "NotebookEdit",
    "Agent",
    "Task",
  ]);
  const empty = pmDeny(VALID.replace('kind = "PM"', 'kind = "PM"\ndeny = []'));
  assert.deepEqual(empty.deny, []);
  const custom = pmDeny(
    VALID.replace('kind = "PM"', 'kind = "PM"\ndeny = ["Agent"]'),
  );
  assert.deepEqual(custom.deny, ["Agent"]);
  assert.equal(
    new Set([byDefault.configHash, empty.configHash, custom.configHash]).size,
    3,
    "the hash covers the effective deny list",
  );
  const verifier = withConfig(VALID, (directory) =>
    loadCapstanConfig(directory).roles.find((r) => r.kind === "Verifier")!,
  );
  assert.deepEqual(
    verifier.deny,
    [],
    "only the PM gets a default deny list: a tester that writes tests must not lose its tools",
  );
});

test("a role hash changes with the role and with its host block", () => {
  const hashes = (text: string) =>
    withConfig(text, (directory) =>
      Object.fromEntries(
        loadCapstanConfig(directory).roles.map((role) => [
          role.name,
          role.configHash,
        ]),
      ),
    );
  const base = hashes(VALID);
  const modelChanged = hashes(
    VALID.replace(
      '[roles.reviewer]\nkind = "Verifier"',
      '[roles.reviewer]\nkind = "Verifier"\nmodel = "opus"',
    ),
  );
  assert.equal(modelChanged.pm, base.pm);
  assert.notEqual(modelChanged.reviewer, base.reviewer);
  const hostChanged = hashes(
    VALID.replace(
      'kind = "claude"',
      'kind = "claude"\nwait_timeout_seconds = 30',
    ),
  );
  assert.notEqual(hostChanged.pm, base.pm);
  assert.notEqual(hostChanged.reviewer, base.reviewer);
});

const rejections: ReadonlyArray<[string, string, RegExp]> = [
  [
    "an invalid herdr_session",
    VALID.replace(
      "schema_version = 1\n",
      'schema_version = 1\nherdr_session = "-bad name"\n',
    ),
    /herdr_session must match/,
  ],
  [
    "a non-string herdr_session",
    VALID.replace(
      "schema_version = 1\n",
      "schema_version = 1\nherdr_session = 5\n",
    ),
    /herdr_session/,
  ],
  [
    "both notification channels off",
    `${VALID}\n[notifications]\nherdr = false\nfallback = false\n`,
    /must not both be false/,
  ],
  [
    "a non-boolean notification channel",
    `${VALID}\n[notifications]\nherdr = "yes"\n`,
    /notifications\.herdr must be true or false/,
  ],
  [
    "an unknown notification key",
    `${VALID}\n[notifications]\nemail = true\n`,
    /notifications has 1 unknown key/,
  ],
  [
    "a wrong schema version",
    VALID.replace("schema_version = 1", "schema_version = 2"),
    /schema_version/,
  ],
  [
    "an unknown top-level key",
    VALID.replace("schema_version = 1\n", "schema_version = 1\nextra = 1\n"),
    /top level has 1 unknown key/,
  ],
  [
    "extra_args, which this version does not accept",
    VALID.replace(
      'host = "claude"\n\n[roles.reviewer]',
      'host = "claude"\nextra_args = ["--yolo"]\n\n[roles.reviewer]',
    ),
    /roles\.pm has 1 unknown key/,
  ],
  [
    "a second PM role",
    `${VALID}\n[roles.lead]\nkind = "PM"\nhost = "claude"\n`,
    /exactly one role must have kind PM/,
  ],
  [
    "no PM role",
    VALID.replace('kind = "PM"', 'kind = "Developer"'),
    /exactly one role must have kind PM/,
  ],
  [
    "an unknown host",
    VALID.replace(
      '[roles.reviewer]\nkind = "Verifier"\nhost = "claude"',
      '[roles.reviewer]\nkind = "Verifier"\nhost = "nowhere"',
    ),
    /roles\.reviewer\.host does not name a configured host/,
  ],
  [
    "an unknown role kind",
    VALID.replace('kind = "Verifier"', 'kind = "Reviewer"'),
    /roles\.reviewer\.kind must be one of/,
  ],
  [
    "an unknown host kind",
    VALID.replace('kind = "claude"', 'kind = "gemini"'),
    /hosts\.claude\.kind must be one of/,
  ],
  [
    "bypassPermissions",
    VALID.replace(
      'kind = "Verifier"',
      'kind = "Verifier"\npermission_mode = "bypassPermissions"',
    ),
    /permission_mode must be one of default, acceptEdits, plan, auto/,
  ],
  [
    "an uppercase role name",
    `${VALID}\n[roles.Lead]\nkind = "Developer"\nhost = "claude"\n`,
    /roles has a role name that does not match/,
  ],
  [
    "a wait timeout that is not below the shell timeout, naming both values",
    VALID.replace(
      'kind = "claude"',
      'kind = "claude"\nshell_command_timeout_seconds = 60\nwait_timeout_seconds = 60',
    ),
    /wait_timeout_seconds \(60\) must be below hosts\.claude\.shell_command_timeout_seconds \(60\)/,
  ],
  [
    "a float where an integer is required",
    VALID.replace(
      "schema_version = 1\n",
      "schema_version = 1\n[timers]\nstall_after_seconds = 1.5\n\n",
    ),
    /timers\.stall_after_seconds must be an integer/,
  ],
  [
    "a whole-number float",
    VALID.replace(
      "schema_version = 1\n",
      "schema_version = 1\n[timers]\nmax_deferral_seconds = 2.0\n\n",
    ),
    /timers\.max_deferral_seconds must be an integer/,
  ],
  [
    "an out-of-range timer",
    VALID.replace(
      "schema_version = 1\n",
      "schema_version = 1\n[timers]\nmax_deferral_seconds = 0\n\n",
    ),
    /timers\.max_deferral_seconds must be between 1 and 3600/,
  ],
  [
    "no hosts",
    'schema_version = 1\n[roles.pm]\nkind = "PM"\nhost = "claude"\n',
    /hosts must be a table/,
  ],
  [
    "both prompt and prompt_file",
    VALID.replace(
      'kind = "Verifier"',
      'kind = "Verifier"\nprompt = "a"\nprompt_file = "p.md"',
    ),
    /sets both prompt and prompt_file/,
  ],
  [
    "an absolute prompt_file",
    VALID.replace(
      'kind = "Verifier"',
      'kind = "Verifier"\nprompt_file = "/etc/hostname"',
    ),
    /prompt_file must be relative/,
  ],
  [
    "a prompt_file that escapes the project",
    VALID.replace(
      'kind = "Verifier"',
      'kind = "Verifier"\nprompt_file = "../outside.md"',
    ),
    /prompt_file (does not exist|must stay inside)/,
  ],
  [
    "a control character in a string",
    VALID.replace(
      'kind = "Verifier"',
      'kind = "Verifier"\nmodel = "a\\u0001b"',
    ),
    /model contains control, format or line-separator characters/,
  ],
  [
    "too many allow entries",
    VALID.replace(
      'kind = "Verifier"',
      `kind = "Verifier"\nallow = [${Array.from({ length: 65 }, (_, i) => `"Bash(cmd${i})"`).join(", ")}]`,
    ),
    /allow exceeds 64 entries/,
  ],
  [
    "a dash-leading allow entry",
    VALID.replace(
      'kind = "Verifier"',
      'kind = "Verifier"\nallow = ["--dangerously-skip-permissions"]',
    ),
    /allow\[0\] must not start with a dash/,
  ],
  [
    "a dash-leading deny entry",
    VALID.replace('kind = "Verifier"', 'kind = "Verifier"\ndeny = ["-x"]'),
    /deny\[0\] must not start with a dash/,
  ],
  [
    "a dash-leading model",
    VALID.replace(
      'kind = "Verifier"',
      'kind = "Verifier"\nmodel = "--settings"',
    ),
    /model must not start with a dash/,
  ],
];
for (const [name, content, pattern] of rejections)
  test(`the loader rejects ${name}`, () => assertRejected(content, pattern));

test("a prompt file holding a credential shape is rejected and a path outside the project is refused", () => {
  const directory = projectDirectory();
  const outside = path.join(
    path.dirname(directory),
    `${path.basename(directory)}-outside.md`,
  );
  try {
    writeFileSync(
      path.join(directory, "leaky.md"),
      "token sk-live-ABCDEFGHIJKLMNOP1234\n",
    );
    write(
      directory,
      VALID.replace(
        'kind = "Verifier"',
        'kind = "Verifier"\nprompt_file = "leaky.md"',
      ),
    );
    assert.throws(
      () => loadCapstanConfig(directory),
      (error: unknown) =>
        error instanceof ConfigError &&
        /prompt_file looks like a credential/.test(error.message) &&
        !error.message.includes("ABCDEFGH"),
    );
    writeFileSync(outside, "exists but outside");
    write(
      directory,
      VALID.replace(
        'kind = "Verifier"',
        `kind = "Verifier"\nprompt_file = "../${path.basename(outside)}"`,
      ),
    );
    assert.throws(
      () => loadCapstanConfig(directory),
      /prompt_file must stay inside the project/,
    );
  } finally {
    rmSync(directory, { recursive: true, force: true });
    rmSync(outside, { force: true });
  }
});

test("a dangling capstan.toml symlink is rejected as not a regular file", () => {
  const directory = projectDirectory();
  try {
    symlinkSync("missing.toml", path.join(directory, CONFIG_FILE_NAME));
    assert.throws(() => loadCapstanConfig(directory), /must be a regular file/);
  } finally {
    rmSync(directory, { recursive: true, force: true });
  }
});

test("the loader rejects an oversize file, invalid UTF-8 and a missing file", () => {
  assertRejected(`${VALID}\n# ${"x".repeat(70_000)}\n`, /exceeds 65536 bytes/);
  assertRejected(Buffer.from([0x73, 0xff, 0xfe]), /not valid UTF-8/);
  const directory = projectDirectory();
  try {
    assert.throws(
      () => loadCapstanConfig(directory),
      /capstan\.toml does not exist/,
    );
  } finally {
    rmSync(directory, { recursive: true, force: true });
  }
});

test("CRLF and LF files give the same prompt hash and multi-line prompts load", () => {
  const withPrompt = (eol: string): string =>
    VALID.replace(
      'kind = "Verifier"',
      'kind = "Verifier"\nprompt = """\nLine one\n\tindented\nLine two"""',
    ).replaceAll("\n", eol);
  const hashOf = (text: string): string | null =>
    withConfig(text, (directory) => {
      const reviewer = loadCapstanConfig(directory).roles.find(
        (role) => role.name === "reviewer",
      )!;
      return reviewer.prompt.hash;
    });
  const lf = hashOf(withPrompt("\n"));
  assert.match(lf ?? "", /^[0-9a-f]{64}$/);
  assert.equal(hashOf(withPrompt("\r\n")), lf);

  const directory = projectDirectory();
  try {
    writeFileSync(path.join(directory, "lf.md"), "One\nTwo\n");
    writeFileSync(path.join(directory, "crlf.md"), "﻿One\r\nTwo\r\n");
    const fileHash = (name: string): string | null => {
      write(
        directory,
        VALID.replace(
          'kind = "Verifier"',
          `kind = "Verifier"\nprompt_file = "${name}"`,
        ),
      );
      return loadCapstanConfig(directory).roles.find(
        (role) => role.name === "reviewer",
      )!.prompt.hash;
    };
    assert.equal(fileHash("crlf.md"), fileHash("lf.md"));
  } finally {
    rmSync(directory, { recursive: true, force: true });
  }
});

const unsafeValues: ReadonlyArray<[string, string]> = [
  ["a newline escape in model", 'model = "a\\nb"'],
  ["a tab escape in model", 'model = "a\\tb"'],
  ["a C1 control in model", 'model = "a\\u0085b"'],
  ["a bidi override in model", 'model = "a\\u202Eb"'],
  ["a zero-width space in model", 'model = "a\\u200Bb"'],
  ["a line separator in allow", 'allow = ["a\\u2028b"]'],
  ["a carriage-return escape in model", 'model = "a\\r\\nb"'],
  ["a whitespace-only allow entry", 'allow = ["   "]'],
  ["a whitespace-only prompt", 'prompt = "  \\n "'],
  ["an escape sequence in a prompt", 'prompt = "a\\u001bb"'],
  ["a bidi override in a prompt", 'prompt = "a\\u202Eb"'],
];
for (const [name, line] of unsafeValues)
  test(`the loader rejects ${name}`, () =>
    assertRejected(
      VALID.replace('kind = "Verifier"', `kind = "Verifier"\n${line}`),
      /(contains control, format or line-separator characters|must be a non-empty string)/,
    ));

test("a prompt may contain joiners but single-line fields may not, and single-line values must be trimmed", () => {
  withConfig(
    VALID.replace(
      'kind = "Verifier"',
      'kind = "Verifier"\nprompt = "a\\u200d\\u200cb"',
    ),
    (directory) => {
      assert.equal(
        loadCapstanConfig(directory).roles.find(
          (role) => role.name === "reviewer",
        )!.prompt.source,
        "inline",
      );
    },
  );
  assertRejected(
    VALID.replace(
      'kind = "Verifier"',
      'kind = "Verifier"\nmodel = "a\\u200db"',
    ),
    /model contains control, format or line-separator characters/,
  );
  assertRejected(
    VALID.replace('kind = "Verifier"', 'kind = "Verifier"\nmodel = " opus"'),
    /model must not have leading or trailing whitespace/,
  );
  assertRejected(
    VALID.replace(
      'kind = "Verifier"',
      'kind = "Verifier"\nallow = ["Bash(ls) "]',
    ),
    /allow\[0\] must not have leading or trailing whitespace/,
  );
});

test("a file with a byte-order mark and CRLF endings loads", () => {
  withConfig(
    Buffer.from(`\uFEFF${VALID.replaceAll("\n", "\r\n")}`),
    (directory) => {
      assert.equal(loadCapstanConfig(directory).roles.length, 2);
    },
  );
});

for (const [name, command] of [
  ["a leading dash", "-rf"],
  ["a bare dot", "."],
  ["a double dot", ".."],
  ["a space", "my tool"],
])
  test(`the loader rejects a host command with ${name}`, () =>
    assertRejected(
      VALID.replace(
        'kind = "claude"',
        `kind = "claude"\ncommand = "${command}"`,
      ),
      /command must be an executable name or path/,
    ));

test("the loader accepts a relative or absolute host command path", () => {
  for (const command of ["./bin/claude", "/usr/local/bin/claude", "claude-2"])
    withConfig(
      VALID.replace(
        'kind = "claude"',
        `kind = "claude"\ncommand = "${command}"`,
      ),
      (directory) => {
        assert.equal(loadCapstanConfig(directory).hosts[0]!.command, command);
      },
    );
});

test("Bearer in prose is accepted but a bearer token is not", () => {
  const withPrompt = (text: string): string =>
    VALID.replace(
      'kind = "Verifier"',
      `kind = "Verifier"\nprompt = """\n${text}"""`,
    );
  withConfig(
    withPrompt(
      "Use Bearer authentication for the API.\nBearer\nauthentication follows.",
    ),
    (directory) => {
      assert.equal(loadCapstanConfig(directory).roles.length, 2);
    },
  );
  assertRejected(
    withPrompt("Authorization: Bearer abcdefgh12345678"),
    /prompt looks like a credential/,
    "abcdefgh12",
  );
});

test("role and host names that look like credentials are rejected without echo", () => {
  assertRejected(
    `${VALID}\n[roles.sk-live-abcdefghijkl]\nkind = "Developer"\nhost = "claude"\n`,
    /roles role name looks like a credential/,
    "abcdefghij",
  );
  assertRejected(
    VALID.replace("[hosts.claude]", "[hosts.sk-live-abcdefghijkl]").replace(
      'host = "claude"',
      'host = "x"',
    ),
    /hosts host name looks like a credential/,
    "abcdefghij",
  );
});

for (const [name, command] of [
  ["a bare slash", "/"],
  ["a trailing slash", "bin/"],
  ["a parent-directory tail", "/usr/bin/.."],
  ["a current-directory tail", "bin/."],
])
  test(`the loader rejects a host command that is ${name}`, () =>
    assertRejected(
      VALID.replace(
        'kind = "claude"',
        `kind = "claude"\ncommand = "${command}"`,
      ),
      /command must be an executable name or path/,
    ));

test("a project name that looks like a credential is rejected", () => {
  assertRejected(
    VALID.replace(
      "schema_version = 1\n",
      'schema_version = 1\n[project]\nname = "sk-live-ABCDEFGHIJKLMNOP1234"\n\n',
    ),
    /project\.name looks like a credential/,
    "ABCDEFGH",
  );
});

test("a prompt may contain newlines and tabs", () => {
  withConfig(
    VALID.replace(
      'kind = "Verifier"',
      'kind = "Verifier"\nprompt = "a\\n\\tb"',
    ),
    (directory) => {
      const reviewer = loadCapstanConfig(directory).roles.find(
        (role) => role.name === "reviewer",
      )!;
      assert.equal(reviewer.prompt.source, "inline");
    },
  );
});

test("an empty or control-laden prompt file is rejected", () => {
  const directory = projectDirectory();
  try {
    const config = VALID.replace(
      'kind = "Verifier"',
      'kind = "Verifier"\nprompt_file = "p.md"',
    );
    write(directory, config);
    writeFileSync(path.join(directory, "p.md"), " \n");
    assert.throws(
      () => loadCapstanConfig(directory),
      /prompt_file must not be empty/,
    );
    writeFileSync(path.join(directory, "p.md"), "a\u001bb");
    assert.throws(
      () => loadCapstanConfig(directory),
      /prompt_file contains control, format or line-separator characters/,
    );
  } finally {
    rmSync(directory, { recursive: true, force: true });
  }
});

test("worker_ack_timeout_seconds is configurable and bounded", () => {
  const withTimer = (value: string): string =>
    VALID.replace(
      "schema_version = 1\n",
      `schema_version = 1\n[timers]\nworker_ack_timeout_seconds = ${value}\n\n`,
    );
  withConfig(withTimer("45"), (directory) => {
    assert.equal(
      loadCapstanConfig(directory).timers.workerAckTimeoutSeconds,
      45,
    );
  });
  assertRejected(
    withTimer("0"),
    /timers\.worker_ack_timeout_seconds must be between 1 and 86400/,
  );
  assertRejected(
    withTimer("1.5"),
    /timers\.worker_ack_timeout_seconds must be an integer/,
  );
});

test("the loader accepts a byte-order mark", () => {
  withConfig(
    Buffer.concat([Buffer.from("﻿"), Buffer.from(VALID)]),
    (directory) => {
      assert.equal(loadCapstanConfig(directory).roles.length, 2);
    },
  );
});

test("the loader rejects a group-writable file and a symlinked file", () => {
  const directory = projectDirectory();
  try {
    write(directory, VALID, 0o664);
    assert.throws(
      () => loadCapstanConfig(directory),
      /must not be writable by group or others/,
    );
    write(directory, VALID, 0o644);
    assert.equal(loadCapstanConfig(directory).roles.length, 2);
    rmSync(path.join(directory, CONFIG_FILE_NAME));
    writeFileSync(path.join(directory, "real.toml"), VALID, { mode: 0o600 });
    symlinkSync("real.toml", path.join(directory, CONFIG_FILE_NAME));
    assert.throws(() => loadCapstanConfig(directory), /must be a regular file/);
  } finally {
    rmSync(directory, { recursive: true, force: true });
  }
});

test("error text never contains a secret-shaped value", () => {
  const secret = "sk-live-ABCDEFGHIJKLMNOP1234";
  assertRejected(
    `model = ${secret}\n`,
    /is not valid TOML at line 1, column \d+/,
    "ABCDEFGH",
  );
  assertRejected(
    `schema_version = 1\nmodel = "${secret}\n`,
    /is not valid TOML at line 2/,
    "ABCDEFGH",
  );
  assertRejected(
    VALID.replace(
      'kind = "Verifier"',
      `kind = "Verifier"\nmodel = "${secret}"`,
    ),
    /model looks like a credential/,
    "ABCDEFGH",
  );
  assertRejected(
    VALID.replace(
      'kind = "Verifier"',
      `kind = "Verifier"\nallow = ["${secret}"]`,
    ),
    /allow\[0\] looks like a credential/,
    "ABCDEFGH",
  );
  assertRejected(
    VALID.replace(
      'kind = "Verifier"',
      `kind = "Verifier"\nprompt = "use ${secret}"`,
    ),
    /prompt looks like a credential/,
    "ABCDEFGH",
  );
  assertRejected(
    VALID.replace(
      "schema_version = 1\n",
      `schema_version = 1\n${secret} = 1\n`,
    ),
    /top level has 1 unknown key/,
    "ABCDEFGH",
  );
  assertRejected(
    `${VALID}\n[roles.${secret}]\nkind = "Developer"\nhost = "claude"\n`,
    /role name that does not match/,
    "ABCDEFGH",
  );
  assertRejected(
    VALID.replace(
      'kind = "Verifier"',
      `kind = "Verifier"\npermission_mode = "${secret}"`,
    ),
    /permission_mode must be one of/,
    "ABCDEFGH",
  );
});

test("a prompt file is resolved inside the project and only its hash is kept", () => {
  const directory = projectDirectory();
  try {
    mkdirSync(path.join(directory, "prompts"));
    const promptPath = path.join(directory, "prompts", "reviewer.md");
    writeFileSync(promptPath, "Review carefully. SECRET-FREE-PROMPT-TEXT");
    write(
      directory,
      VALID.replace(
        'kind = "Verifier"',
        'kind = "Verifier"\nprompt_file = "prompts/reviewer.md"',
      ),
    );
    const first = loadCapstanConfig(directory);
    const reviewer = first.roles.find((role) => role.name === "reviewer")!;
    assert.equal(reviewer.prompt.source, "file");
    assert.equal(
      reviewer.prompt.path,
      path.join(path.dirname(promptPath), "reviewer.md"),
    );
    assert.match(reviewer.prompt.hash ?? "", /^[0-9a-f]{64}$/);
    assert.ok(!JSON.stringify(first).includes("SECRET-FREE-PROMPT-TEXT"));

    writeFileSync(promptPath, "Changed prompt");
    const second = loadCapstanConfig(directory).roles.find(
      (role) => role.name === "reviewer",
    )!;
    assert.notEqual(second.configHash, reviewer.configHash);

    const outside = mkdtempSync(path.join(tmpdir(), "capstan-config-outside-"));
    try {
      writeFileSync(path.join(outside, "leak.md"), "outside");
      symlinkSync(
        path.join(outside, "leak.md"),
        path.join(directory, "prompts", "link.md"),
      );
      write(
        directory,
        VALID.replace(
          'kind = "Verifier"',
          'kind = "Verifier"\nprompt_file = "prompts/link.md"',
        ),
      );
      assert.throws(
        () => loadCapstanConfig(directory),
        /prompt_file must stay inside the project/,
      );
    } finally {
      rmSync(outside, { recursive: true, force: true });
    }
  } finally {
    rmSync(directory, { recursive: true, force: true });
  }
});

test("the prompt text is kept on the role for the launcher and is not part of the role hash", () => {
  const inline = VALID.replace(
    'kind = "PM"',
    'kind = "PM"\nprompt = "Work carefully.\\nAsk when unsure."',
  );
  withConfig(inline, (directory) => {
    const role = loadCapstanConfig(directory).roles.find(
      (r) => r.name === "pm",
    )!;
    assert.equal(role.promptText, "Work carefully.\nAsk when unsure.");
    assert.equal(role.prompt.source, "inline");
    assert.ok(
      !JSON.stringify(role).includes("Work carefully"),
      "config check prints roles as JSON and must not echo the text",
    );
  });
  withConfig(VALID, (directory) => {
    assert.ok(
      loadCapstanConfig(directory).roles.every((r) => r.promptText === null),
    );
  });
});
