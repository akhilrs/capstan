import assert from "node:assert/strict";
import { test } from "node:test";
import { ESLint } from "eslint";

const eslint = new ESLint({ cwd: process.cwd() });

async function restricted(filePath: string, code: string): Promise<boolean> {
  const [result] = await eslint.lintText(code, { filePath });
  return (result?.messages ?? []).some(
    (message) => message.ruleId === "no-restricted-imports",
  );
}

test("modules of src/controller/ other than the public ones cannot be imported from outside it", async () => {
  for (const name of [
    "kernel",
    "areas",
    "prompt-relay",
    "records",
    "a-new-area",
  ]) {
    assert.equal(
      await restricted(
        "src/zz-probe.ts",
        `import { x } from "./controller/${name}.js";\nexport const y = x;\n`,
      ),
      true,
      `${name} must be restricted`,
    );
  }
  assert.equal(
    await restricted(
      "test/zz-probe.ts",
      `import { x } from "../src/controller/a-new-area.js";\nexport const y = x;\n`,
    ),
    true,
  );
});

test("the public controller modules stay importable from outside, and controller files import each other freely", async () => {
  for (const name of ["core", "types", "auth", "messaging"]) {
    assert.equal(
      await restricted(
        "src/zz-probe.ts",
        `import { x } from "./controller/${name}.js";\nexport const y = x;\n`,
      ),
      false,
      `${name} must stay public`,
    );
  }
  assert.equal(
    await restricted(
      "src/controller/zz-probe.ts",
      `import { x } from "./kernel.js";\nexport const y = x;\n`,
    ),
    false,
  );
});
