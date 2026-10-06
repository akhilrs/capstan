// Times SQL strings from statusSnapshot against a COPY of a ledger. usage: node query-timing.mjs COPY.sqlite PROJECT_ID
import { DatabaseSync } from "node:sqlite";
const [file, pid] = process.argv.slice(2);
const db = new DatabaseSync(file, { readOnly: true });
const time = (name, sql, params, n = 30) => {
  const st = db.prepare(sql); st.all(...params);
  const t = process.hrtime.bigint();
  for (let i = 0; i < n; i++) st.all(...params);
  console.log(name.padEnd(28), (Number(process.hrtime.bigint() - t) / 1e6 / n).toFixed(2), "ms/query,", st.all(...params).length, "rows");
  if (process.env.PLAN) for (const r of db.prepare("EXPLAIN QUERY PLAN " + sql).all(...params)) console.log("    ", r.detail);
};
time("roles (seats+subqueries)", `SELECT s.role, s.seat_id, s.state AS seat_state,
 EXISTS(SELECT 1 FROM actors a WHERE a.project_id = s.project_id AND a.seat_id = s.seat_id AND a.active = 1 AND a.revoked_at IS NULL) AS actor_active,
 (SELECT a.assignment_id FROM assignments a WHERE a.project_id = s.project_id AND a.seat_id = s.seat_id AND a.authority_state IN ('active', 'unknown') ORDER BY a.created_at DESC LIMIT 1) AS assignment_id
 FROM seats s WHERE s.project_id = ? ORDER BY s.role, s.seat_id`, [pid]);
time("agents list ids", "SELECT agent_id FROM agents WHERE project_id = ? ORDER BY agent_id", [pid]);
time("agents all (one query)", "SELECT * FROM agents WHERE project_id = ?", [pid]);
const ids = db.prepare("SELECT agent_id FROM agents WHERE project_id = ?").all(pid).map((r) => r.agent_id);
let t = process.hrtime.bigint();
for (let k = 0; k < 10; k++) for (const id of ids) db.prepare("SELECT * FROM agents WHERE project_id = ? AND agent_id = ?").get(pid, id);
console.log("N+1 agentRow (prepare each)".padEnd(28), (Number(process.hrtime.bigint() - t) / 1e6 / 10).toFixed(2), "ms per listAgents,", ids.length, "agents");
const st = db.prepare("SELECT * FROM agents WHERE project_id = ? AND agent_id = ?");
t = process.hrtime.bigint();
for (let k = 0; k < 10; k++) for (const id of ids) st.get(pid, id);
console.log("N+1 agentRow (cached stmt)".padEnd(28), (Number(process.hrtime.bigint() - t) / 1e6 / 10).toFixed(2), "ms per listAgents");
