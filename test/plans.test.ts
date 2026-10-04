import assert from "node:assert/strict";
import { test } from "node:test";
import {
  MAX_PLAN_BODY_BYTES,
  parsePlanBody,
  type PlanErrorCode,
} from "../src/plans.js";

type Json = Record<string, unknown>;

const LIMITS = { maxPackages: 8 };

function pkg(id: string, overrides: Json = {}): Json {
  return {
    id,
    title: `Package ${id}`,
    owns: [`src/${id}/`],
    estimate_hours: 2,
    acceptance: [`${id} works`],
    ...overrides,
  };
}

function body(overrides: Json = {}): string {
  return JSON.stringify({
    summary: "one paragraph",
    packages: [pkg("wp1"), pkg("wp2", { depends_on: ["wp1"] })],
    ...overrides,
  });
}

function refusal(text: string, limits = LIMITS): [PlanErrorCode, string] {
  const result = parsePlanBody(text, limits);
  assert.equal(result.ok, false);
  if (result.ok) throw new Error("unreachable");
  return [result.code, result.reason];
}

function accepted(text: string, limits = LIMITS) {
  const result = parsePlanBody(text, limits);
  assert.equal(result.ok, true, result.ok ? "" : result.reason);
  if (!result.ok) throw new Error("unreachable");
  return result.plan;
}

test("a valid plan parses with defaults filled in", () => {
  const plan = accepted(body());
  assert.equal(plan.summary, "one paragraph");
  assert.deepEqual(plan.risks, []);
  assert.deepEqual(plan.integrationOrder, ["wp1", "wp2"]);
  assert.deepEqual(plan.packages[1], {
    id: "wp2",
    title: "Package wp2",
    role: "developer",
    owns: ["src/wp2"],
    interfaces: [],
    dependsOn: ["wp1"],
    estimateHours: 2,
    acceptance: ["wp2 works"],
    risks: [],
    type: null,
    scope: null,
    breaking: false,
  });
});

test("package type, scope and breaking are optional; a bad one is invalid_shape", () => {
  const plan = accepted(
    body({
      packages: [pkg("wp1", { type: "fix", scope: "plans", breaking: true })],
    }),
  );
  assert.equal(plan.packages[0]?.type, "fix");
  assert.equal(plan.packages[0]?.scope, "plans");
  assert.equal(plan.packages[0]?.breaking, true);
  for (const bad of [
    { type: "feature" },
    { type: 3 },
    { scope: "Has Space" },
    { scope: "" },
    { breaking: "yes" },
  ]) {
    assert.equal(
      refusal(body({ packages: [pkg("wp1", bad)] }))[0],
      "invalid_shape",
    );
  }
});

test("a plan keeps every optional field it supplies", () => {
  const plan = accepted(
    body({
      risks: ["wire format"],
      integration_order: ["wp1"],
      packages: [
        pkg("wp1", {
          role: "backend",
          interfaces: ["exports parseFoo"],
          risks: ["touches parser"],
          estimate_hours: 80,
        }),
      ],
    }),
  );
  assert.deepEqual(plan.risks, ["wire format"]);
  assert.equal(plan.packages[0]?.role, "backend");
  assert.deepEqual(plan.packages[0]?.interfaces, ["exports parseFoo"]);
  assert.equal(plan.packages[0]?.estimateHours, 80);
});

test("table: structural refusals", () => {
  const cases: Array<[string, string, PlanErrorCode, RegExp]> = [
    ["not JSON", "{nope", "invalid_json", /not valid JSON/],
    ["array body", "[]", "invalid_shape", /plan must be an object/],
    [
      "unknown plan key",
      body({ extra: 1 }),
      "invalid_shape",
      /unknown key "extra"/,
    ],
    [
      "empty summary",
      body({ summary: "  " }),
      "invalid_shape",
      /summary must not be empty/,
    ],
    [
      "no packages",
      body({ packages: [] }),
      "invalid_shape",
      /packages must be a non-empty array/,
    ],
    [
      "duplicate id",
      body({ packages: [pkg("wp1"), pkg("wp1", { owns: ["docs/x.md"] })] }),
      "duplicate_id",
      /"wp1" is repeated/,
    ],
    [
      "bad id",
      body({ packages: [pkg("WP-1")] }),
      "invalid_shape",
      /must match/,
    ],
    [
      "missing acceptance",
      body({ packages: [pkg("wp1", { acceptance: undefined })] }),
      "invalid_shape",
      /acceptance must be an array/,
    ],
    [
      "empty acceptance",
      body({ packages: [pkg("wp1", { acceptance: [] })] }),
      "invalid_shape",
      /acceptance must have at least one entry/,
    ],
    [
      "missing owns",
      body({ packages: [pkg("wp1", { owns: undefined })] }),
      "invalid_shape",
      /owns must be an array/,
    ],
    [
      "escaping path",
      body({ packages: [pkg("wp1", { owns: ["../etc/passwd"] })] }),
      "invalid_shape",
      /inside the repository/,
    ],
    [
      "absolute path",
      body({ packages: [pkg("wp1", { owns: ["/etc"] })] }),
      "invalid_shape",
      /inside the repository/,
    ],
    [
      "zero estimate",
      body({ packages: [pkg("wp1", { estimate_hours: 0 })] }),
      "invalid_shape",
      /estimate_hours/,
    ],
    [
      "estimate over 80",
      body({ packages: [pkg("wp1", { estimate_hours: 80.5 })] }),
      "invalid_shape",
      /estimate_hours/,
    ],
    [
      "string estimate",
      body({ packages: [pkg("wp1", { estimate_hours: "3" })] }),
      "invalid_shape",
      /estimate_hours/,
    ],
    [
      "unknown package key",
      body({ packages: [pkg("wp1", { colour: "red" })] }),
      "invalid_shape",
      /unknown key "colour"/,
    ],
    [
      "unknown dependency",
      body({ packages: [pkg("wp1", { depends_on: ["ghost"] })] }),
      "unknown_dependency",
      /unknown package "ghost"/,
    ],
    [
      "self dependency",
      body({ packages: [pkg("wp1", { depends_on: ["wp1"] })] }),
      "cycle",
      /cycle among wp1/,
    ],
    [
      "dependency cycle",
      body({
        packages: [
          pkg("wp1", { depends_on: ["wp2"] }),
          pkg("wp2", { depends_on: ["wp1"] }),
          pkg("wp3"),
        ],
      }),
      "cycle",
      /cycle among wp1, wp2/,
    ],
  ];
  for (const [name, text, code, reason] of cases) {
    const [gotCode, gotReason] = refusal(text);
    assert.equal(gotCode, code, name);
    assert.match(gotReason, reason, name);
  }
});

test("more packages than the limit are refused", () => {
  const packages = ["a", "b", "c"].map((id) => pkg(id));
  const [code, reason] = refusal(body({ packages }), { maxPackages: 2 });
  assert.equal(code, "too_many_packages");
  assert.match(reason, /3 packages, at most 2/);
  accepted(body({ packages }), { maxPackages: 3 });
});

test("overlapping owns without an order are refused", () => {
  const cases: Array<[string, string[], string[]]> = [
    ["equal path", ["src/a.ts"], ["src/a.ts"]],
    ["directory contains file", ["src/foo/"], ["src/foo/bar.ts"]],
    ["file inside directory, reversed", ["src/foo/bar.ts"], ["src/foo"]],
    ["dot-slash spelling", ["./src/a.ts"], ["src//a.ts"]],
  ];
  for (const [name, left, right] of cases) {
    const [code, reason] = refusal(
      body({
        packages: [pkg("wp1", { owns: left }), pkg("wp2", { owns: right })],
      }),
    );
    assert.equal(code, "overlap", name);
    assert.match(reason, /wp1 and wp2/, name);
  }
});

test("owns that only share a name prefix do not overlap", () => {
  accepted(
    body({
      packages: [
        pkg("wp1", { owns: ["src/foo/"] }),
        pkg("wp2", { owns: ["src/foobar.ts"] }),
      ],
    }),
  );
});

test("overlapping owns with a depends_on order are accepted", () => {
  const direct = accepted(
    body({
      packages: [
        pkg("wp1", { owns: ["src/a.ts"] }),
        pkg("wp2", { owns: ["src/a.ts"], depends_on: ["wp1"] }),
      ],
    }),
  );
  assert.deepEqual(direct.integrationOrder, ["wp1", "wp2"]);
  const transitive = accepted(
    body({
      packages: [
        pkg("wp3", { owns: ["src/a.ts"], depends_on: ["wp2"] }),
        pkg("wp2", { owns: ["src/b.ts"], depends_on: ["wp1"] }),
        pkg("wp1", { owns: ["src/a.ts"] }),
      ],
    }),
  );
  assert.deepEqual(transitive.integrationOrder, ["wp1", "wp2", "wp3"]);
});

test("an unordered pair is refused even when another pair is ordered", () => {
  const [code, reason] = refusal(
    body({
      packages: [
        pkg("wp1", { owns: ["src/a.ts"] }),
        pkg("wp2", { owns: ["src/a.ts"], depends_on: ["wp1"] }),
        pkg("wp3", { owns: ["src/a.ts"] }),
      ],
    }),
  );
  assert.equal(code, "overlap");
  assert.match(reason, /wp1 and wp3/);
});

test("integration_order must be a topological permutation", () => {
  const cases: Array<[string, string[], RegExp]> = [
    ["wrong order", ["wp2", "wp1"], /wp2 before its dependency wp1/],
    ["missing id", ["wp1"], /every package exactly once/],
    ["repeated id", ["wp1", "wp1"], /repeats "wp1"/],
    ["unknown id", ["wp1", "wp2", "wp9"], /unknown "wp9"/],
  ];
  for (const [name, order, reason] of cases) {
    const [code, gotReason] = refusal(body({ integration_order: order }));
    assert.equal(code, "bad_order", name);
    assert.match(gotReason, reason, name);
  }
  const independent = body({
    packages: [pkg("wp1"), pkg("wp2")],
    integration_order: ["wp2", "wp1"],
  });
  assert.deepEqual(accepted(independent).integrationOrder, ["wp2", "wp1"]);
});

test("the default integration order follows dependencies, then package order", () => {
  const plan = accepted(
    body({
      packages: [pkg("wp3", { depends_on: ["wp1"] }), pkg("wp1"), pkg("wp2")],
    }),
  );
  assert.deepEqual(plan.integrationOrder, ["wp1", "wp3", "wp2"]);
});

test("a body over 32 KiB is refused, one at the limit is read", () => {
  const filler = (size: number) =>
    JSON.stringify({
      summary: "x".repeat(size),
      packages: [pkg("wp1")],
    });
  const base = Buffer.byteLength(filler(0), "utf8");
  const atLimit = filler(MAX_PLAN_BODY_BYTES - base);
  assert.equal(Buffer.byteLength(atLimit, "utf8"), MAX_PLAN_BODY_BYTES);
  accepted(atLimit);
  const [code] = refusal(filler(MAX_PLAN_BODY_BYTES - base + 1));
  assert.equal(code, "too_large");
});

test("the size limit counts bytes, not characters", () => {
  const [code] = refusal(
    JSON.stringify({
      summary: "é".repeat(MAX_PLAN_BODY_BYTES / 2),
      packages: [pkg("wp1")],
    }),
  );
  assert.equal(code, "too_large");
});

test("control characters are normalized in every text field", () => {
  const plan = accepted(
    body({
      summary: "line one\r\nline\u0000two‮three ",
      risks: ["a b"],
      packages: [
        pkg("wp1", {
          title: "tab\there",
          interfaces: ["x\u0007y"],
          acceptance: ["ok​!"],
        }),
      ],
    }),
  );
  assert.equal(plan.summary, "line one\nline two three");
  assert.deepEqual(plan.risks, ["a\nb"]);
  const first = plan.packages[0];
  assert.equal(first?.title, "tab here");
  assert.deepEqual(first?.interfaces, ["x y"]);
  assert.deepEqual(first?.acceptance, ["ok !"]);
});

test("a text entry that is only control characters is refused", () => {
  const [code, reason] = refusal(
    body({ packages: [pkg("wp1", { acceptance: ["\u0000\u0001"] })] }),
  );
  assert.equal(code, "invalid_shape");
  assert.match(reason, /acceptance\[0\] must not be empty/);
});
