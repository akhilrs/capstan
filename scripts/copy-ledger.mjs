// Takes a consistent snapshot of a controller ledger without ever opening the source for writing.
//   node scripts/copy-ledger.mjs <source controller.sqlite> <target dir under /tmp>
// The snapshot comes from the SQLite backup API over a read-only connection, so the live WAL is
// folded into the copy; the copy's integrity_check must pass. The target directory is created
// 0700 (and must not already hold a ledger). The copy lands at <target>/controller.sqlite.
import fs from "node:fs";
import os from "node:os";
import path from "node:path";

export async function copyLedger(source, targetDirectory) {
  const from = path.resolve(source);
  const dir = path.resolve(targetDirectory);
  const tmp = fs.realpathSync(os.tmpdir());
  const parent = fs.realpathSync(path.dirname(dir));
  if (!(
    parent === "/tmp" ||
    parent.startsWith("/tmp/") ||
    parent === tmp ||
    parent.startsWith(tmp + path.sep)
  ))
    throw new Error(`the target must be under /tmp, not ${dir}`);
  fs.lstatSync(from);
  fs.mkdirSync(dir, { recursive: true, mode: 0o700 });
  fs.chmodSync(dir, 0o700);
  const target = path.join(dir, "controller.sqlite");
  if (fs.existsSync(target)) throw new Error(`${target} already exists`);
  const sqlite = process.getBuiltinModule("node:sqlite");
  // A WAL-mode ledger with no -wal file beside it has no open connection; it is read as immutable, so this
  // never creates a -shm or -wal file next to the source. With a -wal file the live reader protocol is used.
  const quiet = !fs.existsSync(`${from}-wal`);
  const origin = new sqlite.DatabaseSync(
    quiet
      ? `file:${encodeURI(from).replace(/[?#]/g, encodeURIComponent)}?immutable=1`
      : from,
    { readOnly: true },
  );
  try {
    await sqlite.backup(origin, target);
  } finally {
    origin.close();
  }
  const copy = new sqlite.DatabaseSync(target);
  try {
    const rows = copy.prepare("PRAGMA integrity_check").all();
    if (rows.length !== 1 || rows[0].integrity_check !== "ok")
      throw new Error(
        `integrity_check failed on the copy: ${JSON.stringify(rows)}`,
      );
    copy.exec("PRAGMA wal_checkpoint(TRUNCATE)");
  } finally {
    copy.close();
  }
  fs.chmodSync(target, 0o600);
  return target;
}

if (
  process.argv[1] &&
  path.resolve(process.argv[1]) === new URL(import.meta.url).pathname
) {
  const [source, target] = process.argv.slice(2);
  if (!source || !target || process.argv.length !== 4) {
    process.stderr.write(
      "usage: copy-ledger.mjs <source controller.sqlite> <target dir under /tmp>\n",
    );
    process.exit(2);
  }
  copyLedger(source, target).then(
    (file) => process.stdout.write(`${file}\n`),
    (error) => {
      process.stderr.write(`copy-ledger: ${error.message}\n`);
      process.exit(1);
    },
  );
}
