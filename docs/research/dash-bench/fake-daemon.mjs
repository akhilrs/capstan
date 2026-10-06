// Replays a captured daemon `status` result over a unix socket, marking N active agents as just-active so the dash shows them working.
// usage: node fake-daemon.mjs SOCKET STATUS_JSON [WORKING_COUNT]
import fs from "node:fs";
import net from "node:net";
const [sock, file, workingArg] = process.argv.slice(2);
const base = JSON.parse(fs.readFileSync(file, "utf8"));
const working = Number(workingArg ?? 0);
const agents = base.agents.map((a) => ({ ...a }));
for (let i = 0; i < working; i++) { agents[i].state = "active"; agents[i].roleName = "developer"; }
try { fs.unlinkSync(sock); } catch {}
net.createServer((c) => {
  let buf = "";
  c.on("data", (d) => {
    buf += d;
    if (!buf.includes("\n")) return;
    for (let i = 0; i < working; i++) agents[i].lastActivityAt = new Date().toISOString();
    c.end(JSON.stringify({ ok: true, requestId: "x", result: { ...base, agents } }) + "\n");
  });
}).listen(sock);
