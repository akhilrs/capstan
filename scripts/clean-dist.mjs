import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

// Runs before every build so that output of deleted sources cannot linger in dist/ (a stale dist/test file
// once failed the suite after its source was removed). A daemon running from dist/ is unavailable until the build ends.
const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
fs.rmSync(path.join(root, "dist"), { recursive: true, force: true });
