import { createHash } from "node:crypto";
import fs from "node:fs";
import path from "node:path";
import Database from "better-sqlite3";

export class DatabaseMigrationError extends Error {
  override readonly name = "DatabaseMigrationError";
}

const migrations: Readonly<
  Record<number, { version: number; name: string; url: URL }>
> = {
  1: {
    version: 1,
    name: "0001_initial.sql",
    url: new URL("../../migrations/0001_initial.sql", import.meta.url),
  },
  2: {
    version: 2,
    name: "0002_runtime_identity_and_finding_reports.sql",
    url: new URL(
      "../../migrations/0002_runtime_identity_and_finding_reports.sql",
      import.meta.url,
    ),
  },
  3: {
    version: 3,
    name: "0003_command_start_intent.sql",
    url: new URL(
      "../../migrations/0003_command_start_intent.sql",
      import.meta.url,
    ),
  },
  4: {
    version: 4,
    name: "0004_assignment_actor.sql",
    url: new URL("../../migrations/0004_assignment_actor.sql", import.meta.url),
  },
  5: {
    version: 5,
    name: "0005_work_item_acceptance_criteria.sql",
    url: new URL(
      "../../migrations/0005_work_item_acceptance_criteria.sql",
      import.meta.url,
    ),
  },
  6: {
    version: 6,
    name: "0006_final_verification.sql",
    url: new URL(
      "../../migrations/0006_final_verification.sql",
      import.meta.url,
    ),
  },
  7: {
    version: 7,
    name: "0007_candidate_developer_evidence.sql",
    url: new URL(
      "../../migrations/0007_candidate_developer_evidence.sql",
      import.meta.url,
    ),
  },
};

export async function openDatabase(
  databasePath: string,
): Promise<Database.Database> {
  const database = new Database(databasePath, { timeout: 5_000 });
  try {
    database.pragma("foreign_keys = ON");
    database.pragma("journal_mode = WAL");
    database.pragma("synchronous = FULL");
    database.pragma("busy_timeout = 5000");
    await migrate(database, databasePath);
    return database;
  } catch (error) {
    database.close();
    throw error;
  }
}

export function openDatabaseReadOnly(databasePath: string): Database.Database {
  return new Database(databasePath, {
    readonly: true,
    fileMustExist: true,
    timeout: 5_000,
  });
}

async function migrate(
  database: Database.Database,
  databasePath: string,
): Promise<void> {
  const hasLedger = database
    .prepare(
      "SELECT 1 AS present FROM sqlite_master WHERE type = 'table' AND name = 'schema_migrations'",
    )
    .get();
  const existingTables = database
    .prepare(
      "SELECT COUNT(*) AS count FROM sqlite_master WHERE type = 'table' AND name NOT LIKE 'sqlite_%'",
    )
    .get() as { count: number };
  if (!hasLedger && existingTables.count !== 0) {
    throw new DatabaseMigrationError(
      "database has tables but no migration ledger",
    );
  }

  const applied = hasLedger
    ? (database
        .prepare(
          "SELECT version, name, checksum FROM schema_migrations ORDER BY version",
        )
        .all() as Array<{
        version: number;
        name: string;
        checksum: string;
      }>)
    : [];
  for (const [index, row] of applied.entries()) {
    if (row.version !== index + 1)
      throw new DatabaseMigrationError(
        `migration ledger has a gap before version ${row.version}`,
      );
    if (!Object.hasOwn(migrations, row.version))
      throw new DatabaseMigrationError(
        `database schema version ${row.version} is newer than this controller`,
      );
    const migration = migrations[row.version];
    if (!migration)
      throw new DatabaseMigrationError(
        `unknown migration record ${row.version}`,
      );
    if (migration.name !== row.name)
      throw new DatabaseMigrationError(
        `unknown migration record ${row.version}`,
      );
    const bytes = fs.readFileSync(migration.url);
    const checksum = createHash("sha256").update(bytes).digest("hex");
    if (checksum !== row.checksum)
      throw new DatabaseMigrationError(
        `migration ${row.name} checksum changed`,
      );
  }

  let currentVersion = applied.at(-1)?.version ?? 0;
  for (const migration of Object.values(migrations)) {
    if (migration.version <= currentVersion) continue;
    if (migration.version !== currentVersion + 1)
      throw new DatabaseMigrationError(
        `missing migration after version ${currentVersion}`,
      );
    if (currentVersion > 0) {
      const backupPath = `${databasePath}.pre-v${migration.version}-${Date.now()}.sqlite`;
      await database.backup(backupPath);
    }
    const bytes = fs.readFileSync(migration.url);
    const checksum = createHash("sha256").update(bytes).digest("hex");
    let sql: string;
    try {
      sql = new TextDecoder("utf-8", { fatal: true, ignoreBOM: true }).decode(
        bytes,
      );
    } catch {
      throw new DatabaseMigrationError(
        `migration ${migration.name} is not valid UTF-8`,
      );
    }
    database.exec("BEGIN IMMEDIATE");
    try {
      database.exec(sql);
      database
        .prepare(
          "INSERT INTO schema_migrations(version, name, checksum, applied_at) VALUES (?, ?, ?, ?)",
        )
        .run(
          migration.version,
          migration.name,
          checksum,
          new Date().toISOString(),
        );
      database.exec("COMMIT");
    } catch (error) {
      database.exec("ROLLBACK");
      throw error;
    }
    currentVersion = migration.version;
  }
}

export function resolveDatabasePath(stateDirectory: string): string {
  if (!path.isAbsolute(stateDirectory))
    throw new TypeError("state directory must be absolute");
  return path.join(stateDirectory, "controller.sqlite");
}
