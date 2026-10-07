import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import {
  chmodSync,
  mkdirSync,
  mkdtempSync,
  rmSync,
  symlinkSync,
  writeFileSync,
} from "node:fs";
import os from "node:os";
import path from "node:path";
import { after, test } from "node:test";
import { cstanWrapperScript, frontEndPath } from "../src/launcher/shared.js";

const scratch = mkdtempSync(path.join(os.tmpdir(), "capstan-wrapper-"));
after(() => rmSync(scratch, { recursive: true, force: true }));

function executable(file: string, text = "#!/bin/sh\n"): string {
  mkdirSync(path.dirname(file), { recursive: true });
  writeFileSync(file, text);
  chmodSync(file, 0o755);
  return file;
}

const NODE = "/usr/bin/node";
const CLI = "/opt/capstan/cli.js";

test("without a front end the npm wrapper is what it always was", () => {
  const wrapper = cstanWrapperScript(NODE, CLI, { env: {}, sea: false });
  assert.equal(wrapper, `#!/bin/sh\nexec '${NODE}' '${CLI}' "$@"\n`);
});

test("under SEA without a sibling cstan the wrapper runs the binary", () => {
  const binary = executable(path.join(scratch, "bare", "cstan-node"));
  const wrapper = cstanWrapperScript(NODE, CLI, {
    env: {},
    sea: true,
    execPath: binary,
  });
  assert.equal(wrapper, `#!/bin/sh\nexec '${binary}' "$@"\n`);
});

test("under SEA a sibling cstan is the front end and is told where the binary is", () => {
  const dir = path.join(scratch, "sea");
  const binary = executable(path.join(dir, "cstan-node"));
  const front = executable(path.join(dir, "cstan"));
  const site = { env: {}, sea: true, execPath: binary };
  assert.equal(frontEndPath(site), front);
  assert.equal(
    cstanWrapperScript(NODE, CLI, site),
    `#!/bin/sh\nCSTAN_NODE_CLI='${binary}' exec '${front}' "$@"\n`,
  );
});

test("under SEA a cstan that is the binary itself, or a link to it, is not a front end", () => {
  const dir = path.join(scratch, "self");
  const binary = executable(path.join(dir, "cstan"));
  assert.equal(
    frontEndPath({ env: {}, sea: true, execPath: binary }),
    undefined,
  );
  const linked = path.join(scratch, "linked");
  const real = executable(path.join(linked, "cstan-node"));
  symlinkSync(real, path.join(linked, "cstan"));
  assert.equal(frontEndPath({ env: {}, sea: true, execPath: real }), undefined);
  const reversed = path.join(scratch, "reversed");
  const front = executable(path.join(reversed, "cstan"));
  symlinkSync(front, path.join(reversed, "cstan-node"));
  assert.equal(
    frontEndPath({
      env: {},
      sea: true,
      execPath: path.join(reversed, "cstan-node"),
    }),
    undefined,
  );
});

test("under SEA a sibling cstan that is not an executable file is ignored", () => {
  const dir = path.join(scratch, "plain");
  const binary = executable(path.join(dir, "cstan-node"));
  writeFileSync(path.join(dir, "cstan"), "");
  chmodSync(path.join(dir, "cstan"), 0o644);
  assert.equal(
    frontEndPath({ env: {}, sea: true, execPath: binary }),
    undefined,
  );
  rmSync(path.join(dir, "cstan"));
  mkdirSync(path.join(dir, "cstan"));
  assert.equal(
    frontEndPath({ env: {}, sea: true, execPath: binary }),
    undefined,
  );
});

test("outside SEA a sibling cstan is not looked for", () => {
  const dir = path.join(scratch, "npm");
  const binary = executable(path.join(dir, "node"));
  executable(path.join(dir, "cstan"));
  assert.equal(
    frontEndPath({ env: {}, sea: false, execPath: binary }),
    undefined,
  );
});

test("CSTAN_FRONT_END names the front end and the wrapper hands it the Node CLI", () => {
  const front = executable(path.join(scratch, "configured", "cstan"));
  const site = { env: { CSTAN_FRONT_END: front }, sea: false };
  assert.equal(frontEndPath(site), front);
  assert.equal(
    cstanWrapperScript(NODE, CLI, site),
    `#!/bin/sh\nCSTAN_NODE_CLI='${CLI}' CSTAN_NODE='${NODE}' exec '${front}' "$@"\n`,
  );
});

test("CSTAN_FRONT_END beats a sibling under SEA", () => {
  const dir = path.join(scratch, "both");
  const binary = executable(path.join(dir, "cstan-node"));
  executable(path.join(dir, "cstan"));
  const front = executable(path.join(scratch, "both-configured", "cstan"));
  assert.equal(
    frontEndPath({
      env: { CSTAN_FRONT_END: front },
      sea: true,
      execPath: binary,
    }),
    front,
  );
});

test("a CSTAN_FRONT_END that is relative, missing or not executable is ignored", () => {
  const dir = path.join(scratch, "bad");
  const binary = executable(path.join(dir, "cstan-node"));
  executable(path.join(dir, "cstan"));
  const notExecutable = path.join(scratch, "bad-file");
  writeFileSync(notExecutable, "");
  chmodSync(notExecutable, 0o644);
  for (const value of [
    "relative/cstan",
    path.join(scratch, "missing"),
    notExecutable,
    dir,
  ]) {
    assert.equal(
      frontEndPath({
        env: { CSTAN_FRONT_END: value },
        sea: true,
        execPath: binary,
      }),
      undefined,
      value,
    );
    assert.equal(
      cstanWrapperScript(NODE, CLI, {
        env: { CSTAN_FRONT_END: value },
        sea: false,
      }),
      `#!/bin/sh\nexec '${NODE}' '${CLI}' "$@"\n`,
    );
  }
  assert.equal(
    frontEndPath({ env: { CSTAN_FRONT_END: "" }, sea: false }),
    undefined,
  );
});

test("the wrapper quotes paths and passes the arguments and the Node CLI location through", () => {
  const dir = path.join(scratch, "it's here");
  const out = path.join(scratch, "wrapper-out");
  const front = executable(
    path.join(dir, "cstan"),
    `#!/bin/sh\nprintf '%s\\n' "$CSTAN_NODE_CLI" "$CSTAN_NODE" "$#" "$@" > '${out}'\n`,
  );
  const cli = path.join(scratch, "dist dir", "cli.js");
  const wrapper = path.join(scratch, "wrapper.sh");
  writeFileSync(
    wrapper,
    cstanWrapperScript("/opt/n o/node", cli, {
      env: { CSTAN_FRONT_END: front },
      sea: false,
    }),
  );
  chmodSync(wrapper, 0o755);
  const result = spawnSync(wrapper, ["ping", "--json", "a b", ""], {
    encoding: "utf8",
  });
  assert.equal(result.status, 0, result.stderr);
  const lines = spawnSync("cat", [out], { encoding: "utf8" }).stdout.split(
    "\n",
  );
  assert.deepEqual(lines.slice(0, 7), [
    cli,
    "/opt/n o/node",
    "4",
    "ping",
    "--json",
    "a b",
    "",
  ]);
});
