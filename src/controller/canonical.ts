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
    const normalized: unknown[] = [];
    for (const key of Reflect.ownKeys(value)) {
      if (key === "length") continue;
      if (typeof key !== "string" || !/^(0|[1-9]\d*)$/.test(key))
        throw new TypeError(
          "canonical JSON arrays cannot have extra properties",
        );
      const descriptor = Object.getOwnPropertyDescriptor(value, key);
      if (
        !descriptor ||
        "get" in descriptor ||
        "set" in descriptor ||
        !descriptor.enumerable
      )
        throw new TypeError(
          "canonical JSON does not accept accessor properties or non-enumerable properties",
        );
    }
    for (let index = 0; index < value.length; index += 1) {
      const descriptor = Object.getOwnPropertyDescriptor(value, String(index));
      normalized.push(
        normalizeJson(descriptor ? descriptor.value : null, ancestors),
      );
    }
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
    for (const key of Reflect.ownKeys(value)) {
      if (typeof key !== "string")
        throw new TypeError("canonical JSON does not accept symbol properties");
      const descriptor = Object.getOwnPropertyDescriptor(value, key);
      if (
        !descriptor ||
        "get" in descriptor ||
        "set" in descriptor ||
        !descriptor.enumerable
      )
        throw new TypeError(
          "canonical JSON does not accept accessor properties or non-enumerable properties",
        );
      if (descriptor.value === undefined)
        throw new TypeError("canonical JSON does not accept undefined values");
      normalized[key] = normalizeJson(descriptor.value, ancestors);
    }
    const sorted: Record<string, unknown> = Object.create(null) as Record<
      string,
      unknown
    >;
    for (const key of Object.keys(normalized).sort())
      sorted[key] = normalized[key];
    ancestors.delete(value);
    return sorted;
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
