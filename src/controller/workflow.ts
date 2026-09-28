import { createHash } from "node:crypto";
import { canonicalJson } from "./canonical.js";

export interface WorkflowLimits {
  readonly maxSlices: number;
  readonly maxRunMs: number;
  readonly maxDispatches: number;
}

export interface WorkflowSlice {
  readonly id: string;
  readonly title: string;
  readonly description: string;
  readonly role: "Developer";
  readonly dependsOn: readonly string[];
  readonly writeScope: readonly string[];
  readonly acceptanceCriteria: readonly string[];
}

export interface WorkflowPlan {
  readonly schemaVersion: 1;
  readonly taskId: string;
  readonly objective: string;
  readonly acceptanceCriteria: readonly string[];
  readonly limits: WorkflowLimits;
  readonly slices: readonly WorkflowSlice[];
}

export interface ValidatedWorkflowPlan {
  readonly plan: WorkflowPlan;
  readonly hash: string;
  readonly order: readonly string[];
}

function exactKeys(
  value: Record<string, unknown>,
  keys: readonly string[],
  label: string,
): void {
  const actual = Object.keys(value).sort();
  const expected = [...keys].sort();
  if (
    actual.length !== expected.length ||
    actual.some((key, index) => key !== expected[index])
  )
    throw new TypeError(`${label} has unknown or missing fields`);
}

function string(value: unknown, label: string, max = 16_384): string {
  if (
    typeof value !== "string" ||
    value.trim().length === 0 ||
    value.length > max
  )
    throw new TypeError(
      `${label} must be a non-empty string of at most ${max} characters`,
    );
  return value;
}

function stringList(
  value: unknown,
  label: string,
  min = 0,
  max = 128,
): readonly string[] {
  if (!Array.isArray(value) || value.length < min || value.length > max)
    throw new TypeError(`${label} must contain ${min}..${max} strings`);
  const result = value.map((entry, index) =>
    string(entry, `${label}[${index}]`, 4_096),
  );
  if (new Set(result).size !== result.length)
    throw new TypeError(`${label} contains duplicates`);
  return Object.freeze(result);
}

export function validateWorkflowPlan(value: unknown): ValidatedWorkflowPlan {
  if (value === null || typeof value !== "object" || Array.isArray(value))
    throw new TypeError("brief must be a JSON object");
  const raw = value as Record<string, unknown>;
  exactKeys(
    raw,
    [
      "schemaVersion",
      "taskId",
      "objective",
      "acceptanceCriteria",
      "limits",
      "slices",
    ],
    "brief",
  );
  if (raw.schemaVersion !== 1) throw new TypeError("schemaVersion must be 1");
  const taskId = string(raw.taskId, "taskId", 128);
  const objective = string(raw.objective, "objective");
  const acceptanceCriteria = stringList(
    raw.acceptanceCriteria,
    "acceptanceCriteria",
    1,
    32,
  );
  if (
    raw.limits === null ||
    typeof raw.limits !== "object" ||
    Array.isArray(raw.limits)
  )
    throw new TypeError("limits must be an object");
  const limits = raw.limits as Record<string, unknown>;
  exactKeys(limits, ["maxSlices", "maxRunMs", "maxDispatches"], "limits");
  const { maxSlices, maxRunMs, maxDispatches } = limits;
  if (
    !Number.isSafeInteger(maxSlices) ||
    (maxSlices as number) < 2 ||
    (maxSlices as number) > 8
  )
    throw new TypeError("limits.maxSlices must be an integer from 2 through 8");
  if (
    !Number.isSafeInteger(maxRunMs) ||
    (maxRunMs as number) < 1_000 ||
    (maxRunMs as number) > 86_400_000
  )
    throw new TypeError(
      "limits.maxRunMs must be an integer from 1000 through 86400000",
    );
  if (
    !Number.isSafeInteger(maxDispatches) ||
    (maxDispatches as number) < 2 ||
    (maxDispatches as number) > 64
  )
    throw new TypeError(
      "limits.maxDispatches must be an integer from 2 through 64",
    );
  if (
    !Array.isArray(raw.slices) ||
    raw.slices.length < 2 ||
    raw.slices.length > (maxSlices as number)
  )
    throw new TypeError(
      "slices must contain at least two entries and not exceed maxSlices",
    );
  // PM, Developer/Verifier for each slice, final-parent Verifier, Supervisor.
  const requiredDispatches = 2 * raw.slices.length + 3;
  if ((maxDispatches as number) < requiredDispatches)
    throw new TypeError(
      `limits.maxDispatches must allow at least ${requiredDispatches} role dispatches for the declared slices`,
    );
  const slices = raw.slices.map((value, index): WorkflowSlice => {
    if (value === null || typeof value !== "object" || Array.isArray(value))
      throw new TypeError(`slices[${index}] must be an object`);
    const rawSlice = value as Record<string, unknown>;
    exactKeys(
      rawSlice,
      [
        "id",
        "title",
        "description",
        "role",
        "dependsOn",
        "writeScope",
        "acceptanceCriteria",
      ],
      `slices[${index}]`,
    );
    if (rawSlice.role !== "Developer")
      throw new TypeError(`slices[${index}].role must be Developer`);
    const id = string(rawSlice.id, `slices[${index}].id`, 128);
    if (!/^[A-Za-z0-9][A-Za-z0-9._-]*$/.test(id))
      throw new TypeError(`slices[${index}].id is not a stable identifier`);
    const writeScope = stringList(
      rawSlice.writeScope,
      `slices[${index}].writeScope`,
      1,
      64,
    );
    for (const scope of writeScope) {
      if (
        scope === "." ||
        scope.startsWith("./") ||
        scope.startsWith("/") ||
        scope.split(/[\\/]/).includes("..")
      )
        throw new TypeError(
          `slices[${index}].writeScope must stay within the project`,
        );
    }
    return Object.freeze({
      id,
      title: string(rawSlice.title, `slices[${index}].title`, 512),
      description: string(rawSlice.description, `slices[${index}].description`),
      role: "Developer",
      dependsOn: stringList(
        rawSlice.dependsOn,
        `slices[${index}].dependsOn`,
        0,
        16,
      ),
      writeScope,
      acceptanceCriteria: stringList(
        rawSlice.acceptanceCriteria,
        `slices[${index}].acceptanceCriteria`,
        1,
        32,
      ),
    });
  });
  const sliceCriteria = new Set(
    slices.flatMap((slice) => slice.acceptanceCriteria),
  );
  const uncoveredCriteria = acceptanceCriteria.filter(
    (criterion) => !sliceCriteria.has(criterion),
  );
  if (uncoveredCriteria.length)
    throw new TypeError(
      `slice acceptanceCriteria must cover every parent criterion: ${uncoveredCriteria.join("; ")}`,
    );
  const byId = new Map(slices.map((slice) => [slice.id, slice]));
  if (byId.size !== slices.length)
    throw new TypeError("slice ids must be unique");
  const visiting = new Set<string>();
  const visited = new Set<string>();
  const order: string[] = [];
  const visit = (id: string): void => {
    if (visiting.has(id))
      throw new TypeError("slice dependencies must be acyclic");
    if (visited.has(id)) return;
    const slice = byId.get(id);
    if (!slice) throw new TypeError(`unknown slice dependency: ${id}`);
    visiting.add(id);
    for (const dependency of slice.dependsOn) {
      if (dependency === id)
        throw new TypeError("slice cannot depend on itself");
      visit(dependency);
    }
    visiting.delete(id);
    visited.add(id);
    order.push(id);
  };
  for (const slice of slices) visit(slice.id);
  if (slices[0]?.dependsOn.length !== 0)
    throw new TypeError("the first slice must not depend on another slice");
  if (!slices.slice(1).some((slice) => slice.dependsOn.includes(slices[0]!.id)))
    throw new TypeError(
      "a later slice must explicitly depend on the first slice",
    );
  const plan = Object.freeze({
    schemaVersion: 1 as const,
    taskId,
    objective,
    acceptanceCriteria,
    limits: Object.freeze({
      maxSlices: maxSlices as number,
      maxRunMs: maxRunMs as number,
      maxDispatches: maxDispatches as number,
    }),
    slices: Object.freeze(slices),
  });
  const hash = createHash("sha256").update(canonicalJson(plan)).digest("hex");
  return Object.freeze({ plan, hash, order: Object.freeze(order) });
}
