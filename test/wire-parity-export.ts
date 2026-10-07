/**
 * Writes the fixtures capstan-wire (rust/crates/wire) is tested against: what Node computes with JSON.parse,
 * JSON.stringify, String(number), the CLI's render(), Date.parse and the framing and response decisions of client.ts,
 * on fixed inputs. Run `npm run build && node dist/test/wire-parity-export.js` after an intended change and commit the
 * result; wire-parity.test.ts fails while the committed files differ from a fresh export.
 */
import { mkdirSync, writeFileSync } from "node:fs";
import path from "node:path";

// Date.parse reads a time with no offset in local time; the Rust side reads it as UTC.
process.env.TZ = "UTC";

const root = path.resolve(import.meta.dirname, "..", "..");
export const PARITY_DIRECTORY = path.join(
  root,
  "rust",
  "crates",
  "wire",
  "tests",
  "parity",
);

const MAX_FRAME_BYTES = 65_536;

/** The CLI's render(), copied from src/cli.ts (wire-parity.test.ts checks the copy is still there verbatim). */
export const RENDER_SOURCE = `function render(value: unknown): string {
  if (typeof value === "string") return value;
  if (Array.isArray(value)) return value.map(render).join("\\n");
  if (value && typeof value === "object")
    return Object.entries(value)
      .map(([key, item]) => \`\${key}: \${render(item)}\`)
      .join("\\n");
  return String(value);
}`;

function render(value: unknown): string {
  if (typeof value === "string") return value;
  if (Array.isArray(value)) return value.map(render).join("\n");
  if (value && typeof value === "object")
    return Object.entries(value)
      .map(([key, item]) => `${key}: ${render(item)}`)
      .join("\n");
  return String(value);
}

/** A string too long to write out: `head`, `text` repeated `count` times, `tail`. */
type Expanded = {
  readonly head: string;
  readonly text: string;
  readonly count: number;
  readonly tail: string;
};

function bits(n: number): string {
  const view = new DataView(new ArrayBuffer(8));
  view.setFloat64(0, n);
  return view.getBigUint64(0).toString(16).padStart(16, "0");
}

function fromBits(hex: string): number {
  const view = new DataView(new ArrayBuffer(8));
  view.setBigUint64(0, BigInt(`0x${hex}`));
  return view.getFloat64(0);
}

/** A fixed pseudo-random sequence, so the export is the same on every run. */
function sequence(seed: number): () => number {
  let state = seed >>> 0;
  return () => {
    state = (Math.imul(state, 1664525) + 1013904223) >>> 0;
    return state;
  };
}

function numberCases() {
  const fixed = [
    0,
    -0,
    1,
    -1,
    1.0,
    0.1,
    0.5,
    5e-7,
    1e-7,
    1.5e-7,
    0.000001,
    0.00001234,
    123456789012345680000,
    1e20,
    1e21,
    1.2345e21,
    2 ** 53,
    2 ** 53 + 2,
    9007199254740993,
    4.35,
    0.3,
    1 / 3,
    100,
    1e100,
    -1e-100,
    5e-324,
    2.2250738585072014e-308,
    1.7976931348623157e308,
    Number.EPSILON,
    Infinity,
    -Infinity,
    NaN,
  ].map(bits);
  const next = sequence(20240607);
  const random: string[] = [];
  for (let i = 0; i < 400; i++) {
    const hex = `${next().toString(16).padStart(8, "0")}${next().toString(16).padStart(8, "0")}`;
    random.push(bits(fromBits(hex)));
  }
  for (let i = 0; i < 100; i++) random.push(bits((next() % 1_000_000) / 1000));
  for (let i = 0; i < 100; i++)
    random.push(bits(next() * 2 ** (next() % 60) * (i % 2 ? 1 : -1)));
  return [...fixed, ...random].map((hex) => {
    const n = fromBits(hex);
    return { bits: hex, string: String(n), json: JSON.stringify(n) };
  });
}

function deepArray(depth: number): string {
  return "[".repeat(depth) + "1" + "]".repeat(depth);
}

function deepObject(depth: number): string {
  return '{"a":'.repeat(depth) + "null" + "}".repeat(depth);
}

const PARSE_TEXTS: readonly string[] = [
  "0",
  "-0",
  "1.0",
  "5e-7",
  "1E21",
  "1e+21",
  "2e-1",
  "9007199254740993",
  "1.7976931348623157e308",
  "1e999",
  "-1e999",
  "1e-999",
  "0.1e1",
  "123456789012345678901234567890",
  "true",
  "false",
  "null",
  '""',
  '"abc"',
  '"\\ud800"',
  '"\\udc00"',
  '"\\ud83d\\ude00"',
  '"\\ude00\\ud83d"',
  '"a\\ud800b"',
  '"\\u2028\\u2029"',
  '"  "',
  '"\\u0000\\u001f\\u007f"',
  '"\\b\\f\\n\\r\\t\\/\\\\\\""',
  '"😀 é ✓"',
  '"\\uD83D\\uDE00"',
  '"\\u00e9"',
  "[]",
  "{}",
  "[1,2,3]",
  ' \t\r\n[ 1 , [ 2 ] , { "a" : [ ] } ]\n',
  '{"a":1,"b":[true,false,null],"c":{"d":"e"}}',
  '{"a":1,"a":2}',
  '{"a":1,"b":2,"a":3}',
  '{"b":1,"2":0,"a":2,"b":3,"1":9}',
  '{"10":1,"9":2,"x":3,"01":4,"4294967294":5,"4294967295":6,"-1":7}',
  '{"__proto__":1,"ok":true}',
  '{"\\ud800":1,"\\ud800":2}',
  '{"":1}',
  "﻿{}",
  "﻿1",
  deepArray(256),
  deepObject(256),
  "",
  " ",
  "01",
  "-",
  "+1",
  ".5",
  "1.",
  "1e",
  "1e+",
  "0x10",
  "NaN",
  "Infinity",
  "undefined",
  "tru",
  "nulll",
  "'a'",
  '"a',
  '"\\x41"',
  '"\\u12"',
  '"\\u12g4"',
  '"a\nb"',
  '"a\tb"',
  "[1,]",
  "[,1]",
  "[1 2]",
  '{"a":1,}',
  '{"a" 1}',
  "{a:1}",
  '{"a":1 "b":2}',
  "[1] x",
  "[1][2]",
  "// c\n1",
  " 1",
  "1 ",
  "\u000b1",
];

function parseCases() {
  return PARSE_TEXTS.map((text) => {
    let value: unknown;
    try {
      value = JSON.parse(text);
    } catch {
      return { text, error: true };
    }
    const compact = JSON.stringify(value);
    const pretty = JSON.stringify(value, null, 2);
    return {
      text,
      compact,
      ...(pretty.length < 2000 ? { pretty } : {}),
      render: render(value),
    };
  });
}

/** A frame whose pad argument makes the JSON `target` bytes long. */
function framePad(
  target: number,
  unit: string,
  extra = "",
): {
  readonly credential: string;
  readonly command: string;
  readonly args: readonly (string | Expanded)[];
  readonly frame: string | null | Expanded;
} {
  const credential = "c".repeat(64);
  const command = "run";
  const bytes = (text: string): number => Buffer.byteLength(text);
  const shell = (arg: string): string =>
    JSON.stringify({ v: 1, credential, command, args: ["first", arg] });
  const fixed = bytes(shell(extra));
  const count = Math.floor((target - fixed) / bytes(unit));
  const filler = "~".repeat(target - fixed - count * bytes(unit));
  const arg = filler + unit.repeat(count) + extra;
  const json = shell(arg);
  const expandedArg: Expanded = {
    head: filler,
    text: unit,
    count,
    tail: extra,
  };
  const args = ["first", expandedArg] as const;
  if (bytes(json) > MAX_FRAME_BYTES) {
    return { credential, command, args, frame: null };
  }
  const head = json.slice(0, json.indexOf(arg));
  const tail = json.slice(json.indexOf(arg) + arg.length);
  return {
    credential,
    command,
    args,
    frame: {
      head: `${head}${filler}`,
      text: unit,
      count,
      tail: `${extra}${tail}\n`,
    },
  };
}

function frameCases() {
  const small: {
    credential: string;
    command: string;
    args: readonly string[];
  }[] = [
    { credential: "", command: "", args: [] },
    { credential: "tok", command: "status", args: [] },
    { credential: "tok", command: "task", args: ["a", "b c", ""] },
    {
      credential: 'q"\\/\b\f\n\r\t',
      command: "x\u0000\u001f\u007f",
      args: ["  ", "😀", "é✓", " "],
    },
  ];
  const cases: unknown[] = small.map((c) => ({
    ...c,
    frame: `${JSON.stringify({ v: 1, credential: c.credential, command: c.command, args: c.args })}\n`,
  }));
  for (const target of [
    MAX_FRAME_BYTES - 1,
    MAX_FRAME_BYTES,
    MAX_FRAME_BYTES + 1,
  ])
    cases.push(framePad(target, "a"));
  for (const target of [
    MAX_FRAME_BYTES - 1,
    MAX_FRAME_BYTES,
    MAX_FRAME_BYTES + 1,
  ])
    cases.push(framePad(target, "😀", "é"));
  return cases;
}

type Decision =
  | { readonly kind: "malformed" }
  | { readonly kind: "ok"; readonly result: string | null }
  | {
      readonly kind: "refused";
      readonly code: string;
      readonly message: string;
    };

/** What callDaemon (src/client.ts) decides about a response line, and what its callers read from the answer. */
function decide(line: Buffer): Decision {
  let record: Record<string, unknown>;
  try {
    const body: unknown = JSON.parse(
      new TextDecoder("utf-8", { fatal: true, ignoreBOM: true }).decode(line),
    );
    if (typeof body !== "object" || body === null || Array.isArray(body))
      throw new Error("malformed reply");
    record = body as Record<string, unknown>;
    if (!("ok" in record)) throw new Error("malformed reply");
  } catch {
    return { kind: "malformed" };
  }
  if (record.ok) {
    const result = record.result;
    return {
      kind: "ok",
      result: result === undefined ? null : JSON.stringify(result),
    };
  }
  const text = (value: unknown): string =>
    value === undefined || value === null ? "" : render(value);
  return {
    kind: "refused",
    code: text(record.code),
    message: text(record.message),
  };
}

const RESPONSE_LINES: readonly (string | Buffer)[] = [
  '{"ok":true,"result":{"pid":42}}',
  '{"ok":true}',
  '{"ok":true,"result":null}',
  '{"ok":true,"result":[1,"a",{"b":2.50}]}',
  '{"ok":true,"result":"\\ud800 text"}',
  '{"ok":false,"code":"E_BAD","message":"no way"}',
  '{"ok":false,"code":"E_BAD"}',
  '{"ok":false}',
  '{"ok":false,"code":7,"message":{"a":[1,2],"b":null}}',
  '{"ok":false,"code":null,"message":"\\ud800\\u2028"}',
  '{"ok":1,"result":2}',
  '{"ok":0,"code":"zero"}',
  '{"ok":"","code":"empty"}',
  '{"ok":"false","result":1}',
  '{"ok":null,"code":"null"}',
  '{"ok":[],"result":1}',
  '{"ok":{},"result":1}',
  '{"ok":true,"ok":false,"code":"dup"}',
  '{"ok":false,"ok":true,"result":3}',
  '{"result":1}',
  "{}",
  "[]",
  '[{"ok":true}]',
  '"ok"',
  "null",
  "5",
  "true",
  "",
  " ",
  "not json",
  '{"ok":true',
  '{"ok":true}x',
  ' {"ok":true} ',
  '﻿{"ok":true}',
  Buffer.from([0xff, 0xfe, 0x7b, 0x7d]),
  Buffer.concat([
    Buffer.from('{"ok":true,"result":"'),
    Buffer.from([0xc0, 0x80]),
    Buffer.from('"}'),
  ]),
  Buffer.concat([
    Buffer.from('{"ok":true,"result":"'),
    Buffer.from([0xed, 0xa0, 0x80]),
    Buffer.from('"}'),
  ]),
  Buffer.concat([
    Buffer.from('{"ok":true,"result":"'),
    Buffer.from([0xe2, 0x82]),
    Buffer.from('"}'),
  ]),
];

function responseCases() {
  return RESPONSE_LINES.map((line) => {
    const buffer = typeof line === "string" ? Buffer.from(line) : line;
    return { hex: buffer.toString("hex"), expect: decide(buffer) };
  });
}

const DATE_TEXTS: readonly string[] = [
  "2020",
  "2020-02",
  "2020-01-01",
  "2020-02-29",
  "2020-02-30",
  "2021-02-29",
  "2021-04-31",
  "2021-04-32",
  "2020-01-01T10:00",
  "2020-01-01T10:00Z",
  "2020-01-01T10:00:05Z",
  "2020-01-01T10:00:05.1Z",
  "2020-01-01T10:00:05.12Z",
  "2020-01-01T10:00:05.123Z",
  "2020-01-01T10:00:05.123456Z",
  "2020-01-01T10:00:05.Z",
  "2020-01-01T24:00:00Z",
  "2020-01-01T24:00:01Z",
  "2020-01-01T10:60Z",
  "2020-01-01T10:00:60Z",
  "2020-01-01t10:00:00z",
  "2020-01-01T10:00:00+05:30",
  "2020-01-01T10:00:00+0530",
  "2020-01-01T10:00:00+05",
  "2020-01-01T10:00:00-00:00",
  "2020-01-01T10:00:00.5+01:00",
  "2020-01-01T10:00:00+24:00",
  "2020-01-01T10:00:00+23:59",
  "2020-01-01T09:00:00-1:00",
  "+002020-01-01T00:00:00Z",
  "-000001-01-01T00:00:00Z",
  "-000000-01-01T00:00:00Z",
  "+275760-09-13T00:00:00.000Z",
  "+275760-09-13T00:00:00.001Z",
  "-271821-04-20T00:00:00.000Z",
  "-271821-04-19T23:59:59.999Z",
  "0000-01-01",
  "0000",
  "1969-12-31T23:59:59.999Z",
  "1970-01-01T00:00:00.000Z",
  "2020-12-31T23:59:59.999Z",
  "2024-06-07T12:34:56.789Z",
  "2020-13-01",
  "2020-00-01",
  "2020-01-00",
  "2020-01-32",
  "2020-01-01T1:00Z",
  "20200101",
  "2020-01-01T10:00:00 Z",
  "2020-01-01T",
  "2020-01-01T10Z",
  "2020-01-01T10:00:00,5Z",
  "2020-01-01T10:00:00Z ",
  "",
  "  ",
  "abc",
];

function dateCases() {
  return DATE_TEXTS.map((text) => {
    const ms = Date.parse(text);
    return { text, ms: Number.isNaN(ms) ? null : ms };
  });
}

/** Every fixture file by name, as the text the exporter writes. */
export function exportFixtures(): Map<string, string> {
  const files = new Map<string, string>();
  files.set("numbers.json", JSON.stringify(numberCases()) + "\n");
  files.set("parse.json", JSON.stringify(parseCases()) + "\n");
  files.set("frames.json", JSON.stringify(frameCases()) + "\n");
  files.set("responses.json", JSON.stringify(responseCases()) + "\n");
  files.set("dates.json", JSON.stringify(dateCases()) + "\n");
  return files;
}

if (import.meta.filename === process.argv[1]) {
  mkdirSync(PARITY_DIRECTORY, { recursive: true });
  const files = exportFixtures();
  for (const [name, text] of files)
    writeFileSync(path.join(PARITY_DIRECTORY, name), text);
  process.stdout.write(`wrote ${files.size} fixtures to ${PARITY_DIRECTORY}\n`);
}
