import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const destination = path.join(root, "dist", "migrations");
fs.mkdirSync(destination, { recursive: true });
for (const name of fs
  .readdirSync(path.join(root, "migrations"))
  .filter((entry) => entry.endsWith(".sql"))) {
  fs.copyFileSync(
    path.join(root, "migrations", name),
    path.join(destination, name),
  );
}
