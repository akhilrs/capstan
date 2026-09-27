import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const destination = path.join(root, "dist", "migrations");
fs.mkdirSync(destination, { recursive: true });
fs.copyFileSync(
  path.join(root, "migrations", "0001_initial.sql"),
  path.join(destination, "0001_initial.sql"),
);
