import { createHash } from "node:crypto";

function normalizeJson(value: unknown, ancestors: Set<object>): unknown {
  if (value === null || typeof value === "string" || typeof value === "boolean")
    return value;
  if (typeof value === "number") {
    if (!Number.isFinite(value))
      throw new TypeError("canonical JSON does not accept non-finite numbers");
    return Object.is(value, -0) ? 0 : value;
  }
  if (Array.isArray(value)) {
    if (ancestors.has(value))
      throw new TypeError("canonical JSON does not accept circular values");
    ancestors.add(value);
    const normalized = value.map((item) => normalizeJson(item, ancestors));
    ancestors.delete(value);
    return normalized;
  }
  if (typeof value === "object") {
    if (ancestors.has(value))
      throw new TypeError("canonical JSON does not accept circular values");
    const prototype = Object.getPrototypeOf(value);
    if (prototype !== Object.prototype && prototype !== null)
      throw new TypeError("canonical JSON requires plain objects");
    ancestors.add(value);
    const normalized: Record<string, unknown> = Object.create(null) as Record<
      string,
      unknown
    >;
    for (const key of Object.keys(value).sort()) {
      const property = (value as Record<string, unknown>)[key];
      if (property === undefined)
        throw new TypeError("canonical JSON does not accept undefined values");
      normalized[key] = normalizeJson(property, ancestors);
    }
    ancestors.delete(value);
    return normalized;
  }
  throw new TypeError(`canonical JSON does not accept ${typeof value}`);
}

export function canonicalJson(value: unknown): string {
  return JSON.stringify(normalizeJson(value, new Set()));
}

export function sha256(value: string): string {
  return createHash("sha256").update(value).digest("hex");
}

export function digestJson(value: unknown): string {
  return sha256(canonicalJson(value));
}
