/**
 * A thin better-sqlite3-shaped adapter over the built-in node:sqlite, so the ledger code keeps its
 * synchronous prepare/run/get/all/exec/pragma/transaction calls.
 *
 * Differences from better-sqlite3 that callers can see:
 * - Rows are plain objects (node:sqlite returns null-prototype objects).
 * - Integers are JS numbers; node:sqlite throws ERR_OUT_OF_RANGE above 2^53 where better-sqlite3
 *   silently lost precision.
 * - BLOB columns come back as Uint8Array, not Buffer (the ledger has no blob columns today).
 * - Errors carry code ERR_SQLITE_ERROR with errcode/errstr instead of being a SqliteError.
 */
import fs from "node:fs";
import type * as NodeSqlite from "node:sqlite";

type SqliteModule = typeof NodeSqlite;

let cached: SqliteModule | undefined;

/**
 * Loads node:sqlite while dropping only its one-shot SQLite ExperimentalWarning; every other
 * warning still reaches process.emitWarning.
 */
export function loadNodeSqlite(): SqliteModule {
  if (cached) return cached;
  const getBuiltin = (process as { getBuiltinModule?: (id: string) => unknown })
    .getBuiltinModule;
  if (typeof getBuiltin !== "function")
    throw new Error(
      "this Node.js has no process.getBuiltinModule; capstan needs Node.js 24 or later",
    );
  const original = process.emitWarning;
  const filtered = ((warning: string | Error, ...rest: unknown[]) => {
    const name =
      warning instanceof Error
        ? warning.name
        : typeof rest[0] === "string"
          ? rest[0]
          : (rest[0] as { type?: string } | undefined)?.type;
    const message = warning instanceof Error ? warning.message : warning;
    if (name === "ExperimentalWarning" && /sqlite/i.test(message)) return;
    return (original as (...args: unknown[]) => void).call(
      process,
      warning,
      ...rest,
    );
  }) as typeof process.emitWarning;
  process.emitWarning = filtered;
  let module: SqliteModule | undefined;
  try {
    module = getBuiltin.call(process, "node:sqlite") as
      SqliteModule | undefined;
  } finally {
    process.emitWarning = original;
  }
  if (
    !module ||
    typeof module.DatabaseSync !== "function" ||
    typeof module.backup !== "function"
  )
    throw new Error(
      "this Node.js lacks node:sqlite DatabaseSync or backup; capstan needs Node.js 24.6 or later",
    );
  cached = module;
  return module;
}

export interface RunResult {
  readonly changes: number;
  readonly lastInsertRowid: number;
}

// Typed loosely like better-sqlite3; node:sqlite rejects booleans and undefined at run time.
type Param = unknown;
type SqlParam = NodeSqlite.SQLInputValue;

export class Statement {
  readonly #statement: NodeSqlite.StatementSync;

  constructor(statement: NodeSqlite.StatementSync) {
    this.#statement = statement;
  }

  run(...params: Param[]): RunResult {
    const result = this.#statement.run(...(params as SqlParam[]));
    return {
      changes: Number(result.changes),
      lastInsertRowid: Number(result.lastInsertRowid),
    };
  }

  get(...params: Param[]): unknown {
    const row = this.#statement.get(...(params as SqlParam[]));
    return row === undefined ? undefined : { ...row };
  }

  all(...params: Param[]): unknown[] {
    return this.#statement
      .all(...(params as SqlParam[]))
      .map((row) => ({ ...row }));
  }
}

export interface OpenSqliteOptions {
  readonly readOnly?: boolean;
  /** Milliseconds to wait on a locked database. */
  readonly timeout?: number;
}

export class Database {
  readonly #db: NodeSqlite.DatabaseSync;
  #savepoints = 0;

  constructor(db: NodeSqlite.DatabaseSync) {
    this.#db = db;
  }

  get open(): boolean {
    return this.#db.isOpen;
  }

  prepare(sql: string): Statement {
    return new Statement(this.#db.prepare(sql));
  }

  exec(sql: string): void {
    this.#db.exec(sql);
  }

  /** `PRAGMA <statement>`; returns the result rows (an empty array when it returns none), as better-sqlite3 does. */
  pragma(statement: string): unknown[] {
    return this.prepare(`PRAGMA ${statement}`).all();
  }

  /**
   * Wraps `fn` in BEGIN ... COMMIT (SAVEPOINT ... RELEASE when already inside a transaction);
   * a throw rolls back and rethrows.
   */
  transaction<A extends unknown[], R>(
    fn: (...args: A) => R,
  ): (...args: A) => R {
    return (...args: A): R => {
      const nested = this.#db.isTransaction;
      const name = `capstan_sp_${this.#savepoints++}`;
      this.#db.exec(nested ? `SAVEPOINT ${name}` : "BEGIN");
      try {
        const result = fn(...args);
        this.#db.exec(nested ? `RELEASE ${name}` : "COMMIT");
        return result;
      } catch (error) {
        if (this.#db.isTransaction) {
          if (nested) this.#db.exec(`ROLLBACK TO ${name}; RELEASE ${name}`);
          else this.#db.exec("ROLLBACK");
        }
        throw error;
      } finally {
        this.#savepoints--;
      }
    };
  }

  /** An online backup of this database to `destPath`. */
  async backup(destPath: string): Promise<void> {
    await loadNodeSqlite().backup(this.#db, destPath);
  }

  close(): void {
    if (this.#db.isOpen) this.#db.close();
  }
}

export function openSqlite(
  path: string,
  options: OpenSqliteOptions = {},
): Database {
  const { DatabaseSync } = loadNodeSqlite();
  if (options.readOnly) fs.lstatSync(path); // node:sqlite has no fileMustExist; a missing file throws ENOENT
  const db = new DatabaseSync(path, {
    readOnly: options.readOnly === true,
    enableForeignKeyConstraints: true,
    ...(options.timeout === undefined ? {} : { timeout: options.timeout }),
  });
  return new Database(db);
}
