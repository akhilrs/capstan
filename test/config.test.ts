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
  });
});

test("the starter configuration written by init is valid", () => {
  withConfig(STARTER_CONFIG, (directory) => {
    const config = loadCapstanConfig(directory);
    assert.deepEqual(
      config.roles.map((role) => role.name),
      ["pm", "developer", "reviewer"],
    );
  });
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
    /model contains control characters/,
  ],
  [
    "too many allow entries",
    VALID.replace(
      'kind = "Verifier"',
      `kind = "Verifier"\nallow = [${Array.from({ length: 65 }, (_, i) => `"Bash(cmd${i})"`).join(", ")}]`,
    ),
    /allow exceeds 64 entries/,
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
