/**
 * Writes the fixtures capstan-ledger (rust/crates/ledger) is tested against, and builds the ledgers the interop test
 * migrates with both runners:
 *  - refusals.json: the message of each refusal of the Node migration runner (gap, newer version, renamed file, changed
 *    checksum, tables without a ledger, invalid UTF-8), captured by running the real openDatabase on a ledger broken that way;
 *  - iso.json: new Date(ms).toISOString() for fixed ms, which the Rust applied_at formatting must equal.
 * Run `npm run build && node dist/test/ledger-parity-export.js` after an intended change and commit the result;
 * ledger-rust-interop.test.ts fails while the committed files differ from a fresh export.
 */
import fs, {
  mkdirSync,
  readdirSync,
  readFileSync,
  writeFileSync,
} from "node:fs";
import { createHash } from "node:crypto";
import path from "node:path";
import { openDatabase } from "../src/controller/database.js";
import { openSqlite } from "../src/controller/sqlite.js";
import { removeTempDir, tempDir } from "./tmp.js";

const root = path.resolve(import.meta.dirname, "..", "..");
export const MIGRATIONS_DIRECTORY = path.join(root, "migrations");
export const PARITY_DIRECTORY = path.join(
  root,
  "rust",
  "crates",
  "ledger",
  "tests",
  "parity",
);

export interface MigrationFile {
  readonly version: number;
  readonly name: string;
  readonly bytes: Buffer;
}

/** The repository's migrations/*.sql in version order. */
export function migrationFiles(): MigrationFile[] {
  return readdirSync(MIGRATIONS_DIRECTORY)
    .filter((name) => name.endsWith(".sql"))
    .sort()
    .map((name) => ({
      version: Number(/^(\d+)_/.exec(name)?.[1]),
      name,
      bytes: readFileSync(path.join(MIGRATIONS_DIRECTORY, name)),
    }));
}

/**
 * A ledger at schema version `version` (0 = an empty database file), written the way the Node runner writes each step:
 * the WAL pragmas, then per migration BEGIN IMMEDIATE / exec / INSERT schema_migrations / COMMIT.
 */
export function buildNodeLedger(file: string, version: number): void {
  const database = openSqlite(file, { timeout: 5_000 });
  try {
    database.pragma("foreign_keys = ON");
    database.pragma("journal_mode = WAL");
    database.pragma("synchronous = FULL");
    for (const migration of migrationFiles()) {
      if (migration.version > version) break;
      database.exec("BEGIN IMMEDIATE");
      database.exec(migration.bytes.toString("utf8"));
      database
        .prepare(
          "INSERT INTO schema_migrations(version, name, checksum, applied_at) VALUES (?, ?, ?, ?)",
        )
        .run(
          migration.version,
          migration.name,
          createHash("sha256").update(migration.bytes).digest("hex"),
          new Date().toISOString(),
        );
      database.exec("COMMIT");
    }
  } finally {
    database.close();
  }
}

/** Why the Node runner refuses a ledger broken in each way (the message of the thrown error). */
async function refusalMessage(
  prepare: (file: string) => void,
  invalidUtf8 = false,
): Promise<string> {
  const directory = tempDir("capstan-ledger-export-");
  const file = path.join(directory, "controller.sqlite");
  const original = fs.readFileSync;
  try {
    prepare(file);
    if (invalidUtf8) {
      (fs as { readFileSync: unknown }).readFileSync = (
        target: unknown,
        ...rest: unknown[]
      ) =>
        target instanceof URL && target.pathname.endsWith("/0001_initial.sql")
          ? Buffer.from([0xff, 0xfe, 0x00])
          : (original as (...args: unknown[]) => unknown)(target, ...rest);
    }
    try {
      (await openDatabase(file)).close();
    } catch (error) {
      return (error as Error).message;
    }
    throw new Error("the Node runner accepted a ledger it should refuse");
  } finally {
    (fs as { readFileSync: unknown }).readFileSync = original;
    removeTempDir(directory);
  }
}

function edit(file: string, sql: string): void {
  const database = openSqlite(file, { timeout: 5_000 });
  try {
    database.exec(sql);
  } finally {
    database.close();
  }
}

/** A ledger broken in one way the runner refuses; `invalidUtf8` cases have no ledger, the migration file is the fault. */
export interface RefusalSetup {
  readonly name: string;
  readonly prepare: (file: string) => void;
  readonly invalidUtf8?: true;
}

/** The refusal scenarios; the Rust tests build the same broken ledgers (see tests/refusals.rs). */
export const REFUSAL_SETUPS: readonly RefusalSetup[] = [
  {
    name: "gap",
    prepare: (file) => {
      buildNodeLedger(file, 5);
      edit(file, "DELETE FROM schema_migrations WHERE version = 3");
    },
  },
  {
    name: "newer",
    prepare: (file) => {
      const newer = migrationFiles().length + 1;
      buildNodeLedger(file, newer - 1);
      edit(
        file,
        `INSERT INTO schema_migrations(version, name, checksum, applied_at) VALUES (${newer}, '${String(newer).padStart(4, "0")}_future.sql', 'bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb', 'y')`,
      );
    },
  },
  {
    name: "renamed",
    prepare: (file) => {
      buildNodeLedger(file, 3);
      edit(
        file,
        "UPDATE schema_migrations SET name = '0002_renamed.sql' WHERE version = 2",
      );
    },
  },
  {
    name: "checksum",
    prepare: (file) => {
      buildNodeLedger(file, 3);
      edit(
        file,
        "UPDATE schema_migrations SET checksum = 'bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb' WHERE version = 1",
      );
    },
  },
  {
    name: "tables-without-ledger",
    prepare: (file) => edit(file, "CREATE TABLE stray (id INTEGER)"),
  },
  { name: "invalid-utf8", prepare: () => undefined, invalidUtf8: true },
];

export async function refusalCases(): Promise<
  Array<{ case: string; message: string }>
> {
  const out: Array<{ case: string; message: string }> = [];
  for (const setup of REFUSAL_SETUPS)
    out.push({
      case: setup.name,
      message: await refusalMessage(setup.prepare, setup.invalidUtf8 === true),
    });
  return out;
}

const ISO_MILLIS = [
  0, 1, 999, 1000, 86_399_999, 86_400_000, 951_782_400_000, 951_868_799_999,
  1_582_934_400_000, 1_709_210_096_789, 1_760_000_000_000, 4_102_444_799_999,
  4_107_542_400_000, 253_402_300_799_999, -1, -86_400_000, -62_135_596_800_000,
  -62_167_219_200_000,
];

function isoCases(): Array<{ ms: number; iso: string }> {
  return ISO_MILLIS.map((ms) => ({ ms, iso: new Date(ms).toISOString() }));
}

/** Every fixture file by name, as the text the exporter writes. */
export async function exportFixtures(): Promise<Map<string, string>> {
  const files = new Map<string, string>();
  files.set(
    "refusals.json",
    JSON.stringify(await refusalCases(), null, 2) + "\n",
  );
  files.set("iso.json", JSON.stringify(isoCases()) + "\n");
  return files;
}

if (import.meta.filename === process.argv[1]) {
  mkdirSync(PARITY_DIRECTORY, { recursive: true });
  const files = await exportFixtures();
  for (const [name, text] of files)
    writeFileSync(path.join(PARITY_DIRECTORY, name), text);
  process.stdout.write(`wrote ${files.size} fixtures to ${PARITY_DIRECTORY}\n`);
}
